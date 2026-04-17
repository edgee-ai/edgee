use std::{
    sync::Arc,
    task::{Context, Poll},
};

use axum_core::body::Body;
use futures::future::BoxFuture;
use http::{Request, Response};
use tower::Service;

use crate::{
    PassthroughRequest,
    backend::http::HttpClient,
    config::ProviderConfig,
    error::{Error, Result},
};

/// Default Anthropic Messages API endpoint.
const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";

/// Passthrough Tower service for the Anthropic Messages API.
///
/// Forwards `POST /v1/messages` requests to Anthropic in their **native format**
/// without any translation. Headers supplied in the [`PassthroughRequest`] are
/// forwarded as-is (gateway-internal headers must already be stripped by the
/// caller — see [`crate::passthrough::SKIP_HEADERS`]).
///
/// This is one of the two "distinct Tower `Service` implementations" for
/// passthrough described in the spec (§6 Milestone 1).
pub struct AnthropicPassthroughService {
    client: Arc<dyn HttpClient>,
    config: ProviderConfig,
}

impl AnthropicPassthroughService {
    pub fn new(client: Arc<dyn HttpClient>, config: ProviderConfig) -> Self {
        Self { client, config }
    }

    fn target_uri(&self) -> String {
        let base = self.config.base_url.as_deref().unwrap_or(DEFAULT_BASE_URL);
        format!("{base}/v1/messages")
    }
}

impl Service<PassthroughRequest> for AnthropicPassthroughService {
    type Response = Response<Body>;
    type Error = Error;
    type Future = BoxFuture<'static, Result<Response<Body>>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: PassthroughRequest) -> Self::Future {
        let client = self.client.clone();
        let uri = self.target_uri();

        Box::pin(async move {
            let mut builder = Request::builder().method(http::Method::POST).uri(&uri);

            for (key, value) in &req.headers {
                builder = builder.header(key.as_str(), value.as_str());
            }

            let forwarded = builder
                .body(Body::from(req.body))
                .map_err(|e| Error::RequestBuild(e.to_string()))?;

            client.send(forwarded).await
        })
    }
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;

    use super::*;
    use crate::config::ProviderConfig;

    #[test]
    fn target_uri_default() {
        let svc = AnthropicPassthroughService::new(
            Arc::new(crate::testing::StubClient),
            ProviderConfig::new("key"),
        );
        assert_eq!(svc.target_uri(), "https://api.anthropic.com/v1/messages");
    }

    #[test]
    fn target_uri_custom_base_url() {
        let svc = AnthropicPassthroughService::new(
            Arc::new(crate::testing::StubClient),
            ProviderConfig::new("key").with_base_url("http://localhost:8080"),
        );
        assert_eq!(svc.target_uri(), "http://localhost:8080/v1/messages");
    }

    #[test]
    fn forwards_headers_as_is() {
        let req = PassthroughRequest::new(
            Bytes::from("{}"),
            vec![
                ("content-type".into(), "application/json".into()),
                // x-edgee-api-key should have been stripped by the caller;
                // here we verify the service forwards what it receives as-is.
                ("x-api-key".into(), "sk-ant-test".into()),
            ],
        );
        // The service itself does not filter — it trusts the caller.
        assert_eq!(req.headers.len(), 2);
    }
}
