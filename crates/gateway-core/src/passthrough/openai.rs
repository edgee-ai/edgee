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

/// OpenAI Responses API endpoint for requests authenticated with a project key
/// (`sk-proj-…`). These keys belong to the OpenAI Platform API.
const OPENAI_API_RESPONSES_URL: &str = "https://api.openai.com/v1/responses";

/// Default Responses API endpoint (ChatGPT backend, used by Codex CLI without
/// a project key).
const OPENAI_CHATGPT_RESPONSES_URL: &str = "https://chatgpt.com/backend-api/codex/responses";

/// Passthrough Tower service for the OpenAI Responses API.
///
/// Forwards `POST /v1/responses` requests to OpenAI in their **native format**
/// without any translation. Headers supplied in the [`PassthroughRequest`] are
/// forwarded as-is (gateway-internal headers must already be stripped by the
/// caller — see [`crate::passthrough::SKIP_HEADERS`]).
///
/// Endpoint selection (when `ProviderConfig::base_url` is `None`):
/// - `authorization: Bearer sk-proj-…` → `api.openai.com` (Platform API key)
/// - anything else → `chatgpt.com` backend (Codex CLI default)
///
/// This is one of the two "distinct Tower `Service` implementations" for
/// passthrough described in the spec (§6 Milestone 1).
pub struct OpenAIPassthroughService {
    client: Arc<dyn HttpClient>,
    config: ProviderConfig,
}

impl OpenAIPassthroughService {
    pub fn new(client: Arc<dyn HttpClient>, config: ProviderConfig) -> Self {
        Self { client, config }
    }

    fn target_uri(&self, headers: &[(String, String)]) -> String {
        // If an explicit override is set, use it directly.
        if let Some(base) = &self.config.base_url {
            return format!("{base}/v1/responses");
        }

        // Otherwise select by key prefix (matching reference gateway behaviour).
        let is_proj_key = headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("authorization"))
            .map(|(_, v)| v.starts_with("sk-proj-") || v.starts_with("Bearer sk-proj-"))
            .unwrap_or(false);

        if is_proj_key {
            OPENAI_API_RESPONSES_URL.to_owned()
        } else {
            OPENAI_CHATGPT_RESPONSES_URL.to_owned()
        }
    }
}

impl Service<PassthroughRequest> for OpenAIPassthroughService {
    type Response = Response<Body>;
    type Error = Error;
    type Future = BoxFuture<'static, Result<Response<Body>>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: PassthroughRequest) -> Self::Future {
        let client = self.client.clone();
        let uri = self.target_uri(&req.headers);

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
    use super::*;
    use crate::config::ProviderConfig;

    fn make_svc(base_url: Option<&str>) -> OpenAIPassthroughService {
        let mut config = ProviderConfig::new("key");
        if let Some(u) = base_url {
            config = config.with_base_url(u);
        }
        OpenAIPassthroughService::new(Arc::new(crate::testing::StubClient), config)
    }

    #[test]
    fn routes_proj_key_to_api_openai() {
        let svc = make_svc(None);
        let headers = vec![("authorization".into(), "Bearer sk-proj-abc123".into())];
        assert_eq!(svc.target_uri(&headers), OPENAI_API_RESPONSES_URL);
    }

    #[test]
    fn routes_non_proj_key_to_chatgpt() {
        let svc = make_svc(None);
        let headers = vec![("authorization".into(), "Bearer sk-abc123".into())];
        assert_eq!(svc.target_uri(&headers), OPENAI_CHATGPT_RESPONSES_URL);
    }

    #[test]
    fn custom_base_url_overrides_selection() {
        let svc = make_svc(Some("http://localhost:4000"));
        assert_eq!(svc.target_uri(&[]), "http://localhost:4000/v1/responses");
    }
}
