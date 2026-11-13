//! Authentication primitives: merchant API keys, SIWE, RFC 6238 TOTP.
//!
//! The mock authenticator stays behind a debug build *and* an explicit env flag.
//! TOTP secrets are wrapped at rest and never logged.

use rand::RngCore;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

pub mod authorizer;
pub mod siwe;
pub mod totp;

pub use authorizer::TotpAuthorizer;
pub use siwe::{
    compose as compose_siwe, verify as verify_siwe, verify_for_domain as verify_siwe_for_domain, SiweError, SiweMessage,
};

pub const API_KEY_PREFIX: &str = "pcc_sk_";

/// Generate a merchant API key. Returned once; only the hash is stored.
#[must_use]
pub fn generate_api_key() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    format!("{API_KEY_PREFIX}{}", hex::encode(bytes))
}

/// SHA-256 hex of an API key. Keys are high-entropy random strings, so a
/// plain hash (not a password KDF) is appropriate.
#[must_use]
pub fn hash_api_key(key: &str) -> String {
    let mut h = Sha256::new();
    h.update(b"pcc.apikey.v1|");
    h.update(key.as_bytes());
    hex::encode(h.finalize())
}

/// Constant-time comparison of a presented key against a stored hash.
#[must_use]
pub fn verify_api_key(presented: &str, stored_hash_hex: &str) -> bool {
    let presented_hash = hash_api_key(presented);
    let a = presented_hash.as_bytes();
    let b = stored_hash_hex.as_bytes();
    if a.len() != b.len() {
        return false;
    }
    a.ct_eq(b).into()
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("mock authenticator is not permitted in this build/configuration")]
pub struct MockForbidden;

/// The mock authenticator may only be used when BOTH the binary is a debug
/// build AND the operator explicitly opted in via `ALLOW_MOCK_AUTHENTICATOR=true`.
pub fn mock_authenticator_allowed(allow_flag: Option<&str>) -> Result<(), MockForbidden> {
    let opted_in = allow_flag.is_some_and(|v| v.trim() == "true");
    if cfg!(debug_assertions) && opted_in {
        Ok(())
    } else {
        Err(MockForbidden)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_key_roundtrip() {
        let k = generate_api_key();
        assert!(k.starts_with(API_KEY_PREFIX));
        assert_eq!(k.len(), API_KEY_PREFIX.len() + 64);
        let h = hash_api_key(&k);
        assert!(verify_api_key(&k, &h));
        assert!(!verify_api_key(&format!("{k}x"), &h));
        assert!(!verify_api_key(&k, "deadbeef"));
    }

    #[test]
    fn keys_are_unique() {
        assert_ne!(generate_api_key(), generate_api_key());
    }

    #[test]
    fn mock_guard_requires_explicit_opt_in() {
        assert_eq!(mock_authenticator_allowed(None), Err(MockForbidden));
        assert_eq!(mock_authenticator_allowed(Some("yes")), Err(MockForbidden));
        assert_eq!(mock_authenticator_allowed(Some("1")), Err(MockForbidden));
        // In debug test builds "true" is allowed; release builds always refuse.
        let expected = if cfg!(debug_assertions) {
            Ok(())
        } else {
            Err(MockForbidden)
        };
        assert_eq!(mock_authenticator_allowed(Some("true")), expected);
    }
}
