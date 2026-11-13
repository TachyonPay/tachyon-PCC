# Runbook: M5 wallet-linked TOTP + SIWE

No mainnet value movement. Enrollment and authorization are local API calls.

## What this is

* RFC 6238 TOTP (HMAC-SHA1, 6 digits, 30s, ±1 step) bound to a wallet
  credential after SIWE (EIP-4361).
* A verified code is bound to one `PaymentAttempt` via `amount_bound_hash`
  and consumed once. The same code cannot authorize a second attempt.
* Secrets are XOR-wrapped with `TOTP_WRAP_KEY` at rest and never logged.

## What this is not

TOTP does **not** stop a compromised API process from moving funds that the
payer already authorized. Session-key policy + execution gates + per-tx
approval still apply. Treat TOTP as payer presence, not as a server HSM.

## Local flow

1. Start `pcc-api` with `PCC_DEMO_MERCHANTS=true` (see M6 runbook).
2. Create an intent; open `/checkout/{session}`.
3. SIWE with an injected wallet (loopback domains only: `localhost` /
   `127.0.0.1` on ports 3000/8080).
4. Scan or paste the `otpauth://` URI (shown once).
5. Confirm enrollment, then authorize the attempt with a fresh code.

Negative cases covered in `pcc-auth` / `pcc-api` tests: stolen session id,
unauthenticated signer, lockout after 5 failures, OTP replay, binding-hash
mismatch, arbitrary `attempt_id`.
