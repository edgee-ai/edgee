use std::{
    sync::Arc,
    task::{Context, Poll},
};

use axum_core::body::Body;
use futures::future::BoxFuture;
use http::{Request, Response};
use tower::Service;

use tracing::Instrument as _;

use crate::{
    PassthroughRequest,
    backend::http::HttpClient,
    config::OpenAIPassthroughConfig,
    error::{Error, Result},
};

/// Passthrough Tower service for the OpenAI Responses API.
///
/// Forwards `POST /v1/responses` requests to OpenAI in their **native format**
/// without any translation. Headers supplied in the [`PassthroughRequest`] are
/// forwarded as-is (gateway-internal headers must already be stripped by the
/// caller — see [`crate::passthrough::SKIP_HEADERS`]).
///
/// # Routing
///
/// The OpenAI Responses API is reachable via two production endpoints, both
/// taken from [`OpenAIPassthroughConfig`]:
/// - [`OpenAIPassthroughConfig::api_url`] — used when the request bears an
///   OpenAI Platform project key (`Authorization: Bearer sk-proj-…`).
/// - [`OpenAIPassthroughConfig::chatgpt_url`] — used in every other case
///   (Codex CLI's default path).
///
/// To pin all traffic to one endpoint, set both fields to the same URL on the
/// supplied config.
///
/// This is one of the two "distinct Tower `Service` implementations" for
/// passthrough described in the spec (§6 Milestone 1).
#[derive(Clone)]
pub struct OpenAIPassthroughService {
    client: Arc<dyn HttpClient>,
    config: OpenAIPassthroughConfig,
}

impl OpenAIPassthroughService {
    pub fn new(client: Arc<dyn HttpClient>, config: OpenAIPassthroughConfig) -> Self {
        Self { client, config }
    }

    fn target_uri(&self, headers: &http::HeaderMap) -> &str {
        let is_proj_key = headers
            .get(http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .map(|v| v.starts_with("sk-proj-") || v.starts_with("Bearer sk-proj-"))
            .unwrap_or(false);

        if is_proj_key {
            &self.config.api_url
        } else {
            &self.config.chatgpt_url
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
        let uri = self.target_uri(&req.headers).to_owned();
        let span = tracing::debug_span!("passthrough.openai", url = %uri);

        Box::pin(
            async move {
                tracing::debug!(url = %uri, "forwarding to OpenAI");

                let mut builder = Request::builder().method(http::Method::POST).uri(&uri);

                for (key, value) in &req.headers {
                    builder = builder.header(key, value);
                }

                let body_bytes = serde_json::to_vec(&req.body)
                    .map_err(|e| Error::RequestBuild(e.to_string()))?;
                let forwarded = builder
                    .body(Body::from(body_bytes))
                    .map_err(|e| Error::RequestBuild(e.to_string()))?;

                client.send(forwarded).await
            }
            .instrument(span),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::OpenAIPassthroughConfig;

    fn make_svc(config: OpenAIPassthroughConfig) -> OpenAIPassthroughService {
        OpenAIPassthroughService::new(Arc::new(crate::testing::StubClient), config)
    }

    #[test]
    fn routes_proj_key_to_api_openai() {
        let svc = make_svc(OpenAIPassthroughConfig::default());
        let mut headers = http::HeaderMap::new();
        headers.insert(
            http::header::AUTHORIZATION,
            "Bearer sk-proj-abc123".parse().unwrap(),
        );
        assert_eq!(
            svc.target_uri(&headers),
            OpenAIPassthroughConfig::DEFAULT_API_URL
        );
    }

    #[test]
    fn routes_non_proj_key_to_chatgpt() {
        let svc = make_svc(OpenAIPassthroughConfig::default());
        let mut headers = http::HeaderMap::new();
        headers.insert(
            http::header::AUTHORIZATION,
            "Bearer sk-abc123".parse().unwrap(),
        );
        assert_eq!(
            svc.target_uri(&headers),
            OpenAIPassthroughConfig::DEFAULT_CHATGPT_URL
        );
    }

    #[test]
    fn routes_no_auth_to_chatgpt() {
        let svc = make_svc(OpenAIPassthroughConfig::default());
        assert_eq!(
            svc.target_uri(&http::HeaderMap::new()),
            OpenAIPassthroughConfig::DEFAULT_CHATGPT_URL
        );
    }

    #[test]
    fn custom_api_url_used_for_proj_key() {
        let svc = make_svc(
            OpenAIPassthroughConfig::default().with_api_url("http://localhost:4000/v1/responses"),
        );
        let mut headers = http::HeaderMap::new();
        headers.insert(
            http::header::AUTHORIZATION,
            "Bearer sk-proj-abc123".parse().unwrap(),
        );
        assert_eq!(
            svc.target_uri(&headers),
            "http://localhost:4000/v1/responses"
        );
    }

    #[test]
    fn custom_chatgpt_url_used_for_default_path() {
        let svc = make_svc(
            OpenAIPassthroughConfig::default().with_chatgpt_url("http://localhost:5000/responses"),
        );
        assert_eq!(
            svc.target_uri(&http::HeaderMap::new()),
            "http://localhost:5000/responses"
        );
    }
}
