//! Session storage and management.
//!
//! Sessions are stored as JSONL files (one message per line) under
//! `<data_dir>/sessions/v1/`, using a reversible filesystem-safe encoding of
//! each session key. Legacy files are copied lazily when their key is accessed.

pub mod compaction;
pub mod error;
pub mod key;
pub mod metadata;
pub mod session_events;
pub mod state_store;
#[cfg(test)]
pub(crate) mod test_support;
mod storage_layout;
pub mod store;

/// Persisted message types live in `moltis-protocol` so clients can share them
/// without depending on session storage.
pub use moltis_protocol::message;
pub use {
    error::{Error, Result},
    key::SessionKey,
    message::{ContentBlock, MessageContent, PersistedMessage, UserDocument},
    store::SearchResult,
};

/// Run database migrations for the sessions crate.
///
/// This creates the `sessions` and `channel_sessions` tables. Should be called
/// at application startup after [`moltis_projects::run_migrations`] (sessions
/// has a foreign key to projects).
pub async fn run_migrations(db: &moltis_db::Db) -> Result<()> {
    moltis_db::run_migrations(
        db,
        sqlx::migrate!("./migrations"),
        sqlx::migrate!("./migrations_pg"),
    )
    .await?;
    Ok(())
}
