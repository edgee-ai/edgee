use std::collections::HashMap;

use crate::config::CompressionConfig;

/// Compress tool-result content in a provider-native Anthropic Messages API body.
///
/// Operates directly on the raw JSON (no round-trip through typed structs) so
/// Anthropic-specific fields (thinking blocks, cache_control, images, etc.) are
/// preserved untouched.
///
/// Two sweeps:
///   1. Index `tool_use` blocks from `role:"assistant"` messages → `id → (name, input_json)`
///   2. For each `tool_result` block in `role:"user"` messages, look up the tool name and
///      compress the content in-place.
pub fn compress_passthrough_body(config: &CompressionConfig, body: &mut serde_json::Value) {
    let messages = match body.get_mut("messages") {
        Some(serde_json::Value::Array(m)) => m,
        _ => return,
    };

    // Sweep 1: build tool_use_id → (name, serialized_input) from assistant messages
    let mut tool_use_map: HashMap<String, (String, String)> = HashMap::new();
    for msg in messages.iter() {
        if msg.get("role").and_then(|r| r.as_str()) != Some("assistant") {
            continue;
        }
        let Some(serde_json::Value::Array(content)) = msg.get("content") else {
            continue;
        };
        for block in content {
            if block.get("type").and_then(|t| t.as_str()) != Some("tool_use") {
                continue;
            }
            let Some(id) = block.get("id").and_then(|v| v.as_str()) else {
                continue;
            };
            let Some(name) = block.get("name").and_then(|v| v.as_str()) else {
                continue;
            };
            let arguments = block
                .get("input")
                .map(|v| v.to_string())
                .unwrap_or_default();
            tool_use_map.insert(id.to_owned(), (name.to_owned(), arguments));
        }
    }

    // Sweep 2: compress tool_result content in user messages
    let mut tools_checked: u32 = 0;
    let mut tools_compressed: u32 = 0;
    let mut bytes_before: usize = 0;
    let mut bytes_after: usize = 0;

    for msg in messages.iter_mut() {
        if msg.get("role").and_then(|r| r.as_str()) != Some("user") {
            continue;
        }
        let Some(serde_json::Value::Array(content)) = msg.get_mut("content") else {
            continue;
        };
        for block in content.iter_mut() {
            if block.get("type").and_then(|t| t.as_str()) != Some("tool_result") {
                continue;
            }
            let Some(tool_use_id) = block
                .get("tool_use_id")
                .and_then(|v| v.as_str())
                .map(str::to_owned)
            else {
                continue;
            };
            let Some((name, arguments)) = tool_use_map.get(&tool_use_id) else {
                continue;
            };

            tools_checked += 1;

            match block.get_mut("content") {
                Some(c @ serde_json::Value::String(_)) => {
                    let text = c.as_str().unwrap().to_owned();
                    bytes_before += text.len();
                    if let Some(compressed) =
                        crate::dispatch::compress_with_agent(config, name, arguments, &text)
                    {
                        bytes_after += compressed.len();
                        tools_compressed += 1;
                        *c = serde_json::Value::String(compressed);
                    } else {
                        bytes_after += text.len();
                    }
                }
                Some(serde_json::Value::Array(cbs)) => {
                    for cb in cbs.iter_mut() {
                        if cb.get("type").and_then(|t| t.as_str()) != Some("text") {
                            continue;
                        }
                        let Some(tv) = cb.get_mut("text") else {
                            continue;
                        };
                        let text = tv.as_str().unwrap_or_default().to_owned();
                        bytes_before += text.len();
                        if let Some(compressed) =
                            crate::dispatch::compress_with_agent(config, name, arguments, &text)
                        {
                            bytes_after += compressed.len();
                            tools_compressed += 1;
                            *tv = serde_json::Value::String(compressed);
                        } else {
                            bytes_after += text.len();
                        }
                    }
                }
                _ => {}
            }
        }
    }

    if tools_checked > 0 {
        tracing::debug!(
            tools_checked,
            tools_compressed,
            bytes_before,
            bytes_after,
            "compression complete",
        );
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::config::{AgentType, CompressionConfig};

    use super::compress_passthrough_body;

    fn glob_output(n: usize) -> String {
        let dirs = ["src/alpha", "src/beta", "src/gamma", "src/delta"];
        (0..n)
            .map(|i| format!("{}/file_{i}.rs", dirs[i % dirs.len()]))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn compresses_tool_result_string_content() {
        let output = glob_output(50);
        let original_len = output.len();

        let mut body = json!({
            "model": "claude-3-5-sonnet",
            "messages": [
                {"role": "user", "content": "list files"},
                {
                    "role": "assistant",
                    "content": [
                        {
                            "type": "tool_use",
                            "id": "tu_1",
                            "name": "Glob",
                            "input": {"pattern": "**/*.rs"}
                        }
                    ]
                },
                {
                    "role": "user",
                    "content": [
                        {
                            "type": "tool_result",
                            "tool_use_id": "tu_1",
                            "content": output
                        }
                    ]
                }
            ]
        });

        let config = CompressionConfig {
            agent: AgentType::Claude,
        };
        compress_passthrough_body(&config, &mut body);

        let compressed = body["messages"][2]["content"][0]["content"]
            .as_str()
            .unwrap();
        assert!(
            compressed.len() < original_len,
            "expected compression: {} < {original_len}",
            compressed.len()
        );
    }

    #[test]
    fn compresses_tool_result_block_array_content() {
        let output = glob_output(50);
        let original_len = output.len();

        let mut body = json!({
            "model": "claude-3-5-sonnet",
            "messages": [
                {"role": "user", "content": "list files"},
                {
                    "role": "assistant",
                    "content": [
                        {
                            "type": "tool_use",
                            "id": "tu_1",
                            "name": "Glob",
                            "input": {"pattern": "**/*.rs"}
                        }
                    ]
                },
                {
                    "role": "user",
                    "content": [
                        {
                            "type": "tool_result",
                            "tool_use_id": "tu_1",
                            "content": [{"type": "text", "text": output}]
                        }
                    ]
                }
            ]
        });

        let config = CompressionConfig {
            agent: AgentType::Claude,
        };
        compress_passthrough_body(&config, &mut body);

        let compressed = body["messages"][2]["content"][0]["content"][0]["text"]
            .as_str()
            .unwrap();
        assert!(
            compressed.len() < original_len,
            "expected compression: {} < {original_len}",
            compressed.len()
        );
    }

    #[test]
    fn skips_unmatched_tool_use_id() {
        let mut body = json!({
            "model": "claude-3-5-sonnet",
            "messages": [
                {
                    "role": "user",
                    "content": [
                        {
                            "type": "tool_result",
                            "tool_use_id": "orphan",
                            "content": "some output"
                        }
                    ]
                }
            ]
        });

        let config = CompressionConfig {
            agent: AgentType::Claude,
        };
        compress_passthrough_body(&config, &mut body);

        assert_eq!(
            body["messages"][0]["content"][0]["content"]
                .as_str()
                .unwrap(),
            "some output"
        );
    }

    #[test]
    fn no_op_on_missing_messages_key() {
        let mut body = json!({"model": "claude-3-5-sonnet"});
        let config = CompressionConfig {
            agent: AgentType::Claude,
        };
        compress_passthrough_body(&config, &mut body);
        assert_eq!(body, json!({"model": "claude-3-5-sonnet"}));
    }
}
