use std::collections::HashMap;

use edgee_ai_gateway_core::{
    CompletionRequest,
    types::{Message, MessageContent, Tool},
};
use edgee_compressor::{HeuristicToolSetCompressor, PruneContext, ToolSetCompressor, ToolView};

use crate::config::{AgentType, CompressionConfig};

/// Walk `req.messages`, compressing tool-result content in-place, then
/// optionally prune the `tools` array down to a cache-stable subset.
///
/// Two sweeps over messages:
///   1. Build `tool_call_id → (name, arguments)` from every AssistantMessage.
///   2. For each ToolMessage, look up the tool name + arguments, compress the
///      content, and replace it if the compressor produced a shorter result.
///
/// Then, if `config.tool_pruning.enabled` and the request's tool definitions
/// exceed the configured threshold, drop MCP tools unlikely to be needed for
/// the current turn. The scoring signal is the **stable** portion of the
/// request (system prompts + first user message) so the pruned `tools`
/// array is byte-identical across every turn of the same conversation,
/// keeping the upstream prompt cache warm. The latest user message is fed
/// in as a **pivot signal** that can restore a previously-pruned MCP — a
/// one-time cache reset in exchange for serving the freshly mentioned tool.
pub fn compress_request(
    config: &CompressionConfig,
    mut req: CompletionRequest,
) -> CompletionRequest {
    // Sweep 1 — index tool calls by id
    let mut call_index: HashMap<String, (String, String)> = HashMap::new();
    for msg in &req.messages {
        if let Message::Assistant(a) = msg
            && let Some(calls) = &a.tool_calls
        {
            for call in calls {
                call_index.insert(
                    call.id.clone(),
                    (call.function.name.clone(), call.function.arguments.clone()),
                );
            }
        }
    }

    // Sweep 2 — compress ToolMessage content
    for msg in &mut req.messages {
        if let Message::Tool(tool_msg) = msg {
            let Some((name, arguments)) = call_index.get(&tool_msg.tool_call_id) else {
                continue;
            };

            let text = tool_msg.content.as_text();

            // Each agent type has different tool-name conventions and output
            // formats. Codex outputs include a header ("Exit code: …\nOutput:\n")
            // that must be stripped before compression, so it uses a dedicated
            // pipeline that handles header stripping + segment protection.
            let compressed = match config.agent {
                AgentType::Codex => {
                    edgee_compressor::compress_codex_tool_output(name, arguments, &text)
                }
                AgentType::Claude => edgee_compressor::claude_compressor_for(name).and_then(|c| {
                    edgee_compressor::compress_claude_tool_with_segment_protection(
                        c, arguments, &text,
                    )
                }),
                AgentType::OpenCode => {
                    edgee_compressor::opencode_compressor_for(name).and_then(|c| {
                        edgee_compressor::compress_claude_tool_with_segment_protection(
                            c, arguments, &text,
                        )
                    })
                }
            };

            if let Some(compressed) = compressed {
                tool_msg.content = MessageContent::Text(compressed);
            }
        }
    }

    // Sweep 3 — prune the request's tool definitions if oversized.
    if config.tool_pruning.enabled {
        prune_tool_definitions(config, &mut req);
    }

    req
}

fn prune_tool_definitions(config: &CompressionConfig, req: &mut CompletionRequest) {
    if req.tools.is_empty() {
        return;
    }

    // Pre-compute serialized sizes once.
    let sizes: Vec<usize> = req
        .tools
        .iter()
        .map(|t| serde_json::to_string(t).map(|s| s.len()).unwrap_or(0))
        .collect();
    let total_size: usize = sizes.iter().sum();

    if total_size < config.tool_pruning.threshold_bytes {
        return;
    }

    let names: Vec<Option<&str>> = req
        .tools
        .iter()
        .map(|t| match t {
            Tool::Function { function } => Some(function.name.as_str()),
            Tool::Unknown(_) => None,
        })
        .collect();
    let descriptions: Vec<Option<&str>> = req
        .tools
        .iter()
        .map(|t| match t {
            Tool::Function { function } => function.description.as_deref(),
            Tool::Unknown(_) => None,
        })
        .collect();

    let views: Vec<ToolView<'_>> = (0..req.tools.len())
        .map(|i| ToolView {
            name: names[i],
            description: descriptions[i],
            size_bytes: sizes[i],
        })
        .collect();

    // Stable signal — concatenation of every System message content with the
    // text of the **first** UserMessage. Both are byte-identical on every
    // turn of the same conversation, so the kept-set is identical too.
    let mut stable_parts: Vec<String> = Vec::new();
    for msg in &req.messages {
        match msg {
            Message::System(s) => stable_parts.push(s.content.as_text()),
            Message::Developer(d) => stable_parts.push(d.content.as_text()),
            _ => {}
        }
    }
    if let Some(first_user) = req.messages.iter().find_map(|m| match m {
        Message::User(u) => Some(u.content.as_text()),
        _ => None,
    }) {
        stable_parts.push(first_user);
    }
    let stable_text = stable_parts.join("\n");

    // Pivot signal — the latest UserMessage, only if distinct from the first.
    // Used by the heuristic to restore a previously-pruned MCP when the user
    // explicitly pivots to it.
    let first_user_text = req.messages.iter().find_map(|m| match m {
        Message::User(u) => Some(u.content.as_text()),
        _ => None,
    });
    let latest_user_text = req.messages.iter().rev().find_map(|m| match m {
        Message::User(u) => Some(u.content.as_text()),
        _ => None,
    });
    let pivot_signal: Option<String> = match (first_user_text, latest_user_text) {
        (Some(first), Some(latest)) if first != latest => Some(latest),
        _ => None,
    };

    let core_tools = config.agent.core_tools();

    let ctx = PruneContext {
        tools: &views,
        stable_text: &stable_text,
        pivot_signal_text: pivot_signal.as_deref(),
        core_tools,
    };

    let compressor = HeuristicToolSetCompressor {
        min_score: config.tool_pruning.min_score,
        min_kept: config.tool_pruning.min_kept,
    };
    let decision = compressor.prune(&ctx);

    if decision.dropped == 0 {
        return;
    }

    let before_tools = views.len();
    drop(views);

    let keep: std::collections::HashSet<usize> = decision.keep_indices.iter().copied().collect();
    let mut idx = 0usize;
    req.tools.retain(|_| {
        let keep_it = keep.contains(&idx);
        idx += 1;
        keep_it
    });

    tracing::info!(
        target: "edgee::compression::tool_pruning",
        before_tools,
        after_tools = req.tools.len(),
        dropped = decision.dropped,
        bytes_before = decision.bytes_before,
        bytes_after = decision.bytes_after,
        "pruned MCP tool definitions"
    );
}

#[cfg(test)]
mod tests {
    use edgee_ai_gateway_core::{
        CompletionRequest,
        types::{
            AssistantMessage, FunctionCall, FunctionDefinition, Message, MessageContent, Tool,
            ToolCall, ToolMessage, UserMessage,
        },
    };

    use crate::config::{AgentType, CompressionConfig, ToolPruningConfig};

    use super::compress_request;

    fn glob_output(n: usize) -> String {
        // Produce `n` fake file paths spread across a few directories so the
        // Glob compressor can actually group them (threshold: >30 paths).
        let dirs = ["src/alpha", "src/beta", "src/gamma", "src/delta"];
        (0..n)
            .map(|i| format!("{}/file_{i}.rs", dirs[i % dirs.len()]))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn compresses_glob_tool_result() {
        let output = glob_output(50);
        let original_len = output.len();

        let req = CompletionRequest::new(
            "claude-3-5-sonnet".to_string(),
            vec![
                Message::User(UserMessage {
                    name: None,
                    content: MessageContent::Text("list files".into()),
                    cache_control: None,
                }),
                Message::Assistant(AssistantMessage {
                    name: None,
                    content: None,
                    refusal: None,
                    cache_control: None,
                    tool_calls: Some(vec![ToolCall {
                        id: "call_1".into(),
                        tool_type: "function".into(),
                        function: FunctionCall {
                            name: "Glob".into(),
                            arguments: r#"{"pattern":"**/*.rs"}"#.into(),
                        },
                    }]),
                }),
                Message::Tool(ToolMessage {
                    tool_call_id: "call_1".into(),
                    content: MessageContent::Text(output),
                }),
            ],
        );

        let config = CompressionConfig {
            agent: AgentType::Claude,
            tool_pruning: ToolPruningConfig::default(),
        };
        let compressed = compress_request(&config, req);

        let tool_msg = compressed.messages.iter().find_map(|m| {
            if let Message::Tool(t) = m {
                Some(t)
            } else {
                None
            }
        });

        let compressed_len = tool_msg.unwrap().content.as_text().len();
        assert!(
            compressed_len < original_len,
            "expected compression: {compressed_len} < {original_len}"
        );
    }

    #[test]
    fn skips_unknown_tool_call_id() {
        let req = CompletionRequest::new(
            "claude-3-5-sonnet".to_string(),
            vec![Message::Tool(ToolMessage {
                tool_call_id: "orphan".into(),
                content: MessageContent::Text("some output".into()),
            })],
        );

        let config = CompressionConfig {
            agent: AgentType::Claude,
            tool_pruning: ToolPruningConfig::default(),
        };
        let result = compress_request(&config, req);

        // Content should be unchanged
        let tool_msg = result.messages.iter().find_map(|m| {
            if let Message::Tool(t) = m {
                Some(t)
            } else {
                None
            }
        });
        assert_eq!(tool_msg.unwrap().content.as_text(), "some output");
    }

    fn mcp_tool(name: &str, description: &str) -> Tool {
        // Pad the parameters schema so each MCP tool comfortably contributes
        // to the byte threshold without us having to hand-roll a huge schema.
        let padded_desc = format!("{description} — {}", "extended description ".repeat(20));
        Tool::Function {
            function: FunctionDefinition {
                name: name.to_string(),
                description: Some(padded_desc),
                parameters: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "query": {"type": "string"},
                        "limit": {"type": "integer"}
                    }
                })),
            },
        }
    }

    fn core_tool(name: &str) -> Tool {
        Tool::Function {
            function: FunctionDefinition {
                name: name.to_string(),
                description: Some("core agent tool".to_string()),
                parameters: Some(serde_json::json!({"type": "object"})),
            },
        }
    }

    #[test]
    fn prunes_irrelevant_mcp_tools_when_oversized() {
        let req = CompletionRequest {
            model: "claude-3-5-sonnet".into(),
            messages: vec![Message::User(UserMessage {
                name: None,
                content: MessageContent::Text("please find the linear issue about payments".into()),
                cache_control: None,
            })],
            max_tokens: None,
            stream: false,
            tools: vec![
                core_tool("Bash"),
                mcp_tool("mcp__linear-server__list_issues", "List Linear issues"),
                mcp_tool("mcp__github__create_pr", "Open a pull request"),
                mcp_tool("mcp__notion__search", "Search Notion pages"),
            ],
            tool_choice: None,
            temperature: None,
            top_p: None,
        };

        let config = CompressionConfig {
            agent: AgentType::Claude,
            tool_pruning: ToolPruningConfig {
                enabled: true,
                threshold_bytes: 0, // force pruning
                min_kept: 1,
                min_score: 1,
            },
        };
        let result = compress_request(&config, req);

        let names: Vec<&str> = result
            .tools
            .iter()
            .filter_map(|t| match t {
                Tool::Function { function } => Some(function.name.as_str()),
                Tool::Unknown(_) => None,
            })
            .collect();

        assert!(names.contains(&"Bash"), "core kept: {names:?}");
        assert!(
            names.contains(&"mcp__linear-server__list_issues"),
            "linear kept: {names:?}"
        );
        assert!(
            !names.contains(&"mcp__github__create_pr"),
            "github should be dropped: {names:?}"
        );
    }

    #[test]
    fn pruning_skipped_below_threshold() {
        let req = CompletionRequest {
            model: "claude-3-5-sonnet".into(),
            messages: vec![Message::User(UserMessage {
                name: None,
                content: MessageContent::Text("hello".into()),
                cache_control: None,
            })],
            max_tokens: None,
            stream: false,
            tools: vec![
                core_tool("Bash"),
                mcp_tool("mcp__notion__search", "Search Notion"),
            ],
            tool_choice: None,
            temperature: None,
            top_p: None,
        };
        let original_count = req.tools.len();

        let config = CompressionConfig {
            agent: AgentType::Claude,
            tool_pruning: ToolPruningConfig {
                enabled: true,
                threshold_bytes: usize::MAX,
                min_kept: 0,
                min_score: 1,
            },
        };
        let result = compress_request(&config, req);
        assert_eq!(
            result.tools.len(),
            original_count,
            "below threshold → untouched"
        );
    }

    #[test]
    fn pruning_disabled_keeps_everything() {
        let req = CompletionRequest {
            model: "claude-3-5-sonnet".into(),
            messages: vec![Message::User(UserMessage {
                name: None,
                content: MessageContent::Text("anything".into()),
                cache_control: None,
            })],
            max_tokens: None,
            stream: false,
            tools: vec![
                mcp_tool("mcp__a__x", "first"),
                mcp_tool("mcp__b__y", "second"),
            ],
            tool_choice: None,
            temperature: None,
            top_p: None,
        };
        let original_count = req.tools.len();

        let config = CompressionConfig {
            agent: AgentType::Claude,
            tool_pruning: ToolPruningConfig {
                enabled: false,
                threshold_bytes: 0,
                min_kept: 0,
                min_score: 99,
            },
        };
        let result = compress_request(&config, req);
        assert_eq!(result.tools.len(), original_count);
    }

    #[test]
    fn pivot_restores_previously_pruned_mcp() {
        // Turn 1's user message mentions only Linear → GitHub is pruned.
        // Turn 4's user message mentions GitHub → restored on that turn.
        let req = CompletionRequest {
            model: "claude-3-5-sonnet".into(),
            messages: vec![
                Message::User(UserMessage {
                    name: None,
                    content: MessageContent::Text("find the linear ticket about payments".into()),
                    cache_control: None,
                }),
                Message::Assistant(AssistantMessage {
                    name: None,
                    content: Some(MessageContent::Text("done".into())),
                    refusal: None,
                    cache_control: None,
                    tool_calls: None,
                }),
                Message::User(UserMessage {
                    name: None,
                    content: MessageContent::Text("now check github for related prs".into()),
                    cache_control: None,
                }),
            ],
            max_tokens: None,
            stream: false,
            tools: vec![
                mcp_tool("mcp__linear-server__list_issues", "List Linear issues"),
                mcp_tool("mcp__github__create_pr", "Open a pull request"),
                mcp_tool("mcp__notion__search", "Search Notion pages"),
            ],
            tool_choice: None,
            temperature: None,
            top_p: None,
        };

        let config = CompressionConfig {
            agent: AgentType::Claude,
            tool_pruning: ToolPruningConfig {
                enabled: true,
                threshold_bytes: 0,
                min_kept: 0,
                min_score: 1,
            },
        };
        let result = compress_request(&config, req);
        let names: Vec<&str> = result
            .tools
            .iter()
            .filter_map(|t| match t {
                Tool::Function { function } => Some(function.name.as_str()),
                _ => None,
            })
            .collect();

        assert!(
            names.contains(&"mcp__linear-server__list_issues"),
            "stable-kept Linear: {names:?}"
        );
        assert!(
            names.contains(&"mcp__github__create_pr"),
            "pivot should restore GitHub: {names:?}"
        );
        assert!(
            !names.contains(&"mcp__notion__search"),
            "unrelated Notion still pruned: {names:?}"
        );
    }

    #[test]
    fn tools_array_is_byte_identical_across_turns() {
        // Cache-safety property: the serialized `tools` field of two
        // requests sharing the same system prompt + first user message
        // (but with different later messages and different latest user
        // messages that don't mention a pruned tool) must be identical.
        fn build_req(extra_user_msgs: Vec<&str>) -> CompletionRequest {
            let mut messages = vec![Message::User(UserMessage {
                name: None,
                content: MessageContent::Text(
                    "list the linear issues for the payments project".into(),
                ),
                cache_control: None,
            })];
            for m in extra_user_msgs {
                messages.push(Message::Assistant(AssistantMessage {
                    name: None,
                    content: Some(MessageContent::Text("ok".into())),
                    refusal: None,
                    cache_control: None,
                    tool_calls: None,
                }));
                messages.push(Message::User(UserMessage {
                    name: None,
                    content: MessageContent::Text(m.into()),
                    cache_control: None,
                }));
            }
            CompletionRequest {
                model: "claude-3-5-sonnet".into(),
                messages,
                max_tokens: None,
                stream: false,
                tools: vec![
                    core_tool("Bash"),
                    mcp_tool("mcp__linear-server__list_issues", "List Linear issues"),
                    mcp_tool("mcp__github__create_pr", "Open a pull request"),
                    mcp_tool("mcp__notion__search", "Search Notion pages"),
                ],
                tool_choice: None,
                temperature: None,
                top_p: None,
            }
        }

        let config = CompressionConfig {
            agent: AgentType::Claude,
            tool_pruning: ToolPruningConfig {
                enabled: true,
                threshold_bytes: 0,
                min_kept: 0,
                min_score: 1,
            },
        };

        let turn1 = compress_request(&config, build_req(vec![]));
        let turn5 = compress_request(
            &config,
            build_req(vec!["and the next one", "and the one after that"]),
        );
        let turn20 = compress_request(&config, build_req(vec!["summarize so far", "tell me more"]));

        let names = |r: &CompletionRequest| {
            r.tools
                .iter()
                .filter_map(|t| match t {
                    Tool::Function { function } => Some(function.name.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };

        assert_eq!(names(&turn1), names(&turn5), "turn1 == turn5");
        assert_eq!(names(&turn5), names(&turn20), "turn5 == turn20");

        // And the actual JSON bytes of the tools array — the property the
        // upstream prompt cache actually checks.
        let bytes = |r: &CompletionRequest| serde_json::to_vec(&r.tools).unwrap();
        assert_eq!(bytes(&turn1), bytes(&turn5));
        assert_eq!(bytes(&turn5), bytes(&turn20));
    }

    #[test]
    fn edgee_mcp_tools_never_pruned() {
        // Even when the user's stable text shares nothing with edgee's
        // session-instrumentation tools, they must survive — they're
        // gateway-internal and required on every request.
        let req = CompletionRequest {
            model: "claude-opus-4-7".into(),
            messages: vec![Message::User(UserMessage {
                name: None,
                content: MessageContent::Text("write a poem about cats".into()),
                cache_control: None,
            })],
            max_tokens: None,
            stream: false,
            tools: vec![
                core_tool("Bash"),
                mcp_tool(
                    "mcp__edgee__setSessionName",
                    "Set a human-readable display name for an AI Gateway session",
                ),
                mcp_tool(
                    "mcp__edgee__addSessionPullRequest",
                    "Associate a pull request with an AI Gateway session",
                ),
                mcp_tool(
                    "mcp__edgee__setSessionGitHubRepo",
                    "Set the GitHub repository associated with an AI Gateway session",
                ),
                mcp_tool(
                    "mcp__edgee__addSessionCommit",
                    "Associate a commit with an AI Gateway session",
                ),
                mcp_tool("mcp__github__create_pr", "Open a pull request"),
            ],
            tool_choice: None,
            temperature: None,
            top_p: None,
        };

        let config = CompressionConfig {
            agent: AgentType::Claude,
            tool_pruning: ToolPruningConfig {
                enabled: true,
                threshold_bytes: 0,
                min_kept: 0,
                min_score: 99,
            },
        };
        let result = compress_request(&config, req);
        let names: Vec<&str> = result
            .tools
            .iter()
            .filter_map(|t| match t {
                Tool::Function { function } => Some(function.name.as_str()),
                _ => None,
            })
            .collect();

        for edgee in [
            "mcp__edgee__setSessionName",
            "mcp__edgee__addSessionPullRequest",
            "mcp__edgee__setSessionGitHubRepo",
            "mcp__edgee__addSessionCommit",
        ] {
            assert!(
                names.contains(&edgee),
                "{edgee} must be kept regardless of score; got {names:?}"
            );
        }
        assert!(
            !names.contains(&"mcp__github__create_pr"),
            "unrelated MCP still pruned: {names:?}"
        );
    }

    #[test]
    fn ignores_claude_code_injected_blocks_in_first_user_message() {
        // Faithful to the real Claude Code request shape: first user message is
        // a multi-part content with a stack of <system-reminder> blocks
        // mentioning every connected MCP server in skill descriptions, plus a
        // tiny actual user input at the end. Without scrubbing, the lexical
        // scorer matches every MCP and keeps 100% of them.
        let injected = "<system-reminder>\n# MCP Server Instructions\n\nThe following MCP servers have provided instructions:\n## linear-server\nWhen passing strings, send content directly.\n</system-reminder>\n\n<system-reminder>\nThe following skills are available:\n- figma:figma-implement-design: translates Figma designs into code\n- figma:figma-use: prerequisite for use_figma tool calls\n- frontend-design: build web components\n- mobile-ios-design: SwiftUI patterns for iOS\n- content-creator: SEO content for blog posts and social media\n</system-reminder>\n\n<local-command-caveat>caveat text</local-command-caveat>\n\n<command-name>/clear</command-name>\n<command-message>clear</command-message>\n<command-args></command-args>\n<local-command-stdout></local-command-stdout>\n\nCan you tell me more about this ?";

        let req = CompletionRequest {
            model: "claude-opus-4-7".into(),
            messages: vec![Message::User(UserMessage {
                name: None,
                content: MessageContent::Text(injected.into()),
                cache_control: None,
            })],
            max_tokens: None,
            stream: false,
            tools: vec![
                core_tool("Bash"),
                mcp_tool("mcp__linear-server__list_issues", "List Linear issues"),
                mcp_tool("mcp__plugin_figma_figma__authenticate", "Figma auth"),
                mcp_tool("mcp__claude_ai_Gmail__authenticate", "Gmail auth"),
                mcp_tool(
                    "mcp__claude_ai_Google_Calendar__authenticate",
                    "Calendar auth",
                ),
                mcp_tool("mcp__claude_ai_Google_Drive__authenticate", "Drive auth"),
            ],
            tool_choice: None,
            temperature: None,
            top_p: None,
        };
        let original_mcp_count = req
            .tools
            .iter()
            .filter(|t| match t {
                Tool::Function { function } => function.name.starts_with("mcp__"),
                _ => false,
            })
            .count();

        let config = CompressionConfig {
            agent: AgentType::Claude,
            tool_pruning: ToolPruningConfig {
                enabled: true,
                threshold_bytes: 0,
                min_kept: 0,
                min_score: 1,
            },
        };
        let result = compress_request(&config, req);
        let kept_mcps: Vec<&str> = result
            .tools
            .iter()
            .filter_map(|t| match t {
                Tool::Function { function } if function.name.starts_with("mcp__") => {
                    Some(function.name.as_str())
                }
                _ => None,
            })
            .collect();

        assert!(
            kept_mcps.len() < original_mcp_count,
            "expected some MCPs pruned despite skill-list pollution; got all {} kept: {:?}",
            kept_mcps.len(),
            kept_mcps
        );
    }
}
