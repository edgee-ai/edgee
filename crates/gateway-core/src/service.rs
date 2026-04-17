use std::task::{Context, Poll};

use futures::future::BoxFuture;
use tower::Service;

use crate::{
    error::{Error, Result},
    types::{CompletionRequest, GatewayResponse},
};

/// The innermost Tower service in the core LLM pipeline.
///
/// Routes a [`CompletionRequest`] (OpenAI-compatible canonical format) to the
/// appropriate provider implementation, which translates the request to the
/// provider's native format and calls the provider API.
///
/// This is the innermost service; all middleware layers
/// (`tools-compression`, user-defined layers, etc.) wrap it:
///
/// ```text
/// CompletionRequest
///       │
///       v
/// ┌──────────────────┐
/// │  [User layers]   │  ← Any tower::Layer
/// └──────┬───────────┘
///        │
///        v
/// ┌──────────────────┐
/// │  Provider        │  ← This service
/// │  dispatch        │
/// └──────────────────┘
///        │
///        v
/// GatewayResponse
/// ```
///
/// # Construction
///
/// ```rust,ignore
/// let service = ProviderDispatchService::new(
///     Arc::new(ReqwestHttpClient::new(client)),
///     anthropic_config,
///     openai_config,
/// );
/// ```
pub struct ProviderDispatchService {}

impl ProviderDispatchService {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self {}
    }
}

impl Service<CompletionRequest> for ProviderDispatchService {
    type Response = GatewayResponse;
    type Error = Error;
    type Future = BoxFuture<'static, Result<GatewayResponse>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, _req: CompletionRequest) -> Self::Future {
        Box::pin(async {
            Err(Error::HttpClient(
                "ProviderDispatchService: not yet implemented".into(),
            ))
        })
    }
}

#[cfg(test)]
mod tests {
    use tower::ServiceExt as _;

    use super::*;
    use crate::types::message::{Message, MessageContent, UserMessage};

    fn make_service() -> ProviderDispatchService {
        ProviderDispatchService::new()
    }

    #[tokio::test]
    async fn poll_ready_is_always_ready() {
        let mut svc = make_service();
        let ready = std::future::poll_fn(|cx| svc.poll_ready(cx)).await;
        assert!(ready.is_ok());
    }

    #[tokio::test]
    async fn call_returns_error_for_unimplemented() {
        let svc = make_service();
        let req = CompletionRequest::new(
            "gpt-4o",
            vec![Message::User(UserMessage {
                name: None,
                content: MessageContent::Text("test".into()),
                cache_control: None,
            })],
        );
        let result = svc.oneshot(req).await;
        assert!(result.is_err());
    }
}
