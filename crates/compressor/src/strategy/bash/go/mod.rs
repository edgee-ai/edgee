pub use super::BashCompressor;

#[allow(clippy::module_inception)]
mod go;
mod golangci_lint;

pub use go::GoCompressor;
pub use golangci_lint::GolangciLintCompressor;
