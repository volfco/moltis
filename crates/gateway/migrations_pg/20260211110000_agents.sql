-- Mirrors migrations/20260211110000_agents.sql: epoch-millis timestamps are
-- BIGINT so they decode as i64 in both backends.
CREATE TABLE IF NOT EXISTS agents (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    emoji       TEXT,
    creature    TEXT,
    vibe        TEXT,
    description TEXT,
    created_at  BIGINT NOT NULL,
    updated_at  BIGINT NOT NULL
);
