-- Add encrypted flag to env_variables for vault integration.
-- Mirrors the SQLite migration. BIGINT because the Rust boundary binds/reads
-- this flag as i64 (0 = plaintext, 1 = encrypted by vault).
ALTER TABLE env_variables ADD COLUMN encrypted BIGINT NOT NULL DEFAULT 0;
