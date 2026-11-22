//! Cross-instance memory sync lock.
//!
//! Several moltis instances may point at the same PostgreSQL memory database.
//! A full sync pass rewrites files/chunks/embeddings and must not interleave
//! across instances, so sync runs under a session advisory lock taken with
//! `pg_try_advisory_lock`: the losing instance skips the pass with a log line.
//!
//! The lock is bound to one pooled connection (`PoolConnection` held for the
//! guard's lifetime), so it must never be acquired on a connection that goes
//! back to the pool while still holding it: [`SyncLockGuard::release`] unlocks
//! on that same connection (closing it instead if the unlock fails), and
//! [`Drop`] marks it `close_on_drop` as a fallback — a closed session releases
//! its advisory locks server-side, so the lock cannot outlive the sync run.
//!
//! That guarantee only holds while the guard is *armed*: the connection is put
//! inside the guard before the lock query runs, and both the acquisition query
//! and the unlock query borrow it through the guard instead of holding it in a
//! local. A task abort or timeout therefore never drops a bare connection
//! across an await — sqlx returns cancelled connections to the pool after a
//! repair ping, and a session returned that way keeps its advisory locks.

use {
    moltis_db::Db,
    sqlx::{Postgres, pool::PoolConnection},
    tracing::warn,
};

use crate::error::Result;

/// Advisory lock key for memory sync passes.
///
/// Advisory lock keys share one collision domain per PostgreSQL cluster, so
/// the value must be unique to this feature. The constant is ASCII `mem_sync`,
/// which is recognizable in `pg_locks` output and not derived from any other
/// moltis key.
pub const MEMORY_SYNC_LOCK_KEY: i64 = 0x6D65_6D5F_7379_6E63;

/// Held for the duration of a memory sync pass so only one moltis instance
/// syncs a given database at a time. Reusable by future entry points (e.g. a
/// reindex command) via [`SyncLockGuard::try_acquire`].
pub enum SyncLockGuard {
    /// Single-writer backend (SQLite) or non-DB store: nothing to lock.
    Uncontended,
    /// PostgreSQL session advisory lock, held on `conn` until [`Self::release`].
    Pg {
        /// `Some` while the lock is held; taken by `release`/`Drop`.
        conn: Option<PoolConnection<Postgres>>,
    },
}

impl SyncLockGuard {
    /// Try to take the memory sync lock for `db`.
    ///
    /// Returns `None` when another instance currently holds it and the caller
    /// must skip the sync. SQLite gets an [`SyncLockGuard::Uncontended`]
    /// guard: single-writer engine, no cross-instance lock needed.
    pub async fn try_acquire(db: &Db) -> Result<Option<Self>> {
        if !db.is_postgres() {
            return Ok(Some(Self::Uncontended));
        }
        let pool = db
            .postgres()
            .ok_or_else(|| crate::error::Error::Backend("postgres db without pool".into()))?;
        let conn = pool.acquire().await?;
        // Arm the guard before the lock query: the server may grant the lock
        // while this future is inside the await below, and a dropped future
        // must close that session rather than return it to the pool still
        // holding the lock.
        let mut guard = Self::Pg { conn: Some(conn) };
        let acquired: bool = {
            let Some(conn) = guard.conn_mut() else {
                // Unreachable: the guard was just armed. Fail loudly rather
                // than report a phantom contended lock.
                return Err(crate::error::Error::Backend(
                    "postgres sync lock guard without connection".into(),
                ));
            };
            sqlx::query_scalar("SELECT pg_try_advisory_lock($1)")
                .bind(MEMORY_SYNC_LOCK_KEY)
                .fetch_one(&mut **conn)
                .await?
        };
        if acquired {
            return Ok(Some(guard));
        }
        // Not acquired: this session holds no lock, so the connection is safe
        // to return to the pool.
        drop(guard.take_conn());
        Ok(None)
    }

    /// Borrow the lock connection, if the guard still owns it.
    fn conn_mut(&mut self) -> Option<&mut PoolConnection<Postgres>> {
        match self {
            Self::Pg { conn } => conn.as_mut(),
            Self::Uncontended => None,
        }
    }

    /// Take the connection out of the guard; the guard no longer owns it.
    fn take_conn(&mut self) -> Option<PoolConnection<Postgres>> {
        match self {
            Self::Pg { conn } => conn.take(),
            Self::Uncontended => None,
        }
    }

    /// Release the lock deterministically and return the connection to the pool.
    pub async fn release(mut self) {
        let unlocked = match self.conn_mut() {
            Some(conn) => {
                sqlx::query("SELECT pg_advisory_unlock($1)")
                    .bind(MEMORY_SYNC_LOCK_KEY)
                    .execute(&mut **conn)
                    .await
            },
            None => return,
        };
        // Take the connection only after the unlock resolved: the guard must
        // stay armed across that await, so dropping `release` midway still
        // closes the session instead of pooling a possibly-locked one.
        let Some(conn) = self.take_conn() else {
            return;
        };
        if let Err(error) = unlocked {
            // Returning a connection that still holds the lock would leak it to
            // the next borrower; kill the session instead.
            warn!(%error, "memory sync lock unlock failed; closing lock connection");
            // Best-effort: the connection is consumed either way and cannot
            // return to the pool still holding the lock.
            let _ = conn.close().await;
        }
    }
}

impl Drop for SyncLockGuard {
    /// Safety net for panic/cancellation paths where `release` never ran.
    /// `Drop` cannot await, so the connection is marked `close_on_drop`:
    /// sqlx closes it instead of returning it to the pool, and a closed
    /// session releases its advisory locks server-side.
    fn drop(&mut self) {
        if let Some(mut conn) = self.take_conn() {
            conn.close_on_drop();
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use {
        super::*,
        crate::{config::MemoryConfig, manager::MemoryManager, store_db::DbMemoryStore},
        sqlx::{Connection, postgres::PgConnection},
        tempfile::TempDir,
    };

    fn pg_enabled() -> bool {
        if std::env::var("TEST_POSTGRES_URL").is_err() {
            eprintln!("skipping: TEST_POSTGRES_URL is not set");
            return false;
        }
        true
    }

    /// A second session acting as another moltis instance.
    async fn other_instance() -> PgConnection {
        let url = std::env::var("TEST_POSTGRES_URL").unwrap();
        PgConnection::connect(&url).await.unwrap()
    }

    async fn try_lock(conn: &mut PgConnection) -> bool {
        sqlx::query_scalar("SELECT pg_try_advisory_lock($1)")
            .bind(MEMORY_SYNC_LOCK_KEY)
            .fetch_one(conn)
            .await
            .unwrap()
    }

    async fn unlock(conn: &mut PgConnection) {
        sqlx::query("SELECT pg_advisory_unlock($1)")
            .bind(MEMORY_SYNC_LOCK_KEY)
            .execute(conn)
            .await
            .unwrap();
    }

    /// Manager over the shared Postgres test database, with one memory file on
    /// disk and a second session to act as the competing instance. The caller
    /// must already hold [`crate::test_support::pg_lock`].
    async fn pg_setup() -> (MemoryManager, PgConnection, TempDir) {
        let db = crate::test_support::pg_test_db().await.unwrap();
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().join("memory");
        std::fs::create_dir_all(&mem_dir).unwrap();
        std::fs::write(mem_dir.join("note.md"), "# note\nhello memory").unwrap();
        let config = MemoryConfig {
            db_path: ":memory:".into(),
            memory_dirs: vec![mem_dir],
            chunk_size: 50,
            chunk_overlap: 10,
            vector_weight: 0.7,
            keyword_weight: 0.3,
            ..Default::default()
        };
        let manager = MemoryManager::keyword_only(config, Box::new(DbMemoryStore::new(db)));
        let other = other_instance().await;
        (manager, other, tmp)
    }

    #[tokio::test]
    async fn sqlite_guard_is_uncontended() {
        let pool = sqlx::SqlitePool::connect(":memory:").await.unwrap();
        let db = Db::from(pool);
        let guard = SyncLockGuard::try_acquire(&db)
            .await
            .unwrap()
            .expect("sqlite must be uncontended");
        assert!(matches!(guard, SyncLockGuard::Uncontended));
        guard.release().await;
    }

    #[tokio::test]
    async fn postgres_sync_skips_while_lock_held_elsewhere() {
        if !pg_enabled() {
            return;
        }
        let _guard = crate::test_support::pg_lock().lock().await;
        let (manager, mut other, _tmp) = pg_setup().await;
        assert!(try_lock(&mut other).await, "other instance takes the lock");

        let report = manager.sync().await.unwrap();
        assert!(
            report.skipped,
            "sync must skip while the lock is held elsewhere"
        );
        assert_eq!(
            manager.status().await.unwrap().total_files,
            0,
            "skipped sync must do no partial work"
        );
    }

    #[tokio::test]
    async fn postgres_sync_proceeds_after_lock_released() {
        if !pg_enabled() {
            return;
        }
        let _guard = crate::test_support::pg_lock().lock().await;
        let (manager, mut other, _tmp) = pg_setup().await;
        assert!(try_lock(&mut other).await);
        unlock(&mut other).await;

        let report = manager.sync().await.unwrap();
        assert!(!report.skipped);
        assert_eq!(report.files_updated, 1);

        // The pass must release its lock again: the other session can re-take
        // it immediately, without waiting for any timeout.
        assert!(
            try_lock(&mut other).await,
            "sync must release the lock when done"
        );
        unlock(&mut other).await;
    }

    #[tokio::test]
    async fn postgres_sync_releases_lock_on_error() {
        if !pg_enabled() {
            return;
        }
        let _guard = crate::test_support::pg_lock().lock().await;
        let (manager, mut other, _tmp) = pg_setup().await;
        // Break the store so the sync pass returns Err after taking the lock.
        sqlx::query("DROP TABLE files CASCADE")
            .execute(&mut other)
            .await
            .unwrap();

        assert!(manager.sync().await.is_err());
        assert!(
            try_lock(&mut other).await,
            "error path must not leak the lock"
        );
        unlock(&mut other).await;
    }

    /// A task aborted while holding the lock must not strand it: abort drops
    /// the task's future, the guard's `Drop` closes the session, and the
    /// server releases the session advisory lock.
    ///
    /// Residual window not covered here: cancelling `try_acquire` *inside*
    /// the `pg_try_advisory_lock` await, between the server granting the lock
    /// and the bool arriving. That window is closed by construction (the
    /// guard is armed before the query runs, so the drop closes the session)
    /// rather than by this test, because forcing a mid-query abort would need
    /// a test-only hook inside sqlx's query path.
    #[tokio::test]
    async fn postgres_aborted_task_does_not_strand_the_lock() {
        if !pg_enabled() {
            return;
        }
        let _guard = crate::test_support::pg_lock().lock().await;
        let db = crate::test_support::pg_test_db().await.unwrap();
        let mut other = other_instance().await;

        let (acquired_tx, acquired_rx) = tokio::sync::oneshot::channel();
        let holder = tokio::spawn(async move {
            let guard = SyncLockGuard::try_acquire(&db).await.unwrap().unwrap();
            assert!(matches!(&guard, SyncLockGuard::Pg { .. }));
            let _ = acquired_tx.send(());
            // Hold the lock until the task is aborted.
            std::future::pending::<()>().await;
        });
        acquired_rx.await.unwrap();
        assert!(
            !try_lock(&mut other).await,
            "spawned task must hold the lock before it is aborted"
        );

        holder.abort();
        let _ = holder.await;

        // Drop closes the lock connection asynchronously; wait for the server
        // to notice and release the session lock.
        let mut reacquired = false;
        for _ in 0..50 {
            if try_lock(&mut other).await {
                reacquired = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        assert!(
            reacquired,
            "aborting the holder task must not leave the lock held"
        );
        unlock(&mut other).await;
    }

    #[tokio::test]
    async fn postgres_guard_drop_releases_lock_without_explicit_release() {
        if !pg_enabled() {
            return;
        }
        let _guard = crate::test_support::pg_lock().lock().await;
        let db = crate::test_support::pg_test_db().await.unwrap();
        let mut other = other_instance().await;

        let guard = SyncLockGuard::try_acquire(&db).await.unwrap().unwrap();
        assert!(!try_lock(&mut other).await, "lock must be held");
        drop(guard); // no release(): simulates panic/cancellation

        // Drop closes the lock connection asynchronously; wait for the server
        // to notice and release the session lock.
        let mut acquired = false;
        for _ in 0..50 {
            if try_lock(&mut other).await {
                acquired = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        assert!(acquired, "dropping the guard must not leak the lock");
        unlock(&mut other).await;
    }
}
