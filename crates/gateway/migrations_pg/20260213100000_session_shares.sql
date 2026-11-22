-- Immutable session-share snapshots for public/private sharing.
-- Mirrors migrations/20260213100000_session_shares.sql; counters and
-- epoch-millis timestamps are BIGINT so they decode as i64 in both backends.
CREATE TABLE IF NOT EXISTS session_shares (
    id                     TEXT    PRIMARY KEY,
    session_key            TEXT    NOT NULL,
    visibility             TEXT    NOT NULL CHECK (visibility IN ('public', 'private')),
    snapshot_json          TEXT    NOT NULL,
    snapshot_message_count BIGINT  NOT NULL,
    token_hash             TEXT,
    views                  BIGINT  NOT NULL DEFAULT 0,
    created_at             BIGINT  NOT NULL,
    revoked_at             BIGINT
);

CREATE INDEX IF NOT EXISTS idx_session_shares_session_created
    ON session_shares (session_key, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_session_shares_active
    ON session_shares (id, revoked_at);

CREATE UNIQUE INDEX IF NOT EXISTS idx_session_shares_one_active_per_session
    ON session_shares (session_key)
    WHERE revoked_at IS NULL;
