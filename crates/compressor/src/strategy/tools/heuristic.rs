//! Rule-based heuristic for tool-set pruning.
//!
//! Decision per tool, in priority order:
//!
//! 1. **Always-keep** if the tool name is in the agent's "core" list.
//! 2. **Always-keep** if the tool was already invoked earlier in the conversation
//!    (sticky rule — the model will expect it to remain).
//! 3. **Always-keep** if the tool name is non-MCP (built-in or opaque) and no
//!    explicit MCP marker is present — we only prune things that look like MCP.
//! 4. Otherwise, score the tool by lexical overlap between the latest user
//!    message and the tool's name + description, plus an MCP-server-name
//!    bonus. Keep if `score >= min_score`.
//!
//! A `min_kept` floor guarantees at least N MCP tools survive even if nothing
//! scored above the threshold. Ties break by highest score, then lexicographic
//! tool name (deterministic).

use std::collections::HashSet;

use super::tokenize::{is_mcp_tool_name, mcp_server_segment, tokenize_identifier, tokenize_text};
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
            min_kept: 3,
        }
    }
}

impl ToolSetCompressor for HeuristicToolSetCompressor {
    fn prune(&self, ctx: &PruneContext<'_>) -> PruneDecision {
        let bytes_before: usize = ctx.tools.iter().map(|t| t.size_bytes).sum();

        let user_tokens = ctx.latest_user_text.map(tokenize_text).unwrap_or_default();

        let user_text_lower = ctx
            .latest_user_text
            .map(|s| s.to_lowercase())
            .unwrap_or_default();

        let prior_names: HashSet<&str> = ctx.prior_tool_call_names.iter().copied().collect();
        let core: HashSet<&str> = ctx.core_tools.iter().copied().collect();

        // Pass 1 — classify into "always keep" and "scoreable".
        let mut keep_set: HashSet<usize> = HashSet::new();
        // (index, score) — for MCP tools that didn't get auto-kept.
        let mut scored: Vec<(usize, u32)> = Vec::new();

        for (idx, tool) in ctx.tools.iter().enumerate() {
            let Some(name) = tool.name else {
                // Opaque/Unknown tool — always keep.
                keep_set.insert(idx);
                continue;
            };

            if core.contains(name) {
                keep_set.insert(idx);
                continue;
            }

            if prior_names.contains(name) {
                keep_set.insert(idx);
                continue;
            }

            if !is_mcp_tool_name(name) {
                // Non-MCP, non-core tool (e.g. an agent-builtin we don't have in the
                // core list). Keep — we only prune things that clearly come from MCP.
                keep_set.insert(idx);
                continue;
            }

            // MCP tool — score it.
            let score = score_tool(&user_tokens, &user_text_lower, tool, name);
            if score >= self.min_score {
                keep_set.insert(idx);
            } else {
                scored.push((idx, score));
            }
        }

        // Pass 2 — enforce min_kept floor by reviving the top-scoring dropped MCPs.
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
            // Sort dropped MCPs by score desc, then by name asc for determinism.
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
    user_tokens: &HashSet<String>,
    user_text_lower: &str,
    tool: &ToolView<'_>,
    name: &str,
) -> u32 {
    if user_tokens.is_empty() {
        return 0;
    }

    let mut tool_tokens = tokenize_identifier(name);
    if let Some(desc) = tool.description {
        tool_tokens.extend(tokenize_text(desc));
    }

    let overlap = user_tokens.intersection(&tool_tokens).count() as u32;

    // MCP-server-name bonus: "use linear to …" should keep every Linear tool.
    let server_bonus = mcp_server_segment(name)
        .map(|server| {
            tokenize_identifier(server)
                .iter()
                .any(|tok| user_text_lower.contains(tok)) as u32
        })
        .unwrap_or(0);

    overlap + server_bonus
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
        user_text: Option<&'a str>,
        prior: &'a [&'a str],
        core: &'a [&'a str],
    ) -> PruneContext<'a> {
        PruneContext {
            tools,
            latest_user_text: user_text,
            prior_tool_call_names: prior,
            core_tools: core,
        }
    }

    #[test]
    fn always_keeps_core_tools() {
        let tools = [
            tool("Bash", Some("Run a shell command"), 500),
            tool("mcp__github__create_pr", Some("Open a pull request"), 1200),
        ];
        let c = ctx(
            &tools,
            Some("hello"),
            &[],
            &["Bash", "Read", "Grep", "Glob"],
        );
        let d = HeuristicToolSetCompressor::default().prune(&c);
        assert!(d.keep_indices.contains(&0), "Bash must be kept");
    }

    #[test]
    fn keeps_mcp_matching_user_message() {
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
            Some("please find the linear issue about payments"),
            &[],
            &["Bash"],
        );
        let comp = HeuristicToolSetCompressor {
            min_score: 1,
            min_kept: 0,
        };
        let d = comp.prune(&c);
        assert!(d.keep_indices.contains(&0));
        assert!(d.keep_indices.contains(&1), "Linear should be kept");
        assert!(
            !d.keep_indices.contains(&2),
            "GitHub should be dropped: {:?}",
            d.keep_indices
        );
    }

    #[test]
    fn sticky_rule_keeps_previously_invoked() {
        let tools = [
            tool("mcp__notion__search", Some("Search Notion pages"), 1500),
            tool("mcp__github__create_pr", Some("Open a PR"), 1500),
        ];
        let c = ctx(
            &tools,
            Some("write the report"), // doesn't mention Notion
            &["mcp__notion__search"],
            &[],
        );
        let comp = HeuristicToolSetCompressor {
            min_score: 1,
            min_kept: 0,
        };
        let d = comp.prune(&c);
        assert!(d.keep_indices.contains(&0), "Notion must be sticky-kept");
    }

    #[test]
    fn min_kept_floor_revives_top_scoring() {
        let tools = [
            tool("mcp__a__x", Some("alpha"), 1000),
            tool("mcp__b__y", Some("beta gamma"), 1000),
            tool("mcp__c__z", Some("delta"), 1000),
        ];
        let c = ctx(&tools, Some("kfjlkjsdf"), &[], &[]);
        let comp = HeuristicToolSetCompressor {
            min_score: 99,
            min_kept: 2,
        };
        let d = comp.prune(&c);
        assert_eq!(d.keep_indices.len(), 2, "floor honored");
        // Deterministic tie-break: equal scores → lex order on name → a, b
        assert_eq!(d.keep_indices, vec![0, 1]);
    }

    #[test]
    fn unknown_tool_always_kept() {
        let tools = [ToolView {
            name: None,
            description: None,
            size_bytes: 100,
        }];
        let c = ctx(&tools, Some("anything"), &[], &[]);
        let d = HeuristicToolSetCompressor::default().prune(&c);
        assert_eq!(d.keep_indices, vec![0]);
    }

    #[test]
    fn reports_byte_savings() {
        let tools = [
            tool("Bash", None, 500),
            tool("mcp__x__y", Some("irrelevant"), 5000),
        ];
        let c = ctx(&tools, Some("hello"), &[], &["Bash"]);
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
