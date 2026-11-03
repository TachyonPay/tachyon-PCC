# Security notes — M0/M1

Scope: what exists after M0+M1 and what it can and cannot do.

## What cannot happen yet

* No private key is loaded anywhere. `SOURCE_EXECUTOR_KEY_REF` /
  `SESSION_SIGNER_KEY_REF` are documented but unread (M2/M3).
* No transaction is built with a signer, no `approve()` calldata exists, no
  EIP-7702 authorisation is produced, no ZeroDev account is instantiated at
  runtime (only version pins and pure policy builders, unit-tested).
* 1Click is only called read-only: `GET /v0/tokens`, `POST /v0/quote` with
  `dry: true`. `dry: false` quotes are implemented but require a single-use
  `authorizationRef` and are unreachable without the M4 flow.

## Secrets

* Only `services/integrations-ts` reads `ONECLICK_API_KEY`; sent as
  `X-API-Key` to the pinned HTTPS base URL. Never logged; `redactedSummary()`
  prints only presence booleans.
* The Rust API ↔ worker boundary is loopback + bearer; the worker refuses to
  bind to non-private hosts and rejects tokens shorter than 16 chars.
* `.env` is git-ignored; CI's `secrets-hygiene` job fails on committed `.env`
  or credential assignments.

## Quote integrity

* Official `verifyQuoteSignature()` (Ed25519, 1Click manager key) + echo
  binding for unsigned fields (`confidentiality`, `depositMode`, `deadline`
  on executable quotes). See `docs/sdk-compatibility.md`.
* Rust `validate_quote` is the second, independent check against the *Rust*
  expectation (EXACT_OUTPUT, output amount, max debit, slippage cap, deadline,
  address equality, confidentiality level).
* Raw response JSON is persisted (`quote_raw_response`) for later re-audit.

## Residual risks (open)

* A MITM that can forge the *echoed request* could flip `confidentiality`;
  mitigated by TLS and echo binding, not by the signature. Raised to NEAR
  tooling as a limitation; tracked.
* Session-key call policy vs. per-quote deposit addresses (see sdk-compatibility).
* Local DNS flakiness on the owner machine could push a developer toward
  unreviewed mirrors; the documented shim pins 1.1.1.1/8.8.8.8 only.
