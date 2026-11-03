# ADR-001: Rust-first core with a private TypeScript integration worker

Status: accepted (M0). Owner-reviewable; revisit at M3 and M8.

## Context

The product needs (a) a payment core with strict money semantics — U256
amounts, idempotency, an auditable state machine, bounded authorisations — and
(b) two third-party integrations whose *official* client code is TypeScript:

* **NEAR Intents 1Click** publishes `@defuse-protocol/one-click-sdk-typescript`,
  which contains the only vendor-maintained implementation of
  `verifyQuoteSignature()` (the Ed25519 check over a stable-stringified quote
  payload keyed by the 1Click manager key). There is a community Rust crate
  (`one-click-sdk-rs` 0.1.17) but it does not carry the verifier and is not
  vendor-owned.
* **ZeroDev Kernel** session keys (`@zerodev/sdk`, `@zerodev/permissions`,
  `@zerodev/ecdsa-validator`) are TS-only; the Rust `zerodev-aa` crate is
  `0.0.1-alpha.4` and does not cover permissions/call policies.

Re-implementing either in Rust would mean we own the security-critical parts
(signature canonicalisation, policy encoding) with no upstream to track.

## Decision

1. **Rust owns the product.** Merchant/payment APIs, state machine, quote rule
   enforcement, ledger/storage, settlement, auth/TOTP, execution gates, and the
   plain-EOA deposit executor (Alloy) are Rust crates under `backend/`.
2. **TypeScript is confined to a private worker** (`services/integrations-ts`)
   that wraps the two vendor SDKs. It binds to loopback only, authenticates the
   Rust caller with a bearer token, and exposes a narrow surface.
3. **The boundary contract is Rust-owned.** `pcc-integration-client` defines
   the DTOs with `utoipa`, exports `openapi/internal-v1.json`, and a Rust test
   fails if the committed JSON drifts. The TS worker generates its types from
   that JSON and has a contract test asserting its error-code enum and status
   enum equal the Rust ones. The same pattern is used for the public API
   (`pcc-api` → `openapi/public-v1.json` → `@pcc/api-types`).
4. **Trust-but-verify across the boundary.** The worker verifies and
   request-binds quotes; Rust *re-validates* the projected quote
   (`validate_quote`) against its own expectation before persisting or acting.
   Signature verification results are carried as a boolean plus the raw
   response JSON so Rust can re-check later if a Rust verifier is adopted.
5. **Amount representation:** JSON strings of base units everywhere; `U256`
   (alloy-primitives) in Rust, `bigint`/validated digit strings in TS. Asset
   identity is `chainId + token + decimals + 1Click assetId`.

## Consequences

* Two toolchains in CI (cargo + pnpm); accepted.
* Any 1Click/ZeroDev SDK bump is a reviewed change to pinned versions in
  `services/integrations-ts/package.json` and `docs/sdk-compatibility.md`.
* Known verifier gaps (fields outside the signed payload) are mitigated in the
  worker and documented; they are not silently relied upon by Rust.
* If a vendor-maintained Rust verifier appears, the worker's quote path can be
  swapped without changing the Rust domain model (the `OneClickAdapter` trait
  is the seam).

## Alternatives rejected

* *Rust-only*: owning crypto canonicalisation for 1Click and policy encoding
  for Kernel with no upstream; highest risk for an MVP.
* *TS-only*: weaker guarantees for money handling and the owner's stated
  preference for a Rust core.
* *Shared-process (N-API) embedding*: tighter coupling, harder to audit the
  secret boundary than a loopback HTTP worker.
