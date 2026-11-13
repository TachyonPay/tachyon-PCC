# M7 — hosted checkout + browser SDK

Date: 2026-11-07. No mainnet value movement.

## Scope

* Hosted page `/checkout/{public_session_id}` (Next 16). Public GET; no
  merchant keys in the browser bundle.
* SIWE + TOTP UI. Authorize does not broadcast; executor gates still apply.
* `@pcc/checkout-sdk`: `openHostedCheckout` popup (`noopener,noreferrer`)
  with loopback-only URL allow-list.
* `success_url` open-redirect tests in Rust (`javascript:`, `//evil`,
  public hosts refused).

## SDK tests

* Merchant methods without `apiKey` → 401 locally, key never placed in the
  URL.
* Public `checkout(sessionId)` sends no `Authorization` header.
* `javascript:`, `data:`, credentialed URLs, `evil.example` rejected;
  `window.open` is not called for a disallowed URL.
