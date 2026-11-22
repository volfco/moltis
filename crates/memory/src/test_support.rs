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
/// Returns `None` when `TEST_POSTGRES_URL` is unset. The caller must already
/// hold [`pg_lock`].
pub(crate) async fn pg_test_db() -> Option<Db> {
    let url = std::env::var("TEST_POSTGRES_URL").ok()?;
    let db = Db::connect(&DbOptions::postgres(url, 2)).await.ok()?;
    // Dropping sqlx's history is required: with it in place the migrator
    // believes this version is already applied and creates nothing.
    let reset = moltis_db::execute!(
        &db,
        "DROP TABLE IF EXISTS chunks_fts, chunks, files, embedding_cache, \
         _sqlx_migrations CASCADE"
    );
    if let Err(error) = reset {
        eprintln!("postgres test reset failed: {error}");
        return None;
    }
    if let Err(error) = crate::schema::run_migrations(&db).await {
        eprintln!("postgres test migrations failed: {error}");
        return None;
    }
    Some(db)
}
