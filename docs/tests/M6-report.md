# M6 — merchant API, dashboard, webhooks

Date: 2026-11-07. No mainnet value movement.

## Scope

* Merchant surface: API-key bearer, idempotent `POST /v1/payment-intents`,
  tenant-isolated `GET`, payout snapshot, webhook URL with SSRF guard.
* HMAC-SHA256 webhook signatures (`sha256=` hex) and bounded retry schedule
  (`pcc-settlement` backoff, max 12).
* Dashboard (`@pcc/dashboard:3001`) pastes a key per tab; two-merchant
  tenancy is an API test (Alice 404 on Bob’s intent).
* Postgres schema already committed (`0001_init.sql`). Runtime under test is
  the in-memory store (same uniqueness/tenancy rules as `InMemoryRepository`).

## Negative tests

* Missing / wrong bearer → 401
* Cross-tenant GET → 404
* `https://127.0.0.1`, `https://10.0.0.5`, `https://169.254.169.254`,
  `http://…`, `file:` → `ssrf_blocked`
* Replay of `Idempotency-Key` returns the same intent id
