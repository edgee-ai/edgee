//! Private entry point for the native Linux sandbox bridge. This command does
//! not resolve profiles, read credentials or contact the update service.

use std::path::PathBuf;

use anyhow::Result;

#[derive(Debug, clap::Parser)]
pub struct Options {
    pub spec: PathBuf,
}

pub async fn run(options: Options) -> Result<()> {
    #[cfg(target_os = "linux")]
    return super::launch::sandbox::runtime::internal(&options.spec);
    #[cfg(not(target_os = "linux"))]
    {
        let _ = options;
        anyhow::bail!("The internal sandbox bridge is supported only on Linux.")
    }
}
