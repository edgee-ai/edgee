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
    "host",
    "content-length",
    "transfer-encoding",
    "accept-encoding",
    "connection",
    // Gateway-internal auth / control headers
    "x-edgee-api-key",
];
