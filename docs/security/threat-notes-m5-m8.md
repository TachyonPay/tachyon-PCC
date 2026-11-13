# Security notes — M5–M8

## TOTP

Wallet-linked TOTP proves the payer can produce a current code for one
`PaymentAttempt`. It is bound through `amount_bound_hash` (attempt, max
debit, merchant, payout snapshot, output). It does **not**:

* prevent a compromised server that already holds a valid Kernel session
  grant from submitting that bounded call;
* replace execution gates or the owner’s per-transaction approval;
* survive wrap-key disclosure (`TOTP_WRAP_KEY`).

Lockout (5 failures / 5 minutes) and single-use timesteps are online
abuse controls, not an HSM.

## SIWE

Loopback domains only. Recovered signer must match the challenge address.
A stolen checkout URL without the wallet still cannot enroll or authorize
as that wallet.

## Merchant API / webhooks

* API keys hashed (`pcc.apikey.v1|` SHA-256), compared in constant time.
* Tenant isolation: foreign intent ids are 404.
* Webhook SSRF: https (or explicit loopback http), no private/link-local
  IPs, no `.internal` / `.local` hosts.
* HMAC over the raw body; verification is constant-time.

## Remembered device

`pcc_dev_` tokens are SHA-256 hashed at rest and last 30 days. They replace
SIWE only. A token stolen from the browser (for example by XSS) can view the
checkout and revoke an allowance. It cannot authorize a payment without the
TOTP code, and it cannot sign a new allowance. Do not use it on a shared computer.

## Allowance key storage

`GRANT_STORE_KEY` encrypts worker session keys with AES-256-GCM (HKDF-SHA256,
fresh salt, file mode 0600). The key is an env secret, not committed. A wrong
key or a tampered file fails closed. Without the key, allowances are memory
only and a restart asks the payer to sign again.

## Hosted checkout / SDK

* No merchant keys in the payer bundle.
* Popup URL allow-list (loopback). `javascript:` / `data:` / public hosts
  cannot be used as an open redirect.
* React text interpolation for merchant-supplied order ids (no `dangerouslySetInnerHTML`).

## Execution

Unchanged from M2–M4: default dry-run, mainnet off, persist-then-broadcast,
no ERC-20 `approve`, no unattended send.
