//! Clickable regions recorded during draw and hit-tested on mouse events.

use ratatui::layout::{Position, Rect};

use super::app::{ContextAction, FavSection, SortKey, Tab};

#[derive(Debug, Clone, Copy)]
pub enum Hit {
    Tab(Tab),
    Fav(FavSection),
    ListRow(usize),
    QueueRow(usize),
    AlbumRow(usize),
    Playlist(usize),
    Progress,
    Sort(SortKey),
    SidebarToggle,
    UpNext,
    ContextItem(usize),
}

#[derive(Debug, Default)]
pub struct HitMap {
    pub tabs: Vec<(Rect, Tab)>,
    pub fav: Vec<(Rect, FavSection)>,
    pub list: Option<Rect>,
    pub list_offset: usize,
    pub queue: Option<Rect>,
    pub queue_offset: usize,
    pub albums: Vec<(Rect, usize)>,
    pub playlists: Vec<(Rect, usize)>,
    pub progress: Option<Rect>,
    pub sorts: Vec<(Rect, SortKey)>,
    pub sidebar_toggle: Option<Rect>,
    pub up_next: Option<Rect>,
    pub context: Vec<(Rect, usize)>,
}

impl HitMap {
    pub fn clear(&mut self) {
        self.tabs.clear();
        self.fav.clear();
        self.list = None;
        self.list_offset = 0;
        self.queue = None;
        self.queue_offset = 0;
        self.albums.clear();
        self.playlists.clear();
        self.progress = None;
        self.sorts.clear();
        self.sidebar_toggle = None;
        self.up_next = None;
        self.context.clear();
    }

    pub fn at(&self, col: u16, row: u16) -> Option<Hit> {
        let pos = Position { x: col, y: row };
        for (rect, i) in &self.context {
            if rect.contains(pos) {
                return Some(Hit::ContextItem(*i));
            }
        }
        if let Some(toggle) = self.sidebar_toggle {
            if toggle.contains(pos) {
                return Some(Hit::SidebarToggle);
            }
        }
        for (rect, tab) in &self.tabs {
            if rect.contains(pos) {
                return Some(Hit::Tab(*tab));
            }
        }
        for (rect, sec) in &self.fav {
            if rect.contains(pos) {
                return Some(Hit::Fav(*sec));
            }
        }
        for (rect, key) in &self.sorts {
            if rect.contains(pos) {
                return Some(Hit::Sort(*key));
            }
        }
        for (rect, i) in &self.albums {
            if rect.contains(pos) {
                return Some(Hit::AlbumRow(*i));
            }
        }
        for (rect, i) in &self.playlists {
            if rect.contains(pos) {
                return Some(Hit::Playlist(*i));
            }
        }
        if let Some(bar) = self.progress {
            if bar.contains(pos) {
                return Some(Hit::Progress);
            }
        }
        if let Some(up) = self.up_next {
            if up.contains(pos) {
                return Some(Hit::UpNext);
            }
        }
        if let Some(queue) = self.queue {
            if queue.contains(pos) {
                let idx = self.queue_offset + (row.saturating_sub(queue.y)) as usize;
                return Some(Hit::QueueRow(idx));
            }
        }
        if let Some(list) = self.list {
            if list.contains(pos) {
                let idx = self.list_offset + (row.saturating_sub(list.y)) as usize;
                return Some(Hit::ListRow(idx));
            }
        }
        None
    }

    /// `0.0..=1.0` along the progress bar, or `None` if the click missed it.
    ///
    /// `Rect::contains` is exclusive on the right edge, so the last clickable
    /// column is `x + width - 1`. Map that column to `1.0` so a click at the
    /// end of the bar seeks to the end of the track.
    pub fn progress_ratio(&self, col: u16) -> Option<f64> {
        let bar = self.progress?;
        if bar.width == 0 {
            return None;
        }
        let x = col.saturating_sub(bar.x).min(bar.width.saturating_sub(1));
        if bar.width == 1 {
            return Some(0.0);
        }
        Some(x as f64 / (bar.width as f64 - 1.0))
    }
}

impl ContextAction {
    pub fn label(self, loved: bool) -> &'static str {
        match self {
            ContextAction::GoArtist => "Go to artist",
            ContextAction::GoAlbum => "Go to album",
            ContextAction::ToggleFav if loved => "Remove from favorites",
            ContextAction::ToggleFav => "Add to favorites",
            ContextAction::Like => "Like",
            ContextAction::Dislike => "Don't like",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_ratio_maps_last_cell_to_one() {
        let hits = HitMap {
            progress: Some(Rect::new(10, 4, 11, 1)),
            ..Default::default()
        };
        assert!((hits.progress_ratio(10).unwrap() - 0.0).abs() < 1e-9);
        assert!((hits.progress_ratio(20).unwrap() - 1.0).abs() < 1e-9);
        assert!(hits.progress_ratio(15).unwrap() > 0.4);
        assert!(hits.progress_ratio(15).unwrap() < 0.6);
    }

    #[test]
    fn queue_row_uses_offset() {
        let hits = HitMap {
            queue: Some(Rect::new(0, 10, 20, 5)),
            queue_offset: 8,
            ..Default::default()
        };
        match hits.at(2, 12) {
            Some(Hit::QueueRow(i)) => assert_eq!(i, 10),
            other => panic!("expected queue row, got {other:?}"),
        }
    }

    #[test]
    fn sort_header_beats_list_row() {
        let hits = HitMap {
            list: Some(Rect::new(0, 2, 40, 10)),
            sorts: vec![(Rect::new(10, 2, 8, 1), SortKey::Artist)],
            ..Default::default()
        };
        match hits.at(12, 2) {
            Some(Hit::Sort(SortKey::Artist)) => {}
            other => panic!("expected sort hit, got {other:?}"),
        }
    }

    #[test]
    fn sidebar_playlist_is_clickable() {
        let hits = HitMap {
            playlists: vec![(Rect::new(0, 8, 16, 1), 2)],
            ..Default::default()
        };
        match hits.at(4, 8) {
            Some(Hit::Playlist(2)) => {}
            other => panic!("expected playlist hit, got {other:?}"),
        }
    }
}
