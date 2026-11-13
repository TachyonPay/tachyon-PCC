//! Shape check for merchant-supplied return URLs. Hosts are not restricted.

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ReturnUrlError {
    #[error("return URL is not allowed")]
    Refused,
}

/// Domain and checkout origin for a SIWE challenge.
///
/// A browser `Origin` of `http` or `https` is used when its host is a plain
/// hostname, so a public checkout host matches the page the wallet is on.
/// Anything else falls back to the configured checkout.
pub fn signing_target(origin: Option<&str>, fallback_base: &str, fallback_domain: &str) -> (String, String) {
    let fallback = || {
        (
            fallback_domain.to_owned(),
            fallback_base.trim_end_matches('/').to_owned(),
        )
    };
    let Some(origin) = origin.map(str::trim).filter(|s| !s.is_empty()) else {
        return fallback();
    };
    let Ok(url) = reqwest::Url::parse(origin) else {
        return fallback();
    };
    if url.scheme() != "http" && url.scheme() != "https" {
        return fallback();
    }
    if !url.username().is_empty() || url.password().is_some() {
        return fallback();
    }
    let Some(host) = url.host_str() else {
        return fallback();
    };
    let domain = match url.port() {
        Some(port) if !((url.scheme() == "https" && port == 443) || (url.scheme() == "http" && port == 80)) => {
            format!("{host}:{port}")
        }
        _ => host.to_owned(),
    };
    if !pcc_auth::siwe::domain_allowed(&domain) {
        return fallback();
    }
    (domain, origin.trim_end_matches('/').to_owned())
}

/// Relative paths and any `http`/`https` host. Rejects `javascript:`, `data:`,
/// credentials, and protocol-relative URLs.
pub fn assert_return_url(url: &str) -> Result<(), ReturnUrlError> {
    let trimmed = url.trim();
    if trimmed.starts_with('/') && !trimmed.starts_with("//") && !trimmed.contains('\\') {
        if trimmed.contains('\n') || trimmed.contains('\r') {
            return Err(ReturnUrlError::Refused);
        }
        return Ok(());
    }
    let parsed = reqwest::Url::parse(trimmed).map_err(|_| ReturnUrlError::Refused)?;
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return Err(ReturnUrlError::Refused);
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(ReturnUrlError::Refused);
    }
    if parsed.host_str().is_none() {
        return Err(ReturnUrlError::Refused);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signing_target_uses_a_public_checkout_origin() {
        let (domain, base) = signing_target(
            Some("https://pay.example.trycloudflare.com"),
            "http://127.0.0.1:3000",
            "127.0.0.1:3000",
        );
        assert_eq!(domain, "pay.example.trycloudflare.com");
        assert_eq!(base, "https://pay.example.trycloudflare.com");
        let (local, _) = signing_target(Some("javascript:alert(1)"), "http://127.0.0.1:3000", "127.0.0.1:3000");
        assert_eq!(local, "127.0.0.1:3000");
        let (fallback, _) = signing_target(None, "http://127.0.0.1:3000", "127.0.0.1:3000");
        assert_eq!(fallback, "127.0.0.1:3000");
    }

    #[test]
    fn allows_relative_and_any_http_host() {
        assert!(assert_return_url("/thanks").is_ok());
        assert!(assert_return_url("http://127.0.0.1:3001/ok").is_ok());
        assert!(assert_return_url("http://localhost:3001/ok").is_ok());
        assert!(assert_return_url("https://shop.example/thanks").is_ok());
        assert_eq!(assert_return_url("javascript:alert(1)"), Err(ReturnUrlError::Refused));
        assert_eq!(assert_return_url("//evil.example"), Err(ReturnUrlError::Refused));
        assert_eq!(
            assert_return_url("https://user:pass@127.0.0.1/x"),
            Err(ReturnUrlError::Refused)
        );
    }
}
