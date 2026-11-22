-- Session branching: track parent/child relationships and fork points.
ALTER TABLE sessions ADD COLUMN parent_session_key TEXT;
ALTER TABLE sessions ADD COLUMN fork_point BIGINT;

CREATE INDEX IF NOT EXISTS idx_sessions_parent ON sessions(parent_session_key);
