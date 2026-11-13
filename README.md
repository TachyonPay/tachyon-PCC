# Private Crypto Checkout (MVP)

A merchant checkout where a payer pays with any supported token on any supported
chain and the merchant receives their configured asset, with the cross-chain leg
routed through **NEAR Intents 1Click** in *confidential* mode and the payer side
authorised by a **bounded, single-use EIP-7702 / ZeroDev Kernel session key**.

Status: **M0–M8 in tree**. M2 live Base → Arbitrum USDC via the plain EOA is
recorded. M3 Kernel v3.3 / EIP-7702 session transfer is recorded on **Base
Sepolia**. M4 session-key 1Click on Base mainnet is recorded (owner-approved
0.50 then 0.20 USDC). Hosted checkout now SIWE → dry quote → TOTP → executable
1Click quote → gated session simulate/execute. Defaults remain `DRY_RUN=true`
and `ENABLE_MAINNET_TRANSFERS=false`. No unattended mainnet sends.

## Layout

```
backend/                 Rust workspace (Cargo) — product core
  crates/payments        domain, amounts (U256), asset identity, state machines, quote rules, gates, adapter traits
  crates/integration_client  Rust-OWNED OpenAPI for the internal boundary + reqwest client (OneClickAdapter impl)
  crates/evm             Alloy 1.6.3: ERC-20/native deposit calldata, EIP-7702 inspection, gated EOA executor + persist-then-broadcast ledger
  crates/auth            API keys, SIWE, RFC 6238 TOTP, mock-authenticator gate
  crates/merchants       merchant / payout-config / payment-intent rules
  crates/storage         sqlx migrations + repository trait + in-memory impl
  crates/settlement      status polling + backoff + webhook HMAC/SSRF
  crates/api             Axum server; Rust-OWNED public OpenAPI (merchant + hosted checkout)
services/integrations-ts Private TS worker (loopback only): NEAR 1Click adapter + ZeroDev Kernel pins/policies
frontend/                pnpm workspace: apps/checkout, apps/dashboard (Next 16), packages/{api-types,checkout-sdk,wallet-onboarding}
docs/                    ADRs, SDK compatibility notes, milestone test reports, security, runbooks
infra/                   docker-compose (Postgres), db notes
examples/                confidential-swap recipe; merchant-demo intent script
```

Why two languages: see [`docs/adr/ADR-001-hybrid-rust-ts-stack.md`](docs/adr/ADR-001-hybrid-rust-ts-stack.md).
Which SDK versions and why: [`docs/sdk-compatibility.md`](docs/sdk-compatibility.md).

## Trust boundary

```
browser ──► frontend (Next) ──► Rust API (127.0.0.1:8080, public contract)
                                   │  bearer + loopback only (internal contract, Rust-owned OpenAPI)
                                   ▼
                            TS worker (127.0.0.1:8787) ──► 1Click API (X-API-Key)  /  ZeroDev bundler (M3+)
```

* The TS worker is the only process that holds `ONECLICK_API_KEY`. It refuses
  to bind to a non-private host, refuses non-HTTPS upstreams, and logs only
  method/path/status/latency.
* Every 1Click quote is verified with the official `verifyQuoteSignature()` and
  then **re-bound to the request we sent** for the fields the signature does not
  cover (`confidentiality`, `depositMode`, and `deadline` on executable quotes).
  Rust re-validates the projected quote (`pcc_payments::validate_quote`) before
  anything is persisted.
* Contracts are Rust-owned: `backend/crates/integration_client/openapi/internal-v1.json`
  and `backend/crates/api/openapi/public-v1.json` are generated from Rust types,
  committed, drift-tested in Rust, and consumed by `openapi-typescript` on the
  TS side. Amounts are JSON strings / U256 / bigint — never floats.

## Quick start (offline, no secrets needed)

```sh
# Rust
cd backend && cargo fmt --check && cargo clippy --workspace --all-targets --all-features -- -D warnings && cargo test --workspace --all-features

# TS worker
cd services/integrations-ts && pnpm install --frozen-lockfile && pnpm typecheck && pnpm test

# Frontend
cd frontend && pnpm install --frozen-lockfile && pnpm typecheck && pnpm test && pnpm build
```

Run locally: copy `.env.example` to `.env` (never commit it), start Postgres via
`infra/docker/docker-compose.yml`, then `cargo run -p pcc-api`, `pnpm start` in
`services/integrations-ts`, and `pnpm --filter @pcc/checkout dev`.

## Live read-only checks (owner-run)

Requires `ONECLICK_API_KEY` in your local `.env`. Both scripts are read-only
(`GET /v0/tokens`, `POST /v0/quote` with `dry: true`), never print the key, and
exit with code 2 if the key is missing. See
[`docs/runbooks/live-checks.md`](docs/runbooks/live-checks.md).

```sh
cd services/integrations-ts && pnpm live:tokens && pnpm live:dry-quote
```

## Safety rules enforced in code

| Rule | Where |
| --- | --- |
| Default dry-run, mainnet off, only exact `"true"`/`"false"` flips a gate | `pcc_payments::ExecutionGates` |
| Broadcast needs an approval reference even when gates are open | `ExecutionGates::check_broadcast` |
| Never `approve()` ERC-20 spending; only `transfer` calldata is built | `pcc_evm::transfer` |
| Persist the signed raw tx *before* `eth_sendRawTransaction`; restart rebroadcasts the same hash | `pcc_evm::ledger` + `AlloyEoaExecutor` |
| Never overwrite an unfamiliar EIP-7702 delegation | `pcc_evm::delegation`, `@pcc/wallet-onboarding` |
| Session-key call policy fixes target, selector, recipient and max amount | `services/integrations-ts/src/account-abstraction/policy.ts` |
| Payment UserOp recipient must equal the verified quote deposit address | `AaService` + `assert_plan_matches_quote` |
| Single-use authorisation refs for executable quotes | `NearService.quote`, `MockOneClick` |
| Mock authenticator only in debug builds behind a feature *and* an env flag | `pcc_auth::mock_authenticator_allowed` |
| TOTP bound to one attempt (`amount_bound_hash`); wrap key never logged | `pcc_auth::TotpAuthorizer` |
| Webhook HTTPS + SSRF denylist; HMAC-SHA256 over the raw body | `pcc_settlement::webhook` |
| Hosted checkout popup opens any `http`/`https` URL; `javascript:` and `data:` are refused. `success_url` is the same, and the page returns the payer there with `pcc_result` | `@pcc/checkout-sdk`, `pcc_api::assert_return_url` |
| Secrets never logged; worker config summary is redacted | `services/integrations-ts/src/config.ts` |
| Live sends are payer-approved (TOTP or autopay). `CHECKOUT_APPROVAL_MODE=owner` parks them for an operator echo | `pcc_api::operator` |
| Worker holds no wallet key; payer signs one allowance (per-payment cap, uses ≤ 20, ≤ 30 days), revocable, then pays with TOTP. Session keys are encrypted at rest when `GRANT_STORE_KEY` is set | `SessionGrantService`, `EncryptedGrantStore`, `pcc_api::grant` |
| A remembered device skips the wallet sign-in for 30 days; every payment still needs TOTP | `pcc_api::device`, checkout `pcc.device.v1` |
| Autopay needs TOTP + one signature to set up; each charge is capped, counted, spaced and revocable, on its own session key | `pcc_api::autopay`, `SessionGrantService` (`mandateId`) |

## Milestones

| | Scope | Status |
| --- | --- | --- |
| M0 | Foundation, contracts, mocks, CI | done |
| M1 | Live 1Click adapter, dry quotes, signature verification | done (live dry quote recorded) |
| M2 | Direct EOA deposit executor (Alloy), Anvil + gated mainnet | done (live Base→Arb USDC swap recorded) |
| M3 | ZeroDev Kernel session keys + EIP-7702 onboarding | offline tests + Base Sepolia transfer recorded |
| M4 | Connect 7702 executor to 1Click | done (live 0.50 USDC session-key 1Click recorded) |
| M5 | Wallet-linked TOTP + SIWE | done (local; no new mainnet send) |
| M6 | Merchant API + dashboard + webhooks | done (in-memory + committed SQL schema) |
| M7 | Hosted checkout + browser SDK | done |
| M8 | Hardening, runbooks (no unattended mainnet) | done |
| M9 | Postgres storage | deferred (in-memory store; SQL schema committed) |
| M10 | Owner-approval pause (opt-in `CHECKOUT_APPROVAL_MODE=owner`); default is payer-approved | done (offline tests; gated live E2E) |
| M11 | Real payer flow: configurable SIWE domain, payer-signed standing allowances (sign once, then TOTP), 7702 status, WalletConnect | done (gated live E2E on Base USDC; no send) |
| M12 | Merchant onboarding: invite signup, key / webhook-secret rotation, payout UI | done (offline tests + live smoke) |
| Autopay | Merchant-initiated charges against a payer-signed mandate, with no TOTP per charge ([runbook](docs/runbooks/autopay.md)) | done (gated live E2E: 1 setup, 2 merchant charges, no send) |
| Device | Remember this browser so later checkouts skip SIWE; TOTP still required ([runbook](docs/runbooks/m11-payer-flow.md)) | done (API, SDK, checkout UI; gated E2E resumes payment 2) |
| Key store | Worker session keys encrypted at rest (`GRANT_STORE_KEY`); wrong key or tampering fails closed | done (offline tests; API mandates and TOTP stay in memory until M9) |
| M13 | Checkout sheet: Tachyon Pay as a 420px payment modal (tachyon.pe colour/type, Blade GenUI layout). Amounts in USDC. WalletConnect QR. | done |
| M14 | Merchant dashboard tabs (payments / payout / account / signup) on the same `/v1` proxy | done |
| M15 | SDK `openHostedCheckout` overlay iframe (merchant modal). Popup is `{ mode: "popup" }`. | done |

Milestone evidence lives in `docs/tests/`.
