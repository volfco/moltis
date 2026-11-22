-- Memory system schema (PostgreSQL + pgvector)
--
-- Mirrors migrations/20240205100004_init.sql. Differences from the SQLite
-- schema, and why:
--
--   * Embeddings are `vector` (pgvector) instead of a BLOB of little-endian
--     f32s, so similarity search runs in the database instead of loading every
--     chunk into the process. The column is untyped (`vector`, not
--     `vector(N)`) so any embedding dimension works without a re-index when
--     the provider changes; similarity search is a sequential scan ordered by
--     `<=>`. Add a dimension and an HNSW index when sub-linear search is
--     needed.
--   * Keyword search uses a generated `tsvector` column with a GIN index
--     instead of SQLite's FTS5 virtual table, so no sync triggers are needed.
--   * Integers are BIGINT so they decode as i64 in both backends.
--   * Timestamps stay TEXT in the same `YYYY-MM-DD HH:MM:SS` UTC form SQLite
--     writes, so ordering and comparisons behave identically.

CREATE EXTENSION IF NOT EXISTS vector;

CREATE TABLE IF NOT EXISTS files (
    path   TEXT   NOT NULL PRIMARY KEY,
    source TEXT   NOT NULL,
    hash   TEXT   NOT NULL,
    mtime  BIGINT NOT NULL,
    size   BIGINT NOT NULL
);

CREATE TABLE IF NOT EXISTS chunks (
    id         TEXT   NOT NULL PRIMARY KEY,
    path       TEXT   NOT NULL REFERENCES files(path) ON DELETE CASCADE,
    source     TEXT   NOT NULL,
    start_line BIGINT NOT NULL,
    end_line   BIGINT NOT NULL,
    hash       TEXT   NOT NULL,
    model      TEXT   NOT NULL,
    text       TEXT   NOT NULL,
    embedding  vector,
    updated_at TEXT   NOT NULL DEFAULT (to_char(now() AT TIME ZONE 'utc', 'YYYY-MM-DD HH24:MI:SS')),
    tsv        tsvector GENERATED ALWAYS AS (to_tsvector('english', coalesce(text, ''))) STORED
);

CREATE INDEX IF NOT EXISTS idx_chunks_path ON chunks(path);
CREATE INDEX IF NOT EXISTS idx_chunks_fts ON chunks USING gin(tsv);

CREATE TABLE IF NOT EXISTS embedding_cache (
    provider     TEXT   NOT NULL,
    model        TEXT   NOT NULL,
    provider_key TEXT   NOT NULL,
    hash         TEXT   NOT NULL,
    embedding    vector NOT NULL,
    dims         BIGINT NOT NULL,
    updated_at   TEXT   NOT NULL DEFAULT (to_char(now() AT TIME ZONE 'utc', 'YYYY-MM-DD HH24:MI:SS')),
    PRIMARY KEY (provider, model, provider_key, hash)
);
