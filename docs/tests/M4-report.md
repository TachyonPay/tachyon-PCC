# M4 — Connect 7702 session keys to 1Click (mocks first)

Date: 2026-11-07. 1Click has **no testnet**. This milestone wires the session-key
`SourceExecutor` onto the existing quote path and proves admission + quote-bind
on mocks. **No mainnet 7702 grant, no live 1Click deposit via session key.**

## What landed

* Internal contract (`/internal/v1/aa/simulate|execute|receipt`). Rust-owned
  OpenAPI; TS types generated from it.
* TS `AaService` builds `transfer(quoteDepositAddress, amount)` from a
  `DepositPlan`. `to` must equal `quoteDepositAddress`. Option C remains the
  on-chain policy (any recipient, amount cap, rate-limit); the **service**
  binds each UserOp to the verified quote deposit so a substituted address
  never reaches the bundler.
* `IntegrationClient` implements `SourceExecutor` over those routes.
  Native deposits are refused (ERC-20 only).
* `prepare_deposit` / `execute_approved` are executor-agnostic. A mocked
  session flow (MockOneClick + MockSourceExecutor) binds `plan.to` to the
  quote deposit and is idempotent per attempt id.
* Memory bundler only. Live ZeroDev send stays in the gated Base Sepolia
  script from M3; it is **not** the HTTP worker default.

## Tests

TS worker (`pnpm test`): **105 passed**. Quote-address substitution, unknown
owner, revoked signer, second chain not enrolled, excessive amount,
duplicate execute (same userOpHash), dry-run refuse, HTTP contract for the
new routes.

Rust (`cargo test --workspace --all-features`): **109 passed**. OpenAPI drift,
DTO camelCase, `SESSION_REJECTED` mapping, simulate / execute / receipt
round-trip, 404 receipt → `None`, native plan refused, `plan_from_quote`
bind + substitution helper, mock flow idempotency.

## Live

**None.** Default gates remain `DRY_RUN=true`, `ENABLE_MAINNET_TRANSFERS=false`.
M3 already demonstrated a Base Sepolia session transfer (account behavior
only). M4 live 1Click via 7702 needs a separate owner approval: Kernel
implementation, permission (token, cap, expiry), source/dest assets, max
total dollar loss, and one specific previewed UserOp.

## Limitation (carried forward)

Call policy still cannot name a 1Click deposit address at onboarding time.
Option C + server-side quote bind + single-use rate limit is the POC.
A signature-bound executor (option B) remains the production follow-up.
Do not treat option C as safe against a compromised session key.
