//! Incremental fuzzy filter powered by FFF's SIMD matcher ([`neo_frizbee`]).
//!
//! [FFF](https://github.com/dmtrKovalenko/fff) (`fff-search`) is a filesystem
//! indexer — it ranks file paths, not in-memory catalog rows. The matching
//! algorithm it uses is [`neo_frizbee`] (Smith–Waterman, typo-resistant, SIMD),
//! which is the right tool for filtering playlists, mixes, favorites, and the
//! queue as you type.

use neo_frizbee::{match_list, match_list_indices, Config, MatchIndices};

/// One ranked hit: original row index plus byte offsets of the matched needle
/// inside the haystack (for highlighting).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub index: usize,
    pub indices: Vec<usize>,
}

fn config() -> Config {
    Config {
        // One typo is the sweet spot for catalog titles ("discovry" → Discovery).
        max_typos: Some(1),
        sort: true,
        ..Config::default()
    }
}

/// Rank `haystacks` for `needle`. Empty needle → every row, original order.
pub fn rank(needle: &str, haystacks: &[String]) -> Vec<Hit> {
    let needle = needle.trim();
    if needle.is_empty() {
        return (0..haystacks.len())
            .map(|index| Hit {
                index,
                indices: Vec::new(),
            })
            .collect();
    }
    if haystacks.is_empty() {
        return Vec::new();
    }

    let cfg = config();
    let refs: Vec<&str> = haystacks.iter().map(String::as_str).collect();

    if haystacks.len() <= 512 {
        match_list_indices(needle, &refs, &cfg)
            .into_iter()
            .map(|m: MatchIndices| Hit {
                index: m.index as usize,
                indices: m.indices,
            })
            .collect()
    } else {
        match_list(needle, &refs, &cfg)
            .into_iter()
            .map(|m| Hit {
                index: m.index as usize,
                indices: Vec::new(),
            })
            .collect()
    }
}

/// Split a haystack of `"title {sep} rest"` into highlight sets for each side.
pub fn split_highlights(
    title_len: usize,
    sep_len: usize,
    indices: &[usize],
) -> (Vec<usize>, Vec<usize>) {
    let mut title = Vec::new();
    let mut rest = Vec::new();
    let rest_start = title_len + sep_len;
    for &i in indices {
        if i < title_len {
            title.push(i);
        } else if i >= rest_start {
            rest.push(i - rest_start);
        }
    }
    (title, rest)
}

/// Paint `text` with `hit` style on matched **byte** offsets.
pub fn highlight(
    text: &str,
    matched: &[usize],
    base: ratatui::style::Style,
    hit: ratatui::style::Style,
) -> Vec<ratatui::text::Span<'static>> {
    use ratatui::text::Span;
    if matched.is_empty() || text.is_empty() {
        return vec![Span::styled(text.to_string(), base)];
    }
    let mut set = vec![false; text.len()];
    for &i in matched {
        if i < set.len() {
            set[i] = true;
        }
    }
    let mut spans = Vec::new();
    let mut buf = String::new();
    let mut on = false;
    let mut first = true;
    for (i, ch) in text.char_indices() {
        let now = set.get(i).copied().unwrap_or(false);
        if first {
            on = now;
            first = false;
        } else if now != on {
            spans.push(Span::styled(
                std::mem::take(&mut buf),
                if on { hit } else { base },
            ));
            on = now;
        }
        buf.push(ch);
    }
    if !buf.is_empty() {
        spans.push(Span::styled(buf, if on { hit } else { base }));
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    fn titles() -> Vec<String> {
        [
            "Random Access Memories",
            "Discovery",
            "Homework",
            "Alive 2007",
            "Human After All",
        ]
        .into_iter()
        .map(str::to_string)
        .collect()
    }

    #[test]
    fn empty_needle_keeps_order() {
        let hits = rank("", &titles());
        assert_eq!(
            hits.iter().map(|h| h.index).collect::<Vec<_>>(),
            vec![0, 1, 2, 3, 4]
        );
    }

    #[test]
    fn ranks_discovery_first() {
        let hay = titles();
        let hits = rank("disc", &hay);
        assert!(!hits.is_empty());
        assert_eq!(hay[hits[0].index], "Discovery");
        assert!(hits.iter().any(|h| hay[h.index] == "Discovery"));
    }

    #[test]
    fn typo_still_hits() {
        let hay = titles();
        let hits = rank("discovry", &hay);
        assert!(
            hits.iter().any(|h| hay[h.index] == "Discovery"),
            "expected Discovery in {hits:?}"
        );
    }

    #[test]
    fn no_match_is_empty() {
        let hits = rank("zzzzzzxyz", &titles());
        assert!(hits.is_empty());
    }

    #[test]
    fn split_highlights_title_and_artist() {
        let (title, artist) = split_highlights(4, 1, &[0, 2, 5, 6]);
        assert_eq!(title, vec![0, 2]);
        assert_eq!(artist, vec![0, 1]);
    }
}
