pub use super::BashCompressor;

mod curl;
mod docker;
mod env;
mod make;
mod psql;
mod wc;

pub use curl::CurlCompressor;
pub use docker::DockerCompressor;
pub use env::EnvCompressor;
pub use make::MakeCompressor;
pub use psql::PsqlCompressor;
pub use wc::WcCompressor;
