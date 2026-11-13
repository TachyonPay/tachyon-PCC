//! Minimal EIP-4361 (SIWE) verification. Recovers the signer from an
//! EIP-191 personal_sign and checks address, expiry and nonce.

use alloy::primitives::{Address, Signature};
use chrono::{DateTime, Utc};
use std::str::FromStr;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SiweError {
    #[error("SIWE message is malformed")]
    Malformed,
    #[error("SIWE signature is invalid")]
    BadSignature,
    #[error("SIWE signer does not match claimed address")]
    AddressMismatch,
    #[error("SIWE nonce mismatch")]
    NonceMismatch,
    #[error("SIWE message is expired or not yet valid")]
    Expired,
    #[error("SIWE domain is not allowed")]
    DomainRefused,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SiweMessage {
    pub domain: String,
    pub address: Address,
    pub uri: String,
    pub version: String,
    pub chain_id: u64,
    pub nonce: String,
    pub issued_at: DateTime<Utc>,
    pub expiration_time: DateTime<Utc>,
}

/// Parse a subset of EIP-4361 we issue. Extra fields are ignored.
pub fn parse_message(text: &str) -> Result<SiweMessage, SiweError> {
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() < 8 {
        return Err(SiweError::Malformed);
    }
    let header = lines[0];
    let (domain, rest) = header
        .split_once(" wants you to sign in with your Ethereum account:")
        .ok_or(SiweError::Malformed)?;
    if !rest.is_empty() {
        return Err(SiweError::Malformed);
    }
    let address = lines[1].parse::<Address>().map_err(|_| SiweError::Malformed)?;
    let mut uri = String::new();
    let mut version = String::new();
    let mut chain_id = 0u64;
    let mut nonce = String::new();
    let mut issued_at = None;
    let mut expiration_time = None;
    for line in &lines[2..] {
        if let Some(v) = line.strip_prefix("URI: ") {
            v.clone_into(&mut uri);
        } else if let Some(v) = line.strip_prefix("Version: ") {
            v.clone_into(&mut version);
        } else if let Some(v) = line.strip_prefix("Chain ID: ") {
            chain_id = v.parse().map_err(|_| SiweError::Malformed)?;
        } else if let Some(v) = line.strip_prefix("Nonce: ") {
            v.clone_into(&mut nonce);
        } else if let Some(v) = line.strip_prefix("Issued At: ") {
            issued_at = Some(v.parse().map_err(|_| SiweError::Malformed)?);
        } else if let Some(v) = line.strip_prefix("Expiration Time: ") {
            expiration_time = Some(v.parse().map_err(|_| SiweError::Malformed)?);
        }
    }
    if version != "1" || nonce.is_empty() || uri.is_empty() {
        return Err(SiweError::Malformed);
    }
    Ok(SiweMessage {
        domain: domain.to_owned(),
        address,
        uri,
        version,
        chain_id,
        nonce,
        issued_at: issued_at.ok_or(SiweError::Malformed)?,
        expiration_time: expiration_time.ok_or(SiweError::Malformed)?,
    })
}

#[must_use]
pub fn compose(msg: &SiweMessage) -> String {
    format!(
        "{domain} wants you to sign in with your Ethereum account:\n{address}\n\nURI: {uri}\nVersion: {version}\nChain ID: {chain_id}\nNonce: {nonce}\nIssued At: {issued_at}\nExpiration Time: {expiration_time}",
        domain = msg.domain,
        address = msg.address,
        uri = msg.uri,
        version = msg.version,
        chain_id = msg.chain_id,
        nonce = msg.nonce,
        issued_at = msg.issued_at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        expiration_time = msg.expiration_time.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
    )
}

/// Any host[:port]. The API still accepts only the domain it wrote into the challenge.
pub fn domain_allowed(domain: &str) -> bool {
    let d = domain.trim();
    !d.is_empty()
        && d.len() <= 253
        && !d.contains([' ', '/', '\\', '@', '\n', '\r'])
        && d.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b':' | b'[' | b']'))
}

/// Verify a message whose domain is a hostname. The hosted checkout uses
/// [`verify_for_domain`] so the signed text must match the domain it issued.
pub fn verify(
    text: &str,
    signature_hex: &str,
    expected_nonce: &str,
    now: DateTime<Utc>,
) -> Result<SiweMessage, SiweError> {
    verify_with(text, signature_hex, expected_nonce, now, domain_allowed)
}

/// Verify a message issued for exactly `expected_domain` (case-insensitive host).
pub fn verify_for_domain(
    text: &str,
    signature_hex: &str,
    expected_nonce: &str,
    now: DateTime<Utc>,
    expected_domain: &str,
) -> Result<SiweMessage, SiweError> {
    verify_with(text, signature_hex, expected_nonce, now, |d| {
        !expected_domain.is_empty() && d.eq_ignore_ascii_case(expected_domain)
    })
}

fn verify_with(
    text: &str,
    signature_hex: &str,
    expected_nonce: &str,
    now: DateTime<Utc>,
    domain_ok: impl Fn(&str) -> bool,
) -> Result<SiweMessage, SiweError> {
    let msg = parse_message(text)?;
    if !domain_ok(&msg.domain) {
        return Err(SiweError::DomainRefused);
    }
    if msg.nonce != expected_nonce {
        return Err(SiweError::NonceMismatch);
    }
    if now < msg.issued_at - chrono::Duration::seconds(60) || now > msg.expiration_time {
        return Err(SiweError::Expired);
    }
    let sig = Signature::from_str(signature_hex.trim()).map_err(|_| SiweError::BadSignature)?;
    let recovered = sig
        .recover_address_from_msg(text.as_bytes())
        .map_err(|_| SiweError::BadSignature)?;
    if recovered != msg.address {
        return Err(SiweError::AddressMismatch);
    }
    Ok(msg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::signers::local::PrivateKeySigner;
    use alloy::signers::SignerSync;

    fn signed(now: DateTime<Utc>) -> (PrivateKeySigner, String, String, String) {
        let signer = PrivateKeySigner::random();
        let nonce = "n".repeat(16);
        let msg = SiweMessage {
            domain: "localhost".into(),
            address: signer.address(),
            uri: "http://127.0.0.1:3000".into(),
            version: "1".into(),
            chain_id: 8453,
            nonce: nonce.clone(),
            issued_at: now,
            expiration_time: now + chrono::Duration::minutes(10),
        };
        let text = compose(&msg);
        let sig = signer.sign_message_sync(text.as_bytes()).unwrap();
        (signer, text, format!("{sig}"), nonce)
    }

    #[test]
    fn roundtrip_recovers_signer() {
        let now = Utc::now();
        let (signer, text, sig, nonce) = signed(now);
        let v = verify(&text, &sig, &nonce, now).unwrap();
        assert_eq!(v.address, signer.address());
    }

    #[test]
    fn refuses_wrong_nonce_expired_and_foreign_domain() {
        let now = Utc::now();
        let (_s, text, sig, nonce) = signed(now);
        assert_eq!(verify(&text, &sig, "other", now).unwrap_err(), SiweError::NonceMismatch);
        assert_eq!(
            verify(&text, &sig, &nonce, now + chrono::Duration::hours(1)).unwrap_err(),
            SiweError::Expired
        );
        let tampered = text.replacen("localhost", "evil.example", 1);
        assert_eq!(
            verify(&tampered, &sig, &nonce, now).unwrap_err(),
            SiweError::AddressMismatch,
            "changing the domain changes the signed text"
        );
        assert!(!domain_allowed("javascript:alert(1)"));
        assert!(domain_allowed("pay.example.com"));
    }

    #[test]
    fn configured_domain_is_exact() {
        let now = Utc::now();
        let (signer, text, sig, nonce) = signed(now);
        let ok = verify_for_domain(&text, &sig, &nonce, now, "LOCALHOST").unwrap();
        assert_eq!(ok.address, signer.address());
        for other in ["pay.example.com", "localhost:3000", ""] {
            assert_eq!(
                verify_for_domain(&text, &sig, &nonce, now, other).unwrap_err(),
                SiweError::DomainRefused,
                "{other}"
            );
        }
    }
}
