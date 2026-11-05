//! Load a local signer from a *file path* key reference.
//!
//! `SOURCE_EXECUTOR_KEY_REF` must point at a 0600 file containing a hex private
//! key (optional `0x` prefix). Raw keys in the environment are refused so they
//! cannot leak via process listings or shell history. The key bytes are never
//! logged, Display'd, or included in error messages.

use alloy::signers::local::PrivateKeySigner;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::str::FromStr;

#[derive(Debug, thiserror::Error)]
pub enum KeyError {
    #[error("SOURCE_EXECUTOR_KEY_REF is empty; no signer loaded")]
    Empty,
    #[error(
        "SOURCE_EXECUTOR_KEY_REF looks like a raw private key; put it in a 0600 file and point the ref at the path"
    )]
    RawKeyRefused,
    #[error("SOURCE_EXECUTOR_KEY_REF file is not 0600 (mode {mode:#o}); refusing to load")]
    InsecureMode { mode: u32 },
    #[error("SOURCE_EXECUTOR_KEY_REF file is unreadable")]
    Unreadable,
    #[error("SOURCE_EXECUTOR_KEY_REF file did not contain a usable key")]
    Invalid,
}

/// True when `s` looks like a hex-encoded secp256k1 private key rather than a path.
fn looks_like_raw_key(s: &str) -> bool {
    let t = s.trim();
    let hex = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")).unwrap_or(t);
    hex.len() == 64 && hex.chars().all(|c| c.is_ascii_hexdigit()) && !t.contains('/') && !t.contains('.')
}

/// Resolve a key-ref string to a filesystem path. `file:` prefix is accepted.
fn as_path(key_ref: &str) -> PathBuf {
    let t = key_ref.trim();
    PathBuf::from(t.strip_prefix("file:").unwrap_or(t))
}

pub fn load_signer(key_ref: &str) -> Result<PrivateKeySigner, KeyError> {
    let t = key_ref.trim();
    if t.is_empty() {
        return Err(KeyError::Empty);
    }
    if looks_like_raw_key(t) {
        return Err(KeyError::RawKeyRefused);
    }
    load_signer_from_path(&as_path(t))
}

pub fn load_signer_from_path(path: &Path) -> Result<PrivateKeySigner, KeyError> {
    let meta = fs::metadata(path).map_err(|_| KeyError::Unreadable)?;
    let mode = meta.permissions().mode() & 0o777;
    if mode & 0o077 != 0 {
        return Err(KeyError::InsecureMode { mode });
    }
    let contents = fs::read_to_string(path).map_err(|_| KeyError::Unreadable)?;
    let line = contents
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with('#'))
        .ok_or(KeyError::Invalid)?;
    PrivateKeySigner::from_str(line).map_err(|_| KeyError::Invalid)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn refuses_raw_hex_in_the_ref() {
        let hex = "11".repeat(32);
        assert!(matches!(load_signer(&hex), Err(KeyError::RawKeyRefused)));
        assert!(matches!(load_signer(&format!("0x{hex}")), Err(KeyError::RawKeyRefused)));
    }

    #[test]
    fn empty_ref_is_empty() {
        assert!(matches!(load_signer("  "), Err(KeyError::Empty)));
    }

    #[test]
    fn rejects_world_readable_file() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("key");
        let mut f = fs::File::create(&p).unwrap();
        writeln!(f, "11{}", "22".repeat(31)).unwrap();
        drop(f);
        let mut perms = fs::metadata(&p).unwrap().permissions();
        perms.set_mode(0o644);
        fs::set_permissions(&p, perms).unwrap();
        assert!(matches!(load_signer_from_path(&p), Err(KeyError::InsecureMode { .. })));
    }

    #[test]
    fn loads_0600_hex_file() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("key");
        let mut f = fs::File::create(&p).unwrap();
        // Well-known Anvil account #0 private key (public test vector, not a secret).
        // Well-known Anvil/Hardhat account #0 (public test vector, not a secret).
        writeln!(f, "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80").unwrap();
        drop(f);
        let mut perms = fs::metadata(&p).unwrap().permissions();
        perms.set_mode(0o600);
        fs::set_permissions(&p, perms).unwrap();
        let s = load_signer_from_path(&p).unwrap();
        assert_eq!(
            format!("{:#x}", s.address()),
            "0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266"
        );
    }
}
