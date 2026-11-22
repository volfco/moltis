//! Generic webhook ingress for Moltis.
//!
//! Provides named inbound HTTP endpoints that trigger agent sessions.
//! Each delivery is verified, deduplicated, persisted, and processed
//! asynchronously via the existing chat/session infrastructure.

pub mod auth;
pub mod dedup;
pub mod error;
pub mod filter;
pub mod normalize;
pub mod profiles;
pub mod rate_limit;
pub mod store;
#[cfg(test)]
pub(crate) mod test_support;
pub mod types;
pub mod worker;

pub use error::{Error, Result};

/// Run database migrations for the webhooks crate.
///
/// Creates the `webhooks`, `webhook_deliveries`, and `webhook_response_actions`
/// tables. Call at application startup.
pub async fn run_migrations(db: &moltis_db::Db) -> Result<()> {
    // Foreign key enforcement (for ON DELETE CASCADE) is enabled via
    // `.foreign_keys(true)` on SQLite connections (PostgreSQL always enforces
    // foreign keys), not per-query PRAGMA.
    moltis_db::run_migrations(
        db,
        sqlx::migrate!("./migrations"),
        sqlx::migrate!("./migrations_pg"),
    )
    .await?;
    Ok(())
}
