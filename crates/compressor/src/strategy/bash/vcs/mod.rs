pub use super::BashCompressor;

mod diff;
mod gh;
mod git;

pub use diff::DiffCompressor;
pub use gh::GhCompressor;
pub use git::GitCompressor;
