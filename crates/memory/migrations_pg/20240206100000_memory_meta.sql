-- Engine-agnostic key-value metadata for the memory index (mirrors
-- migrations/20240206100000_memory_meta.sql).
--
-- Records the identity of the embedding provider that produced the stored
-- vectors (`embedding_provider_key`, `embedding_model`, `embedding_dims`).
-- A sync pass compares it against the configured provider and re-embeds every
-- chunk text when the identity changed, because vectors from different
-- providers or dimensions are not comparable.

CREATE TABLE IF NOT EXISTS memory_meta (
    key   TEXT NOT NULL PRIMARY KEY,
    value TEXT NOT NULL
);
