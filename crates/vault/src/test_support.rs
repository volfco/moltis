//! Shared helpers for vault tests against SQLite and PostgreSQL.
//!
//! PostgreSQL cases run only when `TEST_POSTGRES_URL` is set, so `cargo test`
//! still works on a machine without PostgreSQL.

use moltis_db::{Db, DbOptions};

/// Engine a test case runs against.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Backend {
    Sqlite,
    Postgres,
}

/// Serializes PostgreSQL tests: they share one database, so they must not
/// reset the schema while another test is mid-flight. Postgres suites must
/// run one crate per process: this lock is per-process and the suites share
/// `_sqlx_migrations`.
pub(crate) fn pg_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    &LOCK
}

/// Fresh database with the vault schema plus the gateway tables the
/// migration helpers operate on (`env_variables`, `ssh_keys`).
///
/// Skips (returns `None`) only when `TEST_POSTGRES_URL` is unset; when it is
/// set, any connect/reset/migration failure panics so a broken harness can
/// not pass green. The caller must already hold [`pg_lock`] for
/// [`Backend::Postgres`].
pub(crate) async fn test_db(backend: Backend) -> Option<Db> {
    let db = match backend {
        Backend::Sqlite => Db::connect(&DbOptions::sqlite(":memory:", 1))
            .await
            .unwrap_or_else(|error| panic!("sqlite test db connect failed: {error}")),
        Backend::Postgres => {
            let Ok(url) = std::env::var("TEST_POSTGRES_URL") else {
                return None;
            };
            let db = Db::connect(&DbOptions::postgres(url, 2))
                .await
                .unwrap_or_else(|error| panic!("postgres test db connect failed: {error}"));
            // Dropping sqlx's history is required: with it in place the
            // migrator believes this version is already applied and creates
            // nothing.
            moltis_db::execute!(
                &db,
                "DROP TABLE IF EXISTS env_variables, ssh_keys, vault_metadata, \
                 _sqlx_migrations CASCADE"
            )
            .unwrap_or_else(|error| panic!("postgres test reset failed: {error}"));
            db
        },
    };
    crate::run_migrations(&db)
        .await
        .unwrap_or_else(|error| panic!("vault test migrations failed: {error}"));
    create_gateway_tables(&db)
        .await
        .unwrap_or_else(|error| panic!("vault test gateway tables failed: {error}"));
    Some(db)
}

/// `env_variables` / `ssh_keys` are owned by moltis-gateway; vault's migration
/// helpers operate on them, so tests create gateway-shaped copies.
async fn create_gateway_tables(db: &Db) -> Result<(), sqlx::Error> {
    match db.dialect() {
        moltis_db::Dialect::Sqlite => {
            moltis_db::execute!(
                db,
                "CREATE TABLE IF NOT EXISTS env_variables (
                    id         INTEGER PRIMARY KEY AUTOINCREMENT,
                    key        TEXT    NOT NULL UNIQUE,
                    value      TEXT    NOT NULL,
                    encrypted  INTEGER NOT NULL DEFAULT 0,
                    created_at TEXT    NOT NULL DEFAULT (datetime('now')),
                    updated_at TEXT    NOT NULL DEFAULT (datetime('now'))
                )",
            )?;
            moltis_db::execute!(
                db,
                "CREATE TABLE IF NOT EXISTS ssh_keys (
                    id          INTEGER PRIMARY KEY AUTOINCREMENT,
                    name        TEXT    NOT NULL UNIQUE,
                    private_key TEXT    NOT NULL,
                    public_key  TEXT    NOT NULL,
                    fingerprint TEXT    NOT NULL,
                    encrypted   INTEGER NOT NULL DEFAULT 0,
                    created_at  TEXT    NOT NULL DEFAULT (datetime('now')),
                    updated_at  TEXT    NOT NULL DEFAULT (datetime('now'))
                )",
            )?;
        },
        moltis_db::Dialect::Postgres => {
            moltis_db::execute!(
                db,
                "CREATE TABLE IF NOT EXISTS env_variables (
                    id         BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
                    key        TEXT   NOT NULL UNIQUE,
                    value      TEXT   NOT NULL,
                    encrypted  BIGINT NOT NULL DEFAULT 0,
                    created_at TEXT   NOT NULL DEFAULT (to_char(now() AT TIME ZONE 'utc', 'YYYY-MM-DD HH24:MI:SS')),
                    updated_at TEXT   NOT NULL DEFAULT (to_char(now() AT TIME ZONE 'utc', 'YYYY-MM-DD HH24:MI:SS'))
                )",
            )?;
            moltis_db::execute!(
                db,
                "CREATE TABLE IF NOT EXISTS ssh_keys (
                    id          BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
                    name        TEXT   NOT NULL UNIQUE,
                    private_key TEXT   NOT NULL,
                    public_key  TEXT   NOT NULL,
                    fingerprint TEXT   NOT NULL,
                    encrypted   BIGINT NOT NULL DEFAULT 0,
                    created_at  TEXT   NOT NULL DEFAULT (to_char(now() AT TIME ZONE 'utc', 'YYYY-MM-DD HH24:MI:SS')),
                    updated_at  TEXT   NOT NULL DEFAULT (to_char(now() AT TIME ZONE 'utc', 'YYYY-MM-DD HH24:MI:SS'))
                )",
            )?;
        },
    }
    Ok(())
}
