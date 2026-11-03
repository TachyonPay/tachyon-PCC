# Database

Schema migrations are owned by the Rust `pcc-storage` crate and live in
`backend/crates/storage/migrations/` (embedded via `sqlx::migrate!`). This
directory intentionally contains no second copy; `infra/db/migrations/` is
reserved for operational scripts (backups, retention) added in M8.

Local Postgres: `docker compose -f infra/docker/docker-compose.yml up -d`.

Conventions:

- All token amounts are `NUMERIC(78,0)` base units (fits U256). Never floats.
- `payment_intents` is unique on `(merchant_id, idempotency_key)`.
- `payment_attempts.source_tx_hash` is unique — a deposit tx can be recorded once.
- `authorizations` is unique per attempt and single-use (`consumed_at`).
- Sensitive values (API-key hashes, TOTP seed references) are stored hashed or as
  references to the OS secret store, never in plaintext.
