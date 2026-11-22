-- Vault metadata: stores wrapped DEK and KDF parameters (PostgreSQL).
-- Mirrors migrations/20260214000001_vault_metadata.sql. Differences:
--   * `id` stays a plain BIGINT PRIMARY KEY (the SQLite schema has no
--     AUTOINCREMENT; callers insert the single row with id = 1 explicitly,
--     which an IDENTITY column would reject).
--   * Timestamps stay TEXT in SQLite's `YYYY-MM-DD HH:MM:SS` UTC form so
--     both backends sort identically.

CREATE TABLE IF NOT EXISTS vault_metadata (
    id                   BIGINT PRIMARY KEY CHECK (id = 1),
    version              BIGINT NOT NULL DEFAULT 1,
    kdf_salt             TEXT   NOT NULL,
    kdf_params           TEXT   NOT NULL,
    wrapped_dek          TEXT   NOT NULL,
    recovery_wrapped_dek TEXT,
    recovery_key_hash    TEXT,
    created_at           TEXT   NOT NULL DEFAULT (to_char(now() AT TIME ZONE 'utc', 'YYYY-MM-DD HH24:MI:SS')),
    updated_at           TEXT   NOT NULL DEFAULT (to_char(now() AT TIME ZONE 'utc', 'YYYY-MM-DD HH24:MI:SS'))
);
