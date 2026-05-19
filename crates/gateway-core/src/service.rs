use std::task::{Context, Poll};

use futures::future::BoxFuture;
use tower::Service;
use tracing::Instrument as _;

use crate::{
    error::{Error, Result},
    region::Region,
    types::{CompletionRequest, GatewayResponse},
};

/// The innermost Tower service in the core LLM pipeline.
///
/// Routes a [`CompletionRequest`] (OpenAI-compatible canonical format) to the
/// appropriate provider implementation, respecting the configured data-residency
/// [`Region`].
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
/// # Region routing
///
/// On the hosted gateway, the configured [`Region`] selects the
/// appropriate Fastly POP geography. Each region maps to a subdomain
/// (e.g. `eu.api.edgee.ai` for [`Region::Eu`]). The default region
/// ([`Region::Us`]) uses the standard `api.edgee.ai` endpoint.
///
/// If the requested region is unavailable, the service falls back to the
/// default region and emits a warning. The actual region used is recorded
/// in the `request.region` audit event.
///
/// # Construction
///
/// ```rust,ignore
/// let service = ProviderDispatchService::new(
///     Region::Eu,
///     Arc::new(ReqwestHttpClient::new(client)),
///     anthropic_config,
///     openai_config,
/// );
/// ```
#[derive(Clone)]
pub struct ProviderDispatchService {
    region: Region,
}

impl ProviderDispatchService {
    /// Create a new dispatch service for the given data-residency [`Region`].
    pub fn new(region: Region) -> Self {
        Self { region }
    }

    /// Create a new dispatch service with the default region ([`Region::Us`]).
    pub fn new_default() -> Self {
        Self::new(Region::default())
    }

    /// The data-residency region this service routes traffic through.
    pub fn region(&self) -> Region {
        self.region
    }
}

impl Default for ProviderDispatchService {
    fn default() -> Self {
        Self::new_default()
    }
}

impl Service<CompletionRequest> for ProviderDispatchService {
    type Response = GatewayResponse;
    type Error = Error;
    type Future = BoxFuture<'static, Result<GatewayResponse>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: CompletionRequest) -> Self::Future {
        let region = self.region;
        let model = req.model.clone();

        Box::pin(
            async move {
                // Audit event: request.region at dispatch time
                tracing::info!(
                    region = %region,
                    model = %model,
                    "gateway.dispatch: routing request to region {region}"
                );

                tracing::warn!(
                    region = %region,
                    "ProviderDispatchService not yet implemented (region={region})"
                );

                Err(Error::HttpClient(format!(
                    "ProviderDispatchService: not yet implemented (region={})",
                    region.short_code()
                )))
            }
            .instrument(tracing::info_span!(
                "gateway.dispatch",
                request.region = %region,
            )),
        )
    }
}

#[cfg(test)]
mod tests {
    use tower::ServiceExt as _;

    use super::*;
    use crate::types::message::{Message, MessageContent, UserMessage};

    fn make_service(region: Region) -> ProviderDispatchService {
        ProviderDispatchService::new(region)
    }

    fn make_request() -> CompletionRequest {
        CompletionRequest::new(
            "gpt-4o",
            vec![Message::User(UserMessage {
                name: None,
                content: MessageContent::Text("test".into()),
                cache_control: None,
            })],
        )
    }

    #[tokio::test]
    async fn poll_ready_is_always_ready() {
        let mut svc = make_service(Region::Us);
        let ready = std::future::poll_fn(|cx| svc.poll_ready(cx)).await;
        assert!(ready.is_ok());
    }

    #[tokio::test]
    async fn default_uses_us_region() {
        let svc = ProviderDispatchService::default();
        assert_eq!(svc.region(), Region::Us);
    }

    #[tokio::test]
    async fn new_default_uses_us_region() {
        let svc = ProviderDispatchService::new_default();
        assert_eq!(svc.region(), Region::Us);
    }

    #[tokio::test]
    async fn stores_region() {
        for region in Region::ALL {
            let svc = make_service(*region);
            assert_eq!(svc.region(), *region);
        }
    }

    #[tokio::test]
    async fn call_returns_error_for_unimplemented() {
        let svc = make_service(Region::Us);
        let result = svc.oneshot(make_request()).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn error_message_includes_region() {
        let svc = make_service(Region::Eu);
        let result = svc.oneshot(make_request()).await;
        let msg = match result {
            Err(e) => e.to_string(),
            Ok(_) => panic!("expected error"),
        };
        assert!(msg.contains("eu"), "error message should mention region: {msg}");
    }

    #[tokio::test]
    async fn different_regions_produce_different_errors() {
        let req = make_request();
        let err_us = match make_service(Region::Us).oneshot(req.clone()).await {
            Err(e) => e.to_string(),
            Ok(_) => panic!("expected error"),
        };
        let err_eu = match make_service(Region::Eu).oneshot(req.clone()).await {
            Err(e) => e.to_string(),
            Ok(_) => panic!("expected error"),
        };
        let err_apac = match make_service(Region::Apac).oneshot(req).await {
            Err(e) => e.to_string(),
            Ok(_) => panic!("expected error"),
        };
        assert_ne!(err_us, err_eu);
        assert_ne!(err_eu, err_apac);
    }
}
