# M0 + M1 — foundation and live 1Click adapter (dry quotes)

Date: 2026-11-03. Owner machine: macOS, rustc 1.88.0, Node 24.0.0, pnpm 10.11.1.

## Commands run (all offline unless stated)

| Area | Command | Result |
| --- | --- | --- |
| Rust | `cargo fmt --all --check` | clean |
| Rust | `cargo clippy --workspace --all-targets --all-features -- -D warnings` (pedantic) | clean |
| Rust | `cargo test --workspace --all-features` | **88 passed**, 0 failed |
| Rust | `cargo run -p pcc-integration-client --bin export-internal-openapi` | `openapi/internal-v1.json` regenerated, drift test green |
| Rust | `cargo run -p pcc-api --bin export-public-openapi` | `openapi/public-v1.json` generated, drift test green |
| TS worker | `pnpm install --frozen-lockfile` | 168 pkgs; lockfile committed |
| TS worker | `pnpm gen:types` | `src/generated/internal-api.d.ts` from Rust JSON |
| TS worker | `pnpm typecheck` (strict + exactOptionalPropertyTypes) | clean |
| TS worker | `pnpm test` (vitest, networking mocked) | **60 passed**, 0 failed (5 files) |
| TS worker | `pnpm audit --prod --audit-level=high` | clean after zod 3.25→4.6.7; 1 moderate transitive (`bn.js`, see sdk-compatibility) |
| TS worker | `pnpm live:dry-quote` without key | exit 2, refuses (expected) |
| Frontend | `pnpm install && pnpm gen:types && pnpm typecheck && pnpm test && pnpm build` | 5 projects typecheck; **10 tests passed**; both Next 16 apps build |

## Test inventory

### Rust (88)

* `pcc-payments` (42): `BaseUnits` parsing/decimal/overflow/serde-as-string;
  `ChainId`↔1Click slug mapping; `AssetId::from_one_click_token`; intent and
  attempt state-machine legal/illegal transitions; `OneClickStatus` mapping
  (SUCCESS → Processing, not Settled); `ExecutionGates` defaults and
  "only exact true/false flips"; `check_broadcast` requires approval even when
  open; `amount_bound_hash` determinism/sensitivity; `validate_quote` — each
  violation class (not verified, dry vs executable, swap type, confidentiality
  downgrade, asset mismatch, recipient/refund mismatch, zero amounts, output
  mismatch, min-out below expected, max-debit exceeded, slippage buffer cap,
  deposit address missing/invalid, MEMO without memo, deadline too soon,
  stale timestamp); `MockOneClick` behaviours incl. single-use auth refs.
* `pcc-integration-client` (16, wiremock): bearer + body shape sent, dry vs
  executable mapping, malformed JSON, numeric amount rejected, missing field,
  timeout → `Timeout`, structured error codes → `AdapterError`, fallback by
  HTTP status, connection refused → `Unavailable`, assets filtering, status
  with memo, status enum round-trip, committed-OpenAPI drift.
* `pcc-evm` (8): ERC-20 `transfer` encode/decode, native vs token plan, no
  `approve` path exists, delegation parsing (`0xef0100||addr`), onboarding
  decisions (never overwrite unfamiliar delegation).
* `pcc-auth` (7): API-key format, hash, constant-time verify, mock
  authenticator gate requires debug build AND `"true"`.
* `pcc-merchants` (5), `pcc-storage` (4, in-memory repo idempotency/tenancy),
  `pcc-settlement` (3, poll outcomes + backoff), `pcc-api` (3, health, safe
  default gates, public OpenAPI drift).

### TS worker (60)

* `tests/near/verify.test.ts` (12): official `verifyQuoteSignature` against
  staging fixtures (non-dry, dry, status-embedded); tamper detection for
  recipient/refundTo/assets/amount/quote.deadline/timestamp/amounts;
  **documented limitations pinned as tests**: `confidentiality` unsigned,
  `quoteRequest.deadline` shadowed on non-dry quotes; echo-binding rejects
  each unsigned-field mismatch; wrong manager key rejected.
* `tests/near/client.test.ts`: `X-API-Key` header, no key → no header,
  timeout → `UPSTREAM_TIMEOUT`, 401/403 → `UPSTREAM_UNAUTHORIZED`, 429,
  400 no-route text → `NO_ROUTE`, 5xx → `UPSTREAM_UNAVAILABLE`, zod failure →
  `MALFORMED_UPSTREAM`, redirects refused, status 404 → rejected.
* `tests/near/service.test.ts`: DTO validation (strict), unknown asset ids,
  EXACT_OUTPUT + ORIGIN/DESTINATION types forced, `confidentiality` forwarded
  and echo-checked, dry quotes need no auth ref, executable quotes need a
  single-use `authorizationRef` (reuse → 409 `AUTH_REF_REUSED`, released on
  upstream failure), projection to `VerifiedQuoteDto`, status re-verifies the
  embedded quote, token cache.
* `tests/account-abstraction/policy.test.ts`: ERC-20 transfer permission
  requires exact recipient + `<=` amount, rejects zero/invalid, expiry ≤ 90 d,
  pins `KERNEL_V3_3` / EP 0.7.
* `tests/contract/internal-api.test.ts` (9): Rust OpenAPI paths present,
  `ErrorCode` enum == `ERROR_CODES`, `OneClickStatus` enum match, HTTP: bearer
  required (null/empty/wrong/truncated → 401 contract body), unknown route 404,
  invalid JSON 400, body cap 256 KiB → 413, duplicate auth ref → 409.

### Frontend (10)

* `@pcc/wallet-onboarding`: delegation vectors shared with Rust, onboarding
  decisions, `readDelegation` uses only `getCode`.
* `@pcc/checkout-sdk`: typed gates read, non-2xx → `CheckoutApiError`.

## Live calls

Run 2026-11-04 16:52 UTC after the owner installed `ONECLICK_API_KEY` in the
git-ignored `.env` (presence checked only; value never printed; confirmed never
present in any commit). Both calls are read-only. Procedure:
`docs/runbooks/live-checks.md`.

### `pnpm live:tokens` — `GET /v0/tokens` (exit 0)

EVM matrix returned by 1Click (asset counts per chain, USDC asset ids):

| chain | chainId | assets | USDC asset id |
| --- | --- | --- | --- |
| eth | 1 | 29 | `nep141:eth-0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48.omft.near` (dec 6) |
| base | 8453 | 18 | `nep141:base-0x833589fcd6edb6e08f4c7c32d4f71b54bda02913.omft.near` (dec 6) |
| arb | 42161 | 7 | `nep141:arb-0xaf88d065e77c8cc2239327c5edb3a432268e5831.omft.near` (dec 6) |
| bsc | 56 | 10 | `nep245:v2_1.omni.hot.tg:56_…` (**dec 18**) |
| gnosis | 100 | 9 | `nep141:gnosis-0x2a22…omft.near` (dec 6) |
| op | 10 | 6 | `nep245:v2_1.omni.hot.tg:10_…` (dec 6) |
| pol | 137 | 5 | `nep245:v2_1.omni.hot.tg:137_…` (dec 6) |
| avax | 43114 | 3 | `nep245:v2_1.omni.hot.tg:43114_…` (dec 6) |
| scroll | 534352 | 2 | none (ETH, USDT only) |

Observations: two asset-id families coexist (`nep141:<chain>-<addr>.omft.near`
and `nep245:v2_1.omni.hot.tg:<chainId>_<id>`); our `AssetId` carries the 1Click
id verbatim so both work. BSC USDC is 18-decimals — decimals must always come
from the token list, never assumed from the symbol (the code already does this).

### `pnpm live:dry-quote` — `POST /v0/quote` `dry:true` (exit 0)

Base USDC → Arbitrum USDC, `EXACT_OUTPUT`, `confidentiality: basic`,
placeholder recipient/refund. Redacted output:

```
signatureVerified: true          (official verifyQuoteSignature, prod manager key)
dry: true   swapType: EXACT_OUTPUT   confidentiality: basic   depositMode: SIMPLE
originAssetId:      nep141:base-0x833589fcd6edb6e08f4c7c32d4f71b54bda02913.omft.near
destinationAssetId: nep141:arb-0xaf88d065e77c8cc2239327c5edb3a432268e5831.omft.near
amountOut: 1000000   minAmountOut: 1000000
amountIn:  1015663   minAmountIn:  1005506     (≈1.57 % buffer, ≈0.55 % floor)
quoteTimestamp: 2026-11-04T16:52:02.818Z   timeEstimateSeconds: 34
depositAddress present: false (expected for dry)
correlationId: 8f3407b7-a1bc-402c-99cf-dda30602e031
```

Echo-binding passed (confidentiality/depositMode/deadline matched what we
sent). The server accepted `confidentiality: basic` and echoed it, confirming
the field is live on the production API, not only in the SDK model.

## Docs ↔ API differences found

See `docs/sdk-compatibility.md`: X-API-Key vs JWT; unsigned fields in the
quote signature (incl. the deadline-shadowing finding, new in this run);
`amountInUsd` as string; Kernel v4/EP0.9 README vs SDK 5.x v3.3/EP0.7.

## Security caveats

* Nothing can sign or broadcast. No key refs are read.
* Worker ↔ API boundary is loopback + bearer; the worker refuses public binds.
* Local DNS shim used for installs lives in `/tmp`, outside the repo, and only
  redirects Node's resolver to 1.1.1.1/8.8.8.8.
