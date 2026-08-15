//! Authentication, session persistence, and high-level catalog access.
//!
//! [`TidalService`] wraps a [`tidlers::TidalClient`] and is the single entry
//! point every front-end uses to talk to TIDAL. It owns:
//!
//! - the OAuth **device-code** login flow (and completion),
//! - loading/saving the session token to [`Config::session_path`], including a
//!   `TIDAL_SESSION_JSON` environment override handy for headless/CI use,
//! - trimmed catalog queries (search, track, favorites, playlists) returning the
//!   [`crate::model`] view types,
//! - resolving a track id into a playable [`StreamInfo`] for the
//!   [`Player`](crate::player::Player).

use tidlers::auth::TidalAuth;
use tidlers::client::models::search::config::SearchConfig;
use tidlers::client::models::track::config::TrackPlaybackInfoConfig;
use tidlers::client::oauth::OAuthStatus;
use tidlers::TidalClient;

use crate::config::{Config, Quality};
use crate::error::{Error, Result};
use crate::lyrics::{self, LyricLine};
use crate::model::{
    AlbumView, ArtistView, HomeCard, HomeCardKind, MixView, PlaylistView, SearchResults,
    StreamQuality, TrackView,
};

/// Environment variable that can seed a session in headless environments.
pub const SESSION_ENV: &str = "TIDAL_SESSION_JSON";

/// The device-code login challenge shown to the user.
#[derive(Debug, Clone)]
pub struct DeviceLogin {
    /// Verification URI (may omit the scheme; see [`DeviceLogin::url`]).
    pub verification_uri: String,
    /// Short code the user types on the TIDAL page.
    pub user_code: String,
    /// Opaque device code used while polling for completion.
    pub device_code: String,
    /// Seconds until the challenge expires.
    pub expires_in: u64,
    /// Seconds to wait between polls.
    pub interval: u64,
}

impl DeviceLogin {
    /// A clickable URL, guaranteed to carry an `https://` scheme.
    pub fn url(&self) -> String {
        if self.verification_uri.starts_with("http://")
            || self.verification_uri.starts_with("https://")
        {
            self.verification_uri.clone()
        } else {
            format!("https://{}", self.verification_uri)
        }
    }
}

/// A resolved, playable stream for a track.
#[derive(Debug, Clone)]
pub struct StreamInfo {
    pub url: String,
    pub quality: StreamQuality,
}

impl StreamInfo {
    pub fn mime_type(&self) -> Option<&str> {
        self.quality.mime_type.as_deref()
    }

    pub fn codecs(&self) -> Option<&str> {
        self.quality.codecs.as_deref()
    }
}

/// High-level TIDAL service used by every front-end.
pub struct TidalService {
    client: TidalClient,
    config: Config,
    quality: Quality,
}

impl TidalService {
    /// Create an unauthenticated service ready to drive a device-code login.
    pub fn new(config: Config, quality: Quality) -> Self {
        let auth = TidalAuth::with_oauth();
        let mut client = TidalClient::new(&auth);
        client.set_audio_quality(quality.to_api());
        Self {
            client,
            config,
            quality,
        }
    }

    /// Restore a previously saved session, if one exists.
    ///
    /// Looks first at [`Config::session_path`], then at the [`SESSION_ENV`]
    /// environment variable (persisting it to disk for reuse). Returns
    /// `Ok(None)` when there is nothing to restore.
    pub async fn restore(config: Config, quality: Quality) -> Result<Option<Self>> {
        let Some(session_json) = read_session_source(&config)? else {
            return Ok(None);
        };

        let mut client = TidalClient::from_json(&session_json)?;
        // Refresh the access token if it is stale, then load country-scoped
        // user info (required before most catalog calls).
        client.refresh_access_token(false).await?;
        client.refresh_user_info().await?;
        client.set_audio_quality(quality.to_api());

        let service = Self {
            client,
            config,
            quality,
        };
        service.save_session()?;
        Ok(Some(service))
    }

    /// Whether we have an authenticated user.
    pub fn is_authenticated(&self) -> bool {
        self.client.user_info.is_some()
    }

    /// The signed-in username, if known.
    pub fn username(&self) -> Option<String> {
        self.client.user_info.as_ref().map(|u| u.username.clone())
    }

    /// The signed-in user's country code, if known.
    pub fn country(&self) -> Option<String> {
        self.client
            .user_info
            .as_ref()
            .map(|u| u.country_code.clone())
    }

    /// The active streaming quality.
    pub fn quality(&self) -> Quality {
        self.quality
    }

    /// Begin the OAuth device-code flow: returns the URL + code to show the user.
    pub async fn begin_device_login(&mut self) -> Result<DeviceLogin> {
        let oauth = self.client.get_oauth_link().await?;
        Ok(DeviceLogin {
            verification_uri: oauth.verification_uri_complete,
            user_code: oauth.user_code,
            device_code: oauth.device_code,
            expires_in: oauth.expires_in,
            interval: oauth.interval,
        })
    }

    /// Complete a device-code login by polling until the user authorises.
    ///
    /// `status_tx` receives [`OAuthStatus`] updates (waiting/success/error) so a
    /// UI can react while polling. On success the session is saved to disk.
    pub async fn complete_device_login(
        &mut self,
        login: &DeviceLogin,
        status_tx: Option<tokio::sync::mpsc::UnboundedSender<OAuthStatus>>,
    ) -> Result<()> {
        self.client
            .wait_for_oauth(
                &login.device_code,
                login.expires_in,
                login.interval,
                status_tx,
            )
            .await?;
        self.client.refresh_user_info().await?;
        self.client.set_audio_quality(self.quality.to_api());
        self.save_session()?;
        Ok(())
    }

    /// Persist the current session token to [`Config::session_path`].
    pub fn save_session(&self) -> Result<()> {
        self.config.ensure_dir()?;
        let path = self.config.session_path();
        std::fs::write(&path, self.client.get_json()).map_err(|e| Error::io(path, e))
    }

    /// Sign out: best-effort server logout, then delete the local session file.
    pub async fn logout(&mut self) -> Result<()> {
        let _ = self.client.logout().await;
        let path = self.config.session_path();
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(Error::io(path, e)),
        }
    }

    /// Search the catalog and return normalised results.
    pub async fn search(&self, query: impl Into<String>, limit: u32) -> Result<SearchResults> {
        self.require_auth()?;
        let response = self
            .client
            .search(SearchConfig {
                query: query.into(),
                limit,
                ..Default::default()
            })
            .await?;
        Ok(SearchResults::from(&response))
    }

    /// Fetch a single track.
    pub async fn track(&self, id: u64) -> Result<TrackView> {
        self.require_auth()?;
        let track = self.client.get_track(id.to_string()).await?;
        Ok(TrackView::from(&track))
    }

    /// Fetch the user's favorite tracks (one page).
    pub async fn favorite_tracks(&self, limit: u32, offset: u32) -> Result<Vec<TrackView>> {
        self.require_auth()?;
        match self
            .client
            .get_collection_track_favorites(Some(limit), Some(offset))
            .await
        {
            Ok(response) => Ok(response
                .items
                .iter()
                .map(|e| TrackView::from(&e.item))
                .collect()),
            Err(_) => self.favorite_tracks_raw(limit, offset).await,
        }
    }

    /// Every song the user has added to their collection (paginated).
    pub async fn favorite_tracks_all(&self, max: u32) -> Result<Vec<TrackView>> {
        self.require_auth()?;
        let mut all = Vec::new();
        let page = 100u32;
        let mut offset = 0u32;
        loop {
            let batch = self.favorite_tracks_raw(page, offset).await?;
            let n = batch.len() as u32;
            all.extend(batch);
            offset += n;
            if n == 0 || n < page || all.len() as u32 >= max {
                break;
            }
        }
        // Dedup by id, keep first occurrence (most recently added first from API).
        let mut seen = std::collections::HashSet::new();
        all.retain(|t| seen.insert(t.id));
        Ok(all)
    }

    async fn favorite_tracks_raw(&self, limit: u32, offset: u32) -> Result<Vec<TrackView>> {
        let (token, user_id, country) = self.auth_triplet()?;
        let url = format!(
            "https://api.tidal.com/v1/users/{user_id}/favorites/tracks?countryCode={country}&limit={limit}&offset={offset}"
        );
        let value = authenticated_json(&token, &url).await?;
        let items = value
            .get("items")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        Ok(items
            .iter()
            .filter_map(|entry| {
                let item = entry.get("item").unwrap_or(entry);
                track_from_json(item)
            })
            .collect())
    }

    /// List the user's own playlists.
    pub async fn playlists(&self) -> Result<Vec<PlaylistView>> {
        self.require_auth()?;
        let response = self.client.list_playlists().await?;
        Ok(response
            .items
            .iter()
            .map(|p| PlaylistView {
                uuid: p.uuid.clone(),
                title: p.title.clone(),
                tracks: p.number_of_tracks as u32,
                cover: Some(p.square_image.clone()).filter(|s| !s.is_empty()),
                description: Some(p.description.clone()).filter(|s| !s.is_empty()),
            })
            .collect())
    }

    /// Tracks in a user/catalog playlist.
    pub async fn playlist_tracks(&self, uuid: &str) -> Result<Vec<TrackView>> {
        self.require_auth()?;
        use tidlers::client::models::playlist::PlaylistItemsOrder;
        use tidlers::client::models::OrderDirection;
        let response = self
            .client
            .get_playlist_items(
                uuid.to_string(),
                Some(100),
                Some(0),
                Some(PlaylistItemsOrder::Index),
                Some(OrderDirection::Ascending),
            )
            .await?;
        Ok(response
            .items
            .iter()
            .map(|e| TrackView::from(&e.item))
            .collect())
    }

    /// Tracks in a TIDAL mix.
    pub async fn mix_tracks(&self, mix_id: &str) -> Result<Vec<TrackView>> {
        self.require_auth()?;
        let response = self
            .client
            .get_mix_tracks(mix_id.to_string(), Some(100), Some(0))
            .await?;
        Ok(response.items.iter().map(TrackView::from).collect())
    }

    /// Radio seeded from a track (TIDAL's own radio endpoint).
    pub async fn track_radio(&self, track_id: u64) -> Result<Vec<TrackView>> {
        self.require_auth()?;
        let response = self
            .client
            .get_track_radio(track_id.to_string(), Some(50), Some(0))
            .await?;
        Ok(response.items.iter().map(TrackView::from).collect())
    }

    /// Mix id associated with a track, if any.
    pub async fn track_mix_id(&self, track_id: u64) -> Result<Option<String>> {
        self.require_auth()?;
        match self
            .client
            .get_track_mix(track_id.to_string(), Some(1), Some(0))
            .await
        {
            Ok(m) => Ok(Some(m.id)),
            Err(_) => Ok(None),
        }
    }

    /// Timed lyrics for a track (empty if TIDAL has none).
    ///
    /// TIDAL puts synced LRC in `subtitles` and often leaves `lyrics` as plain
    /// text. `tidlers::LyricsResponse` does not deserialize `subtitles` and
    /// requires several fields, so a missing `providerCommontrackId` makes the
    /// typed call fail on every track. We fetch the JSON ourselves and fall
    /// back to the typed helper.
    pub async fn lyrics(&self, track_id: u64) -> Result<Vec<LyricLine>> {
        self.require_auth()?;
        if let Ok(lines) = self.fetch_lyrics_json(track_id).await {
            if !lines.is_empty() {
                return Ok(lines);
            }
        }
        match self.client.get_track_lyrics(track_id.to_string()).await {
            Ok(resp) => Ok(lyrics::from_tidal(&resp.lyrics, None)),
            Err(_) => Ok(Vec::new()),
        }
    }

    async fn fetch_lyrics_json(&self, track_id: u64) -> Result<Vec<LyricLine>> {
        let token = self
            .client
            .session
            .auth
            .access_token
            .as_deref()
            .ok_or(Error::NotAuthenticated)?;
        let country = self.country().unwrap_or_else(|| "US".into());
        let url =
            format!("https://api.tidal.com/v1/tracks/{track_id}/lyrics?countryCode={country}");
        let resp = reqwest::Client::new()
            .get(url)
            .bearer_auth(token)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|e| Error::other(e.to_string()))?;
        if !resp.status().is_success() {
            return Ok(Vec::new());
        }
        let value: serde_json::Value =
            resp.json().await.map_err(|e| Error::other(e.to_string()))?;
        let lyrics = value.get("lyrics").and_then(|v| v.as_str()).unwrap_or("");
        let subtitles = value.get("subtitles").and_then(|v| v.as_str());
        Ok(lyrics::from_tidal(lyrics, subtitles))
    }

    /// Album tracks.
    pub async fn album_tracks(&self, album_id: u64) -> Result<Vec<TrackView>> {
        self.require_auth()?;
        let mut all = Vec::new();
        let mut offset = 0u64;
        loop {
            let response = self
                .client
                .get_album_items(album_id.to_string(), Some(100), Some(offset))
                .await?;
            let n = response.items.len();
            all.extend(response.items.iter().map(|e| TrackView::from(&e.item)));
            offset += n as u64;
            if n == 0 || offset >= response.total_number_of_items as u64 {
                break;
            }
        }
        Ok(all)
    }

    /// Artist profile (picture, mix id, name).
    pub async fn artist(&self, artist_id: u64) -> Result<ArtistView> {
        self.require_auth()?;
        let a = self.client.get_artist(artist_id.to_string()).await?;
        Ok(ArtistView {
            id: a.id,
            name: a.name,
            picture: a.picture.or(a.selected_album_cover_fallback),
            mix_id: a.mixes.as_ref().and_then(|m| {
                m.get("ARTIST_MIX")
                    .cloned()
                    .or_else(|| m.values().next().cloned())
            }),
        })
    }

    /// Artist top tracks.
    pub async fn artist_tracks(&self, artist_id: u64) -> Result<Vec<TrackView>> {
        self.require_auth()?;
        let response = self
            .client
            .get_artist_tracks(artist_id.to_string(), Some(50), Some(0))
            .await?;
        Ok(response.items.iter().map(TrackView::from).collect())
    }

    /// Artist albums.
    pub async fn artist_albums(&self, artist_id: u64) -> Result<Vec<AlbumView>> {
        self.require_auth()?;
        let response = self
            .client
            .get_artist_albums(artist_id.to_string(), Some(50), Some(0))
            .await?;
        Ok(response
            .items
            .iter()
            .map(|a| AlbumView {
                id: a.id as u64,
                title: a.title.clone(),
                artist: a.artist.name.clone(),
                cover: Some(a.cover.clone()),
                release_date: Some(a.release_date.clone()),
                tracks: Some(a.number_of_tracks),
            })
            .collect())
    }

    /// Artist biography (plain text; HTML tags stripped lightly).
    pub async fn artist_bio(&self, artist_id: u64) -> Result<String> {
        self.require_auth()?;
        match self.client.get_artist_bio(artist_id.to_string()).await {
            Ok(bio) => Ok(strip_simple_html(if bio.summary.is_empty() {
                bio.text
            } else {
                bio.summary
            })),
            Err(_) => Ok(String::new()),
        }
    }

    /// Favorite albums.
    pub async fn favorite_albums(&self, limit: u32, offset: u32) -> Result<Vec<AlbumView>> {
        self.require_auth()?;
        let response = self
            .client
            .get_collection_album_favorites(Some(limit), Some(offset))
            .await?;
        Ok(response
            .items
            .iter()
            .map(|e| AlbumView::from(&e.item))
            .collect())
    }

    /// Favorite / followed artists.
    pub async fn favorite_artists(&self, limit: u32) -> Result<Vec<ArtistView>> {
        self.require_auth()?;
        let response = self.client.get_collection_artists(limit).await?;
        Ok(response
            .items
            .iter()
            .map(|e| ArtistView {
                id: e.data.id as u64,
                name: e.data.name.clone(),
                picture: e.data.picture.clone(),
                mix_id: e.data.mixes.as_ref().and_then(|m| {
                    m.get("ARTIST_MIX")
                        .cloned()
                        .or_else(|| m.values().next().cloned())
                }),
            })
            .collect())
    }

    /// Love (favorite) a track.
    pub async fn love_track(&self, track_id: u64) -> Result<()> {
        self.require_auth()?;
        use tidlers::client::models::collection::favorites::FavoriteResourceType;
        self.client
            .add_to_favorites(FavoriteResourceType::Tracks, track_id as u32)
            .await?;
        Ok(())
    }

    /// Unlove a track.
    pub async fn unlove_track(&self, track_id: u64) -> Result<()> {
        self.require_auth()?;
        use tidlers::client::models::collection::favorites::FavoriteResourceType;
        self.client
            .remove_from_favorites(FavoriteResourceType::Tracks, track_id as u32)
            .await?;
        Ok(())
    }

    /// Best-effort "don't like" signal so mixes stop recommending the track.
    pub async fn dislike_track(&self, track_id: u64) -> Result<()> {
        self.require_auth()?;
        let (token, _user_id, country) = self.auth_triplet()?;
        let url =
            format!("https://api.tidal.com/v1/feedback/track/{track_id}?countryCode={country}");
        let client = reqwest::Client::new();
        let resp = client
            .post(&url)
            .bearer_auth(&token)
            .header("Accept", "application/json")
            .header("Content-Type", "application/json")
            .json(&serde_json::json!({ "feedbackType": "NEGATIVE" }))
            .send()
            .await
            .map_err(|e| Error::other(e.to_string()))?;
        if resp.status().is_success() || resp.status().as_u16() == 204 {
            return Ok(());
        }
        // Older clients posted to /feedbacks; ignore failures — skip locally anyway.
        let _ = client
            .post(format!(
                "https://api.tidal.com/v1/feedbacks?countryCode={country}"
            ))
            .bearer_auth(&token)
            .json(&serde_json::json!({
                "itemId": track_id,
                "itemType": "track",
                "feedbackType": "NEGATIVE"
            }))
            .send()
            .await;
        Ok(())
    }

    /// Mixes + playlists pulled from the home feed ("My Mixes" and "For You").
    ///
    /// The typed home-feed parser is brittle (new module types fail the whole
    /// response). We try it first, then fall back to the phone feed, pages, and
    /// a recursive JSON walk so signed-in users still see mixes.
    pub async fn home_cards(&self) -> Result<(Vec<MixView>, Vec<HomeCard>)> {
        self.require_auth()?;
        let mut mixes = Vec::new();
        let mut cards = Vec::new();

        if let Ok(feed) = self.client.get_home_feed(50).await {
            for item in &feed.items {
                collect_home_item(item, &mut mixes, &mut cards);
            }
        }
        if mixes.is_empty() && cards.is_empty() {
            if let Ok(feed) = self.client.get_home_feed_phone(50).await {
                for item in &feed.items {
                    collect_home_item(item, &mut mixes, &mut cards);
                }
            }
        }

        for slug in ["mixes", "for_you", "home", "explore"] {
            if let Ok(page) = self.client.get_page(slug).await {
                if let Ok(value) = serde_json::to_value(&page) {
                    walk_json(&value, &mut mixes, &mut cards, 0);
                }
            }
        }

        // Typed page/feed parsers often fail on new module shapes. Walk the
        // raw JSON so signed-in users still get Mixes and For You.
        if mixes.is_empty() || cards.is_empty() {
            if let Ok((token, _, country)) = self.auth_triplet() {
                let urls = [
                    format!(
                        "https://api.tidal.com/v1/pages/mixes?countryCode={country}&deviceType=BROWSER&locale=en_US"
                    ),
                    format!(
                        "https://api.tidal.com/v1/pages/for_you?countryCode={country}&deviceType=BROWSER&locale=en_US"
                    ),
                    format!(
                        "https://api.tidal.com/v1/pages/home?countryCode={country}&deviceType=BROWSER&locale=en_US"
                    ),
                    format!(
                        "https://tidal.com/v2/home/feed/static?countryCode={country}&deviceType=BROWSER&platform=WEB&limit=50"
                    ),
                ];
                for url in urls {
                    if let Ok(value) = authenticated_json(&token, &url).await {
                        walk_json(&value, &mut mixes, &mut cards, 0);
                    }
                    if !mixes.is_empty() && !cards.is_empty() {
                        break;
                    }
                }
            }
        }

        if let Ok(arrivals) = self.client.get_arrival_mixes().await {
            for (i, mix) in arrivals.data.iter().enumerate() {
                if mixes.iter().any(|m| m.id == mix.id) {
                    continue;
                }
                mixes.push(MixView {
                    id: mix.id.clone(),
                    title: format!("Arrival Mix {}", i + 1),
                    subtitle: "New arrivals".into(),
                    cover_url: None,
                });
            }
        }

        dedup_home(&mut mixes, &mut cards);
        Ok((mixes, cards))
    }

    /// Resolve a track id into a playable stream URL at the given quality.
    pub async fn stream_url(&mut self, id: u64, quality: Quality) -> Result<StreamInfo> {
        self.require_auth()?;
        self.client.set_audio_quality(quality.to_api());
        let playback = self
            .client
            .get_track_postpaywall_playback_info(
                id.to_string(),
                Some(TrackPlaybackInfoConfig {
                    audio_quality: Some(quality.to_api()),
                    ..Default::default()
                }),
            )
            .await?;

        let url = playback.get_primary_url().ok_or(Error::NoStream)?;
        let mut quality = StreamQuality {
            audio_quality: Some(playback.audio_quality.clone()),
            mime_type: playback.get_mime_type(),
            codecs: playback.get_codecs(),
            bitrate_bps: match &playback.manifest_parsed {
                Some(tidlers::client::models::track::playback::ParsedTrackManifest::Dash(d)) => {
                    d.bitrate
                }
                _ => None,
            },
            ..StreamQuality::default()
        };
        // Typical defaults when the decoder hasn't reported yet.
        if quality
            .codecs
            .as_deref()
            .is_some_and(|c| c.contains("flac"))
            && quality.sample_rate_hz.is_none()
        {
            if playback
                .audio_quality
                .to_ascii_uppercase()
                .contains("HI_RES")
            {
                quality.bit_depth = Some(24);
            } else {
                quality.bit_depth = Some(16);
                quality.sample_rate_hz = Some(44_100);
            }
        }
        Ok(StreamInfo { url, quality })
    }

    fn require_auth(&self) -> Result<()> {
        if self.is_authenticated() {
            Ok(())
        } else {
            Err(Error::NotAuthenticated)
        }
    }

    fn auth_triplet(&self) -> Result<(String, u64, String)> {
        let token = self
            .client
            .session
            .auth
            .access_token
            .clone()
            .ok_or(Error::NotAuthenticated)?;
        let user_id = self
            .client
            .session
            .auth
            .user_id
            .ok_or(Error::NotAuthenticated)?;
        let country = self.country().unwrap_or_else(|| "US".into());
        Ok((token, user_id, country))
    }
}

async fn authenticated_json(token: &str, url: &str) -> Result<serde_json::Value> {
    let resp = reqwest::Client::new()
        .get(url)
        .bearer_auth(token)
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| Error::other(e.to_string()))?;
    if !resp.status().is_success() {
        return Err(Error::other(format!("tidal http {}", resp.status())));
    }
    resp.json().await.map_err(|e| Error::other(e.to_string()))
}

fn json_str(v: &serde_json::Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(|x| x.as_str())
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
}

fn json_u64(v: &serde_json::Value, key: &str) -> Option<u64> {
    v.get(key).and_then(|x| {
        x.as_u64()
            .or_else(|| x.as_i64().map(|n| n as u64))
            .or_else(|| x.as_str().and_then(|s| s.parse().ok()))
    })
}

fn json_text_info(v: &serde_json::Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(|info| json_str(info, "text").or_else(|| info.as_str().map(|s| s.to_string())))
        .filter(|s| !s.is_empty())
}

fn track_from_json(v: &serde_json::Value) -> Option<TrackView> {
    let id = json_u64(v, "id")?;
    let title = json_str(v, "title").unwrap_or_else(|| "Track".into());
    let artist = v
        .get("artist")
        .and_then(|a| json_str(a, "name"))
        .or_else(|| {
            v.get("artists")
                .and_then(|a| a.as_array())
                .and_then(|arr| arr.first())
                .and_then(|a| json_str(a, "name"))
        })
        .unwrap_or_default();
    let album = v.get("album").and_then(|a| json_str(a, "title"));
    let album_id = v.get("album").and_then(|a| json_u64(a, "id"));
    let cover = v.get("album").and_then(|a| json_str(a, "cover"));
    let artist_id = v.get("artist").and_then(|a| json_u64(a, "id")).or_else(|| {
        v.get("artists")
            .and_then(|a| a.as_array())
            .and_then(|arr| arr.first())
            .and_then(|a| json_u64(a, "id"))
    });
    Some(TrackView {
        id,
        title,
        artist,
        album,
        duration_secs: json_u64(v, "duration").unwrap_or(0),
        explicit: v.get("explicit").and_then(|x| x.as_bool()).unwrap_or(false),
        cover,
        audio_quality: json_str(v, "audioQuality"),
        bpm: v.get("bpm").and_then(|x| x.as_f64()).map(|n| n as f32),
        album_id,
        artist_id,
        mix_id: v.get("mixes").and_then(|m| m.as_object()).and_then(|m| {
            m.get("TRACK_MIX")
                .or_else(|| m.get("trackMix"))
                .and_then(|x| x.as_str())
                .map(|s| s.to_string())
                .or_else(|| {
                    m.values()
                        .next()
                        .and_then(|x| x.as_str().map(|s| s.to_string()))
                })
        }),
    })
}

fn walk_json(
    value: &serde_json::Value,
    mixes: &mut Vec<MixView>,
    cards: &mut Vec<HomeCard>,
    depth: usize,
) {
    if depth > 8 {
        return;
    }
    match value {
        serde_json::Value::Array(items) => {
            for item in items {
                walk_json(item, mixes, cards, depth + 1);
            }
        }
        serde_json::Value::Object(map) => {
            let wrapped = map.get("item").or_else(|| map.get("data"));
            if let Some(inner) = wrapped {
                consider_catalog_object(
                    inner,
                    map.get("type").and_then(|t| t.as_str()),
                    mixes,
                    cards,
                );
                walk_json(inner, mixes, cards, depth + 1);
            } else {
                consider_catalog_object(
                    value,
                    map.get("type").and_then(|t| t.as_str()),
                    mixes,
                    cards,
                );
            }
            for (k, v) in map {
                if matches!(k.as_str(), "item" | "data") {
                    continue;
                }
                walk_json(v, mixes, cards, depth + 1);
            }
        }
        _ => {}
    }
}

fn consider_catalog_object(
    v: &serde_json::Value,
    type_hint: Option<&str>,
    mixes: &mut Vec<MixView>,
    cards: &mut Vec<HomeCard>,
) {
    let hint = type_hint.unwrap_or("").to_ascii_uppercase();
    let own_type = json_str(v, "type")
        .or_else(|| json_str(v, "mixType"))
        .unwrap_or_default()
        .to_ascii_uppercase();
    let kind = if hint.is_empty() { own_type } else { hint };

    let title = json_str(v, "title")
        .or_else(|| json_text_info(v, "titleTextInfo"))
        .or_else(|| json_str(v, "name"));
    let subtitle = json_str(v, "subTitle")
        .or_else(|| json_str(v, "subtitle"))
        .or_else(|| json_text_info(v, "subtitleTextInfo"))
        .unwrap_or_default();

    if kind.contains("MIX") || v.get("mixType").is_some() || v.get("titleTextInfo").is_some() {
        if let Some(id) = json_str(v, "id") {
            if id.len() >= 8 {
                let cover_url = v
                    .get("mixImages")
                    .and_then(|a| a.as_array())
                    .and_then(|a| a.first())
                    .and_then(|img| json_str(img, "url"))
                    .or_else(|| {
                        v.get("images")
                            .and_then(|img| img.get("SMALL").or_else(|| img.get("MEDIUM")))
                            .and_then(|img| json_str(img, "url"))
                    });
                mixes.push(MixView {
                    id: id.clone(),
                    title: title.clone().unwrap_or_else(|| "Mix".into()),
                    subtitle: subtitle.clone(),
                    cover_url,
                });
                cards.push(HomeCard {
                    title: title.unwrap_or_else(|| "Mix".into()),
                    subtitle,
                    kind: HomeCardKind::Mix { id },
                });
                return;
            }
        }
    }
    if kind.contains("PLAYLIST") {
        if let Some(uuid) = json_str(v, "uuid").or_else(|| json_str(v, "id")) {
            if uuid.contains('-') {
                cards.push(HomeCard {
                    title: title.unwrap_or_else(|| "Playlist".into()),
                    subtitle: json_u64(v, "numberOfTracks")
                        .map(|n| format!("{n} tracks"))
                        .unwrap_or(subtitle),
                    kind: HomeCardKind::Playlist { uuid },
                });
            }
        }
        return;
    }
    if kind.contains("ALBUM") {
        if let Some(id) = json_u64(v, "id") {
            cards.push(HomeCard {
                title: title.unwrap_or_else(|| "Album".into()),
                subtitle,
                kind: HomeCardKind::Album { id },
            });
        }
        return;
    }
    if kind.contains("ARTIST") {
        if let Some(id) = json_u64(v, "id") {
            cards.push(HomeCard {
                title: title.unwrap_or_else(|| "Artist".into()),
                subtitle: "Artist".into(),
                kind: HomeCardKind::Artist { id },
            });
        }
    }
}

fn dedup_home(mixes: &mut Vec<MixView>, cards: &mut Vec<HomeCard>) {
    let mut seen_mix = std::collections::HashSet::new();
    mixes.retain(|m| seen_mix.insert(m.id.clone()));
    let mut seen_card = std::collections::HashSet::new();
    cards.retain(|c| {
        let key = match &c.kind {
            HomeCardKind::Mix { id } => format!("mix:{id}"),
            HomeCardKind::Playlist { uuid } => format!("pl:{uuid}"),
            HomeCardKind::Album { id } => format!("al:{id}"),
            HomeCardKind::Artist { id } => format!("ar:{id}"),
        };
        seen_card.insert(key)
    });
}

fn strip_simple_html(s: String) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '<' {
            out.push(c);
            continue;
        }
        let mut tag = String::new();
        for t in chars.by_ref() {
            if t == '>' {
                break;
            }
            tag.push(t);
        }
        let name = tag
            .split_whitespace()
            .next()
            .unwrap_or("")
            .trim_end_matches('/')
            .to_ascii_lowercase();
        if name == "br" {
            out.push('\n');
        }
    }
    out.replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

fn collect_home_item(
    item: &tidlers::client::models::home::HomeItem,
    mixes: &mut Vec<MixView>,
    cards: &mut Vec<HomeCard>,
) {
    use tidlers::client::models::home::{HomeItem, HomeShortcutItem};
    match item {
        HomeItem::HomeShortcutList { inner } => {
            for it in &inner.items {
                match it {
                    HomeShortcutItem::Mix(m) => push_mix(&m.data, mixes, cards),
                    HomeShortcutItem::Playlist(p) => push_playlist(&p.data, cards),
                    HomeShortcutItem::Album(a) => push_album(&a.data, cards),
                    _ => {}
                }
            }
        }
        HomeItem::HomeHorizontalList { inner } => {
            for it in &inner.items {
                collect_list_item(it, mixes, cards);
            }
        }
        HomeItem::HomeHorizontalListWithContext { inner } => {
            collect_list_item(&inner.header, mixes, cards);
            for it in &inner.items {
                collect_list_item(it, mixes, cards);
            }
        }
        HomeItem::HomeVerticalListCard { inner } => {
            for it in &inner.items {
                collect_list_item(it, mixes, cards);
            }
        }
        HomeItem::HomeTrackList { inner } => {
            for it in &inner.items {
                collect_list_item(it, mixes, cards);
            }
        }
    }
}

fn collect_list_item(
    item: &tidlers::client::models::home::HomeListItem,
    mixes: &mut Vec<MixView>,
    cards: &mut Vec<HomeCard>,
) {
    use tidlers::client::models::home::HomeListItem;
    match item {
        HomeListItem::Mix(m) => push_mix(&m.data, mixes, cards),
        HomeListItem::Playlist(p) => push_playlist(&p.data, cards),
        HomeListItem::Album(a) => push_album(&a.data, cards),
        HomeListItem::Artist(a) => {
            cards.push(HomeCard {
                title: a.data.name.clone(),
                subtitle: "Artist".into(),
                kind: HomeCardKind::Artist { id: a.data.id },
            });
        }
        _ => {}
    }
}

fn push_mix(
    data: &tidlers::client::models::home::HomeMixData,
    mixes: &mut Vec<MixView>,
    cards: &mut Vec<HomeCard>,
) {
    let title = data
        .title_text_info
        .text
        .clone()
        .unwrap_or_else(|| "Mix".into());
    let subtitle = data.subtitle_text_info.text.clone().unwrap_or_default();
    let cover_url = data.mix_images.first().map(|i| i.url.clone());
    mixes.push(MixView {
        id: data.id.clone(),
        title: title.clone(),
        subtitle: subtitle.clone(),
        cover_url,
    });
    cards.push(HomeCard {
        title,
        subtitle,
        kind: HomeCardKind::Mix {
            id: data.id.clone(),
        },
    });
}

fn push_playlist(
    data: &tidlers::client::models::home::HomePlaylistData,
    cards: &mut Vec<HomeCard>,
) {
    cards.push(HomeCard {
        title: data.title.clone(),
        subtitle: format!("{} tracks", data.number_of_tracks),
        kind: HomeCardKind::Playlist {
            uuid: data.uuid.clone(),
        },
    });
}

fn push_album(data: &tidlers::client::models::home::HomeAlbumData, cards: &mut Vec<HomeCard>) {
    let artist = data
        .artists
        .first()
        .map(|a| a.name.clone())
        .unwrap_or_default();
    cards.push(HomeCard {
        title: data.title.clone(),
        subtitle: artist,
        kind: HomeCardKind::Album { id: data.id },
    });
}

/// Read the session JSON from disk or the environment override.
fn read_session_source(config: &Config) -> Result<Option<String>> {
    let path = config.session_path();
    match std::fs::read_to_string(&path) {
        Ok(raw) if !raw.trim().is_empty() => return Ok(Some(raw)),
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(Error::io(path, e)),
    }

    // Fall back to the environment override (headless / CI seeding).
    match std::env::var(SESSION_ENV) {
        Ok(raw) if !raw.trim().is_empty() => Ok(Some(raw)),
        _ => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_login_url_adds_scheme() {
        let login = DeviceLogin {
            verification_uri: "link.tidal.com/ABC12".into(),
            user_code: "ABC12".into(),
            device_code: "dev".into(),
            expires_in: 300,
            interval: 2,
        };
        assert_eq!(login.url(), "https://link.tidal.com/ABC12");

        let already = DeviceLogin {
            verification_uri: "https://link.tidal.com/XYZ".into(),
            ..login
        };
        assert_eq!(already.url(), "https://link.tidal.com/XYZ");
    }

    #[test]
    fn restore_returns_none_without_a_session() {
        // A directory that has no session file and no env override.
        let tmp = std::env::temp_dir().join(format!("tiders-none-{}", std::process::id()));
        let cfg = Config::at(&tmp);
        // SAFETY: single-threaded test; ensure the override is not set.
        unsafe {
            std::env::remove_var(SESSION_ENV);
        }
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let restored = rt
            .block_on(TidalService::restore(cfg, Quality::Lossless))
            .unwrap();
        assert!(restored.is_none());
    }

    #[test]
    fn walk_json_finds_mixes_and_playlists() {
        let payload = serde_json::json!([
            {
                "type": "MIX",
                "item": {
                    "id": "000abc123def4567890",
                    "title": "My Mix 1",
                    "subTitle": "Updated today"
                }
            },
            {
                "type": "PLAYLIST",
                "item": {
                    "uuid": "11111111-2222-3333-4444-555555555555",
                    "title": "Discover Weekly",
                    "numberOfTracks": 30
                }
            }
        ]);
        let mut mixes = Vec::new();
        let mut cards = Vec::new();
        walk_json(&payload, &mut mixes, &mut cards, 0);
        dedup_home(&mut mixes, &mut cards);
        assert_eq!(mixes.len(), 1);
        assert_eq!(mixes[0].title, "My Mix 1");
        assert!(cards.iter().any(|c| c.title == "Discover Weekly"));
    }

    #[test]
    fn track_from_json_reads_nested_album() {
        let v = serde_json::json!({
            "id": 42,
            "title": "Fast Car",
            "duration": 297,
            "explicit": false,
            "artist": {"id": 7, "name": "Tracy Chapman"},
            "album": {"id": 9, "title": "Greatest Hits", "cover": "aaaa-bbbb"}
        });
        let t = track_from_json(&v).unwrap();
        assert_eq!(t.id, 42);
        assert_eq!(t.artist, "Tracy Chapman");
        assert_eq!(t.album.as_deref(), Some("Greatest Hits"));
        assert_eq!(t.artist_id, Some(7));
        assert_eq!(t.album_id, Some(9));
    }

    #[test]
    fn strip_html_keeps_line_breaks_and_entities() {
        assert_eq!(
            strip_simple_html("Verse one<br/>Verse two".into()),
            "Verse one\nVerse two"
        );
        assert_eq!(strip_simple_html("a<br>b<br />c".into()), "a\nb\nc");
        assert_eq!(strip_simple_html("<p>hi &amp; lo</p>".into()), "hi & lo");
        assert_eq!(strip_simple_html("a<br class=\"x\">b".into()), "a\nb");
    }
}
