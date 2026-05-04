use http::HeaderMap;

/// A raw LLM request in a provider's native wire format, ready for passthrough.
///
/// This is the pipeline-level input type for the passthrough Tower services
/// ([`crate::passthrough::anthropic::AnthropicPassthroughService`],
/// [`crate::passthrough::openai::OpenAIPassthroughService`]).
///
/// The HTTP boundary layer above `gateway-core` is responsible for:
/// - Reading the raw request body into [`serde_json::Value`].
/// - Stripping gateway-internal headers (see [`crate::passthrough::SKIP_HEADERS`]).
/// - Constructing this type before handing the request to the pipeline.
///
/// `http` types are used *internally* by the passthrough service implementations
/// only when building the outbound HTTP call to the provider — never in this
/// public interface.
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
