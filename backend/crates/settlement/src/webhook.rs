//! HMAC-signed webhook deliveries and SSRF guards on merchant callback URLs.

use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::net::IpAddr;
use subtle::ConstantTimeEq;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum WebhookError {
    #[error("webhook URL must be https")]
    NotHttps,
    #[error("webhook URL host is not allowed (SSRF)")]
    Ssrf,
    #[error("webhook URL is malformed")]
    Malformed,
    #[error("webhook POST failed: {0}")]
    Transport(String),
}

#[derive(Clone, Copy, Debug, Default)]
pub struct WebhookPolicy {
    pub allow_loopback: bool,
}

pub fn assert_webhook_url(url: &str, policy: WebhookPolicy) -> Result<(), WebhookError> {
    let parsed = reqwest::Url::parse(url).map_err(|_| WebhookError::Malformed)?;
    match parsed.scheme() {
        "https" => {}
        "http" if policy.allow_loopback => {}
        _ => return Err(WebhookError::NotHttps),
    }
    let host = parsed.host_str().ok_or(WebhookError::Malformed)?;
    if host.eq_ignore_ascii_case("localhost") {
        return if policy.allow_loopback {
            Ok(())
        } else {
            Err(WebhookError::Ssrf)
        };
    }
    if let Ok(ip) = host.parse::<IpAddr>() {
        if ip.is_loopback() || is_private(ip) || ip.is_unspecified() || ip.is_multicast() {
            return if policy.allow_loopback && ip.is_loopback() {
                Ok(())
            } else {
                Err(WebhookError::Ssrf)
            };
        }
    } else {
        let last = host.rsplit('.').next().unwrap_or(host);
        if last.eq_ignore_ascii_case("internal") || last.eq_ignore_ascii_case("local") {
            return Err(WebhookError::Ssrf);
        }
    }
    Ok(())
}

fn is_private(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_private() || v4.is_link_local(),
        IpAddr::V6(v6) => v6.is_unique_local() || v6.is_unicast_link_local(),
    }
}

#[must_use]
pub fn sign_body(secret: &[u8], body: &[u8]) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret).expect("hmac key");
    mac.update(body);
    format!("sha256={}", hex::encode(mac.finalize().into_bytes()))
}

#[must_use]
pub fn verify_signature(secret: &[u8], body: &[u8], header: &str) -> bool {
    let expected = sign_body(secret, body);
    let a = expected.as_bytes();
    let b = header.trim().as_bytes();
    if a.len() != b.len() {
        return false;
    }
    a.ct_eq(b).into()
}

/// POST a signed payload. Re-checks the SSRF allow-list. Never follows redirects.
pub async fn deliver_signed(
    client: &reqwest::Client,
    url: &str,
    body: &str,
    signature: &str,
    policy: WebhookPolicy,
) -> Result<(), WebhookError> {
    assert_webhook_url(url, policy)?;
    let resp = client
        .post(url)
        .header("content-type", "application/json")
        .header("x-pcc-signature", signature)
        .body(body.to_owned())
        .send()
        .await
        .map_err(|e| WebhookError::Transport(e.to_string()))?;
    if resp.status().is_success() {
        Ok(())
    } else {
        Err(WebhookError::Transport(format!("status {}", resp.status())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn https_public_ok_http_and_private_refused() {
        let p = WebhookPolicy::default();
        assert!(assert_webhook_url("https://hooks.example.com/pcc", p).is_ok());
        assert_eq!(
            assert_webhook_url("http://hooks.example.com/pcc", p),
            Err(WebhookError::NotHttps)
        );
        assert_eq!(assert_webhook_url("https://127.0.0.1/x", p), Err(WebhookError::Ssrf));
        assert_eq!(assert_webhook_url("https://10.0.0.5/x", p), Err(WebhookError::Ssrf));
        assert_eq!(
            assert_webhook_url("https://169.254.169.254/latest", p),
            Err(WebhookError::Ssrf)
        );
        assert_eq!(assert_webhook_url("file:///etc/passwd", p), Err(WebhookError::NotHttps));
    }

    #[test]
    fn loopback_http_only_when_policy_allows() {
        let p = WebhookPolicy { allow_loopback: true };
        assert!(assert_webhook_url("http://127.0.0.1:9/hook", p).is_ok());
        assert_eq!(assert_webhook_url("https://10.1.1.1/x", p), Err(WebhookError::Ssrf));
    }

    #[tokio::test]
    async fn deliver_signed_posts_hmac_and_refuses_ssrf() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::header("x-pcc-signature", "sha256=dead"))
            .respond_with(wiremock::ResponseTemplate::new(200))
            .mount(&server)
            .await;
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        let policy = WebhookPolicy { allow_loopback: true };
        deliver_signed(&client, &format!("{}/hook", server.uri()), "{}", "sha256=dead", policy)
            .await
            .unwrap();
        assert_eq!(
            deliver_signed(
                &client,
                "https://127.0.0.1/x",
                "{}",
                "sha256=dead",
                WebhookPolicy::default()
            )
            .await,
            Err(WebhookError::Ssrf)
        );
    }

    #[test]
    fn hmac_roundtrip_and_tamper() {
        let sig = sign_body(b"secret", b"{\"id\":\"1\"}");
        assert!(verify_signature(b"secret", b"{\"id\":\"1\"}", &sig));
        assert!(!verify_signature(b"secret", b"{\"id\":\"2\"}", &sig));
        assert!(!verify_signature(b"other", b"{\"id\":\"1\"}", &sig));
    }
}
