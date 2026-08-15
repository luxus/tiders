//! Album-art loading and terminal rendering.
//!
//! Uses [`ratatui_image`], which renders through the terminal's native graphics
//! protocol (sixel / kitty / iTerm2) when available and otherwise falls back to
//! **unicode half-blocks** — colored ▀ cells that work in any truecolor
//! terminal. Set `TIDERS_IMAGE_PROTOCOL=halfblocks|sixel|kitty|iterm2` to force
//! a specific protocol (handy for testing where auto-detection can't probe).

use std::collections::HashMap;

use ratatui_image::picker::{Picker, ProtocolType};
use ratatui_image::protocol::StatefulProtocol;

use tiders_core::images;

/// Fetches, decodes, caches, and builds render protocols for cover art.
pub struct ArtManager {
    picker: Picker,
    /// Decoded images keyed by TIDAL cover id (fetched once, reused).
    cache: HashMap<String, image::DynamicImage>,
}

impl ArtManager {
    /// Build the manager, detecting the terminal's image capability.
    ///
    /// Must be constructed **before** entering the alternate screen / raw mode,
    /// since capability detection talks to the terminal on stdio.
    pub fn new() -> Self {
        // Auto-detect; fall back to a sane font size (→ half-blocks) if the
        // terminal can't be queried (e.g. output is piped).
        let mut picker =
            Picker::from_query_stdio().unwrap_or_else(|_| Picker::from_fontsize((8, 16)));

        if let Ok(forced) = std::env::var("TIDERS_IMAGE_PROTOCOL") {
            if let Some(pt) = parse_protocol(&forced) {
                picker.set_protocol_type(pt);
            }
        }

        Self {
            picker,
            cache: HashMap::new(),
        }
    }

    /// The active protocol type (for display in the UI).
    pub fn protocol_label(&self) -> &'static str {
        match self.picker.protocol_type() {
            ProtocolType::Halfblocks => "halfblocks",
            ProtocolType::Sixel => "sixel",
            ProtocolType::Kitty => "kitty",
            ProtocolType::Iterm2 => "iterm2",
        }
    }

    /// Get a fresh render protocol for a cover id, fetching+decoding on a miss.
    ///
    /// Returns `None` if the cover can't be fetched or decoded. Each call builds
    /// a new [`StatefulProtocol`] from the cached image so independent widgets
    /// (now-playing thumbnail, detail popup) can size it independently.
    pub async fn protocol_for(&mut self, cover_id: &str) -> Option<StatefulProtocol> {
        if !self.cache.contains_key(cover_id) {
            let url = images::cover_url(cover_id, 640);
            let bytes = images::fetch(&url).await.ok()?;
            let image = image::load_from_memory(&bytes).ok()?;
            self.cache.insert(cover_id.to_string(), image);
        }
        let image = self.cache.get(cover_id)?.clone();
        Some(self.picker.new_resize_protocol(image))
    }
}

fn parse_protocol(s: &str) -> Option<ProtocolType> {
    match s.trim().to_ascii_lowercase().as_str() {
        "halfblocks" | "half" | "blocks" => Some(ProtocolType::Halfblocks),
        "sixel" => Some(ProtocolType::Sixel),
        "kitty" => Some(ProtocolType::Kitty),
        "iterm2" | "iterm" => Some(ProtocolType::Iterm2),
        _ => None,
    }
}
