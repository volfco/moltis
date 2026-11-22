-- Sessions table schema (PostgreSQL).
-- Mirrors migrations/20240205100001_init.sql. Differences from the SQLite
-- schema, and why:
--
--   * Integers are BIGINT so they decode as i64 in both backends (the Rust
--     boundary types are i64).
--   * Flag columns (archived, sandbox_enabled, mcp_disabled) are read and
--     written as integers at the Rust boundary, so they stay BIGINT rather
--     than BOOLEAN.
--   * created_at/updated_at are epoch milliseconds (BIGINT), matching SQLite.
--   * The channel_sessions primary key constraint is named explicitly because
--     migration 20260328100000 drops it to add thread_id.

CREATE TABLE IF NOT EXISTS sessions (
    key             TEXT    PRIMARY KEY,
    id              TEXT    NOT NULL,
    label           TEXT,
    model           TEXT,
    created_at      BIGINT  NOT NULL,
    updated_at      BIGINT  NOT NULL,
    message_count   BIGINT  NOT NULL DEFAULT 0,
    project_id      TEXT    REFERENCES projects(id) ON DELETE SET NULL,
    archived        BIGINT  NOT NULL DEFAULT 0,
    worktree_branch TEXT,
    sandbox_enabled BIGINT,
    sandbox_image   TEXT,
    channel_binding TEXT
);

CREATE INDEX IF NOT EXISTS idx_sessions_created_at ON sessions(created_at);

CREATE TABLE IF NOT EXISTS channel_sessions (
    channel_type TEXT   NOT NULL,
    account_id   TEXT   NOT NULL,
    chat_id      TEXT   NOT NULL,
    session_key  TEXT   NOT NULL,
    updated_at   BIGINT NOT NULL,
    CONSTRAINT channel_sessions_pkey
        PRIMARY KEY (channel_type, account_id, chat_id)
);
