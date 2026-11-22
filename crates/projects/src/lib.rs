//! Project management for moltis.
//!
//! A project represents a codebase directory. When a session is bound to a
//! project, moltis loads `CLAUDE.md` and `AGENTS.md` context files from the
//! directory hierarchy and can create git worktrees for session isolation.

pub mod complete;
pub mod context;
pub mod detect;
pub mod error;
pub mod store;
#[cfg(test)]
pub(crate) mod test_support;
pub mod types;
pub mod worktree;

pub use {
    error::{Error, Result},
    store::{DbProjectStore, ProjectStore, TomlProjectStore},
    types::{ContextFile, Project, ProjectContext},
    worktree::WorktreeManager,
};

/// Run database migrations for the projects crate.
///
/// This creates the `projects` table and indexes. Should be called at
/// application startup before using [`DbProjectStore`].
pub async fn run_migrations(db: &moltis_db::Db) -> Result<()> {
    moltis_db::run_migrations(
        db,
        sqlx::migrate!("./migrations"),
        sqlx::migrate!("./migrations_pg"),
    )
    .await?;
    Ok(())
}
