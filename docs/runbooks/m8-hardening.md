# Runbook: M8 hardening (no unattended mainnet)

## Gates (unchanged)

* Default `DRY_RUN=true`, `ENABLE_MAINNET_TRANSFERS=false`.
* Only the exact strings `"true"` / `"false"` flip a gate.
* Broadcast still needs a per-transaction approval reference even when gates
  are open. There is no batch or cron path that signs/broadcasts.

## Secrets

* Never request, paste, log, or commit private keys, mnemonics, TOTP seeds,
  webhook secrets, or `ONECLICK_API_KEY`.
* `.env` is git-ignored; commit `.env.example` only.
* TOTP wrap key: `TOTP_WRAP_KEY` (value not logged).
* Merchant API keys: hash at rest (`pcc.apikey.v1|` SHA-256), constant-time
  verify.

## If something looks live

1. Stop. Do not retry.
2. Confirm `DRY_RUN` / `ENABLE_MAINNET_TRANSFERS` in `/v1/meta/gates`.
3. Do not rotate or print keys in chat. Rotate out of band in the OS secret
   store.
4. Record the public tx hash only in `docs/tests/` if the owner already
   approved that exact send.

## Postgres

Schema: `backend/crates/storage/migrations/0001_init.sql`. Local:
`docker compose -f infra/docker/docker-compose.yml up -d`. The M6 API path
under test is in-memory; do not point a deployed process at this demo store.
