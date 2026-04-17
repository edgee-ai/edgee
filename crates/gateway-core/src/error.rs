/// Crate-level error type.
///
/// Each variant carries enough semantic information to determine its HTTP status
/// mapping, observability category, and whether it is retryable — without
/// inspecting the message string.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The underlying HTTP client failed to send the request or receive the response.
    #[error("http client error: {0}")]
    HttpClient(String),

    /// The provider returned a non-2xx status code.
    #[error("provider error: status={status}, body={body}")]
    ProviderError { status: u16, body: String },

    /// An error occurred while reading or parsing a streaming response.
    #[error("stream error: {0}")]
    Stream(String),

    /// JSON serialization or deserialization failed.
    #[error("serialization error: {0}")]
    Json(#[from] serde_json::Error),

    /// Building the outbound HTTP request failed (e.g. invalid URI or header value).
    #[error("request build error: {0}")]
    RequestBuild(String),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
