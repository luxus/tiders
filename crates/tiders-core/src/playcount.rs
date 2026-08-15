//! Local play-count store used by weighted shuffle modes.
//!
//! Counts increment when a track is listened past 50% (the same threshold
//! Last.fm uses). Favourites shuffle prefers high counts; discovery shuffle
//! prefers tracks that have rarely (or never) been played.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::model::TrackView;

/// Persist play counts next to the session as `playcounts.json`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PlayCountStore {
    /// `"artist\x00title"` → count.
    #[serde(flatten)]
    counts: HashMap<String, u32>,
    #[serde(skip)]
    path: PathBuf,
}

impl PlayCountStore {
    /// Load from `path`, or start empty if the file is missing.
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        match std::fs::read_to_string(&path) {
            Ok(raw) => {
                let mut store: PlayCountStore = serde_json::from_str(&raw)?;
                store.path = path;
                Ok(store)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self {
                counts: HashMap::new(),
                path,
            }),
            Err(e) => Err(Error::io(path, e)),
        }
    }

    /// Load, or an empty store still pointed at `path` (so later saves work).
    pub fn load_or_empty(path: impl AsRef<Path>) -> Self {
        let path = path.as_ref().to_path_buf();
        Self::load(&path).unwrap_or(Self {
            counts: HashMap::new(),
            path,
        })
    }

    /// Persist to disk (best-effort directory creation).
    pub fn save(&self) -> Result<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
        }
        let raw = serde_json::to_string_pretty(&self.counts)?;
        std::fs::write(&self.path, raw).map_err(|e| Error::io(&self.path, e))
    }

    /// Lookup by a track's artist + title.
    pub fn get(&self, track: &TrackView) -> u32 {
        self.counts.get(&track_key(track)).copied().unwrap_or(0)
    }

    /// Increment the count for `track` and persist.
    pub fn bump(&mut self, track: &TrackView) {
        let key = track_key(track);
        *self.counts.entry(key).or_insert(0) += 1;
        let _ = self.save();
    }

    /// Weight used by shuffle: favourites → higher is better; discovery → invert.
    pub fn weight(&self, track: &TrackView, favourites: bool) -> f64 {
        let count = self.get(track) as f64;
        if favourites {
            (count + 1.0).powf(1.35)
        } else {
            1.0 / (count + 1.0)
        }
    }
}

fn track_key(track: &TrackView) -> String {
    format!(
        "{}\x00{}",
        track.artist.to_ascii_lowercase(),
        track.title.to_ascii_lowercase()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::TrackView;

    fn track(title: &str) -> TrackView {
        TrackView {
            title: title.into(),
            artist: "Air".into(),
            ..TrackView::default()
        }
    }

    #[test]
    fn bump_increments_and_roundtrips() {
        let dir = std::env::temp_dir().join(format!("tiders-pc-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("playcounts.json");
        let _ = std::fs::remove_file(&path);

        let mut store = PlayCountStore::load(&path).unwrap();
        let t = track("La Femme d'Argent");
        assert_eq!(store.get(&t), 0);
        store.bump(&t);
        store.bump(&t);
        assert_eq!(store.get(&t), 2);

        let reloaded = PlayCountStore::load(&path).unwrap();
        assert_eq!(reloaded.get(&t), 2);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn discovery_prefers_unplayed() {
        let mut store = PlayCountStore::default();
        let hot = track("hot");
        let cold = track("cold");
        store.counts.insert(track_key(&hot), 12);
        assert!(store.weight(&cold, false) > store.weight(&hot, false));
        assert!(store.weight(&hot, true) > store.weight(&cold, true));
    }

    #[test]
    fn corrupt_json_keeps_path() {
        let dir = std::env::temp_dir().join(format!("tiders-pc-bad-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("playcounts.json");
        std::fs::write(&path, "{not json").unwrap();
        let mut store = PlayCountStore::load_or_empty(&path);
        assert_eq!(store.path, path);
        let t = track("kept");
        store.bump(&t);
        assert_eq!(PlayCountStore::load(&path).unwrap().get(&t), 1);
        let _ = std::fs::remove_file(&path);
    }
}
