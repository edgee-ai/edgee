use bytes::Bytes;

/// A raw LLM request in a provider's native wire format, ready for passthrough.
///
/// This is the pipeline-level input type for the passthrough Tower services
/// ([`crate::passthrough::anthropic::AnthropicPassthroughService`],
/// [`crate::passthrough::openai::OpenAIPassthroughService`]).
///
/// The HTTP boundary layer above `gateway-core` is responsible for:
/// - Reading the raw request body into [`Bytes`].
/// - Stripping gateway-internal headers (see [`crate::passthrough::SKIP_HEADERS`]).
/// - Constructing this type before handing the request to the pipeline.
///
/// `http` types are used *internally* by the passthrough service implementations
/// only when building the outbound HTTP call to the provider — never in this
/// public interface.
#[derive(Debug, Clone)]
pub struct PassthroughRequest {
    /// Raw serialized request body in the provider's native format.
    pub body: Bytes,
    /// Pre-filtered headers to forward (gateway-internal headers already stripped).
    /// Each entry is a `(name, value)` pair as UTF-8 strings.
    pub headers: Vec<(String, String)>,
}

impl PassthroughRequest {
    pub fn new(body: Bytes, headers: Vec<(String, String)>) -> Self {
        Self { body, headers }
    }
}
