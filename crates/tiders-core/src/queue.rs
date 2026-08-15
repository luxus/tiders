//! The play queue, its cursor, and shuffle / repeat modes.
//!
//! Deliberately front-end agnostic and free of I/O so it is trivial to unit
//! test. The [`Player`](crate::player::Player) owns a `Queue` and advances it as
//! tracks finish.

use serde::{Deserialize, Serialize};

use crate::model::TrackView;
use crate::playcount::PlayCountStore;

/// A single entry in the play queue.
pub type QueueItem = TrackView;

/// Shuffle behaviour. Cycle with `s` in the TUI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ShuffleMode {
    #[default]
    Off,
    /// Flat random order.
    Random,
    /// Weight by local play count (more-played earlier).
    Favourites,
    /// Weight inversely by play count (unplayed earlier).
    Discovery,
}

impl ShuffleMode {
    pub fn cycle(self) -> Self {
        match self {
            ShuffleMode::Off => ShuffleMode::Random,
            ShuffleMode::Random => ShuffleMode::Favourites,
            ShuffleMode::Favourites => ShuffleMode::Discovery,
            ShuffleMode::Discovery => ShuffleMode::Off,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ShuffleMode::Off => "off",
            ShuffleMode::Random => "random",
            ShuffleMode::Favourites => "favourites",
            ShuffleMode::Discovery => "discovery",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            ShuffleMode::Off => "⇄",
            ShuffleMode::Random => "⇄",
            ShuffleMode::Favourites => "★",
            ShuffleMode::Discovery => "⊕",
        }
    }
}

/// Repeat behaviour. Cycle with `r` in the TUI: off → all → one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RepeatMode {
    #[default]
    Off,
    /// Repeat the whole queue (wraps; reshuffles when shuffle is on).
    All,
    /// Repeat the current track.
    One,
}

impl RepeatMode {
    pub fn cycle(self) -> Self {
        match self {
            RepeatMode::Off => RepeatMode::All,
            RepeatMode::All => RepeatMode::One,
            RepeatMode::One => RepeatMode::Off,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            RepeatMode::Off => "off",
            RepeatMode::All => "all",
            RepeatMode::One => "one",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            RepeatMode::Off => "↷",
            RepeatMode::All => "🔁",
            RepeatMode::One => "🔂",
        }
    }
}

/// An ordered list of tracks plus a "current" cursor and play-order overlay.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Queue {
    items: Vec<QueueItem>,
    cursor: Option<usize>,
    #[serde(default)]
    shuffle: ShuffleMode,
    #[serde(default)]
    repeat: RepeatMode,
    /// Play order when shuffle is on: indices into `items`.
    #[serde(default)]
    order: Vec<usize>,
}

impl Queue {
    /// An empty queue.
    pub fn new() -> Self {
        Self::default()
    }

    /// Build a queue from tracks, starting the cursor at `start` (clamped).
    pub fn from_tracks(items: Vec<QueueItem>, start: usize) -> Self {
        let cursor = if items.is_empty() {
            None
        } else {
            Some(start.min(items.len() - 1))
        };
        Self {
            items,
            cursor,
            shuffle: ShuffleMode::Off,
            repeat: RepeatMode::Off,
            order: Vec::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn items(&self) -> &[QueueItem] {
        &self.items
    }

    pub fn cursor(&self) -> Option<usize> {
        self.cursor
    }

    pub fn current(&self) -> Option<&QueueItem> {
        self.cursor.and_then(|i| self.items.get(i))
    }

    pub fn shuffle(&self) -> ShuffleMode {
        self.shuffle
    }

    pub fn repeat(&self) -> RepeatMode {
        self.repeat
    }

    pub fn set_repeat(&mut self, repeat: RepeatMode) {
        self.repeat = repeat;
    }

    /// Append an item to the end; if the queue was empty, point the cursor at it.
    pub fn push(&mut self, item: QueueItem) {
        self.items.push(item);
        if self.cursor.is_none() {
            self.cursor = Some(self.items.len() - 1);
        }
        if self.shuffle != ShuffleMode::Off {
            self.order.push(self.items.len() - 1);
        }
    }

    /// Append many items without moving the cursor (unless the queue was empty).
    pub fn extend(&mut self, items: impl IntoIterator<Item = QueueItem>) {
        for item in items {
            self.push(item);
        }
    }

    /// Replace the whole queue and reset the cursor to `start`.
    pub fn replace(&mut self, items: Vec<QueueItem>, start: usize) {
        let shuffle = self.shuffle;
        let repeat = self.repeat;
        *self = Queue::from_tracks(items, start);
        self.repeat = repeat;
        self.shuffle = ShuffleMode::Off;
        if shuffle != ShuffleMode::Off {
            // Caller should call `apply_shuffle` with a play-count store.
            self.shuffle = shuffle;
            self.rebuild_order(None);
        }
    }

    pub fn clear(&mut self) {
        self.items.clear();
        self.cursor = None;
        self.order.clear();
    }

    pub fn set_cursor(&mut self, index: usize) -> Option<&QueueItem> {
        if index < self.items.len() {
            self.cursor = Some(index);
            self.items.get(index)
        } else {
            None
        }
    }

    /// Apply (or rebuild) shuffle. `counts` is used for weighted modes.
    pub fn apply_shuffle(&mut self, mode: ShuffleMode, counts: Option<&PlayCountStore>) {
        self.shuffle = mode;
        self.rebuild_order(counts);
    }

    fn rebuild_order(&mut self, counts: Option<&PlayCountStore>) {
        self.order.clear();
        if self.shuffle == ShuffleMode::Off || self.items.is_empty() {
            return;
        }
        let current = self.cursor.unwrap_or(0);
        let mut rest: Vec<usize> = (0..self.items.len()).filter(|&i| i != current).collect();
        match self.shuffle {
            ShuffleMode::Off => {}
            ShuffleMode::Random => {
                shuffle_indices(&mut rest, None, &self.items);
            }
            ShuffleMode::Favourites => {
                shuffle_indices(&mut rest, counts.map(|c| (c, true)), &self.items);
            }
            ShuffleMode::Discovery => {
                shuffle_indices(&mut rest, counts.map(|c| (c, false)), &self.items);
            }
        }
        self.order.push(current);
        self.order.extend(rest);
    }

    fn position_in_order(&self) -> Option<usize> {
        let cur = self.cursor?;
        if self.order.is_empty() {
            Some(cur)
        } else {
            self.order.iter().position(|&i| i == cur)
        }
    }

    /// Advance to the next item honouring repeat/shuffle; returns it.
    pub fn advance(&mut self) -> Option<&QueueItem> {
        if self.items.is_empty() {
            return None;
        }
        if self.repeat == RepeatMode::One {
            return self.current();
        }
        if self.shuffle != ShuffleMode::Off && !self.order.is_empty() {
            let pos = self.position_in_order().unwrap_or(0);
            if pos + 1 < self.order.len() {
                self.cursor = Some(self.order[pos + 1]);
                return self.current();
            }
            if self.repeat == RepeatMode::All {
                self.rebuild_order(None);
                self.cursor = self.order.first().copied();
                return self.current();
            }
            return None;
        }
        match self.cursor {
            Some(i) if i + 1 < self.items.len() => {
                self.cursor = Some(i + 1);
                self.current()
            }
            Some(_) if self.repeat == RepeatMode::All => {
                self.cursor = Some(0);
                self.current()
            }
            _ => None,
        }
    }

    /// Step back to the previous item; returns it, or `None` at the start
    /// (unless repeat-all, which wraps).
    pub fn previous(&mut self) -> Option<&QueueItem> {
        if self.items.is_empty() {
            return None;
        }
        if self.shuffle != ShuffleMode::Off && !self.order.is_empty() {
            let pos = self.position_in_order().unwrap_or(0);
            if pos > 0 {
                self.cursor = Some(self.order[pos - 1]);
                return self.current();
            }
            if self.repeat == RepeatMode::All {
                self.cursor = self.order.last().copied();
                return self.current();
            }
            return None;
        }
        match self.cursor {
            Some(i) if i > 0 => {
                self.cursor = Some(i - 1);
                self.current()
            }
            Some(_) if self.repeat == RepeatMode::All => {
                self.cursor = Some(self.items.len() - 1);
                self.current()
            }
            _ => None,
        }
    }

    pub fn has_next(&self) -> bool {
        if self.items.is_empty() {
            return false;
        }
        if self.repeat != RepeatMode::Off {
            return true;
        }
        if self.shuffle != ShuffleMode::Off && !self.order.is_empty() {
            return self
                .position_in_order()
                .map(|p| p + 1 < self.order.len())
                .unwrap_or(false);
        }
        matches!(self.cursor, Some(i) if i + 1 < self.items.len())
    }

    pub fn has_previous(&self) -> bool {
        if self.items.is_empty() {
            return false;
        }
        if self.repeat == RepeatMode::All {
            return true;
        }
        if self.shuffle != ShuffleMode::Off && !self.order.is_empty() {
            return self.position_in_order().map(|p| p > 0).unwrap_or(false);
        }
        matches!(self.cursor, Some(i) if i > 0)
    }
}

fn shuffle_indices(
    rest: &mut [usize],
    weighted: Option<(&PlayCountStore, bool)>,
    items: &[QueueItem],
) {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    if let Some((store, favourites)) = weighted {
        // Weighted random sample without replacement (Efraimidis–Spirakis).
        let mut keyed: Vec<(f64, usize)> = rest
            .iter()
            .map(|&i| {
                let w = store.weight(&items[i], favourites).max(1e-9);
                let u: f64 = rng.gen::<f64>().max(1e-12);
                let key = u.powf(1.0 / w);
                (key, i)
            })
            .collect();
        keyed.sort_by(|a, b| b.0.total_cmp(&a.0));
        for (slot, (_, i)) in rest.iter_mut().zip(keyed) {
            *slot = i;
        }
    } else {
        for i in (1..rest.len()).rev() {
            let j = rng.gen_range(0..=i);
            rest.swap(i, j);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(id: u64) -> QueueItem {
        TrackView {
            id,
            title: format!("t{id}"),
            artist: "artist".into(),
            ..TrackView::default()
        }
    }

    #[test]
    fn empty_queue_has_no_cursor() {
        let q = Queue::new();
        assert!(q.is_empty());
        assert!(q.current().is_none());
        assert!(!q.has_next());
        assert!(!q.has_previous());
    }

    #[test]
    fn push_sets_cursor_on_first_item() {
        let mut q = Queue::new();
        q.push(track(1));
        assert_eq!(q.current().map(|t| t.id), Some(1));
        q.push(track(2));
        assert_eq!(q.current().map(|t| t.id), Some(1));
        assert!(q.has_next());
    }

    #[test]
    fn advance_and_previous_walk_the_list() {
        let mut q = Queue::from_tracks(vec![track(1), track(2), track(3)], 0);
        assert_eq!(q.current().map(|t| t.id), Some(1));
        assert_eq!(q.advance().map(|t| t.id), Some(2));
        assert_eq!(q.advance().map(|t| t.id), Some(3));
        assert!(q.advance().is_none());
        assert_eq!(q.current().map(|t| t.id), Some(3));
        assert_eq!(q.previous().map(|t| t.id), Some(2));
        assert_eq!(q.previous().map(|t| t.id), Some(1));
        assert!(q.previous().is_none());
    }

    #[test]
    fn from_tracks_clamps_start() {
        let q = Queue::from_tracks(vec![track(1), track(2)], 99);
        assert_eq!(q.current().map(|t| t.id), Some(2));
    }

    #[test]
    fn set_cursor_out_of_range_is_ignored() {
        let mut q = Queue::from_tracks(vec![track(1)], 0);
        assert!(q.set_cursor(5).is_none());
        assert_eq!(q.cursor(), Some(0));
    }

    #[test]
    fn repeat_one_stays_on_the_same_track() {
        let mut q = Queue::from_tracks(vec![track(1), track(2)], 0);
        q.set_repeat(RepeatMode::One);
        assert_eq!(q.advance().map(|t| t.id), Some(1));
        assert_eq!(q.advance().map(|t| t.id), Some(1));
    }

    #[test]
    fn repeat_all_wraps() {
        let mut q = Queue::from_tracks(vec![track(1), track(2)], 1);
        q.set_repeat(RepeatMode::All);
        assert_eq!(q.advance().map(|t| t.id), Some(1));
        assert_eq!(q.previous().map(|t| t.id), Some(2));
    }

    #[test]
    fn shuffle_keeps_current_first_then_visits_the_rest() {
        let mut q = Queue::from_tracks(vec![track(1), track(2), track(3), track(4)], 0);
        q.apply_shuffle(ShuffleMode::Random, None);
        assert_eq!(q.current().map(|t| t.id), Some(1));
        let mut seen = vec![1u64];
        while let Some(t) = q.advance() {
            seen.push(t.id);
            if seen.len() > 8 {
                break;
            }
        }
        assert_eq!(seen.len(), 4);
        seen.sort();
        assert_eq!(seen, vec![1, 2, 3, 4]);
    }
}
