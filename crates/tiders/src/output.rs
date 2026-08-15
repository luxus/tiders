//! Pretty, plain-text output for the CLI subcommands.
//!
//! Kept intentionally dependency-free and terminal-agnostic (no ANSI) so it
//! reads well in pipes and logs. The TUI is where the colour lives.

use tiders_core::model::{AlbumView, ArtistView, PlaylistView, SearchResults, TrackView};

/// Print a numbered list of tracks under a heading.
pub fn print_tracks(heading: &str, tracks: &[TrackView]) {
    println!("\n{heading} ({})", tracks.len());
    if tracks.is_empty() {
        println!("  (none)");
        return;
    }
    let width = number_width(tracks.len());
    for (i, t) in tracks.iter().enumerate() {
        let explicit = if t.explicit { " [E]" } else { "" };
        let album = t
            .album
            .as_deref()
            .map(|a| format!("  · {a}"))
            .unwrap_or_default();
        println!(
            "  {:>width$}. {}  —  {}{}  [{}]{}   (id {})",
            i + 1,
            t.title,
            t.artist,
            album,
            t.duration(),
            explicit,
            t.id,
            width = width,
        );
    }
}

/// Print albums.
pub fn print_albums(albums: &[AlbumView]) {
    println!("\nAlbums ({})", albums.len());
    for (i, a) in albums.iter().enumerate() {
        println!(
            "  {:>2}. {}  —  {}   (id {})",
            i + 1,
            a.title,
            a.artist,
            a.id
        );
    }
}

/// Print artists.
pub fn print_artists(artists: &[ArtistView]) {
    println!("\nArtists ({})", artists.len());
    for (i, a) in artists.iter().enumerate() {
        println!("  {:>2}. {}   (id {})", i + 1, a.name, a.id);
    }
}

/// Print playlists.
pub fn print_playlists(playlists: &[PlaylistView]) {
    println!("\nPlaylists ({})", playlists.len());
    if playlists.is_empty() {
        println!("  (none)");
        return;
    }
    for (i, p) in playlists.iter().enumerate() {
        println!(
            "  {:>2}. {}  ({} tracks)   (uuid {})",
            i + 1,
            p.title,
            p.tracks,
            p.uuid
        );
    }
}

/// Print a full search result set.
pub fn print_search(results: &SearchResults) {
    if results.total() == 0 {
        println!("No results.");
        return;
    }
    print_tracks("Tracks", &results.tracks);
    if !results.albums.is_empty() {
        print_albums(&results.albums);
    }
    if !results.artists.is_empty() {
        print_artists(&results.artists);
    }
    if !results.playlists.is_empty() {
        print_playlists(&results.playlists);
    }
    println!();
}

fn number_width(len: usize) -> usize {
    len.to_string().len().max(2)
}
