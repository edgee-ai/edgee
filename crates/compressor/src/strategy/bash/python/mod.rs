pub use super::BashCompressor;

mod mypy;
mod pip;
mod pytest;
mod ruff;

pub use mypy::MypyCompressor;
pub use pip::PipCompressor;
pub use pytest::PytestCompressor;
pub use ruff::RuffCompressor;
