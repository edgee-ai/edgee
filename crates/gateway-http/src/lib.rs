//! HTTP boundary for the Edgee AI Gateway, built on `axum-core`.
//!
//! This crate converts raw HTTP requests into typed [`PassthroughRequest`] values
//! and forwards them to a downstream Tower service. It depends only on `axum-core`
//! (not the full `axum` crate or `tokio` directly), keeping it runtime-agnostic
//! like [`edgee_ai_gateway_core`] and [`edgee_compressor`].
//!
//! # Request pipeline
//!
//! 1. [`PassthroughLayer`] / [`PassthroughService`] read the HTTP body (up to 4 MB).
//! 2. Hop-by-hop and gateway-internal headers are stripped.
//! 3. A [`PassthroughRequest`] is constructed and forwarded to the inner service.
//! 4. The inner service's response is returned as-is, including SSE streams.
//!
//! # Error format
//!
//! All errors produced by this crate are serialized in the **OpenAI error schema**:
//!
//! ```json
//! { "error": { "message": "...", "type": "...", "code": "..." } }
//! ```
//!
//! This ensures callers expecting an OpenAI-compatible response receive a consistent
//! format regardless of where the failure occurred.
//!
//! # Usage
//!
//! ```rust,ignore
//! use tower::ServiceBuilder;
//! use edgee_gateway_http::PassthroughLayer;
//!
//! // inner_svc implements Service<PassthroughRequest>
//! let svc = ServiceBuilder::new()
//!     .layer(PassthroughLayer::default())
//!     .service(inner_svc);
//! ```
//!
//! [`PassthroughRequest`]: edgee_ai_gateway_core::PassthroughRequest

pub use error::Error;
pub use passthrough::{PassthroughLayer, PassthroughService};

pub mod error;
pub mod passthrough;
mod service;
