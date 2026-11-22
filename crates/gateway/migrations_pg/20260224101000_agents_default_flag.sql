-- Mirrors migrations/20260224101000_agents_default_flag.sql. BIGINT because
-- the Rust boundary binds/reads is_default as i64.
ALTER TABLE agents ADD COLUMN is_default BIGINT NOT NULL DEFAULT 0;

CREATE UNIQUE INDEX IF NOT EXISTS idx_agents_single_default
ON agents(is_default)
WHERE is_default = 1;
