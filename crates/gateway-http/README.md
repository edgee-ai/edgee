# edgee-gateway-http

HTTP boundary for the Edgee AI Gateway, built on `axum-core`.

## Role in the stack

This crate is the outermost layer of the gateway stack. It receives raw HTTP requests from coding agents or any OpenAI-compatible client, reads and validates the body, strips hop-by-hop and gateway-internal headers, and hands a `PassthroughRequest` to the downstream Tower service chain.

It depends only on `axum-core` (not the full `axum` crate or `tokio` directly), keeping it runtime-agnostic in the same way as `gateway-core` and `compressor`.

```
coding agent (raw HTTP)
        |
  edgee-gateway-http                    <-- this crate
    PassthroughLayer
    PassthroughService
        |   (PassthroughRequest)
  edgee-compression-layer
        |
  edgee-gateway-core
```

## Key types

| Type                    | Description                                                                                                 |
| ----------------------- | ----------------------------------------------------------------------------------------------------------- |
| `PassthroughLayer`      | Tower `Layer<S>` producing a `PassthroughService<S>`                                                        |
| `PassthroughService<S>` | Reads the HTTP body (max 4 MB), filters headers, constructs `PassthroughRequest`, forwards to inner service |
| `Error`                 | OpenAI-compatible error response type; returned to the caller on body-read or routing failures              |

## Error format

All errors are serialized in the OpenAI error schema so callers expecting an OpenAI-compatible response receive consistent output:

```json
{
  "error": {
    "message": "request body too large",
    "type": "invalid_request_error",
    "code": "body_too_large"
  }
}
```

## Usage

```rust
use tower::ServiceBuilder;
use edgee_gateway_http::PassthroughLayer;

// inner_svc implements Service<PassthroughRequest>
let svc = ServiceBuilder::new()
    .layer(PassthroughLayer::default())
    .service(inner_svc);
```

## See also

- [`edgee-gateway-core`](../gateway-core/) — defines `PassthroughRequest` and the inner passthrough services
- [`edgee-compression-layer`](../compression-layer/) — sits between this crate and `gateway-core`
- [`doc/architecture.md`](../../doc/architecture.md) — full request-flow design document
