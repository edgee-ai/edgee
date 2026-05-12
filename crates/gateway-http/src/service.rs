use std::task::{self, Poll};

use axum_core::body::Body;
use bytes::Bytes;
use futures::{StreamExt as _, future::BoxFuture};
use http::{Request, Response};
use http_body::Frame;
use http_body_util::{BodyExt as _, Limited, StreamBody};
use tower::Service;
use tracing::Instrument as _;

use edgee_gateway_core::{CompletionRequest, GatewayResponse, ProviderDispatchService};

use crate::error::Error;
use crate::passthrough::MAX_BODY_BYTES;

// TODO: not exposed publicly yet — `ProviderDispatchService` is still a stub
// (returns `Err` on every call), so wiring `GatewayService` would 500 on every
// request. Re-export from `lib.rs` once the dispatch path is real.
#[allow(dead_code)]
#[derive(Clone, Default)]
pub struct GatewayService {
    dispatch: ProviderDispatchService,
}

#[allow(dead_code)]
impl GatewayService {
    pub fn new(dispatch: ProviderDispatchService) -> Self {
        Self { dispatch }
    }
}

impl Service<Request<Body>> for GatewayService {
    type Response = Response<Body>;
    type Error = Error;
    type Future = BoxFuture<'static, Result<Self::Response, Error>>;

    fn poll_ready(&mut self, cx: &mut task::Context<'_>) -> Poll<Result<(), Error>> {
        self.dispatch.poll_ready(cx).map_err(Into::into)
    }

    fn call(&mut self, req: Request<Body>) -> Self::Future {
        let mut dispatch = self.dispatch.clone();
        let span = tracing::info_span!(
            "gateway.http.request",
            "gen_ai.request.model" = tracing::field::Empty,
            "gen_ai.request.stream" = tracing::field::Empty,
        );

        Box::pin(
            async move {
                let bytes = Limited::new(req.into_body(), MAX_BODY_BYTES)
                    .collect()
                    .await
                    .map_err(crate::error::body_read_error)?
                    .to_bytes();

                let completion_req = serde_json::from_slice::<CompletionRequest>(&bytes)?;

                let current = tracing::Span::current();
                current.record("gen_ai.request.model", completion_req.model.as_str());
                current.record("gen_ai.request.stream", completion_req.stream);

                tracing::debug!("dispatching request to provider");

                let gateway_resp = dispatch.call(completion_req).await?;

                match gateway_resp {
                    GatewayResponse::Complete(resp) => {
                        tracing::debug!("returning complete response");
                        let body = serde_json::to_string(&resp)?;

                        Ok(Response::builder()
                            .status(200)
                            .header(http::header::CONTENT_TYPE, "application/json")
                            .body(Body::from(body))
                            .unwrap())
                    }
                    GatewayResponse::Stream(stream) => {
                        tracing::debug!("returning streaming response");
                        let sse = stream
                            .map(|result| {
                                result
                                    .map(|chunk| {
                                        let json = serde_json::to_string(&chunk)
                                            .unwrap_or_else(|_| "{}".into());
                                        Frame::data(Bytes::from(format!("data: {json}\n\n")))
                                    })
                                    .map_err(|e| edgee_gateway_core::Error::Stream(e.to_string()))
                            })
                            .chain(futures::stream::once(async {
                                Ok(Frame::data(Bytes::from("data: [DONE]\n\n")))
                            }));

                        Ok(Response::builder()
                            .status(200)
                            .header(http::header::CONTENT_TYPE, "text/event-stream")
                            .body(Body::new(StreamBody::new(sse)))
                            .unwrap())
                    }
                }
            }
            .instrument(span),
        )
    }
}
