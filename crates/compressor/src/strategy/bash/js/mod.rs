pub use super::BashCompressor;

mod eslint;
mod jest;
mod npm;
mod tsc;

pub use eslint::EslintCompressor;
pub use jest::JestCompressor;
pub use npm::NpmCompressor;
pub use tsc::TscCompressor;
