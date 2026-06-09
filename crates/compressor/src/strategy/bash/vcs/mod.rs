pub use super::BashCompressor;

mod diff;
mod gh;
mod git;
mod glab;
mod gt;

pub use diff::DiffCompressor;
pub use gh::GhCompressor;
pub use git::GitCompressor;
pub use glab::GlabCompressor;
pub use gt::GtCompressor;
