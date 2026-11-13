# M12 — merchant onboarding

## Config

`MERCHANT_SIGNUP_TOKEN` (≥ 24 chars) is the invite token. Unset or shorter
means signup returns 503. It is stored only as a SHA-256 hash in memory.

## API

| Route | Auth | Result |
| --- | --- | --- |
| `POST /v1/merchants` `{name, payout}` | `X-Signup-Token` | 201 `{merchant_id, name, api_key, webhook_secret}` |
| `POST /v1/api-keys/rotate` | merchant key | `{api_key}`; the old key is revoked immediately |
| `POST /v1/webhook-secret/rotate` | merchant key | `{webhook_secret}`; later deliveries are signed with it |
| `GET /v1/payout-config` | merchant key | current payout destination |

Keys are `pcc_sk_` + 64 hex and secrets are `whsec_` + 64 hex. Both are
returned once and never logged. Names must be 1–80 printable characters.

## Dashboard

`pnpm --filter @pcc/dashboard dev` (port 3001). The page provides:

* signup with an invite token (the payout fields double as the signup payout),
* key and secret rotation, shown once in a copy-now box,
* payout read and edit, webhook URL, and intents.

The API key stays in a password field in the tab and is never persisted.

Accounts live in memory until M9 (Postgres) lands, so restarting the API
drops signed-up merchants.
