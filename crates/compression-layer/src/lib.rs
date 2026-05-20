//! Tower [`Layer`]/[`Service`] that compresses LLM tool outputs in-flight.
//!
//! This crate wraps any downstream Tower service and intercepts requests before
//! they are forwarded to an LLM provider, calling [`edgee_compressor`] to shrink
//! tool-result payloads. Only tool results are touched; all other request fields
//! pass through unchanged.
//!
//! # Agent types
//!
//! Tool names differ between coding agents. Set [`AgentType`] on your
//! [`CompressionConfig`] so the layer dispatches to the right compressor:
//!
//! | Agent | [`AgentType`] variant | Example tool name |
//! |---|---|---|
//! | Claude Code | [`AgentType::Claude`] | `"Read"`, `"Bash"` |
//! | Codex | [`AgentType::Codex`] | `"read_file"`, `"shell_command"` |
//! | OpenCode | [`AgentType::OpenCode`] | `"read"`, `"bash"` |
//!
//! # Usage
//!
//! ```rust,ignore
//! use tower::ServiceBuilder;
//! use edgee_compression_layer::{CompressionLayer, CompressionConfig};
//!
//! // inner_svc implements Service<PassthroughRequest>
//! let svc = ServiceBuilder::new()
//!     .layer(CompressionLayer::new(CompressionConfig::claude()))
//!     .service(inner_svc);
//! ```
//!
//! [`CompressionService`] implements both `Service<CompletionRequest>` and
//! `Service<PassthroughRequest>`, so it fits at any position in a passthrough
//! or typed-dispatch Tower chain.
//!
//! [`Layer`]: tower::Layer
//! [`Service`]: tower::Service

pub mod compress;
pub mod config;
pub mod layer;
pub mod service;

pub use config::{AgentType, CompressionConfig};
pub use layer::CompressionLayer;
pub use service::CompressionService;
