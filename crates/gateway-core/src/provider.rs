use futures::stream::BoxStream;

use crate::{
    config::ProviderConfig,
    error::{Error, Result},
    types::{CompletionChunk, CompletionRequest, CompletionResponse},
};

/// Core abstraction for an LLM provider.
///
/// Implementations translate from the canonical [`CompletionRequest`] (OpenAI
/// Chat Completions format) into the provider's native API format, make the
/// HTTP call via the injected [`crate::http_client::HttpClient`], and parse
/// the response back into the canonical types.
///
/// # Dyn compatibility
///
/// The `complete_stream` method returns a [`BoxStream`] (not `impl Stream`) so
/// that `dyn Provider` is object-safe and can be stored in a `Vec` or `Arc`.
/// `async_trait` boxes the future returned by `complete` for the same reason.
///
/// # Ownership note
///
/// Both methods take `CompletionRequest` by value so implementations can move
/// data directly into the response future or stream without an extra clone.
/// `config` is borrowed — implementations must clone any fields they need to
/// keep (e.g. the API key) before the borrow ends when building a `'static`
/// stream.
#[async_trait::async_trait]
pub trait Provider: Send + Sync {
    /// Perform a non-streaming completion. Waits for the full response.
    async fn complete(
        &self,
        request: CompletionRequest,
        config: &ProviderConfig,
    ) -> Result<CompletionResponse>;

    /// Begin a streaming completion. Returns a stream of response chunks as they arrive.
    async fn complete_stream(
        &self,
        request: CompletionRequest,
        config: &ProviderConfig,
    ) -> BoxStream<'static, Result<CompletionChunk, Error>>;
}
