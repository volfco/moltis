-- Cron tables schema (PostgreSQL)
-- Owned by: moltis-cron crate
--
-- Mirrors migrations/20240205100002_init.sql. Differences from the SQLite
-- schema, and why:
--
--   * `cron_runs.id` is `GENERATED ALWAYS AS IDENTITY` instead of
--     `INTEGER PRIMARY KEY AUTOINCREMENT`; inserts never write it.
--   * Millisecond timestamps and token counts are BIGINT so they decode as
--     i64 on both backends; they are epoch values, not timestamps.

CREATE TABLE IF NOT EXISTS cron_jobs (
    id   TEXT PRIMARY KEY,
    data TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS cron_runs (
    id             BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    job_id         TEXT   NOT NULL,
    started_at_ms  BIGINT NOT NULL,
    finished_at_ms BIGINT NOT NULL,
    status         TEXT   NOT NULL,
    error          TEXT,
    duration_ms    BIGINT NOT NULL,
    output         TEXT,
    input_tokens   BIGINT,
    output_tokens  BIGINT,
    FOREIGN KEY (job_id) REFERENCES cron_jobs(id)
);

CREATE INDEX IF NOT EXISTS idx_cron_runs_job_id
    ON cron_runs(job_id, started_at_ms DESC);
