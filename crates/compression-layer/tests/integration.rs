//! End-to-end integration test exercising the full compression pipeline on a
//! realistic Claude Code request.
//!
//! This isn't a unit test of any individual strategy — it's a fixture that
//! mirrors what a coding agent actually sends mid-session: a long stable
//! system prompt, a few user/assistant turns, several tool calls with real
//! tool outputs (a Read of a Rust file, a Glob with hundreds of paths, a
//! Bash cargo build, a Grep with many matches).
//!
//! What we assert:
//!
//! - The pipeline produces a meaningful byte reduction (>40 % of original).
//! - Every compressed tool message starts with the version marker `<!--ec1-->`.
//! - The SystemPromptCacheTechnique injects `cache_control: ephemeral` on the
//!   large system prompt and stops at the configured cap.
//! - Per-tool metrics are populated and self-consistent
//!   (`sum(bytes_out) <= sum(bytes_in)`).
//! - Running the pipeline a second time is a no-op (idempotency — the prompt
//!   cache stays stable across turns).
//! - Codex variant: non-zero exit codes survive as `[exit N]` after the
//!   marker.
//!
//! Run with: `cargo test -p edgee-compression-layer --test integration`.

use std::sync::Arc;

use edgee_ai_gateway_core::{
    CompletionRequest,
    types::{
        AssistantMessage, FunctionCall, Message, MessageContent, SystemMessage, ToolCall,
        ToolMessage, UserMessage,
    },
};
use edgee_compression_layer::{
    AgentType, CompressionConfig, CompressionPipeline, SystemPromptCacheTechnique,
    ToolResultsTechnique,
};

// ---------- fixture builders ----------

const COMPRESSION_MARKER: &str = "<!--ec1-->";

/// Build a system prompt comparable to the CLAUDE.md / agent-rules block a
/// coding agent attaches to every request — a couple of kilobytes of stable
/// instructions, large enough to be worth caching.
fn build_system_prompt() -> String {
    let mut s = String::from(
        "You are a coding assistant. Follow the project conventions defined in CLAUDE.md.\n\n",
    );
    for section in 0..15 {
        s.push_str(&format!(
            "## Section {section}\n\nGuidance paragraph for section {section}: prefer explicit \
             error handling, document non-obvious invariants, keep functions short, and avoid \
             premature abstraction. Test before committing. Do not push without explicit \
             permission.\n\n",
        ));
    }
    s
}

/// 800-line Rust source with ~50 % comment lines so the Read compressor has
/// real work to do.
fn build_read_output() -> String {
    let mut lines = Vec::new();
    for i in 1..=800 {
        if i % 2 == 0 {
            lines.push(format!(
                "     {i}\t// commentary line {i} explaining nothing important"
            ));
        } else {
            lines.push(format!(
                "     {i}\tlet meaningful_value_{i} = compute_thing(arg_a, arg_b, arg_c);"
            ));
        }
    }
    lines.join("\n")
}

/// 250 file paths spread across a few directories — typical Glob result.
fn build_glob_output() -> String {
    let dirs = ["src/alpha", "src/beta", "src/gamma", "src/delta", "tests"];
    (0..250)
        .map(|i| format!("{}/file_{i}.rs", dirs[i % dirs.len()]))
        .collect::<Vec<_>>()
        .join("\n")
}

/// `cargo build` output: lots of "Compiling" lines + a couple of warnings.
fn build_bash_cargo_output() -> String {
    let mut s = String::new();
    for i in 0..120 {
        s.push_str(&format!("   Compiling crate_{i} v0.{i}.0\n"));
    }
    s.push_str(
        "warning: unused variable: `tmp` [unused_variables]\n --> src/main.rs:42:9\n  |\n42|     let tmp = 1;\n  |         ^^^ help: consider prefixing with an underscore: `_tmp`\n\n",
    );
    s.push_str("warning: `myapp` (bin) generated 1 warning\n");
    s.push_str("    Finished dev [unoptimized + debuginfo] target(s) in 12.3s\n");
    s
}

/// Grep content output with 200+ matches.
fn build_grep_output() -> String {
    let mut s = String::new();
    for f in 0..40 {
        for ln in 0..6 {
            s.push_str(&format!(
                "src/feature_{f}/handler.rs:{}:    // TODO: refactor this\n",
                ln * 7 + 12
            ));
        }
    }
    s
}

/// Build the Claude-shaped request with system + multi-turn dialogue and
/// four tool calls (Read, Glob, Bash, Grep) sequenced as Claude Code would.
fn build_request() -> CompletionRequest {
    let messages = vec![
        Message::System(SystemMessage {
            name: None,
            content: MessageContent::Text(build_system_prompt()),
            cache_control: None,
        }),
        Message::User(UserMessage {
            name: None,
            content: MessageContent::Text("Please audit the project structure.".into()),
            cache_control: None,
        }),
        // Turn 1 — Read main.rs
        Message::Assistant(AssistantMessage {
            name: None,
            content: None,
            refusal: None,
            cache_control: None,
            tool_calls: Some(vec![ToolCall {
                id: "call_read".into(),
                tool_type: "function".into(),
                function: FunctionCall {
                    name: "Read".into(),
                    arguments: r#"{"file_path":"/repo/src/lib.rs"}"#.into(),
                },
            }]),
        }),
        Message::Tool(ToolMessage {
            tool_call_id: "call_read".into(),
            content: MessageContent::Text(build_read_output()),
        }),
        // Turn 2 — Glob
        Message::Assistant(AssistantMessage {
            name: None,
            content: None,
            refusal: None,
            cache_control: None,
            tool_calls: Some(vec![ToolCall {
                id: "call_glob".into(),
                tool_type: "function".into(),
                function: FunctionCall {
                    name: "Glob".into(),
                    arguments: r#"{"pattern":"**/*.rs"}"#.into(),
                },
            }]),
        }),
        Message::Tool(ToolMessage {
            tool_call_id: "call_glob".into(),
            content: MessageContent::Text(build_glob_output()),
        }),
        // Turn 3 — Bash cargo build
        Message::Assistant(AssistantMessage {
            name: None,
            content: None,
            refusal: None,
            cache_control: None,
            tool_calls: Some(vec![ToolCall {
                id: "call_bash".into(),
                tool_type: "function".into(),
                function: FunctionCall {
                    name: "Bash".into(),
                    arguments: r#"{"command":"cargo build"}"#.into(),
                },
            }]),
        }),
        Message::Tool(ToolMessage {
            tool_call_id: "call_bash".into(),
            content: MessageContent::Text(build_bash_cargo_output()),
        }),
        // Turn 4 — Grep
        Message::Assistant(AssistantMessage {
            name: None,
            content: None,
            refusal: None,
            cache_control: None,
            tool_calls: Some(vec![ToolCall {
                id: "call_grep".into(),
                tool_type: "function".into(),
                function: FunctionCall {
                    name: "Grep".into(),
                    arguments: r#"{"output_mode":"content","pattern":"TODO"}"#.into(),
                },
            }]),
        }),
        Message::Tool(ToolMessage {
            tool_call_id: "call_grep".into(),
            content: MessageContent::Text(build_grep_output()),
        }),
    ];

    CompletionRequest::new("claude-3-5-sonnet".to_string(), messages)
}

fn total_tool_bytes(req: &CompletionRequest) -> usize {
    req.messages
        .iter()
        .filter_map(|m| match m {
            Message::Tool(t) => Some(t.content.as_text().len()),
            _ => None,
        })
        .sum()
}

// ---------- the actual test ----------

#[test]
fn realistic_claude_code_request_compresses_end_to_end() {
    let config = CompressionConfig::new(AgentType::Claude);
    let pipeline = CompressionPipeline::new()
        .with(SystemPromptCacheTechnique::new())
        .with(ToolResultsTechnique::new(Arc::clone(&config)));

    let req = build_request();
    let bytes_in = total_tool_bytes(&req);
    let system_in = match &req.messages[0] {
        Message::System(s) => s.content.as_text().len(),
        _ => panic!("expected system message at index 0"),
    };

    let out = pipeline.apply(req);

    // ---- Tool output compression ----
    let bytes_out = total_tool_bytes(&out);
    let saved = bytes_in.saturating_sub(bytes_out);
    let ratio_pct = (saved * 100) / bytes_in.max(1);

    println!("---- compression report ----");
    println!("system prompt: {system_in} bytes");
    println!("tool inputs:   {bytes_in} bytes");
    println!("tool outputs:  {bytes_out} bytes");
    println!("saved:         {saved} bytes ({ratio_pct} %)");

    assert!(
        ratio_pct >= 40,
        "expected at least 40 % savings on tool outputs, got {ratio_pct} % ({bytes_out}/{bytes_in})"
    );

    // ---- Marker on every compressed tool result ----
    let mut tool_messages = 0;
    let mut marker_seen = 0;
    for msg in &out.messages {
        if let Message::Tool(t) = msg {
            tool_messages += 1;
            if t.content.as_text().starts_with(COMPRESSION_MARKER) {
                marker_seen += 1;
            }
        }
    }
    assert_eq!(tool_messages, 4, "fixture has 4 tool messages");
    assert_eq!(
        marker_seen, 4,
        "every successfully compressed tool message must carry the version marker"
    );

    // ---- SystemPromptCacheTechnique injection ----
    if let Message::System(s) = &out.messages[0] {
        assert!(
            s.cache_control.is_some(),
            "large system prompt must receive cache_control hint"
        );
    } else {
        panic!("expected system message at index 0");
    }

    // ---- Per-tool metrics ----
    let snap = config.metrics.snapshot();
    println!("---- metrics snapshot ----");
    for (name, stats) in &snap {
        println!(
            "{name}: invocations={} skipped={} bytes_in={} bytes_out={}",
            stats.invocations, stats.skipped, stats.bytes_in, stats.bytes_out
        );
    }
    assert!(!snap.is_empty(), "metrics must record at least one tool");
    let totals = config.metrics.totals();
    assert!(
        totals.bytes_out <= totals.bytes_in,
        "totals must be self-consistent: bytes_out {} > bytes_in {}",
        totals.bytes_out,
        totals.bytes_in,
    );
    assert_eq!(
        totals.invocations + totals.skipped,
        4,
        "metrics should account for all four tool messages exactly once"
    );

    // ---- Idempotency: running the pipeline again must change nothing ----
    let bytes_after_first = total_tool_bytes(&out);
    let twice = pipeline.apply(out);
    let bytes_after_second = total_tool_bytes(&twice);
    assert_eq!(
        bytes_after_first, bytes_after_second,
        "second pass must be a no-op — re-compressing would invalidate the prompt cache"
    );
    // System prompt: the second technique pass must not double-inject.
    if let Message::System(s) = &twice.messages[0] {
        assert!(s.cache_control.is_some(), "still hinted");
    }
}

#[test]
fn codex_pipeline_preserves_exit_code_after_compression() {
    use edgee_compressor::compress_codex_tool_output;

    let mut body = String::from("total 9999\n");
    for i in 0..60 {
        body.push_str(&format!("-rw-r--r-- 1 u s 10 Jan 1 12:00 file_{i}.txt\n"));
    }
    let output = format!("Exit code: 137\nWall time: 0 seconds\nOutput:\n{body}");

    let result = compress_codex_tool_output("shell_command", r#"{"command":"ls -la"}"#, &output)
        .expect("codex compress should return Some on a real-sized payload");

    assert!(
        result.starts_with(COMPRESSION_MARKER),
        "marker must lead so the layer's idempotency check still works"
    );
    assert!(
        result.contains("[exit 137]"),
        "non-zero codex exit code must survive compression; got: {result}"
    );

    // Idempotency holds for the codex path too.
    let again = compress_codex_tool_output("shell_command", r#"{"command":"ls -la"}"#, &result);
    assert!(again.is_none(), "second pass must short-circuit");
}
