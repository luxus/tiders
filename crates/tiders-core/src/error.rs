use std::path::PathBuf;

/// Convenience alias used throughout the crate.
pub type Result<T> = std::result::Result<T, Error>;

/// Errors surfaced by `tiders-core`.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The requested operation needs an authenticated session but none exists.
    #[error("not signed in — run `tiders login` first")]
    NotAuthenticated,

    /// A track/album/etc. cannot be streamed (no playable stream URL).
    #[error("no playable stream for this item")]
    NoStream,

    /// The configured playback backend is unavailable (e.g. `mpv` not found).
    #[error("audio backend unavailable: {0}")]
    Backend(String),

    /// Error coming from the TIDAL client library.
    #[error("tidal api error: {0}")]
    Tidal(#[from] tidlers::TidalError),

    /// JSON (de)serialisation failure (session file, IPC payloads, …).
    #[error("serialization error: {0}")]
    Json(#[from] serde_json::Error),

    /// Filesystem error, annotated with the path that caused it.
    #[error("io error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// Any other failure with a human-readable message.
    #[error("{0}")]
    Other(String),
}

impl Error {
    /// Build an [`Error::Io`] tagged with the offending path.
    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Error::Io {
            path: path.into(),
            source,
        }
    }

    /// Build an [`Error::Other`] from anything string-like.
    pub fn other(msg: impl Into<String>) -> Self {
        Error::Other(msg.into())
    }
}
