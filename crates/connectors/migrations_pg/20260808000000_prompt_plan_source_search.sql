-- Prompt-plan source state and item full-text search (PostgreSQL).
-- Mirrors migrations/20260808000000_prompt_plan_source_search.sql. Differences:
--   * The FTS5 virtual table and its sync triggers become a generated
--     `tsvector` column with a GIN index. FTS5 here uses the 'unicode61'
--     tokenizer (no stemming), so the tsvector uses the matching 'simple'
--     regconfig; queries use websearch_to_tsquery('simple', ...).
--   * Integers are BIGINT so they decode as i64 in both backends.

ALTER TABLE connector_datasets ADD COLUMN instruction TEXT;
ALTER TABLE connector_datasets ADD COLUMN plan_revision BIGINT NOT NULL DEFAULT 1;
ALTER TABLE connector_datasets ADD COLUMN synced_plan_revision BIGINT;

ALTER TABLE connector_items ADD COLUMN search_text TEXT NOT NULL DEFAULT '';

CREATE TABLE IF NOT EXISTS connector_source_states (
    dataset_id             TEXT   NOT NULL REFERENCES connector_datasets(id) ON DELETE CASCADE,
    remote_id              TEXT   NOT NULL,
    remote_version         TEXT,
    disposition            TEXT   NOT NULL CHECK (disposition IN ('included', 'filtered')),
    filter_reason          TEXT,
    evaluated_plan_revision BIGINT NOT NULL CHECK (evaluated_plan_revision > 0),
    last_seen_run_id       TEXT   NOT NULL,
    observed_at            BIGINT NOT NULL,
    PRIMARY KEY (dataset_id, remote_id)
);

CREATE INDEX IF NOT EXISTS idx_connector_source_states_run
    ON connector_source_states(dataset_id, last_seen_run_id, disposition);

ALTER TABLE connector_items ADD COLUMN tsv tsvector
    GENERATED ALWAYS AS (to_tsvector('simple', coalesce(search_text, ''))) STORED;
CREATE INDEX IF NOT EXISTS idx_connector_items_fts ON connector_items USING gin(tsv);
