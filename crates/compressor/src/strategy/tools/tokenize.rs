//! Lightweight tokenization helpers for tool-set pruning.
//!
//! Pure character-based; no regex compilation on the hot path apart from the
//! single combined pattern used by [`strip_injected_tags`].

use std::collections::HashSet;
use std::sync::LazyLock;

use regex::Regex;

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

/// MCP servers whose tools must never be pruned by the tool-set compressor.
///
/// The gateway's own `edgee` MCP server (session naming, PR/commit linking,
/// repo association) is injected by the CLI into every request. Dropping any
/// of its tools silently breaks session instrumentation the system prompt
/// requires the agent to call, so we keep the entire server unconditionally.
pub fn is_protected_mcp_server(server: &str) -> bool {
    server == "edgee"
}

/// Remove Claude-Code-injected tag blocks from a user-message string so the
/// pruning heuristic only scores against actual user-typed content.
///
/// Claude Code wraps the first user message of every conversation with a stack
/// of `<system-reminder>` blocks (skill list, project CLAUDE.md, memory pointers,
/// MCP-server instructions) and `<local-command-*>` / `<command-*>` blocks for
/// slash-command invocations. These blocks are stable across turns *and*
/// mention every MCP server by name in their descriptions, so when scored
/// lexically every MCP looks "relevant" — leaving the pruner with nothing to
/// drop. Stripping them isolates the user's actual intent.
pub fn strip_injected_tags(text: &str) -> String {
    let stripped = INJECTED_TAG_RE.replace_all(text, "");
    stripped.trim().to_string()
}

static INJECTED_TAG_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"(?s)",
        r"<system-reminder\b[^>]*>.*?</system-reminder>",
        r"|<local-command-[\w-]+\b[^>]*>.*?</local-command-[\w-]+>",
        r"|<command-name\b[^>]*>.*?</command-name>",
        r"|<command-message\b[^>]*>.*?</command-message>",
        r"|<command-args\b[^>]*>.*?</command-args>",
        r"|<user-prompt-[\w-]+\b[^>]*>.*?</user-prompt-[\w-]+>",
    ))
    .expect("valid injected-tag regex")
});

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

    #[test]
    fn strip_removes_system_reminders() {
        let polluted = "<system-reminder>\n# MCP Server Instructions\nlinear-server figma notion\n</system-reminder>\n\nfind the bug";
        assert_eq!(strip_injected_tags(polluted), "find the bug");
    }

    #[test]
    fn strip_removes_multiple_and_nested_block_types() {
        let polluted = "<system-reminder>skills: figma, linear, notion</system-reminder>\n<local-command-caveat>caveat text</local-command-caveat>\n<command-name>/clear</command-name>\n<command-message>clear</command-message>\n<command-args></command-args>\n<local-command-stdout></local-command-stdout>\nWhat is this?";
        assert_eq!(strip_injected_tags(polluted), "What is this?");
    }

    #[test]
    fn strip_leaves_plain_text_untouched() {
        let plain = "now check github for related PRs";
        assert_eq!(strip_injected_tags(plain), plain);
    }

    #[test]
    fn strip_handles_empty_user_content() {
        // If everything was injected, the result is empty.
        let only_tags = "<system-reminder>foo</system-reminder>";
        assert_eq!(strip_injected_tags(only_tags), "");
    }
}
