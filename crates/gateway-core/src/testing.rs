//! Shared test utilities for `gateway-core` unit tests.
//!
//! This module is compiled only in test builds (`#[cfg(test)]`).

use axum_core::body::Body;
use http::{Request, Response};

use crate::{Error, backend::http::HttpClient, error::Result};

/// A no-op [`HttpClient`] that always returns an error.
///
/// Useful for tests that need to construct services without exercising the
/// HTTP transport layer.
pub struct StubClient;

#[async_trait::async_trait]
impl HttpClient for StubClient {
    async fn send(&self, _req: Request<Body>) -> Result<Response<Body>> {
        Err(Error::HttpClient("StubClient always fails".into()))
    }
}
