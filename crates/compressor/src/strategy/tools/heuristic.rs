//! Rule-based heuristic for tool-set pruning, cache-safe by construction.
//!
//! The scoring signal is the **stable** part of the request: system prompts
//! concatenated with the first user message. Both are byte-identical on
//! every turn of the same conversation, so the kept-set — and therefore the
//! emitted `tools` array — is identical on every turn, keeping Anthropic's
//! prompt cache warm.
//!
//! Decision per tool, in priority order:
//!
//! 1. **Always-keep** if the tool name is in the agent's "core" list.
//! 2. **Always-keep** if the tool name is non-MCP (built-in or opaque) —
//!    we only prune things that clearly come from an MCP server.
//! 3. **Always-keep** if the MCP server segment is protected (e.g. the
//!    gateway's own `edgee` server, which carries session-instrumentation
//!    tools the CLI requires on every request).
//! 4. Score the remaining MCP tools by lexical overlap between
//!    [`PruneContext::stable_text`] and the tool's name + description,
//!    plus an MCP-server-name bonus. Keep if `score >= min_score`.
//! 5. **`min_kept` floor**: keep at least N MCP tools even if scoring
//!    rejected them all (top score → lexicographic name).
//! 6. **Restore-on-pivot**: if [`PruneContext::pivot_signal_text`] is set
//!    and distinct from `stable_text`, scan it for an MCP server segment
//!    that was pruned. If matched, restore every tool from that server.
//!    This intentionally invalidates the prompt cache for the pivot turn
//!    in exchange for serving the tool the user just asked for; subsequent
//!    turns re-cache with the new (slightly larger) prefix and keep hitting.

use std::collections::HashSet;

use super::tokenize::{
    is_mcp_tool_name, is_protected_mcp_server, mcp_server_segment, strip_injected_tags,
    tokenize_identifier, tokenize_text,
};
use super::{PruneContext, PruneDecision, ToolSetCompressor, ToolView};

#[derive(Debug, Clone, Copy)]
pub struct HeuristicToolSetCompressor {
    pub min_score: u32,
    pub min_kept: usize,
}

impl Default for HeuristicToolSetCompressor {
    fn default() -> Self {
        Self {
            min_score: 1,
            min_kept: 5,
        }
    }
}

impl ToolSetCompressor for HeuristicToolSetCompressor {
    fn prune(&self, ctx: &PruneContext<'_>) -> PruneDecision {
        let bytes_before: usize = ctx.tools.iter().map(|t| t.size_bytes).sum();

        // Scrub Claude-Code-injected scaffolding (system-reminder blocks,
        // slash-command markup, hook output) before scoring. Those blocks
        // mention every connected MCP server by name in skill descriptions,
        // which would otherwise make every MCP look "relevant".
        let stable_text = strip_injected_tags(ctx.stable_text);
        let stable_tokens = tokenize_text(&stable_text);
        let stable_text_lower = stable_text.to_lowercase();

        let core: HashSet<&str> = ctx.core_tools.iter().copied().collect();

        // Pass 1 — classify into "always keep" and "scoreable".
        let mut keep_set: HashSet<usize> = HashSet::new();
        let mut scored: Vec<(usize, u32)> = Vec::new();

        for (idx, tool) in ctx.tools.iter().enumerate() {
            let Some(name) = tool.name else {
                keep_set.insert(idx);
                continue;
            };

            if core.contains(name) {
                keep_set.insert(idx);
                continue;
            }

            if !is_mcp_tool_name(name) {
                keep_set.insert(idx);
                continue;
            }

            // Gateway-internal MCP servers (e.g. `edgee`) are load-bearing
            // for session instrumentation and must never be pruned.
            if let Some(server) = mcp_server_segment(name)
                && is_protected_mcp_server(server)
            {
                keep_set.insert(idx);
                continue;
            }

            let score = score_tool(&stable_tokens, &stable_text_lower, tool, name);
            if score >= self.min_score {
                keep_set.insert(idx);
            } else {
                scored.push((idx, score));
            }
        }

        // Pass 2 — enforce min_kept floor.
        let mcp_currently_kept = ctx
            .tools
            .iter()
            .enumerate()
            .filter(|(idx, t)| {
                keep_set.contains(idx) && t.name.map(is_mcp_tool_name).unwrap_or(false)
            })
            .count();

        if mcp_currently_kept < self.min_kept {
            let needed = self.min_kept - mcp_currently_kept;
            scored.sort_by(|a, b| {
                b.1.cmp(&a.1).then_with(|| {
                    let na = ctx.tools[a.0].name.unwrap_or("");
                    let nb = ctx.tools[b.0].name.unwrap_or("");
                    na.cmp(nb)
                })
            });
            for (idx, _) in scored.iter().take(needed) {
                keep_set.insert(*idx);
            }
        }

        // Pass 3 — restore-on-pivot. Only fires when a distinct pivot signal
        // is present (i.e. the latest user message differs from the stable
        // first-user signal already incorporated in pass 1). Accepts a
        // one-time cache reset to serve the just-mentioned tool.
        if let Some(raw_pivot) = ctx.pivot_signal_text {
            let pivot = strip_injected_tags(raw_pivot);
            if pivot != stable_text && !pivot.is_empty() {
                let pivot_lower = pivot.to_lowercase();
                let pivot_tokens = tokenize_text(&pivot);

                for (idx, tool) in ctx.tools.iter().enumerate() {
                    if keep_set.contains(&idx) {
                        continue;
                    }
                    let Some(name) = tool.name else { continue };
                    if pivot_mentions_tool(&pivot_lower, &pivot_tokens, tool, name) {
                        keep_set.insert(idx);
                    }
                }
            }
        }

        let mut keep_indices: Vec<usize> = keep_set.into_iter().collect();
        keep_indices.sort_unstable();

        let bytes_after: usize = keep_indices.iter().map(|i| ctx.tools[*i].size_bytes).sum();
        let dropped = ctx.tools.len() - keep_indices.len();

        PruneDecision {
            keep_indices,
            bytes_before,
            bytes_after,
            dropped,
        }
    }
}

fn score_tool(
    text_tokens: &HashSet<String>,
    text_lower: &str,
    tool: &ToolView<'_>,
    name: &str,
) -> u32 {
    if text_tokens.is_empty() {
        return 0;
    }

    let mut tool_tokens = tokenize_identifier(name);
    if let Some(desc) = tool.description {
        tool_tokens.extend(tokenize_text(desc));
    }

    let overlap = text_tokens.intersection(&tool_tokens).count() as u32;

    let server_bonus = mcp_server_segment(name)
        .map(|server| {
            tokenize_identifier(server)
                .iter()
                .any(|tok| text_lower.contains(tok)) as u32
        })
        .unwrap_or(0);

    overlap + server_bonus
}

/// Whether the pivot user message mentions this tool strongly enough to
/// justify restoring it. Uses the same overlap + server-name signal as
/// [`score_tool`] but with a fixed threshold of 1.
fn pivot_mentions_tool(
    pivot_lower: &str,
    pivot_tokens: &HashSet<String>,
    tool: &ToolView<'_>,
    name: &str,
) -> bool {
    score_tool(pivot_tokens, pivot_lower, tool, name) >= 1
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool<'a>(name: &'a str, desc: Option<&'a str>, size: usize) -> ToolView<'a> {
        ToolView {
            name: Some(name),
            description: desc,
            size_bytes: size,
        }
    }

    fn ctx<'a>(
        tools: &'a [ToolView<'a>],
        stable: &'a str,
        pivot: Option<&'a str>,
        core: &'a [&'a str],
    ) -> PruneContext<'a> {
        PruneContext {
            tools,
            stable_text: stable,
            pivot_signal_text: pivot,
            core_tools: core,
        }
    }

    #[test]
    fn always_keeps_core_tools() {
        let tools = [
            tool("Bash", Some("Run a shell command"), 500),
            tool("mcp__github__create_pr", Some("Open a pull request"), 1200),
        ];
        let c = ctx(&tools, "hello", None, &["Bash", "Read", "Grep", "Glob"]);
        let d = HeuristicToolSetCompressor::default().prune(&c);
        assert!(d.keep_indices.contains(&0), "Bash must be kept");
    }

    #[test]
    fn keeps_mcp_matching_stable_text() {
        let tools = [
            tool("Bash", Some("shell"), 500),
            tool(
                "mcp__linear-server__list_issues",
                Some("List Linear issues"),
                2000,
            ),
            tool(
                "mcp__github__create_pr",
                Some("Open a pull request on GitHub"),
                2200,
            ),
        ];
        let c = ctx(
            &tools,
            "please find the linear issue about payments",
            None,
            &["Bash"],
        );
        let comp = HeuristicToolSetCompressor {
            min_score: 1,
            min_kept: 0,
        };
        let d = comp.prune(&c);
        assert!(d.keep_indices.contains(&1), "Linear should be kept");
        assert!(
            !d.keep_indices.contains(&2),
            "GitHub should be dropped: {:?}",
            d.keep_indices
        );
    }

    #[test]
    fn pivot_restores_pruned_mcp() {
        let tools = [
            tool("mcp__linear-server__list_issues", Some("List Linear"), 2000),
            tool("mcp__github__create_pr", Some("Open a PR on GitHub"), 2200),
        ];
        // Stable text mentions only Linear; pivot brings up GitHub.
        let c = ctx(
            &tools,
            "find the linear ticket",
            Some("now check github for related prs"),
            &[],
        );
        let comp = HeuristicToolSetCompressor {
            min_score: 1,
            min_kept: 0,
        };
        let d = comp.prune(&c);
        assert!(d.keep_indices.contains(&0), "Linear (stable) kept");
        assert!(
            d.keep_indices.contains(&1),
            "GitHub should be restored by pivot: {:?}",
            d.keep_indices
        );
    }

    #[test]
    fn no_pivot_when_signal_equals_stable() {
        let tools = [
            tool("mcp__linear-server__list_issues", Some("List Linear"), 2000),
            tool("mcp__github__create_pr", Some("PR"), 2200),
        ];
        let stable = "find the linear ticket";
        // On the first turn the latest-user-message equals the first-user-message.
        let c = ctx(&tools, stable, Some(stable), &[]);
        let comp = HeuristicToolSetCompressor {
            min_score: 1,
            min_kept: 0,
        };
        let d = comp.prune(&c);
        assert!(d.keep_indices.contains(&0));
        assert!(
            !d.keep_indices.contains(&1),
            "GitHub should not be restored when pivot==stable"
        );
    }

    #[test]
    fn kept_set_is_byte_stable_across_turns() {
        // Same stable text, different pivot signals that don't mention any
        // pruned tool — kept-set must be identical (cache-safe property).
        let tools = [
            tool("Bash", None, 500),
            tool("mcp__linear-server__list_issues", Some("Linear"), 2000),
            tool("mcp__github__create_pr", Some("Open PR"), 2200),
            tool("mcp__notion__search", Some("Notion search"), 2400),
        ];
        let stable = "list the linear issues for the payments project";
        let comp = HeuristicToolSetCompressor {
            min_score: 1,
            min_kept: 0,
        };

        let turn1 = comp.prune(&ctx(&tools, stable, Some(stable), &["Bash"]));
        let turn5 = comp.prune(&ctx(
            &tools,
            stable,
            Some("ok and the next one please"),
            &["Bash"],
        ));
        let turn20 = comp.prune(&ctx(
            &tools,
            stable,
            Some("summarize what we found so far"),
            &["Bash"],
        ));

        assert_eq!(turn1.keep_indices, turn5.keep_indices);
        assert_eq!(turn5.keep_indices, turn20.keep_indices);
    }

    #[test]
    fn min_kept_floor_revives_top_scoring() {
        let tools = [
            tool("mcp__a__x", Some("alpha"), 1000),
            tool("mcp__b__y", Some("beta gamma"), 1000),
            tool("mcp__c__z", Some("delta"), 1000),
        ];
        let c = ctx(&tools, "kfjlkjsdf", None, &[]);
        let comp = HeuristicToolSetCompressor {
            min_score: 99,
            min_kept: 2,
        };
        let d = comp.prune(&c);
        assert_eq!(d.keep_indices.len(), 2, "floor honored");
        assert_eq!(d.keep_indices, vec![0, 1]);
    }

    #[test]
    fn always_keeps_edgee_mcp_tools() {
        // edgee MCP tools must survive even with a stable text that shares no
        // tokens with them and min_score/min_kept tuned to drop everything.
        let tools = [
            tool(
                "mcp__edgee__setSessionName",
                Some("Set a human-readable display name for an AI Gateway session"),
                900,
            ),
            tool(
                "mcp__edgee__addSessionPullRequest",
                Some("Associate a pull request with an AI Gateway session"),
                900,
            ),
            tool("mcp__github__create_pr", Some("Open a pull request"), 1200),
        ];
        let c = ctx(&tools, "kfjlkjsdf totally unrelated", None, &[]);
        let comp = HeuristicToolSetCompressor {
            min_score: 99,
            min_kept: 0,
        };
        let d = comp.prune(&c);
        assert!(d.keep_indices.contains(&0), "edgee setSessionName kept");
        assert!(
            d.keep_indices.contains(&1),
            "edgee addSessionPullRequest kept"
        );
        assert!(
            !d.keep_indices.contains(&2),
            "unrelated mcp dropped: {:?}",
            d.keep_indices
        );
    }

    #[test]
    fn unknown_tool_always_kept() {
        let tools = [ToolView {
            name: None,
            description: None,
            size_bytes: 100,
        }];
        let c = ctx(&tools, "anything", None, &[]);
        let d = HeuristicToolSetCompressor::default().prune(&c);
        assert_eq!(d.keep_indices, vec![0]);
    }

    #[test]
    fn reports_byte_savings() {
        let tools = [
            tool("Bash", None, 500),
            tool("mcp__x__y", Some("irrelevant"), 5000),
        ];
        let c = ctx(&tools, "hello", None, &["Bash"]);
        let comp = HeuristicToolSetCompressor {
            min_score: 99,
            min_kept: 0,
        };
        let d = comp.prune(&c);
        assert_eq!(d.bytes_before, 5500);
        assert_eq!(d.bytes_after, 500);
        assert_eq!(d.dropped, 1);
    }
}
