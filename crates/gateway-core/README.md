# edgee-ai-gateway-core

Core LLM request/response pipeline for the Edgee AI Gateway.

## Role in the stack

This crate is the foundation that all other gateway crates build on. It defines the canonical request/response types (OpenAI Chat Completions format), the `Provider` trait, and the two working passthrough services for Anthropic and OpenAI. It has no hard dependency on tokio or reqwest, making it portable to any async runtime including `wasm32-wasip1`.

```
coding agent (Claude Code / Codex / OpenCode)
        |
  compression-layer (edgee-compression-layer)
        |
  gateway-core                          <-- this crate
    AnthropicPassthroughService
    OpenAIPassthroughService
    ProviderDispatchService (future)
        |
  LLM provider (Anthropic / OpenAI)
```

## Key types and traits

| Item                          | Description                                                                               |
| ----------------------------- | ----------------------------------------------------------------------------------------- |
| `CompletionRequest`           | Canonical request; OpenAI Chat Completions compatible; `input` is an alias for `messages` |
| `Message`                     | Enum over Developer / System / User / Assistant / Tool message variants                   |
| `CompletionResponse`          | Non-streaming response with choices and usage counts                                      |
| `CompletionChunk`             | Single SSE event for streaming responses                                                  |
| `GatewayResponse`             | `Complete(CompletionResponse)` or `Stream(BoxStream<...>)`                                |
| `PassthroughRequest`          | Raw JSON body + pre-filtered headers for the passthrough path                             |
| `Provider`                    | Trait defining `complete` and `complete_stream` for typed provider backends               |
| `ProviderDispatchService`     | Tower `Service<CompletionRequest>` (stub; see note below)                                 |
| `AnthropicPassthroughService` | Working Tower service: forwards to `POST /v1/messages`                                    |
| `OpenAIPassthroughService`    | Working Tower service: routes to api.openai.com or chatgpt.com by key type                |
| `HttpClient`                  | Abstract HTTP backend trait; decouple from any specific async runtime                     |

> **Note:** `ProviderDispatchService` is currently a stub. The production request path today is the passthrough path via `AnthropicPassthroughService` or `OpenAIPassthroughService`.

## Feature flags

| Flag    | Effect                                                                               |
| ------- | ------------------------------------------------------------------------------------ |
| `tokio` | Enables `ReqwestHttpClient`, a concrete `HttpClient` backed by `reqwest` and `tokio` |

## Usage

```rust
use edgee_ai_gateway_core::{
    AnthropicPassthroughConfig, PassthroughRequest,
    passthrough::anthropic::AnthropicPassthroughService,
};
use edgee_ai_gateway_core::backend::http::ReqwestHttpClient;
use std::sync::Arc;
use tower::Service;

let client = Arc::new(ReqwestHttpClient::default());
let config = AnthropicPassthroughConfig::default();
let mut svc = AnthropicPassthroughService::new(client, config);
// svc now implements Service<PassthroughRequest>
```

## See also

- [`edgee-compressor`](../compressor/) — pure compression library
- [`edgee-compression-layer`](../compression-layer/) — Tower layer wrapping this crate's services
- [`edgee-gateway-http`](../gateway-http/) — HTTP boundary that feeds requests into this crate
- [`doc/architecture.md`](../../doc/architecture.md) — full request-flow design document
