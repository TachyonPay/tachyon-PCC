# SDK & toolchain compatibility (M0/M1)

Everything below was checked against the *installed* package contents (not just
docs) on 2026-11-03. Pins are exact (no `^`). Bumps are reviewed changes.

## NEAR Intents 1Click

| Item | Value | Evidence |
| --- | --- | --- |
| API base | `https://1click.chaindefuser.com` | live `GET /docs-json` (OpenAPI 0.1.10) |
| Auth | `X-API-Key` header (recommended); JWT bearer still accepted (legacy) | live OpenAPI `securitySchemes` |
| TS SDK | `@defuse-protocol/one-click-sdk-typescript` **0.1.26** (2026-09-22) | `node_modules/.../package.json`; `OpenAPI.VERSION = "0.1.10"` |
| Verifier | `verifyQuoteSignature(response, managerPublicKey?)`, `quoteHash`, `hashQuote` exported | `dist/index.d.ts:14620-14621` |
| Manager key (prod) | `ed25519:reYaWhvwu8Jzo3WUM3zhn6VrhuMEF4eADL17qtRVifc` | `dist/index.mjs:1593` |
| Staging key (fixtures only) | `ed25519:5J5tkaxyPoR3Q9S8LXfo5bWnXK5Z2bctJ4mB9gENh7co` | SDK test fixtures (`fixtures/one-click-staging-quotes.json`) |
| `confidentiality` field | present in request model (`public\|basic\|advanced`) | `dist/index.d.ts:1428` and live OpenAPI |
| Status enum | `KNOWN_DEPOSIT_TX, PENDING_DEPOSIT, INCOMPLETE_DEPOSIT, PROCESSING, SUCCESS, REFUNDED, FAILED` | SDK d.ts + Rust `OneClickStatus` (contract test) |
| Rust crate `one-click-sdk-rs` 0.1.17 | reference only, not used | no verifier, community-maintained |

### What the signature covers (verified in `dist/index.mjs`)

```
hash = sha256(stableStringify({ ...signedRequest, ...signedQuote, timestamp }))
signedRequest = { dry, swapType, slippageTolerance, originAsset, depositType, destinationAsset,
                  amount, refundTo, refundType, recipient, recipientType, deadline,
                  quoteWaitingTimeMs?, referral?, virtualChainRecipient?, virtualChainRefundRecipient?,
                  customRecipientMsg? }   // sessionId, connectedWallets, correlationId, appFees,
                                          // partnerId, userAccountId, depositMode forced undefined
signedQuote   = amounts (+ depositAddress, depositMemo, deadline, timeWhenInactive,
                  timeEstimate, refundFee, withdrawFee for non-dry)
```

**Limitations pinned by tests (`tests/near/verify.test.ts`):**

1. `confidentiality`, `depositMode`, `appFees`, `sessionId`, `connectedWallets`
   are **not** signed. A tampered echo would still verify.
2. Because of the object spread, on **executable** quotes `quote.deadline`
   shadows `quoteRequest.deadline`, so the echoed *request* deadline is not
   signed either (it is for dry quotes).

**Mitigation:** `verifyQuoteResponse()` runs the official verifier and then
requires the echoed `quoteRequest` to equal what we sent for all of
`ECHO_BOUND_FIELDS` (includes `confidentiality`, `depositMode`, `deadline`,
assets, amounts, recipient/refund). Rust `validate_quote` re-checks
confidentiality ≥ required, amounts, addresses, deadline. Transport is HTTPS
only (non-https base URLs are refused at config load).

### Docs vs. live API differences noted

* Docs name JWT auth prominently; the live schema marks `X-API-Key` as the
  recommended scheme. We use `X-API-Key` only.
* `quoteRequest` echoes include nullable fields not in the request model
  (`sessionId`, `connectedWallets`, etc.); our zod schema is `loose` (passthrough) with
  explicit nullables for the known ones.
* `amountInUsd`/`amountOutUsd` are strings (not numbers) in live responses.

## ZeroDev / Kernel

| Item | Value | Evidence |
| --- | --- | --- |
| `@zerodev/sdk` | **5.5.11** (2026-10-16) | installed |
| `@zerodev/permissions` | **5.6.4** (2026-10-16) | installed; `CallPolicyVersion` has `V0_0_1 … V0_0_5` |
| `@zerodev/ecdsa-validator` | **5.4.9** | installed |
| `viem` | **2.57.16** | installed (worker + frontend share the pin) |
| Kernel version pinned | `KERNEL_V3_3` with `getEntryPoint("0.7")` | `src/account-abstraction/pins.ts` |
| Call policy version | `V0_0_4` (we do not adopt V0_0_5 until reviewed) | `policy.ts` |

**Do not mix:** the Kernel repo `dev` branch README documents **Kernel v4 on
EntryPoint v0.9**. The published SDK 5.x line targets v3.x / EP 0.7. We pin
v3.3 / EP 0.7 and treat any v4 migration as a separate reviewed milestone.

**Design risk carried to M3/M4:** `toCallPolicy` fixes `target` and arg
conditions at grant time, but 1Click deposit addresses are *per quote*. A
session key therefore cannot pre-authorise "transfer to whatever deposit
address the next quote returns". Options documented in `policy.ts`:
A) grant per quote (one short-lived permission per attempt; our default
candidate), B) route through an owner-controlled forwarder with a fixed
address, C) use the plain-EOA executor for the deposit leg. Decision deferred
to the M3 gate.

## Rust toolchain

* `rustc 1.88.0` on the owner machine. Latest `ruint` (1.21) and `uuid` (1.29)
  require newer rustc, so the lockfile pins: `uuid 1.18.1`, `ruint 1.16.0`,
  `alloy-sol-types/-macro/-macro-expander/-macro-input/alloy-json-abi/syn-solidity 1.3.1`
  (`alloy-primitives` 1.7.3). CI uses `dtolnay/rust-toolchain@1.88.0`.
* Alloy **1.6.3** (exact pin, plus matching `alloy-consensus` / `alloy-network` /
  `alloy-provider` / `alloy-signer*` / `alloy-rpc-types-eth` / `alloy-node-bindings`).
  Alloy 2.x requires rustc 1.94; we stay on 1.6.x until the toolchain moves.
  Features: `providers`, `provider-http`, `signers`, `signer-local`,
  `reqwest-rustls-tls`, `sol-types`. No `approve` helpers are used.
* `sqlx 0.8.6` with `macros,migrate,postgres,runtime-tokio,tls-rustls`.
* `utoipa 5` generates both OpenAPI documents.
* Clippy pedantic + `-D warnings`; `unsafe_code = forbid`; `cast_precision_loss = deny`.

## Node toolchain

* Node 24, pnpm 10.11.1 (`packageManager` field in `frontend/package.json`).
* `vitest 4.1.12`, `typescript 5.9.3` (TS 6/7 exist; deferred), `zod 4.6.7`
  (upgraded from 3.25 to clear GHSA-mrhx-c7xc-m3w5), `openapi-typescript 7.13.0`, `tsx 4.23.15`.
* Next **16.4.5** / React **19.3.1** for the two shells (Turbopack build).
* pnpm reports 4 deprecated transitive packages under `@zerodev/*`
  (`@simplewebauthn/types`, `crypto-js`); not on our code paths (no WebAuthn,
  no passkeys). Tracked for the M3 review.
* `pnpm audit --prod`: one remaining **moderate** advisory, `bn.js < 4.12.3`
  (infinite loop) via `@zerodev/permissions → merkletreejs → web3-utils →
  ethjs-unit`. Not reachable from our policy builders (we never call
  merkle/unit helpers); CI gates on `high`. Re-check on the next
  `@zerodev/permissions` bump.

## Local network caveat (owner machine)

The LAN router's resolver (`192.168.0.1`) returned SERVFAIL for
Cloudflare-fronted hosts (`registry.npmjs.org`, `static.rust-lang.org`) while
`1.1.1.1` resolved them. Installs were run with a *process-local* DNS shim
(`NODE_OPTIONS=--require /tmp/pcc-spike/dns-shim.cjs`, outside the repo) that
makes Node's `dns.lookup` query 1.1.1.1/8.8.8.8. No system DNS settings were
changed. Fix properly by setting the Mac's DNS to 1.1.1.1 / 8.8.8.8 or
repairing the router; CI runners are unaffected.
