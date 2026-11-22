-- Webhook tables schema (PostgreSQL)
-- Owned by: moltis-webhooks crate
--
-- Mirrors migrations/20260407000000_initial.sql. Differences from the SQLite
-- schema, and why:
--
--   * Auto-increment ids are `GENERATED ALWAYS AS IDENTITY` instead of
--     `INTEGER PRIMARY KEY AUTOINCREMENT`; inserts never write them and
--     RETURNING id yields the new key on both engines.
--   * Flag columns (enabled) are BOOLEAN and are bound and decoded as Rust
--     `bool` on both backends; SQLite keeps INTEGER 0/1.
--   * Counts and sizes are BIGINT so they decode as i64 on both backends.
--   * body_blob is BYTEA, the binary counterpart of SQLite BLOB.

CREATE TABLE IF NOT EXISTS webhooks (
    id                   BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    name                 TEXT    NOT NULL,
    description          TEXT,
    enabled              BOOLEAN NOT NULL DEFAULT true,
    public_id            TEXT    NOT NULL UNIQUE,
    agent_id             TEXT,
    model                TEXT,
    system_prompt_suffix TEXT,
    tool_policy_json     TEXT,
    auth_mode            TEXT    NOT NULL DEFAULT 'static_header',
    auth_config_json     TEXT,
    source_profile       TEXT    NOT NULL DEFAULT 'generic',
    source_config_json   TEXT,
    event_filter_json    TEXT,
    session_mode         TEXT    NOT NULL DEFAULT 'per_delivery',
    named_session_key    TEXT,
    allowed_cidrs_json   TEXT,
    max_body_bytes       BIGINT  NOT NULL DEFAULT 1048576,
    rate_limit_per_minute BIGINT NOT NULL DEFAULT 60,
    delivery_count       BIGINT  NOT NULL DEFAULT 0,
    last_delivery_at     TEXT,
    created_at           TEXT    NOT NULL,
    updated_at           TEXT    NOT NULL
);

CREATE TABLE IF NOT EXISTS webhook_deliveries (
    id               BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    webhook_id       BIGINT NOT NULL,
    received_at      TEXT   NOT NULL,
    status           TEXT   NOT NULL DEFAULT 'received',
    event_type       TEXT,
    entity_key       TEXT,
    delivery_key     TEXT,
    http_method      TEXT,
    content_type     TEXT,
    remote_ip        TEXT,
    headers_json     TEXT,
    body_size        BIGINT NOT NULL DEFAULT 0,
    body_blob        BYTEA,
    session_key      TEXT,
    rejection_reason TEXT,
    run_error        TEXT,
    started_at       TEXT,
    finished_at      TEXT,
    duration_ms      BIGINT,
    input_tokens     BIGINT,
    output_tokens    BIGINT,
    FOREIGN KEY (webhook_id) REFERENCES webhooks(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS webhook_response_actions (
    id            BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    delivery_id   BIGINT NOT NULL,
    tool_name     TEXT   NOT NULL,
    input_json    TEXT,
    output_json   TEXT,
    status        TEXT   NOT NULL,
    error_message TEXT,
    created_at    TEXT   NOT NULL,
    FOREIGN KEY (delivery_id) REFERENCES webhook_deliveries(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_webhook_deliveries_webhook_id
    ON webhook_deliveries(webhook_id, received_at DESC);

CREATE INDEX IF NOT EXISTS idx_webhook_deliveries_status
    ON webhook_deliveries(status);

CREATE INDEX IF NOT EXISTS idx_webhook_deliveries_delivery_key
    ON webhook_deliveries(delivery_key);

CREATE INDEX IF NOT EXISTS idx_webhook_deliveries_entity_key
    ON webhook_deliveries(entity_key);

CREATE INDEX IF NOT EXISTS idx_webhook_response_actions_delivery
    ON webhook_response_actions(delivery_id);
