//! Trimmed-down "view models" the front-ends render.
//!
//! `tidlers` returns rich API structs; the TUI/CLI only need a handful of
//! fields, so we normalise them into small `Copy`/`Clone`-friendly shapes and
//! keep the mapping in one place.

use serde::{Deserialize, Serialize};

use crate::format;

/// A track reduced to what a list row and the now-playing bar need.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackView {
    pub id: u64,
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub duration_secs: u64,
    pub explicit: bool,
    /// TIDAL cover-art id (dashes); resolve with [`TrackView::cover_url`].
    pub cover: Option<String>,
}

impl TrackView {
    /// `"Artist — Title"`.
    pub fn label(&self) -> String {
        format!("{} — {}", self.artist, self.title)
    }

    /// `M:SS` duration string.
    pub fn duration(&self) -> String {
        format::duration(self.duration_secs)
    }

    /// Public cover-art URL at `size`×`size` (e.g. 80, 160, 320, 640, 1280).
    pub fn cover_url(&self, size: u32) -> Option<String> {
        self.cover
            .as_deref()
            .map(|id| crate::images::cover_url(id, size))
    }
}

impl From<&tidlers::client::models::track::Track> for TrackView {
    fn from(t: &tidlers::client::models::track::Track) -> Self {
        let artist = if t.artists.is_empty() {
            t.artist.name.clone()
        } else {
            format::artists(&t.artists.iter().map(|a| a.name.clone()).collect::<Vec<_>>())
        };
        TrackView {
            id: t.id,
            title: t.title.clone(),
            artist,
            album: t.album.as_ref().map(|a| a.title.clone()),
            duration_secs: t.duration,
            explicit: t.explicit,
            cover: t.album.as_ref().and_then(|a| a.cover.clone()),
        }
    }
}

impl From<tidlers::client::models::track::Track> for TrackView {
    fn from(t: tidlers::client::models::track::Track) -> Self {
        (&t).into()
    }
}

impl From<&tidlers::client::models::search::SearchTrackHit> for TrackView {
    fn from(h: &tidlers::client::models::search::SearchTrackHit) -> Self {
        let names: Vec<String> = h.artists.iter().filter_map(|a| a.name.clone()).collect();
        TrackView {
            id: h.id,
            title: h.title.clone(),
            artist: format::artists(&names),
            album: h.album.as_ref().map(|a| a.title.clone()),
            duration_secs: h.duration,
            explicit: h.explicit,
            cover: h.album.as_ref().map(|a| a.cover.clone()),
        }
    }
}

/// An album search hit reduced for display.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlbumView {
    pub id: u64,
    pub title: String,
    pub artist: String,
}

impl From<&tidlers::client::models::search::SearchAlbumHit> for AlbumView {
    fn from(h: &tidlers::client::models::search::SearchAlbumHit) -> Self {
        let names: Vec<String> = h.artists.iter().filter_map(|a| a.name.clone()).collect();
        AlbumView {
            id: h.id,
            title: h.title.clone(),
            artist: format::artists(&names),
        }
    }
}

/// An artist search hit reduced for display.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtistView {
    pub id: u64,
    pub name: String,
}

impl From<&tidlers::client::models::search::SearchArtistHit> for ArtistView {
    fn from(h: &tidlers::client::models::search::SearchArtistHit) -> Self {
        ArtistView {
            id: h.id,
            name: h.name.clone(),
        }
    }
}

/// A playlist search hit reduced for display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaylistView {
    pub uuid: String,
    pub title: String,
    pub tracks: u32,
}

impl From<&tidlers::client::models::search::SearchPlaylistHit> for PlaylistView {
    fn from(h: &tidlers::client::models::search::SearchPlaylistHit) -> Self {
        PlaylistView {
            uuid: h.uuid.clone(),
            title: h.title.clone(),
            tracks: h.number_of_tracks.unwrap_or(0),
        }
    }
}

/// Normalised bundle of search results for the front-ends.
#[derive(Debug, Clone, Default)]
pub struct SearchResults {
    pub tracks: Vec<TrackView>,
    pub albums: Vec<AlbumView>,
    pub artists: Vec<ArtistView>,
    pub playlists: Vec<PlaylistView>,
}

impl From<&tidlers::client::models::search::SearchResultsResponse> for SearchResults {
    fn from(r: &tidlers::client::models::search::SearchResultsResponse) -> Self {
        SearchResults {
            tracks: r
                .tracks
                .as_ref()
                .map(|s| s.items.iter().map(TrackView::from).collect())
                .unwrap_or_default(),
            albums: r
                .albums
                .as_ref()
                .map(|s| s.items.iter().map(AlbumView::from).collect())
                .unwrap_or_default(),
            artists: r
                .artists
                .as_ref()
                .map(|s| s.items.iter().map(ArtistView::from).collect())
                .unwrap_or_default(),
            playlists: r
                .playlists
                .as_ref()
                .map(|s| s.items.iter().map(PlaylistView::from).collect())
                .unwrap_or_default(),
        }
    }
}

impl SearchResults {
    /// Total number of hits across all sections.
    pub fn total(&self) -> usize {
        self.tracks.len() + self.albums.len() + self.artists.len() + self.playlists.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn track_view_label_and_duration() {
        let tv = TrackView {
            id: 1,
            title: "Digital Love".into(),
            artist: "Daft Punk".into(),
            album: Some("Discovery".into()),
            duration_secs: 301,
            explicit: false,
            cover: None,
        };
        assert_eq!(tv.label(), "Daft Punk — Digital Love");
        assert_eq!(tv.duration(), "5:01");
    }
}
