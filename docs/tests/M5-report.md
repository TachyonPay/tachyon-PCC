# M5 — wallet-linked TOTP + SIWE

Date: 2026-11-07. No mainnet value movement.

## Scope

* RFC 6238 TOTP in `pcc-auth` (HMAC-SHA1, 6 digits, 30s, ±1 window), RFC
  vectors `59 → 287082` and `1111111109 → 081804`.
* Wrap/unwrap of the secret with a process master key; `otpauth://` URI has
  no newlines and is returned once on enroll (never logged).
* SIWE subset (EIP-4361 + EIP-191 recover): nonce, expiry, loopback domain
  allow-list, signer recovery.
* `TotpAuthorizer` implements `PaymentAuthorizer`: enroll, confirm owner,
  lockout (5 / 5 min), single-use timestep, `amount_bound_hash` on consume.
* Hosted-checkout routes bind SIWE → wallet credential → attempt → OTP.

## Commands

| Area | Command | Result |
| --- | --- | --- |
| Rust | `cargo test -p pcc-auth --all-features` | **14 passed** (TOTP RFC vectors, SIWE, authorizer) |
| Rust | `cargo test -p pcc-api --all-features` | **10 passed** (stolen session, unauthenticated signer, foreign attempt, replay, open redirect) |

## Residual

TOTP is payer presence. A compromised API that already holds a valid
session grant can still submit the bounded UserOp; gates + per-tx approval
remain the value-moving control. No unattended mainnet.
