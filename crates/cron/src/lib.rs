//! Scheduled agent runs with cron expressions.
//! Persistent storage at `<data_dir>/cron/jobs.json`.
//! Isolated agent execution (no session), optional delivery to a channel.

pub mod error;
pub mod heartbeat;
pub mod parse;
pub mod schedule;
pub mod service;
pub mod store;
pub mod store_db;
pub mod store_file;
pub mod store_memory;
pub mod system_events;
pub mod types;

#[cfg(test)]
pub(crate) mod test_support;

pub use {
    error::{Error, Result},
    service::{DEFAULT_WAKE_COOLDOWN_MS, WAKE_REASON_CRON_EVENT, WAKE_REASON_EXEC_EVENT},
};

/// Run database migrations for the cron crate.
///
/// This creates the `cron_jobs` and `cron_runs` tables. Should be called at
/// application startup when using [`store_db::DbStore`].
pub async fn run_migrations(db: &moltis_db::Db) -> Result<()> {
    moltis_db::run_migrations(
        db,
        sqlx::migrate!("./migrations"),
        sqlx::migrate!("./migrations_pg"),
    )
    .await?;
    Ok(())
}
