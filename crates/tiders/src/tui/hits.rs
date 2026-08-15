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
}
