//! Shared helpers for database-backed tests.
//!
//! PostgreSQL tests run only when `TEST_POSTGRES_URL` is set, so `cargo test`
//! still works on a machine without PostgreSQL.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use moltis_db::{Db, DbOptions, execute};

/// Serializes PostgreSQL tests: they share one database, so they must not
/// reset the schema while another test is mid-flight.
pub(crate) fn pg_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    &LOCK
}

/// Open the test database after removing every table the auth and vault
/// crates touch. The test body then brings the schema up exactly like the
/// SQLite tests do (via `CredentialStore::new` / `moltis_vault::run_migrations`).
///
/// Returns `None` when `TEST_POSTGRES_URL` is unset (the caller's test skips).
/// When it IS set, any setup failure panics: a green run must never mean "the
/// database was unreachable and nothing was tested". The caller must already
/// hold [`pg_lock`].
pub(crate) async fn pg_test_db() -> Option<Db> {
    let Ok(url) = std::env::var("TEST_POSTGRES_URL") else {
        return None;
    };
    let db = Db::connect(&DbOptions::postgres(url, 2))
        .await
        .unwrap_or_else(|error| {
            panic!("TEST_POSTGRES_URL is set but connecting to it failed: {error}")
        });
    // Dropping sqlx's history is required: with it in place the vault
    // migrator believes its version is already applied and creates nothing.
    let reset = execute!(
        &db,
        "DROP TABLE IF EXISTS auth_password, passkeys, api_keys, auth_sessions, \
         auth_audit_log, env_variables, ssh_keys, ssh_targets, auth_state, \
         vault_metadata, _sqlx_migrations CASCADE"
    );
    reset.unwrap_or_else(|error| panic!("postgres test schema reset failed: {error}"));
    Some(db)
}

/// In-memory SQLite database; `CredentialStore::new` creates the schema.
pub(crate) async fn sqlite_db() -> Db {
    Db::connect(&DbOptions::sqlite(":memory:", 1))
        .await
        .unwrap_or_else(|error| panic!("sqlite test db connect failed: {error}"))
}

/// Defines one store test as a pair of runners: `$body` runs against SQLite
/// and, when `TEST_POSTGRES_URL` is set, against PostgreSQL with the same
/// assertions. Postgres runs are serialized because they reset the schema.
macro_rules! backend_test {
    ($sqlite_name:ident, $postgres_name:ident, $body:ident) => {
        #[tokio::test]
        async fn $sqlite_name() {
            $body(&crate::test_support::sqlite_db().await).await;
        }

        #[tokio::test]
        async fn $postgres_name() {
            if std::env::var("TEST_POSTGRES_URL").is_err() {
                eprintln!("skipping: TEST_POSTGRES_URL is not set");
                return;
            }
            let _guard = crate::test_support::pg_lock().lock().await;
            let Some(db) = crate::test_support::pg_test_db().await else {
                return;
            };
            $body(&db).await;
        }
    };
}
pub(crate) use backend_test;
