pub mod compress;
pub mod config;
mod dispatch;
pub mod layer;
pub mod passthrough;
pub mod service;

pub use config::{AgentType, CompressionConfig};
pub use layer::CompressionLayer;
pub use passthrough::{PassthroughCompressionLayer, PassthroughCompressionService};
pub use service::CompressionService;
