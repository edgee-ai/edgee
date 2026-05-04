//! Core LLM request→response pipeline for the Edgee AI Gateway.
//!
//! # Architecture
//!
//! The pipeline is modelled as a Tower [`Service`] chain. This crate defines the
//! innermost service ([`service::ProviderDispatchService`]) and the foundational
//! types/traits that all other gateway crates depend on.
//!
//! ```text
//! CompletionRequest
//!       │
//!       v
//! ┌──────────────────────┐
//! │  [User layers]       │  ← Any tower::Layer (compression, logging, …)
//! └──────┬───────────────┘
//!        │
//!        v
//! ┌──────────────────────┐
//! │  ProviderDispatch    │  ← Service<CompletionRequest>
//! │  Service             │
//! └──────────────────────┘
//!        │
//!        v
//! GatewayResponse
//! ```
//!
//! # Passthrough
//!
//! Two additional Tower services handle the passthrough path, where requests
//! arrive in provider-native format and are forwarded without translation:
//!
//! - [`passthrough::anthropic::AnthropicPassthroughService`]  — `POST /v1/messages`
//! - [`passthrough::openai::OpenAIPassthroughService`]        — `POST /v1/responses`
//!
//! # Platform compatibility
//!
//! This crate has **no hard dependency on tokio or reqwest**. Enable the `tokio`
//! feature to get a concrete `backend::http::ReqwestHttpClient` backed by reqwest.
//! On other platforms (e.g. Fastly `wasm32-wasip1`), provide your own
//! [`backend::http::HttpClient`] implementation.
//!
//! [`Service`]: tower::Service

pub mod backend;
pub mod config;
pub mod error;
pub mod passthrough;
pub mod provider;
pub mod service;
pub mod types;

// Flat re-exports for convenience
pub use backend::http::HttpClient;
#[cfg(feature = "tokio")]
pub use backend::http::ReqwestHttpClient;
pub use config::ProviderConfig;
pub use error::{Error, Result};
pub use provider::Provider;
pub use service::ProviderDispatchService;
pub use types::{
    CompletionChunk, CompletionRequest, CompletionResponse, GatewayResponse, Message,
    PassthroughRequest, Usage,
};

// ── Test utilities (compiled only for tests) ─────────────────────────────

#[cfg(test)]
pub(crate) mod testing;
