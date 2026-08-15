//! Synced lyrics (LRC) parsing and lookup.

/// One timed lyric line.
#[derive(Debug, Clone, PartialEq)]
pub struct LyricLine {
    /// Offset from the start of the track, in seconds.
    pub timestamp: f64,
    pub text: String,
}

/// Build lyric lines from TIDAL's `lyrics` (often plain text) and `subtitles`
/// (LRC) fields.
///
/// `tidlers` currently drops `subtitles`, so callers should prefer a lenient
/// JSON fetch. This helper still works when only one of the two strings is
/// available: timed LRC wins, otherwise unsynced lines are kept so the TUI can
/// show *something* instead of "no lyrics".
pub fn from_tidal(lyrics: &str, subtitles: Option<&str>) -> Vec<LyricLine> {
    if let Some(sub) = subtitles {
        let timed = parse_lrc(sub);
        if !timed.is_empty() {
            return timed;
        }
    }
    let timed = parse_lrc(lyrics);
    if !timed.is_empty() {
        return timed;
    }
    parse_plain(lyrics)
}

/// Parse LRC-format timed lyrics into a sorted list of lines.
///
/// Un-timed lines are ignored. Empty timestamps (instrumental gaps) are kept
/// when they carry text.
pub fn parse_lrc(lrc: &str) -> Vec<LyricLine> {
    let mut lines = Vec::new();
    for raw in lrc.lines() {
        let raw = raw.trim();
        if raw.is_empty() {
            continue;
        }
        // A line may carry several stamps: `[00:12.00][00:45.00]chorus`
        let mut rest = raw;
        let mut stamps = Vec::new();
        while rest.starts_with('[') {
            let Some(end) = rest.find(']') else {
                break;
            };
            let inner = &rest[1..end];
            rest = &rest[end + 1..];
            if let Some(ts) = parse_stamp(inner) {
                stamps.push(ts);
            } else {
                // Not a timestamp tag (`[ar:]`, `[ti:]`, …) — stop.
                break;
            }
        }
        let text = rest.trim();
        if text.is_empty() {
            continue;
        }
        for timestamp in stamps {
            lines.push(LyricLine {
                timestamp,
                text: text.to_string(),
            });
        }
    }
    lines.sort_by(|a, b| a.timestamp.total_cmp(&b.timestamp));
    lines
}

/// Unsynced lyrics: one line per non-empty row, spaced 8s apart so the
/// highlighter still walks the page instead of pinning the last line.
fn parse_plain(text: &str) -> Vec<LyricLine> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .enumerate()
        .map(|(i, text)| LyricLine {
            timestamp: i as f64 * 8.0,
            text: text.to_string(),
        })
        .collect()
}

fn parse_stamp(s: &str) -> Option<f64> {
    // `mm:ss` or `mm:ss.xx` or `mm:ss.xxx`
    let (mm, ss) = s.split_once(':')?;
    let minutes: f64 = mm.parse().ok()?;
    let seconds: f64 = ss.parse().ok()?;
    Some(minutes * 60.0 + seconds)
}

/// Index of the current lyric line for `position` seconds, or `None` if empty.
pub fn current_line_index(lines: &[LyricLine], position: f64) -> Option<usize> {
    if lines.is_empty() {
        return None;
    }
    let mut idx = 0;
    for (i, line) in lines.iter().enumerate() {
        if line.timestamp <= position {
            idx = i;
        } else {
            break;
        }
    }
    Some(idx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_standard_lrc() {
        let src = "\
[ar:Daft Punk]
[00:12.50]Work it
[00:15.00]Make it
[01:02.250]Harder
";
        let lines = parse_lrc(src);
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0].timestamp, 12.5);
        assert_eq!(lines[0].text, "Work it");
        assert!((lines[2].timestamp - 62.25).abs() < 1e-6);
    }

    #[test]
    fn current_line_tracks_position() {
        let lines = parse_lrc("[00:00.00]a\n[00:10.00]b\n[00:20.00]c\n");
        assert_eq!(current_line_index(&lines, 0.0), Some(0));
        assert_eq!(current_line_index(&lines, 10.0), Some(1));
        assert_eq!(current_line_index(&lines, 19.9), Some(1));
        assert_eq!(current_line_index(&lines, 99.0), Some(2));
        assert_eq!(current_line_index(&[], 1.0), None);
    }

    #[test]
    fn from_tidal_prefers_subtitles_lrc() {
        let lines = from_tidal("plain line\nanother", Some("[00:12.00]Work it"));
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text, "Work it");
        assert_eq!(lines[0].timestamp, 12.0);
    }

    #[test]
    fn from_tidal_falls_back_to_lyrics_lrc() {
        let lines = from_tidal("[00:01.00]Hello", Some(""));
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text, "Hello");
    }

    #[test]
    fn from_tidal_keeps_plain_unsynced_lyrics() {
        let lines = from_tidal("line one\n\nline two", None);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].text, "line one");
        assert_eq!(lines[1].text, "line two");
        assert_eq!(lines[1].timestamp, 8.0);
    }

    #[test]
    fn from_tidal_empty_when_blank() {
        assert!(from_tidal("", None).is_empty());
        assert!(from_tidal("   \n  ", Some("  ")).is_empty());
    }
}
