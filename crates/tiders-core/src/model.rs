//! Trimmed-down "view models" the front-ends render.
//!
//! `tidlers` returns rich API structs; the TUI/CLI only need a handful of
//! fields, so we normalise them into small `Copy`/`Clone`-friendly shapes and
//! keep the mapping in one place.

use serde::{Deserialize, Serialize};

use crate::format;

/// A track reduced to what a list row and the now-playing bar need.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TrackView {
    pub id: u64,
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub duration_secs: u64,
    pub explicit: bool,
    /// TIDAL cover-art id (dashes); resolve with [`TrackView::cover_url`].
    pub cover: Option<String>,
    /// TIDAL quality tag (`LOW` / `HIGH` / `LOSSLESS` / `HI_RES` / `HI_RES_LOSSLESS`).
    pub audio_quality: Option<String>,
    pub bpm: Option<f32>,
    pub album_id: Option<u64>,
    pub artist_id: Option<u64>,
    /// Mix id for "radio from this track", if TIDAL supplied one.
    pub mix_id: Option<String>,
}

impl Default for TrackView {
    fn default() -> Self {
        Self {
            id: 0,
            title: String::new(),
            artist: String::new(),
            album: None,
            duration_secs: 0,
            explicit: false,
            cover: None,
            audio_quality: None,
            bpm: None,
            album_id: None,
            artist_id: None,
            mix_id: None,
        }
    }
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

    /// Compact quality badge, e.g. `HIRES` / `FLAC` / `AAC`.
    pub fn quality_badge(&self) -> Option<&'static str> {
        format::quality_badge(self.audio_quality.as_deref())
    }
}

fn mix_id_from(map: &Option<std::collections::HashMap<String, String>>) -> Option<String> {
    map.as_ref().and_then(|m| {
        m.get("TRACK_MIX")
            .or_else(|| m.get("trackMix"))
            .cloned()
            .or_else(|| m.values().next().cloned())
    })
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
            audio_quality: Some(t.audio_quality.clone()),
            bpm: t.bpm,
            album_id: t.album.as_ref().map(|a| a.id as u64),
            artist_id: Some(t.artist.id),
            mix_id: mix_id_from(&t.mixes),
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
            audio_quality: h.audio_quality.clone(),
            bpm: None,
            album_id: h.album.as_ref().map(|a| a.id),
            artist_id: h.artists.first().and_then(|a| a.id),
            mix_id: mix_id_from(&h.mixes),
        }
    }
}

/// An album search hit reduced for display.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlbumView {
    pub id: u64,
    pub title: String,
    pub artist: String,
    pub cover: Option<String>,
    pub release_date: Option<String>,
    pub tracks: Option<u32>,
}

impl From<&tidlers::client::models::search::SearchAlbumHit> for AlbumView {
    fn from(h: &tidlers::client::models::search::SearchAlbumHit) -> Self {
        let names: Vec<String> = h.artists.iter().filter_map(|a| a.name.clone()).collect();
        AlbumView {
            id: h.id,
            title: h.title.clone(),
            artist: format::artists(&names),
            cover: h.cover.clone(),
            release_date: h.release_date.clone(),
            tracks: h.number_of_tracks,
        }
    }
}

impl From<&tidlers::client::models::album::Album> for AlbumView {
    fn from(a: &tidlers::client::models::album::Album) -> Self {
        AlbumView {
            id: a.id as u64,
            title: a.title.clone(),
            artist: String::new(),
            cover: a.cover.clone(),
            release_date: a.release_date.clone(),
            tracks: None,
        }
    }
}

/// An artist search hit reduced for display.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtistView {
    pub id: u64,
    pub name: String,
    pub picture: Option<String>,
    pub mix_id: Option<String>,
}

impl From<&tidlers::client::models::search::SearchArtistHit> for ArtistView {
    fn from(h: &tidlers::client::models::search::SearchArtistHit) -> Self {
        ArtistView {
            id: h.id,
            name: h.name.clone(),
            picture: h.picture.clone(),
            mix_id: mix_id_from(&h.mixes),
        }
    }
}

/// A playlist search hit reduced for display.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlaylistView {
    pub uuid: String,
    pub title: String,
    pub tracks: u32,
    pub cover: Option<String>,
    pub description: Option<String>,
}

impl From<&tidlers::client::models::search::SearchPlaylistHit> for PlaylistView {
    fn from(h: &tidlers::client::models::search::SearchPlaylistHit) -> Self {
        PlaylistView {
            uuid: h.uuid.clone(),
            title: h.title.clone(),
            tracks: h.number_of_tracks.unwrap_or(0),
            cover: h.square_image.clone().or_else(|| h.image.clone()),
            description: h.description.clone(),
        }
    }
}

/// A TIDAL Mix (My Mix, radio mix, arrival mix, …).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MixView {
    pub id: String,
    pub title: String,
    pub subtitle: String,
    pub cover_url: Option<String>,
}

/// A "For You" / home-feed card that can be opened.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HomeCard {
    pub title: String,
    pub subtitle: String,
    pub kind: HomeCardKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum HomeCardKind {
    Mix { id: String },
    Playlist { uuid: String },
    Album { id: u64 },
    Artist { id: u64 },
}

/// Details of the stream currently (or about to be) decoded.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamQuality {
    pub audio_quality: Option<String>,
    pub mime_type: Option<String>,
    pub codecs: Option<String>,
    pub sample_rate_hz: Option<u32>,
    pub bit_depth: Option<u8>,
    pub channels: Option<u8>,
    pub bitrate_bps: Option<u32>,
}

impl StreamQuality {
    /// Human label like `FLAC 24-bit 96 kHz` or `AAC 320 kbps`.
    pub fn label(&self) -> String {
        format::stream_quality_label(self)
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
            audio_quality: Some("LOSSLESS".into()),
            bpm: Some(124.0),
            album_id: None,
            artist_id: None,
            mix_id: None,
        };
        assert_eq!(tv.label(), "Daft Punk — Digital Love");
        assert_eq!(tv.duration(), "5:01");
        assert_eq!(tv.quality_badge(), Some("FLAC"));
    }
}
