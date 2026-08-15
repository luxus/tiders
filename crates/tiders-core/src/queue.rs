//! The play queue and its cursor.
//!
//! Deliberately front-end agnostic and free of I/O so it is trivial to unit
//! test. The [`Player`](crate::player::Player) owns a `Queue` and advances it as
//! tracks finish.

use crate::model::TrackView;

/// A single entry in the play queue.
pub type QueueItem = TrackView;

/// An ordered list of tracks plus a "current" cursor.
#[derive(Debug, Clone, Default)]
pub struct Queue {
    items: Vec<QueueItem>,
    cursor: Option<usize>,
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
        Self { items, cursor }
    }

    /// Number of queued items.
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Whether the queue is empty.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// All items, in order.
    pub fn items(&self) -> &[QueueItem] {
        &self.items
    }

    /// Index of the current item, if any.
    pub fn cursor(&self) -> Option<usize> {
        self.cursor
    }

    /// The current item, if any.
    pub fn current(&self) -> Option<&QueueItem> {
        self.cursor.and_then(|i| self.items.get(i))
    }

    /// Append an item to the end; if the queue was empty, point the cursor at it.
    pub fn push(&mut self, item: QueueItem) {
        self.items.push(item);
        if self.cursor.is_none() {
            self.cursor = Some(self.items.len() - 1);
        }
    }

    /// Replace the whole queue and reset the cursor to `start`.
    pub fn replace(&mut self, items: Vec<QueueItem>, start: usize) {
        *self = Queue::from_tracks(items, start);
    }

    /// Clear all items and the cursor.
    pub fn clear(&mut self) {
        self.items.clear();
        self.cursor = None;
    }

    /// Point the cursor at `index` if it is in range; returns the item.
    pub fn set_cursor(&mut self, index: usize) -> Option<&QueueItem> {
        if index < self.items.len() {
            self.cursor = Some(index);
            self.items.get(index)
        } else {
            None
        }
    }

    /// Advance to the next item; returns it, or `None` at the end.
    pub fn advance(&mut self) -> Option<&QueueItem> {
        let next = match self.cursor {
            Some(i) if i + 1 < self.items.len() => i + 1,
            _ => return None,
        };
        self.cursor = Some(next);
        self.items.get(next)
    }

    /// Step back to the previous item; returns it, or `None` at the start.
    pub fn previous(&mut self) -> Option<&QueueItem> {
        let prev = match self.cursor {
            Some(i) if i > 0 => i - 1,
            _ => return None,
        };
        self.cursor = Some(prev);
        self.items.get(prev)
    }

    /// Whether a next item exists.
    pub fn has_next(&self) -> bool {
        matches!(self.cursor, Some(i) if i + 1 < self.items.len())
    }

    /// Whether a previous item exists.
    pub fn has_previous(&self) -> bool {
        matches!(self.cursor, Some(i) if i > 0)
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
            album: None,
            duration_secs: 100,
            explicit: false,
            cover: None,
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
        // cursor stays on the first until we advance
        assert_eq!(q.current().map(|t| t.id), Some(1));
        assert!(q.has_next());
    }

    #[test]
    fn advance_and_previous_walk_the_list() {
        let mut q = Queue::from_tracks(vec![track(1), track(2), track(3)], 0);
        assert_eq!(q.current().map(|t| t.id), Some(1));
        assert_eq!(q.advance().map(|t| t.id), Some(2));
        assert_eq!(q.advance().map(|t| t.id), Some(3));
        assert!(q.advance().is_none()); // end of queue
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
}
