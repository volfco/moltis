//! Shared helpers for PostgreSQL-backed tests.
//!
//! Tests that need a real server run only when `TEST_POSTGRES_URL` is set, so
//! `cargo test` still works on a machine without PostgreSQL. Once the variable
//! is set, every failure fails the test: a typo'd URL must never pass green.

use moltis_db::{Db, DbOptions};

/// Serializes PostgreSQL tests: they share one database, so they must not
/// reset the schema while another test is mid-flight.
pub(crate) fn pg_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    &LOCK
}

/// Open the test database with a freshly migrated gateway schema.
///
/// Returns `None` (skip) only when `TEST_POSTGRES_URL` is unset; connecting,
/// resetting, or migrating failing panics instead. The caller must already
/// hold [`pg_lock`].
pub(crate) async fn pg_test_db() -> Option<Db> {
    let Ok(url) = std::env::var("TEST_POSTGRES_URL") else {
        return None;
    };
    let db = Db::connect(&DbOptions::postgres(url, 2))
        .await
        .unwrap_or_else(|error| panic!("TEST_POSTGRES_URL is set but connect failed: {error}"));
    // Dropping sqlx's history is required: with it in place the migrator
    // believes the version is already applied and creates nothing.
    moltis_db::execute!(
        &db,
        "DROP TABLE IF EXISTS _sqlx_migrations, auth_password, passkeys, api_keys, \
         auth_sessions, auth_audit_log, env_variables, message_log, channels, agents, \
         session_shares, paired_devices, pair_requests, device_tokens, ssh_keys, \
         ssh_targets, voice_personas, trace_links CASCADE"
    )
    .unwrap_or_else(|error| panic!("postgres test schema reset failed: {error}"));
    crate::run_migrations(&db)
        .await
        .unwrap_or_else(|error| panic!("postgres test migrations failed: {error}"));
    Some(db)
}
