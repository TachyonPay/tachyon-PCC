# M3 — EIP-7702 Kernel session keys (offline spike)

Date: 2026-11-07. 1Click has **no testnet**; this milestone is Sepolia account
behavior only. No 7702 authorization has been signed, no delegation installed,
no UserOp sent.

## Framework choice

* `@zerodev/sdk` 5.5.11, `@zerodev/permissions` 5.6.4, `@zerodev/ecdsa-validator` 5.4.9
* Kernel **v3.3** / EntryPoint **0.7**
* 7702 implementation `KERNEL_7702_DELEGATION_ADDRESS` =
  `0xd6CEDDe84be40893d153Be9d467CD6aD37875b28`
* Helpers: `create7702KernelAccount` / `create7702KernelAccountClient`
* Session signer: ECDSA modular signer (`toECDSASigner`), **not** WebAuthn
* Call policy **V0_0_4**, POC option **C** (any recipient, amount cap) plus
  timestamp expiry, gas cap (500k), and `toRateLimitPolicy({ count: 1 })` so
  the grant is single-use. Never sudo.

## Limitation (carried to M4)

Call policy still cannot name a 1Click deposit address at onboarding time.
Option C + single-use rate limit is the POC stand-in; a signature-bound
executor (option B) remains the intended production follow-up. 1Click
settlement stays on mainnet (M4), separately gated.

## Offline tests

`pnpm test` in `services/integrations-ts` covers:

* no session / revoked / expired
* wrong and unsupported chainId
* owner mismatch
* delegatecall, native value, approve, non-transfer selector
* arbitrary token target, excessive amount, excessive gas
* recipient constraint when set (A/B) vs omitted (C)
* DRY_RUN / missing approval / mainnet gate
* invalid owner 7702 auth, wrong chain, wrong implementation, replayed nonce
* already-delegated unfamiliar implementation, contract account refuse
* empty code → install allowed; our Kernel → reuse
* session key ref refuses raw hex and world-readable files
* persist-then-send: one sign, restart rebroadcasts the same userOpHash
* out-of-policy execute never signs

## Live

Base Sepolia install + in-policy 1 USDC session transfer + out-of-policy
`approve` rejection are recorded in the owner session (Kernel designator
`0xef0100d6cedde84be40893d153be9d467cd6ad37875b28`). 1Click was not involved.
