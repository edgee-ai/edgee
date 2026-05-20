# Architecture: Edgee AI Gateway

This document describes the design of the Edgee AI Gateway OSS codebase: the Tower service chain, crate boundaries, and request flow from the coding agent to the LLM provider.

## Overview

Edgee is an LLM gateway that sits between a coding agent (Claude Code, Codex, OpenCode) and an LLM provider (Anthropic, OpenAI). Its primary function today is **tool-output compression**: before each API request is forwarded, tool results in the context window are analyzed and shrunk to reduce token count without changing the model's view of the conversation.

The gateway is built on **[Tower](https://docs.rs/tower/latest/tower/)**, a Rust middleware framework. Every processing step is a Tower [`Service`](https://docs.rs/tower/latest/tower/trait.Service.html), and processing steps are composed by stacking Tower [`Layer`](https://docs.rs/tower/latest/tower/trait.Layer.html)s. This design means the compression pipeline, the HTTP boundary, and the provider dispatch are all independently testable units that can be composed in different configurations using [`ServiceBuilder`](https://docs.rs/tower/latest/tower/builder/struct.ServiceBuilder.html).

## Crate boundaries

| Crate                      | Crate name                | Runtime dependency            | Responsibility                                                                    |
| -------------------------- | ------------------------- | ----------------------------- | --------------------------------------------------------------------------------- |
| `crates/cli`               | `edgee-cli`               | tokio                         | `edgee` binary; launches agents, manages auth, reports stats                      |
| `crates/gateway-core`      | `edgee-ai-gateway-core`   | none (optional tokio feature) | Canonical types, `Provider` trait, passthrough services, HTTP backend abstraction |
| `crates/compressor`        | `edgee-compressor`        | none                          | Pure compression library; per-tool and per-command strategies; no I/O             |
| `crates/compression-layer` | `edgee-compression-layer` | none                          | Tower `Layer`/`Service` wrapping the compressor; agent-aware dispatch             |
| `crates/gateway-http`      | `edgee-gateway-http`      | axum-core (no tokio)          | HTTP boundary; raw `Request<Body>` in, `PassthroughRequest` out                   |

`gateway-core` and `compressor` intentionally carry no tokio or reqwest dependency. This keeps them portable to alternative runtimes (e.g. Fastly Compute / `wasm32-wasip1`) and makes them trivially testable without a real HTTP server.

## Request flow

The following traces a Claude Code `Read` tool call from the agent to Anthropic, showing what happens to the request body at each stage.

### 1. Agent sends a request

Claude Code issues a `POST /v1/messages` (routed through `ANTHROPIC_BASE_URL` pointing at the local gateway) with a `messages` array that includes the tool result of a previous `Read` call:

```json
{
  "model": "claude-opus-4-7",
  "messages": [
    { "role": "user", "content": "Summarise src/lib.rs" },
    {
      "role": "assistant",
      "content": [
        {
          "type": "tool_use",
          "id": "toolu_01",
          "name": "Read",
          "input": { "file_path": "src/lib.rs" }
        }
      ]
    },
    {
      "role": "user",
      "content": [
        {
          "type": "tool_result",
          "tool_use_id": "toolu_01",
          "content": "1: //! Core LLM request…\n2: \n3: pub mod backend;\n… (600 more lines)"
        }
      ]
    }
  ]
}
```

### 2. gateway-http: HTTP boundary

`PassthroughService` receives the raw `Request<Body>`, reads it into memory (up to 4 MB), strips hop-by-hop and gateway-internal headers, and wraps the body in a `PassthroughRequest`:

```
PassthroughRequest {
  body: <the JSON above, unchanged>,
  headers: { "authorization": "Bearer sk-ant-…", "anthropic-version": "…", … }
              (hop-by-hop and X-Edgee-* headers already stripped)
}
```

### 3. compression-layer: tool result compression

`CompressionService` deserializes the body, walks the `messages` array, and for each `tool_result` whose `tool_use_id` maps to a registered compressor (here: `Read`), replaces `content` with the compressed form:

```
Before  →  "1: use std::sync::Arc;\n2: \n3: pub fn compress_tool_output(…\n… (600 more lines)"
After   →  "[ Read: src/lib.rs, showing 18 of 602 lines ]\nuse std::sync::Arc;\n…"
```

All other message fields and any `<system-reminder>` blocks within the content are preserved verbatim. The body is then re-serialized and stored back into the `PassthroughRequest`.

### 4. gateway-core: forward to provider

`AnthropicPassthroughService` appends the provider auth header and forwards the (now-smaller) body to `https://api.anthropic.com/v1/messages`. The response (streaming or complete) is returned as-is to the agent.

### Tower service chain

The above maps directly onto the Tower [`Layer`](https://docs.rs/tower/latest/tower/trait.Layer.html) stack (outer to inner):

```text
coding agent
    │  POST /v1/messages  (large tool results in body)
    v
┌─────────────────────────────────────────────────┐
│  gateway-http                                   │
│  PassthroughLayer / PassthroughService          │
│  → reads body, strips headers                   │
│  → produces PassthroughRequest                  │
└─────────────────────┬───────────────────────────┘
                      │  Service<PassthroughRequest>
                      v
┌─────────────────────────────────────────────────┐
│  compression-layer                              │
│  CompressionLayer / CompressionService          │
│  → compresses tool results in body              │
│  → passes smaller PassthroughRequest forward    │
└─────────────────────┬───────────────────────────┘
                      │  Service<PassthroughRequest>
                      v
┌─────────────────────────────────────────────────┐
│  gateway-core                                   │
│  AnthropicPassthroughService                    │
│    or OpenAIPassthroughService                  │
│  → adds auth header, forwards to provider       │
│  → streams response back                        │
└─────────────────────┬───────────────────────────┘
                      │  HTTP response (may be SSE)
                      v
                 LLM provider
```

### Header stripping

Both `gateway-http` and `gateway-core` strip a shared list of headers (`SKIP_HEADERS` in `crates/gateway-core/src/passthrough/mod.rs`) before forwarding. This list covers:

- Hop-by-hop headers (`Connection`, `Transfer-Encoding`, `Keep-Alive`, ...)
- Framing headers (`Content-Length`, `Content-Encoding`): recalculated after body mutation
- Gateway-internal headers added by the CLI (`X-Edgee-*`)

## LLM router (provider dispatch path)

The LLM router is the second Tower chain. Where the passthrough path forwards a raw body to a single hard-wired endpoint, the router accepts a canonical `CompletionRequest`, resolves which provider(s) are configured for the requested model, and dispatches through them in order, retrying on transient errors and falling back to the next provider on failure.

`ProviderDispatchService` in `crates/gateway-core/src/service.rs` is currently a stub. The sections below describe the intended design.

### Service chain

```text
CompletionRequest (canonical OpenAI Chat Completions format)
    │
    v
CompressionLayer              ← same compression layer as passthrough path
    │
    v
ProviderDispatchService       ← stub today; router logic goes here
    │  1. resolve model → ranked provider list
    │  2. for each provider (primary first, then fallbacks):
    │       a. translate CompletionRequest to provider-native format
    │       b. call Provider::complete / Provider::complete_stream
    │       c. on transient error: retry once, then advance to next provider
    │       d. on streaming error after first chunk: return error immediately
    │  3. return GatewayResponse or propagate final error
    │
    ├─ Provider impl A  (e.g. AnthropicProvider)
    ├─ Provider impl B  (e.g. OpenAIProvider)
    └─ Provider impl …
```

### Provider trait

Every provider implements two methods in `crates/gateway-core/src/provider.rs`:

```rust
pub trait Provider: Send + Sync {
    async fn complete(
        &self,
        request: CompletionRequest,
        config: &ProviderConfig,
    ) -> Result<CompletionResponse>;

    async fn complete_stream(
        &self,
        request: CompletionRequest,
        config: &ProviderConfig,
    ) -> BoxStream<'static, Result<CompletionChunk, Error>>;
}
```

Each `Provider` impl is responsible for translating the canonical `CompletionRequest` into the provider's native wire format and mapping the response back.

### Provider selection and fallback

For a given model, one or more providers may be configured. The router ranks them (by availability metrics or static priority), tries the primary provider first, and falls back to the next on recoverable errors:

| Error kind                        | Primary provider         | Fallback provider                |
| --------------------------------- | ------------------------ | -------------------------------- |
| Transient (5xx, rate-limit)       | retry once, then advance | advance immediately              |
| Timeout                           | advance immediately      | advance immediately              |
| Client error (4xx)                | return error to caller   | (not reached)                    |
| Streaming error after first chunk | return error to caller   | (cannot retry; response started) |

The streaming constraint is important: once the first SSE chunk has been sent to the caller, it is too late to switch providers or retry, so the error is surfaced as-is.

## Runtime portability

`gateway-core` and `compressor` compile without any async runtime. The tokio dependency is entirely optional:

- `gateway-core` with `tokio` feature: provides `ReqwestHttpClient`.
- Without `tokio`: integrate by implementing `HttpClient` with whatever HTTP stack your target supports.

This separation makes it possible to run the compression and type machinery on Fastly Compute (WASM), in unit tests without a running server, or in any future runtime environment without modifying the core crates.
