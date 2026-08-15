//! Small formatting helpers shared by every front-end.

/// Format a duration in seconds as `M:SS` (or `H:MM:SS` past an hour).
///
/// ```
/// use tiders_core::format::duration;
/// assert_eq!(duration(0), "0:00");
/// assert_eq!(duration(65), "1:05");
/// assert_eq!(duration(3661), "1:01:01");
/// ```
pub fn duration(total_secs: u64) -> String {
    let hours = total_secs / 3600;
    let minutes = (total_secs % 3600) / 60;
    let seconds = total_secs % 60;
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

/// Join a list of artist names with commas, e.g. `"A, B & C"`.
pub fn artists(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [only] => only.clone(),
        [rest @ .., last] => format!("{} & {}", rest.join(", "), last),
    }
}

/// Truncate `s` to at most `max` characters, appending `…` when shortened.
///
/// Operates on Unicode scalar values, which is good enough for list rows; the
/// TUI layer additionally accounts for display width where it matters.
pub fn truncate(s: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    let count = s.chars().count();
    if count <= max {
        return s.to_string();
    }
    let keep = max.saturating_sub(1);
    let mut out: String = s.chars().take(keep).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_render_as_expected() {
        assert_eq!(duration(0), "0:00");
        assert_eq!(duration(9), "0:09");
        assert_eq!(duration(60), "1:00");
        assert_eq!(duration(125), "2:05");
        assert_eq!(duration(3600), "1:00:00");
        assert_eq!(duration(3661), "1:01:01");
    }

    #[test]
    fn artist_lists_use_ampersand_for_last() {
        assert_eq!(artists(&[]), "");
        assert_eq!(artists(&["Air".into()]), "Air");
        assert_eq!(artists(&["Air".into(), "Phoenix".into()]), "Air & Phoenix");
        assert_eq!(artists(&["A".into(), "B".into(), "C".into()]), "A, B & C");
    }

    #[test]
    fn truncate_adds_ellipsis_only_when_needed() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("hello", 5), "hello");
        assert_eq!(truncate("hello world", 5), "hell…");
        assert_eq!(truncate("hello", 0), "");
    }
}
