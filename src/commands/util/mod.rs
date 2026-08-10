pub mod session_log;

pub use session_log::*;

use anyhow::Result;
use serde::Serialize;

/// Pretty-print a value as JSON to stdout — the shared emitter for every
/// `--json` command, so the output format lives in one place.
pub fn emit_json<T: Serialize>(value: &T) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}
