//! Tower [`Layer`](tower::Layer) that applies [`edgee_compressor`] to in-flight
//! gateway requests, shrinking tool-result content before it is forwarded to a
//! provider.
//!
//! Compose [`CompressionLayer`] into any [`Service`](tower::Service) chain that
//! handles [`CompletionRequest`](edgee_ai_gateway_core::CompletionRequest).

pub mod compress;
pub mod config;
pub mod layer;
pub mod service;

pub use config::{AgentType, CompressionConfig};
pub use layer::CompressionLayer;
pub use service::CompressionService;
