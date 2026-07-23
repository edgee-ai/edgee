use anyhow::Result;

use crate::crypto::DebugLogKeypair;

/// Name of the env var carrying the E2EE debug-log passphrase.
const ENV_VAR: &str = "EDGEE_DEBUG_LOG_E2EE_PASSPHRASE";

/// Resolve the E2EE debug-log passphrase (env var wins over profile) and
/// derive its keypair. `Ok(None)` means no passphrase is configured — debug
/// logs upload as plaintext. `Err` means one *is* configured but the KDF
/// failed: this must abort the launch rather than silently fall back to
/// plaintext while the user believes logs are encrypted.
pub fn resolve_debug_log_keypair() -> Result<Option<DebugLogKeypair>> {
    let Some(passphrase) = std::env::var(ENV_VAR)
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(crate::config::debug_log_e2ee_passphrase_profile_override)
    else {
        return Ok(None);
    };

    DebugLogKeypair::derive(&passphrase)
        .map(Some)
        .map_err(|e| anyhow::anyhow!("failed to derive debug-log encryption key: {e}"))
}

