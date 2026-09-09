//! `edgee launch omp` — Oh My Pi CLI (<https://github.com/can1357/oh-my-pi>).
//!
//! OMP is a Pi fork with the same custom-provider schema and `$NAME` config
//! references. It therefore reuses Pi's launcher and Pi coding-agent key, but
//! writes the additive Edgee provider to OMP's own
//! `~/.omp/agent/models.json`.

use anyhow::Result;

pub use super::pi::Options;

pub async fn run(opts: Options) -> Result<()> {
    super::pi::run_compatible(opts, super::pi::CompatibleAgent::Omp).await
}
