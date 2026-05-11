use std::collections::HashMap;

use edgee_ai_gateway_core::{
    CompletionRequest,
    types::{Message, MessageContent},
};

use crate::config::{AgentType, CompressionConfig};

/// Walk `req.messages`, compressing tool-result content in-place.
///
/// Two sweeps:
///   1. Build `tool_call_id → (name, arguments)` from every AssistantMessage.
///   2. For each ToolMessage, look up the tool name + arguments, compress the
///      content, and replace it if the compressor produced a shorter result.
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
            let original_len = text.len();

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

            match compressed {
                Some(compressed) => {
                    config
                        .metrics
                        .record_compression(name, original_len, compressed.len());
                    tool_msg.content = MessageContent::Text(compressed);
                }
                None => {
                    config.metrics.record_skip(name, original_len);
                }
            }
        }
    }

    req
}

#[cfg(test)]
mod tests {
    use edgee_ai_gateway_core::{
        CompletionRequest,
        types::{
            AssistantMessage, FunctionCall, Message, MessageContent, ToolCall, ToolMessage,
            UserMessage,
        },
    };

    use crate::config::{AgentType, CompressionConfig};

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

        let config = CompressionConfig::new(AgentType::Claude);
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

        let config = CompressionConfig::new(AgentType::Claude);
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

    #[test]
    fn metrics_record_compression_outcome() {
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

        let config = CompressionConfig::new(AgentType::Claude);
        let _ = compress_request(&config, req);

        let snap = config.metrics.snapshot();
        assert_eq!(snap.len(), 1, "exactly one tool recorded");
        let (name, stats) = &snap[0];
        assert_eq!(name, "Glob");
        assert_eq!(stats.invocations, 1);
        assert_eq!(stats.skipped, 0);
        assert_eq!(stats.bytes_in, original_len as u64);
        assert!(
            stats.bytes_out < stats.bytes_in,
            "bytes_out should be smaller than bytes_in, got {} vs {}",
            stats.bytes_out,
            stats.bytes_in
        );
    }
}
