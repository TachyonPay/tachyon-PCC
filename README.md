# Tachyon private payments

Private checkout and private x402. EIP-7702 turns one allowance into a walletless, card-like payment: a code, a mandate, or autopay releases each later charge.

[![Watch the Tachyon demo](docs/demo/poster.jpg)](docs/demo/tachyon-demo.mp4)

[Watch the demo (4:15)](docs/demo/tachyon-demo.mp4). It opens on the product, then walks through one example shop.

## What that means

**Chain agnostic.** The payer pays with the asset they already hold, on the chain they already use. Today that is USDC on Ethereum, Optimism, Base, or Arbitrum. The merchant sets a payout once: an address, a chain, and an asset. NEAR Intents moves the value between those two sides. The payer and the merchant do not have to share a chain, a token, or a counterparty. An explorer sees a deposit on the payer's chain and a payout on the merchant's chain, and those two transfers do not name each other.

**Accountless.** A person does not open a Tachyon account to pay, and an agent does not register one either. Private checkout uses the wallet the person already has. They sign a bounded allowance once. A code releases each later charge. Private x402 is one transfer from the agent's own wallet. The merchant is the party that holds an API key. Payers and agents stay accountless.

Both products are familiar checkout, the kind a card payment already trained people to expect: an amount, a confirmation, a receipt, a refund when the payment cannot finish, and a record the merchant keeps. The chain work stays on the rails. A person or an agent is not asked to assemble a raw transaction, switch networks, or approve a token as the thing they understand.

## Two products

Both products ask NEAR Intents for an exact output: the amount the merchant must receive, in the asset the merchant configured. Intents quotes a deposit on the payer's chain, in confidential mode, and refunds anything that cannot settle back to the payer on that same chain (`ORIGIN_CHAIN`). Tachyon verifies the quote signature and checks it still matches the payment before anything is stored.

### Private checkout, for people

A human-friendly standing payment.

1. The wallet delegates once (EIP-7702, Kernel). The wallet the person already has stays theirs. MetaMask, or another wallet, only funds it.
2. They sign an allowance: a cap, a few uses, and an end date.
3. Each charge needs a fresh TOTP code. One code, one payment. Then that code is dead.
4. Later charges spend from the funded checkout wallet, inside the allowance.

The payer sees a checkout page, an amount, and a code. The merchant sees an order that lands as their configured asset.

### Private x402, for agents

An agent-friendly payment. Scheme `exact-intents`, x402 version 1.

The agent reads the payment requirements, picks a chain and an asset, and sends one wallet transfer to the deposit address Intents quoted. There is no delegation and no code. The facilitator checks the transfer and Intents bridges it to the merchant's chain and asset. A program can do this from an HTTP client. A person can do the same transfer from a wallet, which is what the example shop's x402 path shows.

## Try it

| | |
| --- | --- |
| Hosted checkout | [pay.tachyon.pe](https://pay.tachyon.pe) |
| Merchant dashboard | [dash.tachyon.pe](https://dash.tachyon.pe) |
| Example shop | [harborline.vercel.app](https://harborline.vercel.app) |

Harborline is a shop that sells one 3D-printed boat for 0.20 USDC and receives USDC on Arbitrum. Tachyon is the checkout. Open the shop, choose **Buy Mini Hull**, then choose **Private checkout** or **Private x402**. Checkout opens on `pay.tachyon.pe`. The shop never puts the merchant key in the browser.

A checkout URL is minted per order: `https://pay.tachyon.pe/checkout/{session}`. The bare homepage is the app. The example shop is how you start an order.

The dashboard is where a merchant signs up, sets the payout, and reads orders. The API key stays in the password field in that tab.

## Keys

Signup is invite-gated. Tachyon issues a `MERCHANT_SIGNUP_TOKEN` (at least 24 characters). With that token, either use the dashboard or call the API. The response includes `api_key` (`pcc_sk_` + 64 hex) and `webhook_secret` (`whsec_` + 64 hex) once. Store both. They are not shown again.

```sh
curl -sS -X POST https://pay.tachyon.pe/v1/merchants \
  -H "X-Signup-Token: $MERCHANT_SIGNUP_TOKEN" \
  -H "content-type: application/json" \
  -d '{
    "name": "Example",
    "payout": {
      "receiving_address": "0xYOUR_MERCHANT_ADDRESS",
      "chain_id": 42161,
      "decimals": 6,
      "one_click_asset_id": "nep141:arb-0xaf88d065e77c8cc2239327c5edb3a432268e5831.omft.near",
      "token_address": "0xaf88d065e77c8cc2239327c5edb3a432268e5831"
    }
  }'
```

That payout is USDC on Arbitrum. Swap `chain_id`, `one_click_asset_id`, and `token_address` for the asset you actually want to receive. Amounts everywhere are integer base units (6 decimals for USDC: `200000` is 0.20).

Then create an order from your server. The browser should never see the key.

```sh
curl -sS -X POST https://pay.tachyon.pe/v1/payment-intents \
  -H "Authorization: Bearer $MERCHANT_API_KEY" \
  -H "Idempotency-Key: order-1001" \
  -H "content-type: application/json" \
  -d '{
    "merchant_order_id": "order-1001",
    "output_amount": "200000",
    "success_url": "https://your-shop.example/return"
  }'
```

The response includes `checkout_url` and `checkout_session_id`. Send the payer to `checkout_url`. Confirm the order with `GET /v1/payment-intents/{id}` on your server before you fulfill it.

Other merchant calls, all with `Authorization: Bearer`:

| Call | What it does |
| --- | --- |
| `PUT /v1/payout-config` | Set where settlement lands. Later intents snapshot it. |
| `PUT /v1/webhook-url` | HTTPS endpoint for payment events. |
| `GET /v1/payment-intents` | List this merchant's orders. |
| `GET /v1/payment-intents/{id}/events` | Signed event log for one order. |
| `POST /v1/api-keys/rotate` | New key, old key revoked immediately. |
| `POST /v1/webhook-secret/rotate` | New secret for later deliveries. |

Webhook bodies are signed `X-PCC-Signature` (HMAC-SHA256 over the raw body). Local demo keys (`pcc_sk_alice_demo`) exist only when `PCC_DEMO_MERCHANTS=true` on a local API. They do not work on `pay.tachyon.pe`.

## Refunds and the merchant's books

A payment can be reversed, and the merchant can see that it was. If Intents cannot complete the swap, the deposit refunds to the payer's wallet on the chain they paid from. Any unused amount above the exact output returns the same way. The attempt becomes `refunded`, and the merchant receives a `payment.refunded` webhook. `payment.succeeded` and `payment.failed` are the other terminal events. Each payload carries the intent, the attempt, and the source and destination transaction hashes.

That is the record a merchant keeps. `GET /v1/payment-intents` and `GET /v1/payment-intents/{id}/events` are the book: order id, amount, payout asset, status, timestamps, and both transfers. A refund is a line in that book, the way a card refund is. The same lines are what a merchant uses for tax: what was received, in which asset, on which chain, what was refunded, and the hashes behind each one. The allowance on private checkout is capped, counted, and dated, so a refund does not leave an open permission to spend.

## Payment adapters, including fiat

Settlement sits behind adapter traits in `backend/crates/payments/src/traits.rs`: quoting (`OneClickAdapter`), the payer-side deposit (`SourceExecutor`), the standing allowance (`SessionGrantPort`), and the code that releases a human payment (`PaymentAuthorizer`). The live quote adapter is NEAR Intents. A compatible offramp plugs in at that same boundary.

A country adapter takes the settled payment and pays the merchant in local fiat through an offramp that is legal there. The checkout, the x402 facilitator, the webhooks, and the tax record stay the same. The merchant still creates an intent for an amount. The adapter decides how that amount becomes money in that country. Anyone who implements the adapter can attach their offramp. The shop and the agent do not change.

## Set up x402 for an API or an agent

x402 uses the same payment intent as checkout. Create the intent with the merchant key, then hand the session id to the agent. The facilitator routes below do not take the merchant key.

```sh
# What this facilitator speaks.
curl -sS https://pay.tachyon.pe/v1/x402/supported

# Options for this order: chain, asset, amount.
curl -sS https://pay.tachyon.pe/v1/x402/sessions/$SESSION/requirements
```

`supported` returns scheme `exact-intents` on `eip155:1`, `eip155:10`, `eip155:8453`, and `eip155:42161`. `requirements` returns `accepts[]`. Pick one. `extra.asset_id` is the NEAR asset id you send to prepare.

```sh
curl -sS -X POST https://pay.tachyon.pe/v1/x402/sessions/$SESSION/prepare \
  -H "content-type: application/json" \
  -d '{"asset_id":"'"$ASSET_ID"'","payer":"'"$PAYER"'"}'
```

Prepare returns `deposit_address`, `quoted_input`, `token_address`, `chain_id`, and `attempt_id`. The agent transfers `quoted_input` base units of that token to `deposit_address` and keeps the transaction hash.

```sh
curl -sS -X POST https://pay.tachyon.pe/v1/x402/verify \
  -H "content-type: application/json" \
  -d @payment.json

curl -sS -X POST https://pay.tachyon.pe/v1/x402/settle \
  -H "content-type: application/json" \
  -d @payment.json
```

`payment.json` is the facilitator body. `payment_requirements` is the `accepts[]` entry you prepared against, with `pay_to` set to the deposit address from prepare:

```json
{
  "payment_payload": {
    "x402_version": 1,
    "scheme": "exact-intents",
    "network": "eip155:8453",
    "payload": {
      "tx_hash": "0xTRANSACTION",
      "from": "0xPAYER",
      "amount": "207419",
      "asset": "0xTOKEN",
      "pay_to": "0xDEPOSIT",
      "attempt_id": "ATTEMPT_UUID"
    }
  },
  "payment_requirements": {
    "scheme": "exact-intents",
    "network": "eip155:8453",
    "max_amount_required": "207419",
    "asset": "0xTOKEN",
    "pay_to": "0xDEPOSIT",
    "max_timeout_seconds": 600,
    "extra": {
      "name": "USDC",
      "chain_id": 8453,
      "chain_name": "Base",
      "asset_id": "nep141:base-0x833589fcd6edb6e08f4c7c32d4f71b54bda02913.omft.near",
      "decimals": 6,
      "symbol": "USDC",
      "description": "Confidential NEAR Intents deposit"
    }
  }
}
```

`verify` answers `is_valid`. `settle` records the deposit and returns `intent_status`. Intents then pays the merchant. Poll `GET /v1/payment-intents/{id}` with the merchant key, or wait for `payment.succeeded`.

The example shop proxies these same routes from its server (`examples/merchant-demo/lib/pcc.mjs`) so a page can pay without holding the key. An agent can skip the page and call `pay.tachyon.pe` directly.

## For developers

Local defaults stay `DRY_RUN=true` and `ENABLE_MAINNET_TRANSFERS=false`. The hosted service at `pay.tachyon.pe` is the live checkout. A local send still needs the gates opened on purpose, and a payer approval (TOTP, or an autopay mandate) before anything is broadcast.

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
