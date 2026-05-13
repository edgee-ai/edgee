use std::collections::HashMap;

use edgee_ai_gateway_core::{
    CompletionRequest,
    types::{Message, MessageContent, Tool},
};
use edgee_compressor::{HeuristicToolSetCompressor, PruneContext, ToolSetCompressor, ToolView};

use crate::config::{AgentType, CompressionConfig};

/// Walk `req.messages`, compressing tool-result content in-place, then
/// optionally prune the `tools` array down to a relevant subset.
///
/// Two sweeps over messages:
///   1. Build `tool_call_id → (name, arguments)` from every AssistantMessage.
///   2. For each ToolMessage, look up the tool name + arguments, compress the
///      content, and replace it if the compressor produced a shorter result.
///
/// Then, if `config.tool_pruning.enabled` and the request's tool definitions
/// exceed the configured threshold, drop MCP tools unlikely to be needed for
/// the current turn. Core (agent-builtin) and previously invoked tools are
/// always retained.
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
        prune_tool_definitions(config, &mut req, &call_index);
    }

    req
}

fn prune_tool_definitions(
    config: &CompressionConfig,
    req: &mut CompletionRequest,
    call_index: &HashMap<String, (String, String)>,
) {
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

    // Latest user message text (most recent UserMessage).
    let latest_user_text = req.messages.iter().rev().find_map(|m| match m {
        Message::User(u) => Some(u.content.as_text()),
        _ => None,
    });

    // Tools the assistant has already invoked in this conversation.
    let prior_names: Vec<&str> = call_index.values().map(|(n, _)| n.as_str()).collect();

    let core_tools = config.agent.core_tools();

    let ctx = PruneContext {
        tools: &views,
        latest_user_text: latest_user_text.as_deref(),
        prior_tool_call_names: &prior_names,
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
    fn sticky_rule_keeps_previously_invoked_mcp() {
        let req = CompletionRequest {
            model: "claude-3-5-sonnet".into(),
            messages: vec![
                Message::User(UserMessage {
                    name: None,
                    content: MessageContent::Text("write the report".into()),
                    cache_control: None,
                }),
                Message::Assistant(AssistantMessage {
                    name: None,
                    content: None,
                    refusal: None,
                    cache_control: None,
                    tool_calls: Some(vec![ToolCall {
                        id: "call_prev".into(),
                        tool_type: "function".into(),
                        function: FunctionCall {
                            name: "mcp__notion__search".into(),
                            arguments: "{}".into(),
                        },
                    }]),
                }),
                Message::Tool(ToolMessage {
                    tool_call_id: "call_prev".into(),
                    content: MessageContent::Text("ok".into()),
                }),
                Message::User(UserMessage {
                    name: None,
                    content: MessageContent::Text("now write it up".into()),
                    cache_control: None,
                }),
            ],
            max_tokens: None,
            stream: false,
            tools: vec![
                mcp_tool("mcp__notion__search", "Search Notion pages"),
                mcp_tool("mcp__github__create_pr", "Open a PR"),
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

        assert!(
            names.contains(&"mcp__notion__search"),
            "sticky-kept: {names:?}"
        );
    }
}
