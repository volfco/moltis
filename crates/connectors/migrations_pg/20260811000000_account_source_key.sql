-- Account source key (PostgreSQL).
-- Mirrors migrations/20260811000000_account_source_key.sql unchanged.

ALTER TABLE connector_accounts ADD COLUMN source_key TEXT;

CREATE UNIQUE INDEX IF NOT EXISTS idx_connector_accounts_kind_source_key
    ON connector_accounts(kind, source_key) WHERE source_key IS NOT NULL;
