pub use super::BashCompressor;

mod eslint;
mod jest;
mod next;
mod npm;
mod playwright;
mod pnpm;
mod prettier;
mod prisma;
mod tsc;

pub use eslint::EslintCompressor;
pub use jest::JestCompressor;
pub use next::NextCompressor;
pub use npm::NpmCompressor;
pub use playwright::PlaywrightCompressor;
pub use pnpm::PnpmCompressor;
pub use prettier::PrettierCompressor;
pub use prisma::PrismaCompressor;
pub use tsc::TscCompressor;
