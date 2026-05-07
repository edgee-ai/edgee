//! Display-width helpers for terminal output.
//!
//! Statusline merging needs to know how many terminal cells a string occupies,
//! ignoring ANSI escape sequences and accounting for wide Unicode codepoints
//! (CJK ideographs, emoji, etc.). The naive `str::len()` would mis-count
//! colored output and cause Edgee's segment to be truncated in practice — so
//! the `--wrap` precedence guarantee depends on this measurement being right.

use unicode_width::UnicodeWidthChar;

/// Returns the number of terminal cells `s` occupies, ignoring ANSI escape
/// sequences (CSI, OSC and a small set of common 2-byte escapes).
pub fn display_width(s: &str) -> usize {
    let mut iter = AnsiAwareChars::new(s);
    let mut width = 0;
    while let Some(c) = iter.next_visible() {
        width += UnicodeWidthChar::width(c).unwrap_or(0);
    }
    width
}

/// Truncate `s` to fit within `max_width` terminal cells.
///
/// If truncation occurs, the truncated suffix is replaced with `ellipsis`
/// (which itself contributes to the width budget). ANSI escape sequences are
/// preserved verbatim and never count against the width budget. When
/// `max_width` is `0` or smaller than the ellipsis width, returns an empty
/// string.
pub fn truncate_display(s: &str, max_width: usize, ellipsis: &str) -> String {
    let total = display_width(s);
    if total <= max_width {
        return s.to_string();
    }
    let ellipsis_width = display_width(ellipsis);
    if max_width <= ellipsis_width {
        return String::new();
    }
    let budget = max_width - ellipsis_width;

    let mut out = String::with_capacity(s.len());
    let mut iter = AnsiAwareChars::new(s);
    let mut visible_width = 0;
    while let Some(piece) = iter.next_piece() {
        match piece {
            Piece::Escape(esc) => out.push_str(esc),
            Piece::Visible(c) => {
                let w = UnicodeWidthChar::width(c).unwrap_or(0);
                if visible_width + w > budget {
                    out.push_str(ellipsis);
                    return out;
                }
                visible_width += w;
                out.push(c);
            }
        }
    }
    // Should not be reached because total > max_width was checked, but stay
    // safe rather than panic.
    out.push_str(ellipsis);
    out
}

/// Strip ANSI escape sequences from `s`. Used for tests and debugging.
#[cfg(test)]
pub fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut iter = AnsiAwareChars::new(s);
    while let Some(c) = iter.next_visible() {
        out.push(c);
    }
    out
}

enum Piece<'a> {
    Escape(&'a str),
    Visible(char),
}

struct AnsiAwareChars<'a> {
    s: &'a str,
    pos: usize,
}

impl<'a> AnsiAwareChars<'a> {
    fn new(s: &'a str) -> Self {
        Self { s, pos: 0 }
    }

    fn next_visible(&mut self) -> Option<char> {
        loop {
            match self.next_piece()? {
                Piece::Escape(_) => continue,
                Piece::Visible(c) => return Some(c),
            }
        }
    }

    fn next_piece(&mut self) -> Option<Piece<'a>> {
        let bytes = self.s.as_bytes();
        if self.pos >= bytes.len() {
            return None;
        }

        if bytes[self.pos] == 0x1b {
            // Try to consume an ANSI escape sequence. We accept:
            //   ESC [ ... <final-byte 0x40..0x7E>     (CSI)
            //   ESC ] ... BEL or ESC \                (OSC, used by some terminals)
            //   ESC <single byte>                     (other 2-byte escapes)
            let start = self.pos;
            let len = bytes.len();
            let mut i = self.pos + 1;
            if i < len {
                match bytes[i] {
                    b'[' => {
                        i += 1;
                        while i < len {
                            let b = bytes[i];
                            i += 1;
                            if (0x40..=0x7e).contains(&b) {
                                break;
                            }
                        }
                    }
                    b']' => {
                        i += 1;
                        while i < len {
                            let b = bytes[i];
                            if b == 0x07 {
                                i += 1;
                                break;
                            }
                            if b == 0x1b && i + 1 < len && bytes[i + 1] == b'\\' {
                                i += 2;
                                break;
                            }
                            i += 1;
                        }
                    }
                    _ => {
                        i += 1;
                    }
                }
            }
            self.pos = i;
            return Some(Piece::Escape(&self.s[start..i]));
        }

        let rest = &self.s[self.pos..];
        let mut chars = rest.char_indices();
        let (_, c) = chars.next()?;
        let next_pos = chars.next().map(|(i, _)| self.pos + i).unwrap_or(bytes.len());
        self.pos = next_pos;
        Some(Piece::Visible(c))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_width_matches_length() {
        assert_eq!(display_width("hello"), 5);
        assert_eq!(display_width(""), 0);
    }

    #[test]
    fn ansi_codes_do_not_count() {
        let s = "\x1b[31mhello\x1b[0m";
        assert_eq!(display_width(s), 5);
        assert_eq!(strip_ansi(s), "hello");
    }

    #[test]
    fn extended_color_codes_do_not_count() {
        let s = "\x1b[38;5;128mEdgee\x1b[0m";
        assert_eq!(display_width(s), 5);
    }

    #[test]
    fn cjk_chars_count_as_two() {
        // 三 is a wide CJK ideograph (2 cells)
        assert_eq!(display_width("三"), 2);
        assert_eq!(display_width("三 Edgee"), 2 + 1 + 5);
    }

    #[test]
    fn emoji_counts_as_two() {
        // 🚀 occupies 2 cells in most terminals
        assert_eq!(display_width("🚀"), 2);
    }

    #[test]
    fn truncate_no_op_when_short_enough() {
        assert_eq!(truncate_display("hello", 10, "…"), "hello");
        assert_eq!(truncate_display("hello", 5, "…"), "hello");
    }

    #[test]
    fn truncate_replaces_overflow_with_ellipsis() {
        // "hello" = 5 cells, ellipsis = 1 cell, budget 4 → "hel" + "…"
        assert_eq!(truncate_display("hello", 4, "…"), "hel…");
    }

    #[test]
    fn truncate_preserves_ansi_codes() {
        let s = "\x1b[31mhello\x1b[0m world";
        // "hello world" = 11 cells, ellipsis = 1 cell, budget 6 → "hello " + "…"
        // ANSI sequences should still appear in output.
        let out = truncate_display(s, 7, "…");
        assert!(out.contains("\x1b[31m"));
        // visible width should be at most 7
        assert!(display_width(&out) <= 7);
    }

    #[test]
    fn truncate_zero_budget_returns_empty() {
        assert_eq!(truncate_display("hello", 0, "…"), "");
    }

    #[test]
    fn truncate_budget_smaller_than_ellipsis_returns_empty() {
        // ellipsis "…" has width 1; budget 0 → empty
        assert_eq!(truncate_display("hello world", 1, "…"), "");
    }

    #[test]
    fn truncate_does_not_split_wide_char() {
        // "三三三" = 6 cells, budget 3 (after 1-cell ellipsis = 2 cells visible)
        let out = truncate_display("三三三", 4, "…");
        // visible width must not exceed 4
        assert!(display_width(&out) <= 4);
        // And we should not have a half-character — strip_ansi reflects only
        // whole codepoints we wrote, so check it parses cleanly.
        assert!(strip_ansi(&out).chars().all(|c| c == '三' || c == '…'));
    }
}
