//! Synced lyrics (LRC) parsing and lookup.

/// One timed lyric line.
#[derive(Debug, Clone, PartialEq)]
pub struct LyricLine {
    /// Offset from the start of the track, in seconds.
    pub timestamp: f64,
    pub text: String,
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
}
