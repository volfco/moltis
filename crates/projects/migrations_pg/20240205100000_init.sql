-- Projects table schema (PostgreSQL)
-- Owned by: moltis-projects crate
--
-- Mirrors migrations/20240205100000_init.sql. Differences from the SQLite
-- schema, and why:
--
--   * Flag columns (auto_worktree, detected) are BOOLEAN and are bound and
--     decoded as Rust `bool` on both backends; SQLite keeps INTEGER 0/1,
--     which sqlx decodes as bool.
--   * Timestamps (created_at, updated_at) are epoch milliseconds as BIGINT,
--     matching SQLite INTEGER decoded as i64.

CREATE TABLE IF NOT EXISTS projects (
    id               TEXT    PRIMARY KEY,
    label            TEXT    NOT NULL,
    directory        TEXT    NOT NULL,
    system_prompt    TEXT,
    auto_worktree    BOOLEAN NOT NULL DEFAULT false,
    setup_command    TEXT,
    teardown_command TEXT,
    branch_prefix    TEXT,
    sandbox_image    TEXT,
    detected         BOOLEAN NOT NULL DEFAULT false,
    created_at       BIGINT  NOT NULL,
    updated_at       BIGINT  NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_projects_updated_at ON projects(updated_at);
