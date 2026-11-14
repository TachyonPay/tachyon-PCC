# M11 — real payer flow

The hosted checkout now runs: SIWE → allowance (signed once, then reused) →
TOTP → authorize. The integrations worker loads **no** wallet key.

## What a payer does, and how often

| Step | How often |
| --- | --- |
| EIP-7702 delegation to Kernel v3.3 | once per chain, from the checkout page, and only when the account has no delegation yet |
| TOTP enrollment | once per wallet address; SIWE returns `totp_enrolled` and the page skips setup |
| SIWE | every checkout, unless this browser was remembered at the last TOTP authorize |
| Remembered device | optional, 30 days; skips the wallet sign-in only. Paying still needs a TOTP code |
| Allowance signature | once per wallet + chain + token, until its uses run out, it expires, or the payer revokes it |
| TOTP code | every checkout |

A payment that is only gated or parked for approval does not spend a use; a
use is spent when the worker actually executes the transfer.

A confirmed TOTP secret is never replaced: a second enroll returns 409
`already_enrolled`. A signed-in wallet can `POST /v1/checkout/{sid}/totp/reset`
and enroll a new authenticator. Enrollments
live in memory until M9, so restarting the API clears them.

## Config

| Env | Meaning |
| --- | --- |
| `SIWE_DOMAIN` | `host[:port]` payers sign. Default: host:port of `CHECKOUT_BASE_URL`. |
| `TOTP_WRAP_KEY` | ≥ 32 chars. Unset or shorter = random per-process key with a warning; enrollments die on restart. |
| `NEXT_PUBLIC_WALLETCONNECT_PROJECT_ID` | 32-hex WalletConnect Cloud id. Used only when no injected wallet exists. |
| `NEXT_PUBLIC_PRIVY_APP_ID` | Privy app id. When set, Wallet A is the Privy embedded wallet (Kernel, SIWE, allowance). MetaMask only sends USDC to that address. TOTP is unchanged. |
| `AA_BUNDLER` | `memory` for a local fake; anything else = real ZeroDev Kernel + Pimlico. |

## Allowance

1. `POST /v1/checkout/{sid}/session-grant` with optional
   `{max_amount, uses, valid_days}`. Defaults: this payment's `max_debit`,
   5 uses, 7 days. Bounds: cap ≥ `max_debit` (400 `cap_below_payment`), uses
   1..=20, days 1..=30 (400 `invalid_allowance`). The worker creates an
   in-memory session key and returns ZeroDev `Enable` typed data for: owner =
   SIWE wallet, chain and token = the selected source, per-transfer cap,
   `uses` transfers, expiry, and gas budget 3e14 wei × uses. The response
   includes `total_exposure` = cap × uses. The payer has 10 minutes to sign.
2. The page switches the wallet to that chain and calls
   `eth_signTypedData_v4` (via `toSignTypedDataV4`, which adds `EIP712Domain`).
3. `POST /v1/checkout/{sid}/session-grant/enable` with `{grant_id, signature}`.
   The worker verifies the signature against the owner. A new allowance for
   the same wallet, chain and token replaces the old one.
4. `GET /v1/checkout/{sid}/session-grant` returns the active allowance for the
   signed-in wallet and selected source, with `uses_remaining` and
   `covers_this_payment`; 404 `no_allowance` otherwise. The page skips
   signing when it covers the payment.
5. `POST /v1/checkout/{sid}/session-grant/revoke` destroys the session key.
   The on-chain permission stays installed but nobody holds its key.
6. Authorize asks the worker and returns 409 `session_grant_required` unless
   an active allowance covers this owner, chain, token and `max_debit`.

Limits to know:

* The recipient is not pinned on-chain, because 1Click deposit addresses
  change per quote. The worker checks the quote's deposit address off-chain.
  A compromised worker could move up to cap × remaining uses.
* Session keys stay in worker memory unless `GRANT_STORE_KEY` (≥ 32 characters)
  is set. Then each change is written to `GRANT_STORE_DIR` (default
  `.pcc-grant-store`) as one AES-256-GCM file, mode 0600, with a fresh HKDF
  salt per write. A wrong key or a modified file refuses to start rather than
  dropping allowances that are still installed on chain. API mandates and TOTP
  enrollments stay in memory until M9.

## Remembered device

Opt in with `remember_device: true` on `POST /v1/checkout/{sid}/authorize`.
The response includes `device_token` (`pcc_dev_` + 64 hex) once. The API stores
only its SHA-256. `POST /v1/checkout/{sid}/device/resume` with that token binds
the wallet the way SIWE does and returns the same body, including `address`.
A bad, expired, or not-yet-enrolled token is 401 `device_unknown` (an
unenrolled token is forgotten). `POST /v1/devices/forget` drops it. Tokens last
30 days. A stolen token can open a checkout as that wallet and can revoke an
allowance; it cannot pay without the TOTP code, and a new allowance or mandate
still needs a wallet signature. After a payment is authorized, a second wallet
cannot take over that checkout (`409 session_wallet_locked`).

## 7702 delegation

`GET /v1/checkout/{sid}/delegation` reads Base and Arbitrum code for the SIWE
wallet: `none`, `delegated` (ready only for Kernel v3.3
`0xd6CEDDe84be40893d153Be9d467CD6aD37875b28`), `contract`, or `unknown` on RPC
error. When the account has no code, the page asks the wallet to sign one
EIP-7702 authorization and send it. The page reads the code again before
signing and again before sending, and it will not replace a delegation or a
contract. A wallet that cannot sign an authorization shows that error and
leaves the account unchanged.

## Gated E2E

```sh
set -a && source .env && set +a
cd services/integrations-ts && unset NODE_OPTIONS && pnpm -s e2e:checkout-gated
```

Expected phases: `payout`, `siwe`, `allowance`, `authorize` with
`execution: "gated"` and `simulationOk: true`. With `E2E_PAYMENTS=2`, payment 2
logs `device_resume` (no second SIWE) and `allowance` with `reused: true`, and
the summary shows `allowanceSignatures: 1` and `siweSignatures: 1`.
`E2E_ALLOWANCE_CAP` overrides the per-payment cap (default 2 × max debit).
