# edgee-compression-layer

Tower `Layer`/`Service` that compresses LLM tool outputs in-flight.

## Role in the stack

This crate wraps any downstream Tower service and intercepts requests before they leave the gateway. It calls `edgee-compressor` to shrink tool-result payloads, then forwards the mutated request to the inner service. Only tool results are modified; all other request fields pass through unchanged.

```
coding agent (Claude Code / CodeBuddy / Codex / OpenCode)
        |
  edgee-compression-layer               <-- this crate
    CompressionLayer
    CompressionService
        |
  edgee-gateway-core
    AnthropicPassthroughService / OpenAIPassthroughService
```

## Key types

| Type                    | Description                                                                                             |
| ----------------------- | ------------------------------------------------------------------------------------------------------- |
| `CompressionLayer`      | Tower `Layer<S>`; wraps a downstream service in a `CompressionService`                                  |
| `CompressionService<S>` | Implements `Service<CompletionRequest>` and `Service<PassthroughRequest>`; mutates requests in place    |
| `AgentType`             | Selects the tool-naming convention: `Claude` (PascalCase), `Codex` (snake_case), `OpenCode` (lowercase) |
| `CompressionConfig`     | Holds `AgentType`; built with `CompressionConfig::claude()`, `::codex()`, or `::opencode()`             |

## Usage

```rust
use tower::ServiceBuilder;
use edgee_compression_layer::{CompressionLayer, CompressionConfig};

// inner_svc implements Service<PassthroughRequest> (e.g. AnthropicPassthroughService)
let svc = ServiceBuilder::new()
    .layer(CompressionLayer::new(CompressionConfig::claude()))
    .service(inner_svc);
```

The layer works in both typed-dispatch and passthrough Tower chains since `CompressionService<S>` implements both `Service<CompletionRequest>` and `Service<PassthroughRequest>`.

## See also

- [`edgee-compressor`](../compressor/): the underlying compression library
- [`edgee-gateway-core`](../gateway-core/): provides `CompletionRequest`, `PassthroughRequest`, and the inner services
- [`doc/architecture.md`](../../doc/architecture.md): full Tower chain diagram and design notes
