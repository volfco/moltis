//! Per-session key-value state store.
//!
//! Provides a database-backed KV store scoped to `(session_key, namespace, key)`
//! so that skills and extensions can persist context across messages.

use std::time::{SystemTime, UNIX_EPOCH};

use moltis_db::{Db, execute, fetch_all, fetch_optional};

use crate::Result;

/// A single state entry.
#[derive(Debug, Clone)]
pub struct StateEntry {
    pub namespace: String,
    pub key: String,
    pub value: String,
    pub updated_at: u64,
}

#[derive(sqlx::FromRow)]
struct StateRow {
    namespace: String,
    key: String,
    value: String,
    updated_at: i64,
}

impl From<StateRow> for StateEntry {
    fn from(r: StateRow) -> Self {
        Self {
            namespace: r.namespace,
            key: r.key,
            value: r.value,
            updated_at: r.updated_at as u64,
        }
    }
}

/// Database-backed per-session state store (SQLite or PostgreSQL).
pub struct SessionStateStore {
    db: Db,
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

impl SessionStateStore {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    /// Get a value by session key, namespace, and key.
    pub async fn get(
        &self,
        session_key: &str,
        namespace: &str,
        key: &str,
    ) -> Result<Option<String>> {
        let row = fetch_optional!(
            &self.db,
            (String,),
            "SELECT value FROM session_state WHERE session_key = ? AND namespace = ? AND key = ?",
            session_key,
            namespace,
            key
        )?
        .map(|(value,)| value);
        Ok(row)
    }

    /// Set a value. Inserts or updates the entry.
    pub async fn set(
        &self,
        session_key: &str,
        namespace: &str,
        key: &str,
        value: &str,
    ) -> Result<()> {
        let now = now_ms();
        execute!(
            &self.db,
            r#"INSERT INTO session_state (session_key, namespace, key, value, updated_at)
               VALUES (?, ?, ?, ?, ?)
               ON CONFLICT(session_key, namespace, key) DO UPDATE SET
                 value = excluded.value,
                 updated_at = excluded.updated_at"#,
            session_key,
            namespace,
            key,
            value,
            now
        )?;
        Ok(())
    }

    /// Delete a single key.
    pub async fn delete(&self, session_key: &str, namespace: &str, key: &str) -> Result<bool> {
        let rows = execute!(
            &self.db,
            "DELETE FROM session_state WHERE session_key = ? AND namespace = ? AND key = ?",
            session_key,
            namespace,
            key
        )?;
        Ok(rows > 0)
    }

    /// List all entries in a namespace for a session.
    pub async fn list(&self, session_key: &str, namespace: &str) -> Result<Vec<StateEntry>> {
        let rows = fetch_all!(
            &self.db,
            StateRow,
            "SELECT namespace, key, value, updated_at FROM session_state \
             WHERE session_key = ? AND namespace = ? ORDER BY key",
            session_key,
            namespace
        )?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    /// Delete all entries in a namespace for a session.
    pub async fn delete_all(&self, session_key: &str, namespace: &str) -> Result<u64> {
        let rows = execute!(
            &self.db,
            "DELETE FROM session_state WHERE session_key = ? AND namespace = ?",
            session_key,
            namespace
        )?;
        Ok(rows)
    }

    /// Delete all state for a session (cascade on session delete).
    pub async fn delete_session(&self, session_key: &str) -> Result<u64> {
        let rows = execute!(
            &self.db,
            "DELETE FROM session_state WHERE session_key = ?",
            session_key
        )?;
        Ok(rows)
    }
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use {super::*, crate::test_support::backend_test};

    async fn run_set_and_get(db: &Db) {
        let store = SessionStateStore::new(db.clone());

        store
            .set("session:1", "my-skill", "count", "42")
            .await
            .unwrap();
        let val = store.get("session:1", "my-skill", "count").await.unwrap();
        assert_eq!(val.as_deref(), Some("42"));
    }

    async fn run_get_missing(db: &Db) {
        let store = SessionStateStore::new(db.clone());

        let val = store.get("session:1", "ns", "missing").await.unwrap();
        assert!(val.is_none());
    }

    async fn run_set_overwrites(db: &Db) {
        let store = SessionStateStore::new(db.clone());

        store.set("s1", "ns", "k", "v1").await.unwrap();
        store.set("s1", "ns", "k", "v2").await.unwrap();
        let val = store.get("s1", "ns", "k").await.unwrap();
        assert_eq!(val.as_deref(), Some("v2"));
    }

    async fn run_delete(db: &Db) {
        let store = SessionStateStore::new(db.clone());

        store.set("s1", "ns", "k", "v").await.unwrap();
        let deleted = store.delete("s1", "ns", "k").await.unwrap();
        assert!(deleted);
        assert!(store.get("s1", "ns", "k").await.unwrap().is_none());
    }

    async fn run_delete_missing(db: &Db) {
        let store = SessionStateStore::new(db.clone());

        let deleted = store.delete("s1", "ns", "k").await.unwrap();
        assert!(!deleted);
    }

    async fn run_list(db: &Db) {
        let store = SessionStateStore::new(db.clone());

        store.set("s1", "ns", "a", "1").await.unwrap();
        store.set("s1", "ns", "b", "2").await.unwrap();
        store.set("s1", "other", "c", "3").await.unwrap();

        let entries = store.list("s1", "ns").await.unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].key, "a");
        assert_eq!(entries[1].key, "b");
    }

    async fn run_delete_all(db: &Db) {
        let store = SessionStateStore::new(db.clone());

        store.set("s1", "ns", "a", "1").await.unwrap();
        store.set("s1", "ns", "b", "2").await.unwrap();
        store.set("s1", "other", "c", "3").await.unwrap();

        let count = store.delete_all("s1", "ns").await.unwrap();
        assert_eq!(count, 2);
        // "other" namespace untouched.
        assert!(store.get("s1", "other", "c").await.unwrap().is_some());
    }

    async fn run_delete_session(db: &Db) {
        let store = SessionStateStore::new(db.clone());

        store.set("s1", "ns1", "a", "1").await.unwrap();
        store.set("s1", "ns2", "b", "2").await.unwrap();
        store.set("s2", "ns1", "a", "3").await.unwrap();

        let count = store.delete_session("s1").await.unwrap();
        assert_eq!(count, 2);
        // s2 untouched.
        assert!(store.get("s2", "ns1", "a").await.unwrap().is_some());
    }

    async fn run_namespace_isolation(db: &Db) {
        let store = SessionStateStore::new(db.clone());

        store.set("s1", "ns-a", "key", "val-a").await.unwrap();
        store.set("s1", "ns-b", "key", "val-b").await.unwrap();

        assert_eq!(
            store.get("s1", "ns-a", "key").await.unwrap().as_deref(),
            Some("val-a")
        );
        assert_eq!(
            store.get("s1", "ns-b", "key").await.unwrap().as_deref(),
            Some("val-b")
        );
    }

    async fn run_session_isolation(db: &Db) {
        let store = SessionStateStore::new(db.clone());

        store.set("s1", "ns", "key", "val-1").await.unwrap();
        store.set("s2", "ns", "key", "val-2").await.unwrap();

        assert_eq!(
            store.get("s1", "ns", "key").await.unwrap().as_deref(),
            Some("val-1")
        );
        assert_eq!(
            store.get("s2", "ns", "key").await.unwrap().as_deref(),
            Some("val-2")
        );
    }

    backend_test!(test_set_and_get, test_postgres_set_and_get, run_set_and_get);
    backend_test!(test_get_missing, test_postgres_get_missing, run_get_missing);
    backend_test!(
        test_set_overwrites,
        test_postgres_set_overwrites,
        run_set_overwrites
    );
    backend_test!(test_delete, test_postgres_delete, run_delete);
    backend_test!(
        test_delete_missing,
        test_postgres_delete_missing,
        run_delete_missing
    );
    backend_test!(test_list, test_postgres_list, run_list);
    backend_test!(test_delete_all, test_postgres_delete_all, run_delete_all);
    backend_test!(
        test_delete_session,
        test_postgres_delete_session,
        run_delete_session
    );
    backend_test!(
        test_namespace_isolation,
        test_postgres_namespace_isolation,
        run_namespace_isolation
    );
    backend_test!(
        test_session_isolation,
        test_postgres_session_isolation,
        run_session_isolation
    );
}
