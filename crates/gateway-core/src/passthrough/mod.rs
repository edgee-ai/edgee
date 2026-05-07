pub mod anthropic;
pub mod openai;

/// HTTP headers stripped from all outbound passthrough requests.
///
/// These are either hop-by-hop headers that must not be forwarded, or
/// gateway-internal headers that must not leak to providers.
///
/// The HTTP boundary layer above `gateway-core` should apply this list when
/// constructing a [`crate::PassthroughRequest`] from an incoming HTTP request.
pub const SKIP_HEADERS: &[&str] = &[
    // Hop-by-hop headers (RFC 7230 §6.1) — must not be forwarded.
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "proxy-connection",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
    // Body framing — invalid once the body is buffered and re-serialised.
    "content-length",
    // Hostname — must be set to the upstream provider, not echoed.
    "host",
    // Encoding — let the upstream client negotiate compression itself.
    "accept-encoding",
    // Gateway-internal auth / control headers
    "x-edgee-api-key",
];
