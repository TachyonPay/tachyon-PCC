//! RFC 6238 TOTP (HMAC-SHA1, 6 digits, 30s step). Secrets are never logged.

use data_encoding::BASE32_NOPAD;
use hmac::{Hmac, Mac};
use rand::RngCore;
use sha1::Sha1;
use sha2::{Digest, Sha256};

const DIGITS: u32 = 6;
const STEP: u64 = 30;
const WINDOW: i64 = 1;

pub const ISSUER: &str = "PCC";

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum TotpError {
    #[error("invalid TOTP secret")]
    BadSecret,
    #[error("invalid TOTP code")]
    BadCode,
}

/// 20 random bytes, base32-encoded without padding (otpauth convention).
#[must_use]
pub fn generate_secret() -> String {
    let mut bytes = [0u8; 20];
    rand::thread_rng().fill_bytes(&mut bytes);
    BASE32_NOPAD.encode(&bytes)
}

#[must_use]
pub fn otpauth_uri(account: &str, secret_base32: &str) -> String {
    format!("otpauth://totp/{ISSUER}:{account}?secret={secret_base32}&issuer={ISSUER}&algorithm=SHA1&digits={DIGITS}&period={STEP}")
}

pub fn decode_secret(secret_base32: &str) -> Result<Vec<u8>, TotpError> {
    BASE32_NOPAD
        .decode(secret_base32.as_bytes())
        .or_else(|_| data_encoding::BASE32.decode(secret_base32.as_bytes()))
        .map_err(|_| TotpError::BadSecret)
}

#[must_use]
pub fn code_at(secret: &[u8], unix: u64) -> u32 {
    let timestep = unix / STEP;
    hotp(secret, timestep)
}

pub fn verify(secret: &[u8], code: &str, unix: u64) -> Result<u64, TotpError> {
    let presented = parse_code(code)?;
    let center = i64::try_from(unix / STEP).map_err(|_| TotpError::BadCode)?;
    for delta in -WINDOW..=WINDOW {
        let ts = center + delta;
        if ts < 0 {
            continue;
        }
        let t = u64::try_from(ts).map_err(|_| TotpError::BadCode)?;
        if hotp(secret, t) == presented {
            return Ok(t);
        }
    }
    Err(TotpError::BadCode)
}

pub fn parse_code(code: &str) -> Result<u32, TotpError> {
    let t = code.trim();
    if t.len() != 6 || !t.bytes().all(|b| b.is_ascii_digit()) {
        return Err(TotpError::BadCode);
    }
    t.parse().map_err(|_| TotpError::BadCode)
}

fn hotp(secret: &[u8], counter: u64) -> u32 {
    let mut mac = Hmac::<Sha1>::new_from_slice(secret).expect("hmac key");
    mac.update(&counter.to_be_bytes());
    let hash = mac.finalize().into_bytes();
    let offset = (hash[19] & 0x0f) as usize;
    let bin = u32::from_be_bytes([hash[offset], hash[offset + 1], hash[offset + 2], hash[offset + 3]]) & 0x7fff_ffff;
    bin % 10u32.pow(DIGITS)
}

/// Wrap a TOTP secret for at-rest storage. Master key is never the secret itself.
#[must_use]
pub fn wrap_secret(master: &[u8], slot: &[u8], secret_base32: &str) -> Vec<u8> {
    let key = wrap_key(master, slot);
    secret_base32
        .as_bytes()
        .iter()
        .enumerate()
        .map(|(i, b)| b ^ key[i % key.len()])
        .collect()
}

pub fn unwrap_secret(master: &[u8], slot: &[u8], wrapped: &[u8]) -> Result<String, TotpError> {
    let key = wrap_key(master, slot);
    let bytes: Vec<u8> = wrapped
        .iter()
        .enumerate()
        .map(|(i, b)| b ^ key[i % key.len()])
        .collect();
    String::from_utf8(bytes).map_err(|_| TotpError::BadSecret)
}

fn wrap_key(master: &[u8], slot: &[u8]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(b"pcc.totp.wrap.v1|");
    h.update(master);
    h.update(slot);
    h.finalize().into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc6238_sha1_test_vector_at_known_time() {
        // SHA1 secret "12345678901234567890" from RFC 6238; 6-digit codes.
        let secret = b"12345678901234567890";
        assert_eq!(code_at(secret, 59), 287_082);
        assert_eq!(code_at(secret, 1_111_111_109), 81_804);
        assert_eq!(verify(secret, "081804", 1_111_111_109).unwrap(), 1_111_111_109 / 30);
    }

    #[test]
    fn window_accepts_adjacent_step() {
        let secret = b"12345678901234567890";
        let code = format!("{:06}", code_at(secret, 1_111_111_109));
        assert!(verify(secret, &code, 1_111_111_109 + 29).is_ok());
    }

    #[test]
    fn wrap_roundtrip() {
        let s = generate_secret();
        let w = wrap_secret(b"master", b"slot", &s);
        assert_eq!(unwrap_secret(b"master", b"slot", &w).unwrap(), s);
        assert_ne!(unwrap_secret(b"other", b"slot", &w).unwrap_or_default(), s);
    }

    #[test]
    fn otpauth_does_not_embed_newlines() {
        let u = otpauth_uri("0xabc", "MFRGGZDFMZTWQ2LK");
        assert!(u.starts_with("otpauth://totp/"));
        assert!(!u.contains('\n'));
    }
}
