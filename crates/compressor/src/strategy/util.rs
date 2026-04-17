//! Shared utilities for compression strategies.

use std::sync::LazyLock;

use regex::Regex;

/// Matches `<system-reminder>…</system-reminder>` blocks, including newlines.
static SYSTEM_REMINDER_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)<system-reminder>.*?</system-reminder>").unwrap());

/// A segment of text that is either eligible for compression or must be passed through verbatim.
#[derive(Debug, PartialEq)]
pub enum TextSegment {
    Compressible(String),
    /// Content that must be preserved exactly (e.g. `<system-reminder>` blocks).
    Protected(String),
}

/// Split `text` into alternating compressible / protected segments.
///
/// The result always starts and ends with a `Compressible` segment (possibly empty).
pub fn split_into_segments(text: &str) -> Vec<TextSegment> {
    let mut segments = Vec::new();
    let mut last_end = 0usize;
    for m in SYSTEM_REMINDER_RE.find_iter(text) {
        segments.push(TextSegment::Compressible(
            text[last_end..m.start()].to_string(),
        ));
        segments.push(TextSegment::Protected(m.as_str().to_string()));
        last_end = m.end();
    }
    segments.push(TextSegment::Compressible(text[last_end..].to_string()));
    segments
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── split_into_segments ───────────────────────────────────────────────

    #[test]
    fn split_no_protected_returns_single_compressible() {
        let text = "plain text with no tags";
        let segments = split_into_segments(text);
        assert_eq!(segments, vec![TextSegment::Compressible(text.to_string())]);
    }

    #[test]
    fn split_block_in_middle() {
        let text = "before<system-reminder>tag content</system-reminder>after";
        let segments = split_into_segments(text);
        assert_eq!(
            segments,
            vec![
                TextSegment::Compressible("before".to_string()),
                TextSegment::Protected(
                    "<system-reminder>tag content</system-reminder>".to_string()
                ),
                TextSegment::Compressible("after".to_string()),
            ]
        );
    }

    #[test]
    fn split_block_at_start() {
        let text = "<system-reminder>tag</system-reminder>after";
        let segments = split_into_segments(text);
        assert_eq!(
            segments,
            vec![
                TextSegment::Compressible(String::new()),
                TextSegment::Protected("<system-reminder>tag</system-reminder>".to_string()),
                TextSegment::Compressible("after".to_string()),
            ]
        );
    }

    #[test]
    fn split_block_at_end() {
        let text = "before<system-reminder>tag</system-reminder>";
        let segments = split_into_segments(text);
        assert_eq!(
            segments,
            vec![
                TextSegment::Compressible("before".to_string()),
                TextSegment::Protected("<system-reminder>tag</system-reminder>".to_string()),
                TextSegment::Compressible(String::new()),
            ]
        );
    }

    #[test]
    fn split_multiple_blocks() {
        let text = "a<system-reminder>x</system-reminder>b<system-reminder>y</system-reminder>c";
        let segments = split_into_segments(text);
        assert_eq!(
            segments,
            vec![
                TextSegment::Compressible("a".to_string()),
                TextSegment::Protected("<system-reminder>x</system-reminder>".to_string()),
                TextSegment::Compressible("b".to_string()),
                TextSegment::Protected("<system-reminder>y</system-reminder>".to_string()),
                TextSegment::Compressible("c".to_string()),
            ]
        );
    }

    #[test]
    fn split_multiline_block() {
        let text = "before\n<system-reminder>\nmultiline\ncontent\n</system-reminder>\nafter";
        let segments = split_into_segments(text);
        assert_eq!(segments.len(), 3);
        assert!(
            matches!(&segments[1], TextSegment::Protected(s) if s.contains("multiline\ncontent"))
        );
    }
}
