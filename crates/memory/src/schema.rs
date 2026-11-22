//! Row types for the memory database and migration runner.

use serde::{Deserialize, Serialize};

/// A tracked file row.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileRow {
    pub path: String,
    pub source: String,
    pub hash: String,
    pub mtime: i64,
    pub size: i64,
}

/// A chunk row.
#[derive(Debug, Clone)]
pub struct ChunkRow {
    pub id: String,
    pub path: String,
    pub source: String,
    pub start_line: i64,
    pub end_line: i64,
    pub hash: String,
    pub model: String,
    pub text: String,
    pub embedding: Option<Vec<u8>>,
    pub updated_at: String,
}

/// Run database migrations for the memory system.
///
/// SQLite creates `files`, `chunks`, `embedding_cache` and the `chunks_fts`
/// virtual table; PostgreSQL creates the same tables with a pgvector
/// `vector` column and a generated `tsvector` column instead.
pub async fn run_migrations(db: &moltis_db::Db) -> crate::error::Result<()> {
    moltis_db::run_migrations(
        db,
        sqlx::migrate!("./migrations"),
        sqlx::migrate!("./migrations_pg"),
    )
    .await?;
    Ok(())
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_run_migrations() {
        let pool = sqlx::SqlitePool::connect(":memory:").await.unwrap();
        let db = moltis_db::Db::from(pool);
        run_migrations(&db).await.unwrap();
        // Verify tables exist
        let row: (i64,) = moltis_db::fetch_one!(&db, (i64,), "SELECT count(*) FROM files").unwrap();
        assert_eq!(row.0, 0);
    }
}
