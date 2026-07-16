# edgee-compressor

Pure tool-output compression library for AI coding agents.

## Role in the stack

This crate has no network I/O and no async runtime dependency. The Edgee
hosted / on-prem gateway depends on it (from crates.io) to compress tool
results before they are forwarded to LLM providers. Adding a new compression
strategy only requires changes inside this crate.

```
Edgee gateway (AWS / Fastly / on-prem)
        |
  edgee-compressor      <-- this crate
    strategy/claude/
    strategy/codex/
    strategy/opencode/
    strategy/bash/
```

## Key types and functions

| Item                                                                     | Description                                                                                              |
| ------------------------------------------------------------------------ | -------------------------------------------------------------------------------------------------------- |
| `ToolCompressor` trait                                                   | `compress(arguments, output) -> Option<String>`; `None` means "keep original"                            |
| `BashCompressor` trait                                                   | Like `ToolCompressor` but also receives the parsed command name                                          |
| `compress_tool_output(tool, args, output)`                               | Main entry point: looks up the Claude Code compressor and applies it with `<system-reminder>` protection |
| `compress_claude_tool_with_segment_protection(compressor, args, output)` | Applies any `ToolCompressor` while preserving `<system-reminder>` blocks verbatim                        |
| `claude_compressor_for(tool_name)`                                       | Returns the compressor for a Claude Code tool name                                                       |
| `codex_compressor_for(tool_name)`                                        | Returns the compressor for a Codex CLI tool name                                                         |
| `opencode_compressor_for(tool_name)`                                     | Returns the compressor for an OpenCode tool name                                                         |
| `bash_compressor_for(command)`                                           | Returns the per-command compressor for bash output                                                       |

## Strategy layout

Compressors live under `src/strategy/`:

| Path                 | Agent       | Tool names                                                   |
| -------------------- | ----------- | ------------------------------------------------------------ |
| `strategy/claude/`   | Claude Code | `Bash`, `Read`, `Grep`, `Glob`                               |
| `strategy/codex/`    | Codex CLI   | `shell_command`, `read_file`, ...                            |
| `strategy/opencode/` | OpenCode    | `bash`, `read`, ...                                          |
| `strategy/bash/`     | (shared)    | `ls`, `cargo`, `npm`, `tsc`, `pytest`, `diff`, `docker`, ... |

## Usage

```rust
use edgee_compressor::compress_tool_output;

// Compress the output of a Claude Code Read tool call.
let result = compress_tool_output(
    "Read",
    r#"{"file_path": "src/main.rs"}"#,
    &long_file_contents,
);
match result {
    Some(shorter) => { /* forward shorter to the LLM */ }
    None => { /* no compressor registered; use original */ }
}
```

Compressing for a different agent:

```rust
use edgee_compressor::{codex_compressor_for, compress_claude_tool_with_segment_protection};

if let Some(compressor) = codex_compressor_for("read_file") {
    let result = compress_claude_tool_with_segment_protection(compressor, args, output);
}
```

## Extending compression

### ToolCompressor trait

Every compression strategy implements one of two traits:

```rust
pub trait ToolCompressor: Send + Sync {
    fn compress(&self, arguments: &str, output: &str) -> Option<String>;
}

pub trait BashCompressor: Send + Sync {
    fn compress(&self, command: &str, arguments: &str, output: &str) -> Option<String>;
}
```

Returning `None` tells the caller to keep the original output unchanged.

### Agent dispatch

Tool names differ by agent. The lookup functions map tool names to compressors:

| Agent       | Tool naming                               | Lookup                          |
| ----------- | ----------------------------------------- | ------------------------------- |
| Claude Code | PascalCase (`Read`, `Bash`)               | `claude_compressor_for(name)`   |
| Codex CLI   | snake_case (`read_file`, `shell_command`) | `codex_compressor_for(name)`    |
| OpenCode    | lowercase (`read`, `bash`)                | `opencode_compressor_for(name)` |

The hosted / on-prem gateway selects the right lookup based on the agent type.

### System-reminder protection

Claude Code injects `<system-reminder>` blocks into tool output. These blocks carry runtime instructions and must be forwarded verbatim to the model. The compression utilities split output into compressible and protected segments before compressing, then reassemble the result. See `util::compress_claude_tool_with_segment_protection` and `util::split_into_segments` in `src/util.rs`.

### Bash sub-compressors

The `Bash` tool compressor parses the command name from the tool arguments and delegates to a per-command compressor (e.g. `cargo`, `ls`, `npm`). This two-level dispatch lives in `src/strategy/bash/`.

### Adding a new compression strategy

1. Create a new file under `src/strategy/<agent>/`.
2. Implement `ToolCompressor` (or `BashCompressor` for bash sub-commands).
3. Register the new compressor in the `compressor_for` function in that directory's `mod.rs`.
4. Add tests; at minimum cover the compressed output and `<system-reminder>` passthrough.

`src/strategy/claude/read.rs` is a well-commented reference example.

## See also

- [`doc/architecture.md`](../../doc/architecture.md): how this crate relates to the CLI and hosted gateway
- [`CONTRIBUTING.md`](../../CONTRIBUTING.md#adding-a-compression-strategy): full contribution guide
