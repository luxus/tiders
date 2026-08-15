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

    /// Fetch the user's favorite tracks.
    pub async fn favorite_tracks(&self, limit: u32, offset: u32) -> Result<Vec<TrackView>> {
        self.require_auth()?;
        let response = self
            .client
            .get_collection_track_favorites(Some(limit), Some(offset))
            .await?;
        Ok(response
            .items
            .iter()
            .map(|e| TrackView::from(&e.item))
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
    pub async fn lyrics(&self, track_id: u64) -> Result<Vec<LyricLine>> {
        self.require_auth()?;
        match self.client.get_track_lyrics(track_id.to_string()).await {
            Ok(resp) => Ok(lyrics::parse_lrc(&resp.lyrics)),
            Err(_) => Ok(Vec::new()),
        }
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

    /// Mixes + playlists pulled from the home feed ("My Mixes" and "For You").
    pub async fn home_cards(&self) -> Result<(Vec<MixView>, Vec<HomeCard>)> {
        self.require_auth()?;
        let feed = self.client.get_home_feed(50).await?;
        let mut mixes = Vec::new();
        let mut cards = Vec::new();
        for item in &feed.items {
            collect_home_item(item, &mut mixes, &mut cards);
        }
        let mut seen = std::collections::HashSet::new();
        mixes.retain(|m| seen.insert(m.id.clone()));
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
