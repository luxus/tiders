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
use crate::model::{PlaylistView, SearchResults, TrackView};

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
    pub mime_type: Option<String>,
    pub codecs: Option<String>,
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
            })
            .collect())
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
        Ok(StreamInfo {
            url,
            mime_type: playback.get_mime_type(),
            codecs: playback.get_codecs(),
        })
    }

    fn require_auth(&self) -> Result<()> {
        if self.is_authenticated() {
            Ok(())
        } else {
            Err(Error::NotAuthenticated)
        }
    }
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
}
