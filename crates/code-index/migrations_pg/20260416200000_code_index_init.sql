-- Initial schema for code-index (PostgreSQL).
-- Mirrors migrations/20260416200000_code_index_init.sql. Differences from the
-- SQLite schema, and why:
--
--   * Keyword search uses a generated `tsvector` column with a GIN index
--     instead of the FTS5 virtual table and its sync triggers, so no trigger
--     maintenance is needed. The FTS5 index uses the 'porter' stemmer, so the
--     tsvector uses the matching 'english' regconfig.
--     Known divergence: PostgreSQL's 'english' dictionary drops stopwords
--     while FTS5's porter tokenizer keeps them — stopword-only queries match
--     chunks on SQLite but return nothing on PostgreSQL.
--   * Integers are BIGINT so they decode as i64 in both backends.
--   * `created_at` stays TEXT in the same `YYYY-MM-DD HH:MM:SS` UTC form
--     SQLite's CURRENT_TIMESTAMP writes, so both backends sort identically.

CREATE TABLE IF NOT EXISTS code_chunks (
    id          BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    project_id  TEXT   NOT NULL,
    file_path   TEXT   NOT NULL,
    chunk_index BIGINT NOT NULL,
    content     TEXT   NOT NULL,
    embedding   BYTEA,  -- Quantized i8 vector
    start_line  BIGINT NOT NULL,
    end_line    BIGINT NOT NULL,
    created_at  TEXT   DEFAULT (to_char(now() AT TIME ZONE 'utc', 'YYYY-MM-DD HH24:MI:SS')),
    tsv         tsvector GENERATED ALWAYS AS (to_tsvector('english', coalesce(content, ''))) STORED,
    UNIQUE(project_id, file_path, chunk_index)
);

CREATE INDEX IF NOT EXISTS idx_chunks_project ON code_chunks(project_id);
CREATE INDEX IF NOT EXISTS idx_chunks_file ON code_chunks(project_id, file_path);
CREATE INDEX IF NOT EXISTS idx_chunks_fts ON code_chunks USING gin(tsv);
