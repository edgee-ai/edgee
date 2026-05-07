use http::HeaderMap;

/// A raw LLM request in a provider's native wire format, ready for passthrough.
///
/// This is the pipeline-level input type for the passthrough Tower services
/// ([`crate::passthrough::anthropic::AnthropicPassthroughService`],
/// [`crate::passthrough::openai::OpenAIPassthroughService`]).
///
/// # Crate boundary
///
/// `PassthroughRequest` lives in `gateway-core` because it is a
/// **pipeline-level** value: it carries the request after the HTTP framing
/// has been resolved (body buffered and parsed, headers materialised). The
/// network boundary — receiving raw [`http_body::Body`] streams, applying
/// per-frame size limits, and decoding the wire body — belongs to a separate
/// crate (today: `gateway-http`).
///
/// HTTP metadata types such as [`http::HeaderMap`] are intentionally allowed
/// here: the `http` crate is `no_std`-compatible so it does not compromise
/// the portability story (WASM/Fastly), and using a real header map preserves
/// multi-valued headers and avoids ad-hoc string pairs at every call site.
/// What `gateway-core` deliberately avoids is the *transport* surface: bodies
/// as byte streams, async runtime types, server abstractions.
///
/// # Caller responsibilities
///
/// The HTTP boundary layer above `gateway-core` is responsible for:
/// - Reading the raw request body into [`serde_json::Value`].
/// - Stripping gateway-internal and hop-by-hop headers
///   (see [`crate::passthrough::SKIP_HEADERS`]).
/// - Constructing this type before handing the request to the pipeline.
#[derive(Debug, Clone)]
pub struct PassthroughRequest {
    /// The raw request body, parsed as JSON.
    pub body: serde_json::Value,
    /// Pre-filtered headers to forward (gateway-internal headers already stripped).
    /// The HTTP boundary layer is responsible for stripping out any headers that
    /// are meant for internal use only and should not be forwarded to the provider.
    pub headers: HeaderMap,
}

impl PassthroughRequest {
    pub fn new(body: serde_json::Value, headers: HeaderMap) -> Self {
        Self { body, headers }
    }
}
