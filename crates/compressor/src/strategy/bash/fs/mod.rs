pub use super::BashCompressor;

mod find;
mod grep;
mod ls;
mod rg;
mod tree;

pub use find::FindCompressor;
pub use grep::GrepCompressor;
pub use ls::LsCompressor;
pub use rg::RgCompressor;
pub use tree::TreeCompressor;
