# M10 — owner approval of parked deposits

With gates closed (`DRY_RUN=true` or `ENABLE_MAINNET_TRANSFERS=false`) nothing
changes: authorize returns `execution: "gated"`. This runbook is only for a
run where the owner has explicitly opened gates for a specific send.

## Config

| Env | Meaning |
| --- | --- |
| `CHECKOUT_APPROVAL_MODE` | `payer` (default). Only the exact value `owner` parks the deposit. |
| `OPERATOR_APPROVAL_TOKEN` | ≥ 24 random chars. Unset or shorter = operator routes return 503 and nothing can be approved. |

Open gates **per process only**, never by editing `.env`:

```sh
set -a && source .env && set +a
DRY_RUN=false ENABLE_MAINNET_TRANSFERS=true PCC_DEMO_MERCHANTS=true \
  cargo run -p pcc-api --bin pcc-api
```

## Flow

1. Payer authorizes. The API verifies the quote, simulates, and parks the
   `DepositPlan`; the response is `execution: "awaiting_approval"`. Nothing is
   signed.
2. Operator lists pending approvals:

   ```sh
   curl -s -H "Authorization: Bearer $OPERATOR_APPROVAL_TOKEN" \
     http://127.0.0.1:8080/v1/operator/approvals
   ```

   Each entry shows `chain_id`, `token`, `deposit_address`, `amount`,
   `max_fee_wei` and `expires_at` (10 minutes, or the quote deadline minus 5
   minutes, whichever is sooner).
3. Check those values against the merchant order and the 1Click quote. To
   approve, echo them back exactly:

   ```sh
   curl -s -X POST -H "Authorization: Bearer $OPERATOR_APPROVAL_TOKEN" \
     -H 'Content-Type: application/json' \
     -d '{"chain_id":8453,"deposit_address":"0x…","amount":"…","max_fee_wei":"…"}' \
     http://127.0.0.1:8080/v1/operator/approvals/<attempt_id>/approve
   ```

   Or reject: `POST /v1/operator/approvals/<attempt_id>/reject` (204).

## Responses

| Status | Code | Meaning |
| --- | --- | --- |
| 200 | | Sent once; `payment.broadcast` webhook queued. |
| 401 | `unauthorized` | Wrong or missing token. |
| 404 | `not_found` | Nothing pending (already approved, rejected, or never parked). |
| 409 | `approval_mismatch` | An echoed field differs. The approval stays pending. |
| 409 | `gates_closed` | Gates closed since the payer authorized. The approval stays pending. |
| 410 | `approval_expired` | Window passed; the payer must authorize again. |
| 503 | `operator_unconfigured` | No `OPERATOR_APPROVAL_TOKEN`. |

Approvals are single use and live in memory: restarting the API drops them.
