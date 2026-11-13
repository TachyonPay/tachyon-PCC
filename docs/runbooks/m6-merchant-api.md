# Runbook: M6 merchant API + dashboard

The public contract is `backend/crates/api/openapi/public-v1.json`. The
process uses the in-memory store; Postgres schema is already in
`backend/crates/storage/migrations/0001_init.sql` and is the durable target.

## Demo merchants

Only when `PCC_DEMO_MERCHANTS=true` (never in a deployed build):

| Merchant | API key (local demo only) | Webhook secret |
| --- | --- | --- |
| Alice Demo | `pcc_sk_alice_demo` | `whsec_alice_demo` |
| Bob Demo | `pcc_sk_bob_demo` | `whsec_bob_demo` |

The API process does not log these values. Dashboard: paste a key per tab
(two tabs = two tenants). Each `GET /v1/payment-intents/{id}` is 404 for the
other merchant.

## Endpoints

* `POST /v1/payment-intents` — `Authorization: Bearer` + `Idempotency-Key`
* `GET /v1/payment-intents/{id}` — tenant-scoped
* `PUT /v1/payout-config` — snapshotted into later intents
* `PUT /v1/webhook-url` — HTTPS, SSRF-denied (no loopback/private/link-local)
* `GET /v1/payment-intents/{id}/events` — payload + `sha256=` HMAC

Webhook URLs: `https` only unless `ALLOW_WEBHOOK_LOOPBACK=true` (http loopback).
`file:`, `https://127.0.0.1`, `https://10.0.0.0/8`, `169.254.169.254` are
refused.

## Dashboard

```sh
PCC_DEMO_MERCHANTS=true cargo run -p pcc-api
cd frontend && pnpm --filter @pcc/dashboard dev   # :3001
```

No merchant key is baked into the Next bundle.
