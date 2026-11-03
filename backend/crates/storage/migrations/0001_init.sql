-- Private Crypto Checkout — initial schema (M0 scaffold; extended in M6).
-- Amounts are stored as NUMERIC(78,0) (fits U256) and exposed as strings.

CREATE TABLE merchants (
    id                  UUID PRIMARY KEY,
    status              TEXT NOT NULL CHECK (status IN ('active', 'suspended')),
    api_key_hash        TEXT NOT NULL UNIQUE,
    webhook_url         TEXT,
    webhook_secret_ref  TEXT,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE payout_configs (
    id                  UUID PRIMARY KEY,
    merchant_id         UUID NOT NULL REFERENCES merchants(id),
    chain_id            BIGINT NOT NULL,
    token_kind          TEXT NOT NULL CHECK (token_kind IN ('native', 'erc20')),
    token_address       TEXT,
    decimals            SMALLINT NOT NULL,
    one_click_asset_id  TEXT NOT NULL,
    receiving_address   TEXT NOT NULL,
    active              BOOLEAN NOT NULL DEFAULT TRUE,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX payout_configs_merchant_idx ON payout_configs(merchant_id);

CREATE TABLE wallet_credentials (
    id                  UUID PRIMARY KEY,
    owner_address       TEXT NOT NULL,
    chain_id            BIGINT NOT NULL,
    delegation_impl     TEXT,
    session_signer      TEXT,
    permissions_hash    TEXT,
    expires_at          TIMESTAMPTZ,
    status              TEXT NOT NULL CHECK (status IN ('pending_enrollment', 'active', 'revoked', 'expired')),
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (owner_address, chain_id)
);

CREATE TABLE payment_intents (
    id                          UUID PRIMARY KEY,
    merchant_id                 UUID NOT NULL REFERENCES merchants(id),
    merchant_order_id           TEXT NOT NULL,
    payout_config_snapshot      JSONB NOT NULL,
    output_amount_base_units    NUMERIC(78,0) NOT NULL CHECK (output_amount_base_units > 0),
    status                      TEXT NOT NULL,
    expires_at                  TIMESTAMPTZ NOT NULL,
    idempotency_key             TEXT NOT NULL,
    created_at                  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at                  TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (merchant_id, idempotency_key)
);

CREATE TABLE payment_attempts (
    id                      UUID PRIMARY KEY,
    payment_intent_id       UUID NOT NULL REFERENCES payment_intents(id),
    source_wallet           TEXT NOT NULL,
    source_chain_id         BIGINT NOT NULL,
    source_asset            JSONB NOT NULL,
    quoted_input            NUMERIC(78,0) NOT NULL,
    max_debit               NUMERIC(78,0) NOT NULL,
    quote_signature         TEXT NOT NULL,
    quote_raw_response      JSONB NOT NULL,          -- immutable signed quote bytes
    quote_expires_at        TIMESTAMPTZ NOT NULL,
    deposit_address         TEXT,
    deposit_memo            TEXT,
    source_tx_hash          TEXT UNIQUE,
    destination_tx_hash     TEXT,
    status                  TEXT NOT NULL,
    created_at              TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at              TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX payment_attempts_intent_idx ON payment_attempts(payment_intent_id);
CREATE INDEX payment_attempts_deposit_idx ON payment_attempts(deposit_address);

CREATE TABLE authorizations (
    id                      UUID PRIMARY KEY,
    wallet_credential_id    UUID NOT NULL REFERENCES wallet_credentials(id),
    payment_attempt_id      UUID NOT NULL REFERENCES payment_attempts(id),
    challenge_id            TEXT NOT NULL,
    amount_bound_hash       TEXT NOT NULL,
    verified_at             TIMESTAMPTZ,
    consumed_at             TIMESTAMPTZ,
    expires_at              TIMESTAMPTZ NOT NULL,
    UNIQUE (payment_attempt_id)                       -- one authorization per attempt
);

CREATE TABLE payment_events (
    id                  UUID PRIMARY KEY,
    payment_intent_id   UUID NOT NULL REFERENCES payment_intents(id),
    type                TEXT NOT NULL,
    payload_hash        TEXT NOT NULL,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX payment_events_intent_idx ON payment_events(payment_intent_id, created_at);

CREATE TABLE webhook_deliveries (
    id              UUID PRIMARY KEY,
    event_id        UUID NOT NULL REFERENCES payment_events(id),
    attempt         INTEGER NOT NULL DEFAULT 0,
    status          TEXT NOT NULL CHECK (status IN ('pending', 'delivered', 'failed')),
    next_retry_at   TIMESTAMPTZ,
    last_error      TEXT
);
CREATE INDEX webhook_deliveries_due_idx ON webhook_deliveries(next_retry_at) WHERE status = 'pending';
