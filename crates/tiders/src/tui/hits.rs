//! Clickable regions recorded during draw and hit-tested on mouse events.

use ratatui::layout::{Position, Rect};

use super::app::{FavSection, LibSection, Tab};

#[derive(Debug, Clone, Copy)]
pub enum Hit {
    Tab(Tab),
    Lib(LibSection),
    Fav(FavSection),
    ListRow(usize),
    Progress,
}

#[derive(Debug, Default)]
pub struct HitMap {
    pub tabs: Vec<(Rect, Tab)>,
    pub lib: Vec<(Rect, LibSection)>,
    pub fav: Vec<(Rect, FavSection)>,
    pub list: Option<Rect>,
    pub list_offset: usize,
    pub progress: Option<Rect>,
}

impl HitMap {
    pub fn clear(&mut self) {
        self.tabs.clear();
        self.lib.clear();
        self.fav.clear();
        self.list = None;
        self.list_offset = 0;
        self.progress = None;
    }

    pub fn at(&self, col: u16, row: u16) -> Option<Hit> {
        let pos = Position { x: col, y: row };
        for (rect, tab) in &self.tabs {
            if rect.contains(pos) {
                return Some(Hit::Tab(*tab));
            }
        }
        for (rect, sec) in &self.lib {
            if rect.contains(pos) {
                return Some(Hit::Lib(*sec));
            }
        }
        for (rect, sec) in &self.fav {
            if rect.contains(pos) {
                return Some(Hit::Fav(*sec));
            }
        }
        if let Some(bar) = self.progress {
            if bar.contains(pos) {
                return Some(Hit::Progress);
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
    pub fn progress_ratio(&self, col: u16) -> Option<f64> {
        let bar = self.progress?;
        if bar.width == 0 {
            return None;
        }
        let x = col.saturating_sub(bar.x) as f64;
        Some((x / bar.width as f64).clamp(0.0, 1.0))
    }
}
