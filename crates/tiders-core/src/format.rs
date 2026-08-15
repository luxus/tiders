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

/// Short list-row badge for a TIDAL `audioQuality` tag.
pub fn quality_badge(tag: Option<&str>) -> Option<&'static str> {
    let t = tag.unwrap_or("").to_ascii_uppercase();
    if t.contains("HI_RES") || t.contains("HIRES") || t == "MAX" {
        Some("HIRES")
    } else if t.contains("LOSSLESS") || t.contains("FLAC") {
        Some("FLAC")
    } else if t.contains("HIGH") || t.contains("320") || t.contains("LOW") || t.contains("96") {
        Some("AAC")
    } else {
        None
    }
}

/// Pretty-print a decoded stream: `FLAC 24-bit 96 kHz`, `AAC 320 kbps`, …
pub fn stream_quality_label(q: &crate::model::StreamQuality) -> String {
    let codec = codec_name(
        q.codecs.as_deref(),
        q.mime_type.as_deref(),
        q.audio_quality.as_deref(),
    );
    let mut parts = vec![codec];
    if let Some(bits) = q.bit_depth {
        parts.push(format!("{bits}-bit"));
    }
    if let Some(hz) = q.sample_rate_hz {
        parts.push(format_hz(hz));
    }
    if q.sample_rate_hz.is_none() {
        if let Some(bps) = q.bitrate_bps {
            if bps >= 1000 {
                parts.push(format!("{} kbps", (bps + 500) / 1000));
            }
        }
    }
    if let Some(ch) = q.channels {
        if ch > 2 {
            parts.push(format!("{ch}ch"));
        }
    }
    parts.join(" ")
}

fn codec_name(codecs: Option<&str>, mime: Option<&str>, quality: Option<&str>) -> String {
    let c = codecs.unwrap_or("").to_ascii_lowercase();
    let m = mime.unwrap_or("").to_ascii_lowercase();
    let q = quality.unwrap_or("").to_ascii_uppercase();
    if c.contains("flac") || m.contains("flac") {
        if q.contains("HI_RES") {
            "Hi-Res FLAC".into()
        } else {
            "FLAC".into()
        }
    } else if c.contains("alac") {
        "ALAC".into()
    } else if c.contains("mp4a") || c.contains("aac") || m.contains("mp4") || m.contains("aac") {
        "AAC".into()
    } else if !c.is_empty() {
        c
    } else if q.contains("HI_RES") {
        "Hi-Res".into()
    } else if q.contains("LOSSLESS") {
        "Lossless".into()
    } else if q.contains("HIGH") || q.contains("LOW") {
        "AAC".into()
    } else {
        "Stream".into()
    }
}

fn format_hz(hz: u32) -> String {
    if hz % 1000 == 0 {
        format!("{} kHz", hz / 1000)
    } else {
        format!("{:.1} kHz", hz as f32 / 1000.0)
    }
}

/// Infer bit depth from an mpv `audio-params/format` string (`s16`, `s32`, `float`, …).
pub fn bit_depth_from_format(fmt: &str) -> Option<u8> {
    let f = fmt.to_ascii_lowercase();
    if f.contains("s16") || f.contains("u16") {
        Some(16)
    } else if f.contains("s24")
        || f.contains("u24")
        || f.contains("s32")
        || f.contains("u32")
        || f.contains("float")
        || f.contains("dbl")
    {
        // s32/float playback is typically a 24-bit master.
        Some(24)
    } else {
        None
    }
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

    #[test]
    fn stream_quality_renders_flac_and_aac() {
        use crate::model::StreamQuality;
        let flac = StreamQuality {
            audio_quality: Some("HI_RES".into()),
            mime_type: Some("audio/flac".into()),
            codecs: Some("flac".into()),
            sample_rate_hz: Some(96000),
            bit_depth: Some(24),
            channels: Some(2),
            bitrate_bps: None,
        };
        assert_eq!(stream_quality_label(&flac), "Hi-Res FLAC 24-bit 96 kHz");
        let aac = StreamQuality {
            audio_quality: Some("HIGH".into()),
            mime_type: Some("audio/mp4".into()),
            codecs: Some("mp4a.40.2".into()),
            bitrate_bps: Some(320_000),
            ..StreamQuality::default()
        };
        assert_eq!(stream_quality_label(&aac), "AAC 320 kbps");
        assert_eq!(quality_badge(Some("HI_RES_LOSSLESS")), Some("HIRES"));
        assert_eq!(bit_depth_from_format("s16"), Some(16));
    }
}
