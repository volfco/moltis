-- Audit log for authentication events (login attempts, key creation, etc.).
-- Mirrors migrations/20260412180100_auth_audit_log.sql.
CREATE TABLE IF NOT EXISTS auth_audit_log (
    id         BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    event_type TEXT   NOT NULL,  -- login_success, login_failure, setup, key_created, key_revoked, password_changed, auth_reset
    client_ip  TEXT,
    detail     TEXT,
    created_at TEXT   NOT NULL DEFAULT (to_char(now() AT TIME ZONE 'utc', 'YYYY-MM-DD HH24:MI:SS'))
);

-- Keep the audit log from growing without bounds; older entries can be
-- pruned by a periodic cleanup task.
CREATE INDEX IF NOT EXISTS idx_auth_audit_log_created ON auth_audit_log (created_at);
