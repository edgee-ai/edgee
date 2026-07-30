//! Passphrase-derived X25519 keypair for E2EE debug logs.
//!
//! See `gateway/docs/features/encrypted-debug-logs.md` for the full
//! construction, Argon2id parameters, and cross-repo invariants.

use anyhow::Result;
use argon2::{Algorithm, Argon2, Params, Version};
use base64::Engine as _;
use rand::RngExt;
use x25519_dalek::{PublicKey, StaticSecret};

// Must stay byte-identical to gateway's copy; see the doc linked above.
const KDF_M_COST_KIB: u32 = 19456;
const KDF_T_COST: u32 = 2;
const KDF_P_COST: u32 = 1;
const KDF_KEY_LEN: usize = 32;

/// A passphrase-derived debug-log public key plus the salt used to derive it.
/// Only these two values ever leave this process (via
/// [`header_values`](Self::header_values)); the private key is discarded
/// immediately after deriving the public key, since the CLI never decrypts —
/// only the console does, from the passphrase, independently.
pub struct DebugLogKeypair {
    public_key: PublicKey,
    salt: [u8; 16],
}

impl DebugLogKeypair {
    /// Derive a fresh keypair from `passphrase`, generating a new random salt
    /// for this launch session.
    pub fn derive(passphrase: &str) -> Result<Self> {
        let mut salt = [0u8; 16];
        rand::rng().fill(&mut salt);
        Self::derive_with_salt(passphrase, salt)
    }

    fn derive_with_salt(passphrase: &str, salt: [u8; 16]) -> Result<Self> {
        let params = Params::new(KDF_M_COST_KIB, KDF_T_COST, KDF_P_COST, Some(KDF_KEY_LEN))
            .map_err(|e| anyhow::anyhow!("invalid Argon2 params: {e}"))?;
        let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);

        let mut key_bytes = [0u8; 32];
        argon2
            .hash_password_into(passphrase.as_bytes(), &salt, &mut key_bytes)
            .map_err(|e| anyhow::anyhow!("failed to derive debug-log key: {e}"))?;

        let secret = StaticSecret::from(key_bytes);
        let public_key = PublicKey::from(&secret);
        Ok(Self { public_key, salt })
    }

    /// Base64-encoded values for the `x-edgee-debug-pubkey`/`x-edgee-debug-salt`
    /// headers attached to every gateway-proxied request in this session.
    pub fn header_values(&self) -> DebugLogHeaderValues {
        let b64 = base64::engine::general_purpose::STANDARD;
        DebugLogHeaderValues {
            pubkey: b64.encode(self.public_key.as_bytes()),
            salt: b64.encode(self.salt),
        }
    }
}

/// Base64-encoded pubkey/salt pair to attach as `x-edgee-debug-*` headers.
#[derive(Clone)]
pub struct DebugLogHeaderValues {
    pub pubkey: String,
    pub salt: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_passphrase_and_salt_derive_same_keypair() {
        let a = DebugLogKeypair::derive_with_salt("hunter2", [3u8; 16]).unwrap();
        let b = DebugLogKeypair::derive_with_salt("hunter2", [3u8; 16]).unwrap();
        assert_eq!(a.public_key.as_bytes(), b.public_key.as_bytes());
    }

    #[test]
    fn different_passphrases_derive_different_keypairs() {
        let a = DebugLogKeypair::derive_with_salt("hunter2", [3u8; 16]).unwrap();
        let b = DebugLogKeypair::derive_with_salt("hunter3", [3u8; 16]).unwrap();
        assert_ne!(a.public_key.as_bytes(), b.public_key.as_bytes());
    }

    #[test]
    fn header_values_round_trip_base64() {
        let keypair = DebugLogKeypair::derive("hunter2").unwrap();
        let headers = keypair.header_values();
        let b64 = base64::engine::general_purpose::STANDARD;
        assert_eq!(b64.decode(headers.pubkey).unwrap().len(), 32);
        assert_eq!(b64.decode(headers.salt).unwrap().len(), 16);
    }
}
