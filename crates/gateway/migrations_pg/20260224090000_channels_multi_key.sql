-- Mirrors migrations/20260224090000_channels_multi_key.sql. PostgreSQL DDL is
-- transactional, so the table-swap pattern works the same way here.

CREATE TABLE channels_new (
    channel_type TEXT    NOT NULL DEFAULT 'telegram',
    account_id   TEXT    NOT NULL,
    config       TEXT    NOT NULL,
    created_at   BIGINT  NOT NULL,
    updated_at   BIGINT  NOT NULL,
    PRIMARY KEY (channel_type, account_id)
);

INSERT INTO channels_new (channel_type, account_id, config, created_at, updated_at)
SELECT
    COALESCE(channel_type, 'telegram'),
    account_id,
    config,
    created_at,
    updated_at
FROM channels;

DROP TABLE channels;
ALTER TABLE channels_new RENAME TO channels;

DROP INDEX IF EXISTS idx_message_log_account_created;
CREATE INDEX IF NOT EXISTS idx_message_log_channel_account_created
    ON message_log (channel_type, account_id, created_at DESC);
