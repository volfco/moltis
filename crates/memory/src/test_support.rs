//! Shared helpers for PostgreSQL-backed tests.
//!
//! Tests that need a real server run only when `TEST_POSTGRES_URL` is set, so
//! `cargo test` still works on a machine without PostgreSQL.

use moltis_db::{Db, DbOptions};

/// Serializes PostgreSQL tests: they share one database, so they must not
/// reset the schema while another test is mid-flight.
pub(crate) fn pg_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    &LOCK
}

/// Open the test database with an empty memory schema.
///
/// Returns `None` when `TEST_POSTGRES_URL` is unset (the caller's test skips).
/// When it IS set, any setup failure panics: a green run must never mean "the
/// database was unreachable and nothing was tested". The caller must already
/// hold [`pg_lock`].
pub(crate) async fn pg_test_db() -> Option<Db> {
    let db = pg_test_db_pre_migration().await?;
    crate::schema::run_migrations(&db)
        .await
        .unwrap_or_else(|error| panic!("postgres test migrations failed: {error}"));
    Some(db)
}

/// [`pg_test_db`] without the migration pass: the same empty schema for tests
/// that need the pre-migration state. The caller must already hold [`pg_lock`].
pub(crate) async fn pg_test_db_pre_migration() -> Option<Db> {
    let Ok(url) = std::env::var("TEST_POSTGRES_URL") else {
        return None;
    };
    let db = Db::connect(&DbOptions::postgres(url, 2))
        .await
        .unwrap_or_else(|error| {
            panic!("TEST_POSTGRES_URL is set but connecting to it failed: {error}")
        });
    // Dropping sqlx's history is required: with it in place the migrator
    // believes this version is already applied and creates nothing.
    let reset = moltis_db::execute!(
        &db,
        "DROP TABLE IF EXISTS chunks_fts, chunks, files, embedding_cache, memory_meta, \
         _sqlx_migrations CASCADE"
    );
    reset.unwrap_or_else(|error| panic!("postgres test schema reset failed: {error}"));
    Some(db)
}
