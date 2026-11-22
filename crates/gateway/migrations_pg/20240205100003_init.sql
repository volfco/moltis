-- Gateway tables schema (PostgreSQL)
-- Mirrors migrations/20240205100003_init.sql. Differences from the SQLite
-- schema, and why:
--
--   * `INTEGER PRIMARY KEY AUTOINCREMENT` becomes
--     `BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY` so rows decode as i64
--     in both backends; `auth_password.id` stays a plain BIGINT because the
--     application inserts the literal id 1 (single-row table).
--   * BLOBs become BYTEA (`passkeys.credential_id`, `passkeys.passkey_data`).
--   * TEXT timestamps keep the same `YYYY-MM-DD HH:MM:SS` UTC form SQLite's
--     `datetime('now')` writes, so ordering and comparisons behave identically.
--   * `message_log.access_granted` is BOOLEAN because the Rust boundary reads
--     and writes it as `bool`; every other flag column stays numeric because
--     its Rust boundary is i64 (or SQL integer literals).

-- ============================================================================
-- Auth tables
-- ============================================================================

CREATE TABLE IF NOT EXISTS auth_password (
    id            BIGINT PRIMARY KEY CHECK (id = 1),
    password_hash TEXT   NOT NULL,
    created_at    TEXT   NOT NULL DEFAULT (to_char(now() AT TIME ZONE 'utc', 'YYYY-MM-DD HH24:MI:SS')),
    updated_at    TEXT   NOT NULL DEFAULT (to_char(now() AT TIME ZONE 'utc', 'YYYY-MM-DD HH24:MI:SS'))
);

CREATE TABLE IF NOT EXISTS passkeys (
    id            BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    credential_id BYTEA  NOT NULL UNIQUE,
    name          TEXT   NOT NULL,
    passkey_data  BYTEA  NOT NULL,
    created_at    TEXT   NOT NULL DEFAULT (to_char(now() AT TIME ZONE 'utc', 'YYYY-MM-DD HH24:MI:SS'))
);

CREATE TABLE IF NOT EXISTS api_keys (
    id         BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    label      TEXT   NOT NULL,
    key_hash   TEXT   NOT NULL,
    key_prefix TEXT   NOT NULL,
    created_at TEXT   NOT NULL DEFAULT (to_char(now() AT TIME ZONE 'utc', 'YYYY-MM-DD HH24:MI:SS')),
    revoked_at TEXT
);

CREATE TABLE IF NOT EXISTS auth_sessions (
    token      TEXT PRIMARY KEY,
    created_at TEXT NOT NULL DEFAULT (to_char(now() AT TIME ZONE 'utc', 'YYYY-MM-DD HH24:MI:SS')),
    expires_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS env_variables (
    id         BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    key        TEXT   NOT NULL UNIQUE,
    value      TEXT   NOT NULL,
    created_at TEXT   NOT NULL DEFAULT (to_char(now() AT TIME ZONE 'utc', 'YYYY-MM-DD HH24:MI:SS')),
    updated_at TEXT   NOT NULL DEFAULT (to_char(now() AT TIME ZONE 'utc', 'YYYY-MM-DD HH24:MI:SS'))
);

-- ============================================================================
-- Message Log
-- ============================================================================

CREATE TABLE IF NOT EXISTS message_log (
    id             BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    account_id     TEXT    NOT NULL,
    channel_type   TEXT    NOT NULL,
    peer_id        TEXT    NOT NULL,
    username       TEXT,
    sender_name    TEXT,
    chat_id        TEXT    NOT NULL,
    chat_type      TEXT    NOT NULL,
    body           TEXT    NOT NULL,
    access_granted BOOLEAN NOT NULL DEFAULT FALSE,
    created_at     BIGINT  NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_message_log_account_created
    ON message_log (account_id, created_at DESC);

-- ============================================================================
-- Channels
-- ============================================================================

CREATE TABLE IF NOT EXISTS channels (
    account_id   TEXT    PRIMARY KEY,
    channel_type TEXT    NOT NULL DEFAULT 'telegram',
    config       TEXT    NOT NULL,
    created_at   BIGINT  NOT NULL,
    updated_at   BIGINT  NOT NULL
);
