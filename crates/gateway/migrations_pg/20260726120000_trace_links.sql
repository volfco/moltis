-- Correlates a delivered reply with the trace that produced it, so a reaction
-- arriving later can be scored against the right agent run.
--
-- Identifiers only: message bodies stay in message_log rather than being
-- duplicated here.
-- Mirrors migrations/20260726120000_trace_links.sql; created_at is BIGINT
-- (epoch millis) so it decodes as i64 in both backends.
CREATE TABLE IF NOT EXISTS trace_links (
    channel_type TEXT   NOT NULL,
    account_id   TEXT   NOT NULL,
    chat_id      TEXT   NOT NULL,
    message_id   TEXT   NOT NULL,
    trace_id     TEXT   NOT NULL,
    session_key  TEXT,
    created_at   BIGINT NOT NULL,
    PRIMARY KEY (channel_type, account_id, chat_id, message_id)
);

-- Pruning scans by age.
CREATE INDEX IF NOT EXISTS idx_trace_links_created
    ON trace_links (created_at);
