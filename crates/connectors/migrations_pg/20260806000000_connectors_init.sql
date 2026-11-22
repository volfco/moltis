-- Connector schema (PostgreSQL).
-- Mirrors migrations/20260806000000_connectors_init.sql. Differences from the
-- SQLite schema, and why:
--
--   * `PRAGMA foreign_keys` is skipped: PostgreSQL always enforces foreign
--     keys, and run_migrations only issues the pragma on SQLite.
--   * Flag columns written as Rust `bool` are BOOLEAN instead of INTEGER
--     0/1, so the IN (0, 1) checks become tautologies and are dropped.
--   * SQLite's `CHECK (json_valid(col))` has no direct equivalent before
--     PostgreSQL 16's pg_input_is_valid; the cast-based CHECK rejects
--     invalid JSON the same way (statement error instead of constraint
--     violation).
--   * Integers are BIGINT so they decode as i64 in both backends.

CREATE TABLE IF NOT EXISTS connector_accounts (
    id         TEXT    PRIMARY KEY,
    kind       TEXT    NOT NULL,
    name       TEXT    NOT NULL,
    config     TEXT    NOT NULL CHECK (json_typeof(config::json) IS NOT NULL),
    enabled    BOOLEAN NOT NULL,
    created_at BIGINT  NOT NULL,
    updated_at BIGINT  NOT NULL
);

CREATE TABLE IF NOT EXISTS connector_datasets (
    id               TEXT    PRIMARY KEY,
    account_id       TEXT    NOT NULL REFERENCES connector_accounts(id) ON DELETE CASCADE,
    name             TEXT    NOT NULL,
    config           TEXT    NOT NULL CHECK (json_typeof(config::json) IS NOT NULL),
    schedule_minutes BIGINT  CHECK (schedule_minutes IS NULL OR schedule_minutes > 0),
    projection_jsonl BOOLEAN NOT NULL DEFAULT false,
    projection_md    BOOLEAN NOT NULL DEFAULT false,
    enabled          BOOLEAN NOT NULL,
    last_sync_at     BIGINT,
    next_sync_at     BIGINT,
    last_error       TEXT,
    item_count       BIGINT  NOT NULL DEFAULT 0 CHECK (item_count >= 0),
    created_at       BIGINT  NOT NULL,
    updated_at       BIGINT  NOT NULL
);

CREATE TABLE IF NOT EXISTS connector_items (
    id             TEXT   PRIMARY KEY,
    dataset_id     TEXT   NOT NULL REFERENCES connector_datasets(id) ON DELETE CASCADE,
    remote_id      TEXT   NOT NULL,
    kind           TEXT   NOT NULL,
    remote_version TEXT,
    occurred_at    TEXT,
    updated_at     TEXT,
    body_json      TEXT   NOT NULL CHECK (json_typeof(body_json::json) IS NOT NULL),
    content_hash   TEXT   NOT NULL,
    created_at     BIGINT NOT NULL,
    stored_at      BIGINT NOT NULL,
    deleted_at     BIGINT,
    UNIQUE(dataset_id, remote_id)
);

CREATE TABLE IF NOT EXISTS connector_sync_runs (
    id          TEXT   PRIMARY KEY,
    dataset_id  TEXT   NOT NULL REFERENCES connector_datasets(id) ON DELETE CASCADE,
    status      TEXT   NOT NULL CHECK (status IN ('running', 'succeeded', 'failed')),
    started_at  BIGINT NOT NULL,
    finished_at BIGINT,
    upserted    BIGINT NOT NULL DEFAULT 0 CHECK (upserted >= 0),
    deleted     BIGINT NOT NULL DEFAULT 0 CHECK (deleted >= 0),
    active      BIGINT NOT NULL DEFAULT 0 CHECK (active >= 0),
    error       TEXT
);

CREATE INDEX IF NOT EXISTS idx_connector_datasets_account
    ON connector_datasets(account_id, updated_at DESC);
CREATE INDEX IF NOT EXISTS idx_connector_datasets_due
    ON connector_datasets(enabled, next_sync_at);
CREATE INDEX IF NOT EXISTS idx_connector_items_dataset_active
    ON connector_items(dataset_id, deleted_at, occurred_at);
CREATE INDEX IF NOT EXISTS idx_connector_sync_runs_recent
    ON connector_sync_runs(dataset_id, started_at DESC);
CREATE UNIQUE INDEX IF NOT EXISTS idx_connector_sync_runs_one_running
    ON connector_sync_runs(dataset_id) WHERE status = 'running';
