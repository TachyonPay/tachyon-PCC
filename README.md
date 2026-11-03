# Private Crypto Checkout (MVP)

A merchant checkout where a payer pays with any supported token on any supported
chain and the merchant receives their configured asset, with the cross-chain leg
routed through **NEAR Intents 1Click** in *confidential* mode and the payer side
authorised by a **bounded, single-use EIP-7702 / ZeroDev Kernel session key**.

Status: **M0 + M1 delivered** (foundation + live 1Click adapter, dry quotes
only). No code path in this repository can move funds yet: `DRY_RUN=true` and
`ENABLE_MAINNET_TRANSFERS=false` are compiled-in defaults and every value-moving
operation additionally requires an explicit owner approval reference.

## Layout

```
backend/                 Rust workspace (Cargo) — product core
  crates/payments        domain, amounts (U256), asset identity, state machines, quote rules, gates, adapter traits
  crates/integration_client  Rust-OWNED OpenAPI for the internal boundary + reqwest client (OneClickAdapter impl)
  crates/evm             Alloy: ERC-20 transfer calldata, EIP-7702 delegation parsing
  crates/auth            API keys (hash + constant-time verify), mock-authenticator gate
  crates/merchants       merchant / payout-config / payment-intent rules
  crates/storage         sqlx migrations + repository trait + in-memory impl
  crates/settlement      status polling + backoff
  crates/api             Axum server; Rust-OWNED public OpenAPI (frontend types are generated from it)
services/integrations-ts Private TS worker (loopback only): NEAR 1Click adapter + ZeroDev Kernel pins/policies
frontend/                pnpm workspace: apps/checkout, apps/dashboard (Next 16), packages/{api-types,checkout-sdk,wallet-onboarding}
docs/                    ADRs, SDK compatibility notes, milestone test reports, security, runbooks
infra/                   docker-compose (Postgres), db notes
examples/                placeholders for M5/M7 demos
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
| Never overwrite an unfamiliar EIP-7702 delegation | `pcc_evm::delegation`, `@pcc/wallet-onboarding` |
| Session-key call policy fixes target, selector, recipient and max amount | `services/integrations-ts/src/account-abstraction/policy.ts` |
| Single-use authorisation refs for executable quotes | `NearService.quote`, `MockOneClick` |
| Mock authenticator only in debug builds behind a feature *and* an env flag | `pcc_auth::mock_authenticator_allowed` |
| Secrets never logged; worker config summary is redacted | `services/integrations-ts/src/config.ts` |

## Milestones

| | Scope | Status |
| --- | --- | --- |
| M0 | Foundation, contracts, mocks, CI | done |
| M1 | Live 1Click adapter, dry quotes, signature verification | done (live run pending owner key) |
| M2 | Direct EOA deposit executor (Alloy), testnet | next — gated |
| M3 | ZeroDev Kernel session keys + EIP-7702 onboarding | gated |
| M4 | Checkout flow end-to-end on testnet | gated |
| M5 | Confidential swap example | gated |
| M6 | Merchant API + dashboard | gated |
| M7 | Webhooks, settlement, reconciliation | gated |
| M8 | Hardening, runbooks, mainnet rehearsal (owner-approved per tx) | gated |

Milestone evidence lives in `docs/tests/`.
