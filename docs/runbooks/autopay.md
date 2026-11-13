# Autopay mandates

Autopay lets a merchant charge a payer later without the payer present: no
TOTP code and no wallet prompt per charge. The payer approves once, inside a
hosted checkout, with a TOTP code and one wallet signature.

| Who | Step | How often |
| --- | --- | --- |
| Payer | SIWE + TOTP enrollment | once per wallet (as for checkout) |
| Payer | TOTP code + EIP-712 signature for the mandate | once per mandate |
| Merchant | `POST /v1/autopay/{id}/charges` with its API key | each charge |
| Payer | Revoke | any time |

## Limits on every charge

* **Per-charge cap** (source-token base units). A charge whose priced max
  debit is above it gets 409 `over_mandate_cap`.
* **Charges** (1..=20). After the last one, 409 `mandate_exhausted`.
* **Minimum interval** (0..=744 hours). An early charge gets 429
  `mandate_interval`.
* **Expiry** (1..=30 days). After it, 409 `mandate_expired`.
* **Revoked or never signed:** 409 `mandate_inactive`.
* **Worker allowance gone** (revoked, used up, worker restarted): 409
  `mandate_allowance_missing`.

The API counts accepted charges, including gated and parked ones. The worker
counts transfers actually sent. Charging stops when either runs out. A charge
whose quote or preflight fails is not counted.

Each mandate gets its own session key and on-chain permission, separate from
the payer's checkout allowance. Setting one up never replaces the other.

Charges then follow the same path as checkout: an executable 1Click quote
validated against the payout snapshot and the max debit, a bundler preflight,
then the gates. Default `CHECKOUT_APPROVAL_MODE=payer` sends at once. Only
`owner` parks an open-gate charge on `/operator`.

## Routes

Payer (hosted checkout session, after SIWE):

* `POST /v1/checkout/{sid}/autopay` with `{code, max_amount?, charges?,
  interval_hours?, valid_days?}`. Defaults: this checkout's max debit, 12
  charges, 24 h, 30 days. It needs an enrolled wallet (409
  `totp_enrollment_required`) and a fresh TOTP code. It returns typed data
  plus `total_exposure` (cap × charges).
* `POST /v1/checkout/{sid}/autopay/{id}/enable` with `{grant_id, signature}`.
* `GET /v1/checkout/{sid}/autopay` lists the wallet's mandates.
* `POST /v1/checkout/{sid}/autopay/{id}/revoke`. Only the signing wallet can
  revoke; anyone else gets 404.

Merchant (API key):

* `GET /v1/autopay` lists signed mandates with status `active`, `expired`,
  `exhausted` or `revoked`.
* `POST /v1/autopay/{id}/charges` with `{merchant_order_id, output_amount}`
  and an `Idempotency-Key`. A replay returns the first result. The response
  has the same `execution` values as checkout authorize, plus
  `charges_remaining`. Webhooks carry `autopay_mandate_id`.

The checkout page shows an "Autopay with …" section once the wallet is
enrolled. The dashboard lists mandates and has a "Charge now" button.

## Known limits (proof of concept)

* Mandates and session keys are in memory. Restarting the API forgets
  mandates; restarting the worker drops their keys. Encrypted key storage is
  the planned next step.
* The recipient is not pinned on-chain, because 1Click deposit addresses
  change per quote. A compromised worker could move up to cap × remaining
  charges. Biometric approval for on-chain transactions is planned for the
  main product.

## Gated E2E

Run it against a freshly started `pcc-api`:

```sh
set -a && source .env && set +a
cd services/integrations-ts && unset NODE_OPTIONS && pnpm -s e2e:autopay-gated
```

Expected phases:

* `enroll`
* `mandate` with `status: "active"`
* one `charge` per charge, with `execution: "gated"` and `simulationOk: true`
* `over_limit` with 409 `mandate_exhausted`
* `revoke`

`E2E_CHARGES` sets the charge count (default 2). `E2E_AUTOPAY_CAP` overrides
the per-charge cap (default 2 × max debit).
