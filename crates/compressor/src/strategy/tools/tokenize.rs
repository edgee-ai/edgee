//! Lightweight tokenization helpers for tool-set pruning.
//!
//! Pure character-based; no regex compilation on the hot path.

use std::collections::HashSet;

const STOPWORDS: &[&str] = &[
    "a", "an", "the", "and", "or", "but", "of", "in", "on", "at", "to", "from", "for", "by",
    "with", "as", "is", "are", "was", "were", "be", "been", "being", "do", "does", "did", "done",
    "have", "has", "had", "i", "you", "he", "she", "it", "we", "they", "me", "us", "them", "my",
    "your", "his", "her", "its", "our", "their", "this", "that", "these", "those", "what", "which",
    "who", "whom", "when", "where", "why", "how", "can", "could", "should", "would", "will", "may",
    "might", "must", "shall", "if", "then", "else", "so", "than", "not", "no", "any", "some",
    "all", "each", "every", "few", "more", "most", "other", "such", "only", "own", "same", "very",
    "just", "now", "also", "please", "thanks", "thank", "ok", "okay", "about", "into", "out",
    "over", "under", "up", "down", "again", "still", "yet", "here", "there",
    // Identifier prefix that carries no semantic signal on its own.
    "mcp",
];

fn is_stopword(word: &str) -> bool {
    STOPWORDS.contains(&word)
}

/// Extract distinct lowercase word tokens from a free-form text.
///
/// Words are runs of alphanumeric characters. Tokens shorter than 2 chars and
/// stopwords are dropped.
pub fn tokenize_text(text: &str) -> HashSet<String> {
    let mut out = HashSet::new();
    let mut current = String::new();
    for ch in text.chars() {
        if ch.is_alphanumeric() {
            for lc in ch.to_lowercase() {
                current.push(lc);
            }
        } else if !current.is_empty() {
            push_token(&mut out, std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        push_token(&mut out, current);
    }
    out
}

fn push_token(out: &mut HashSet<String>, tok: String) {
    if tok.len() >= 2 && !is_stopword(&tok) {
        out.insert(tok);
    }
}

/// Tokenize a tool identifier (`mcp__linear-server__list_issues`,
/// `read_file`, `getWeather`, …) into searchable lowercase parts.
///
/// Splits on `__`, then on `_`, `-`, `.`, and case boundaries
/// (`getWeather` → `get`, `weather`).
pub fn tokenize_identifier(name: &str) -> HashSet<String> {
    let mut out = HashSet::new();
    for chunk in name.split("__") {
        for piece in chunk.split(|c: char| !c.is_alphanumeric()) {
            for sub in split_camel_case(piece) {
                push_token(&mut out, sub.to_lowercase());
            }
        }
    }
    out
}

fn split_camel_case(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut prev_lower = false;
    for ch in s.chars() {
        if ch.is_ascii_uppercase() && prev_lower && !current.is_empty() {
            out.push(std::mem::take(&mut current));
        }
        current.push(ch);
        prev_lower = ch.is_ascii_lowercase() || ch.is_ascii_digit();
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

/// Return the MCP server segment of a tool name like `mcp__linear-server__list_issues`
/// (`"linear-server"`), or `None` if the name is not in MCP format.
pub fn mcp_server_segment(name: &str) -> Option<&str> {
    let rest = name.strip_prefix("mcp__")?;
    let (server, _) = rest.split_once("__")?;
    Some(server)
}

/// Whether a tool name looks like an MCP-server tool (`mcp__server__action`).
pub fn is_mcp_tool_name(name: &str) -> bool {
    mcp_server_segment(name).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenize_text_drops_stopwords_and_short() {
        let toks = tokenize_text("Please find the Linear issue about payments");
        assert!(toks.contains("linear"));
        assert!(toks.contains("issue"));
        assert!(toks.contains("payments"));
        assert!(toks.contains("find"));
        assert!(!toks.contains("the"));
        assert!(!toks.contains("about"));
    }

    #[test]
    fn tokenize_identifier_splits_mcp_format() {
        let toks = tokenize_identifier("mcp__linear-server__list_issues");
        assert!(toks.contains("linear"));
        assert!(toks.contains("server"));
        assert!(toks.contains("list"));
        assert!(toks.contains("issues"));
        assert!(!toks.contains("mcp"));
    }

    #[test]
    fn tokenize_identifier_camel_case() {
        let toks = tokenize_identifier("getWeatherForecast");
        assert!(toks.contains("get"));
        assert!(toks.contains("weather"));
        assert!(toks.contains("forecast"));
    }

    #[test]
    fn mcp_server_segment_extracts() {
        assert_eq!(
            mcp_server_segment("mcp__linear-server__list_issues"),
            Some("linear-server")
        );
        assert_eq!(mcp_server_segment("Bash"), None);
        assert_eq!(mcp_server_segment("mcp__incomplete"), None);
    }

    #[test]
    fn mcp_tool_recognition() {
        assert!(is_mcp_tool_name("mcp__notion__search"));
        assert!(!is_mcp_tool_name("Read"));
        assert!(!is_mcp_tool_name("read_file"));
    }
}
