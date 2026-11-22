//! Dual-backend cron store (SQLite or PostgreSQL) using moltis-db.

use {
    async_trait::async_trait,
    moltis_db::{Db, execute, fetch_all},
};

use crate::{
    Error, Result,
    store::CronStore,
    types::{CronJob, CronRunRecord},
};

/// Database-backed persistence for cron jobs and run history.
pub struct DbStore {
    db: Db,
}

impl DbStore {
    /// Create a new store with its own connection pool and run migrations.
    ///
    /// Use this for standalone cron databases. For shared pools (e.g., moltis.db),
    /// use [`DbStore::with_pool`] after calling [`crate::run_migrations`].
    /// The URL scheme (`sqlite:` / `postgres:`) picks the backend.
    pub async fn new(database_url: &str) -> Result<Self> {
        let options = moltis_db::DbOptions {
            url: database_url.into(),
            max_connections: 5,
            foreign_keys: true,
        };
        let db = Db::connect(&options)
            .await
            .map_err(|source| Error::external("failed to connect to database", source))?;

        crate::run_migrations(&db).await?;

        Ok(Self { db })
    }

    /// Create a store using an existing pool (migrations must already be run).
    ///
    /// Call [`crate::run_migrations`] before using this constructor.
    pub fn with_pool(db: Db) -> Self {
        Self { db }
    }
}

/// Row type for `cron_runs`; both backends decode the same Rust types.
#[derive(sqlx::FromRow)]
struct RunRow {
    job_id: String,
    started_at_ms: i64,
    finished_at_ms: i64,
    status: String,
    error: Option<String>,
    duration_ms: i64,
    output: Option<String>,
    input_tokens: Option<i64>,
    output_tokens: Option<i64>,
    session_key: Option<String>,
}

/// Convert raw `cron_runs` rows into `CronRunRecord` values.
fn runs_to_records(rows: Vec<RunRow>) -> Result<Vec<CronRunRecord>> {
    rows.into_iter()
        .map(|row| {
            Ok(CronRunRecord {
                job_id: row.job_id,
                started_at_ms: row.started_at_ms as u64,
                finished_at_ms: row.finished_at_ms as u64,
                status: serde_json::from_str(&row.status)?,
                error: row.error,
                duration_ms: row.duration_ms as u64,
                output: row.output,
                input_tokens: row.input_tokens.map(|v| v as u64),
                output_tokens: row.output_tokens.map(|v| v as u64),
                session_key: row.session_key,
            })
        })
        .collect()
}

#[async_trait]
impl CronStore for DbStore {
    async fn load_jobs(&self) -> Result<Vec<CronJob>> {
        let rows = fetch_all!(&self.db, (String,), "SELECT data FROM cron_jobs")?;

        let mut jobs = Vec::with_capacity(rows.len());
        for (data,) in rows {
            let job: CronJob = serde_json::from_str(&data)?;
            jobs.push(job);
        }
        Ok(jobs)
    }

    async fn save_job(&self, job: &CronJob) -> Result<()> {
        let data = serde_json::to_string(job)?;
        execute!(
            &self.db,
            "INSERT INTO cron_jobs (id, data) VALUES (?, ?)
             ON CONFLICT(id) DO UPDATE SET data = excluded.data",
            &job.id,
            &data,
        )?;
        Ok(())
    }

    async fn delete_job(&self, id: &str) -> Result<()> {
        let affected = execute!(&self.db, "DELETE FROM cron_jobs WHERE id = ?", id)?;
        if affected == 0 {
            return Err(Error::job_not_found(id));
        }
        Ok(())
    }

    async fn update_job(&self, job: &CronJob) -> Result<()> {
        let data = serde_json::to_string(job)?;
        let affected = execute!(
            &self.db,
            "UPDATE cron_jobs SET data = ? WHERE id = ?",
            &data,
            &job.id,
        )?;
        if affected == 0 {
            return Err(Error::job_not_found(job.id.clone()));
        }
        Ok(())
    }

    async fn append_run(&self, job_id: &str, run: &CronRunRecord) -> Result<()> {
        let status = serde_json::to_string(&run.status)?;
        execute!(
            &self.db,
            "INSERT INTO cron_runs (job_id, started_at_ms, finished_at_ms, status, error, duration_ms, output, input_tokens, output_tokens, session_key)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            job_id,
            run.started_at_ms as i64,
            run.finished_at_ms as i64,
            &status,
            &run.error,
            run.duration_ms as i64,
            &run.output,
            run.input_tokens.map(|v| v as i64),
            run.output_tokens.map(|v| v as i64),
            &run.session_key,
        )?;
        Ok(())
    }

    async fn get_runs(&self, job_id: &str, limit: usize) -> Result<Vec<CronRunRecord>> {
        let rows = fetch_all!(
            &self.db,
            RunRow,
            "SELECT job_id, started_at_ms, finished_at_ms, status, error, duration_ms, output, input_tokens, output_tokens, session_key
             FROM cron_runs
             WHERE job_id = ?
             ORDER BY started_at_ms DESC
             LIMIT ?",
            job_id,
            limit as i64,
        )?;

        let mut runs = runs_to_records(rows)?;
        // Reverse so oldest first (consistent with other stores).
        runs.reverse();
        Ok(runs)
    }

    async fn prune_runs_before(&self, before_ms: u64) -> Result<u64> {
        let affected = execute!(
            &self.db,
            "DELETE FROM cron_runs WHERE started_at_ms < ?",
            before_ms as i64,
        )?;
        Ok(affected)
    }

    async fn list_session_keys_before(&self, before_ms: u64) -> Result<Vec<String>> {
        let rows = fetch_all!(
            &self.db,
            (String,),
            "SELECT DISTINCT session_key FROM cron_runs WHERE session_key IS NOT NULL AND started_at_ms < ?",
            before_ms as i64,
        )?;
        Ok(rows.into_iter().map(|(key,)| key).collect())
    }
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use {super::*, crate::types::*};

    async fn sqlite_store() -> DbStore {
        DbStore::new("sqlite::memory:").await.unwrap()
    }

    /// PostgreSQL store for one test, serialized and freshly migrated.
    /// The guard keeps the crate lock until the test ends.
    async fn pg_store() -> Option<(tokio::sync::MutexGuard<'static, ()>, DbStore)> {
        if std::env::var("TEST_POSTGRES_URL").is_err() {
            eprintln!("skipping: TEST_POSTGRES_URL is not set");
            return None;
        }
        let guard = crate::test_support::pg_lock().lock().await;
        let db = crate::test_support::pg_test_db().await?;
        Some((guard, DbStore::with_pool(db)))
    }

    fn make_job(id: &str) -> CronJob {
        CronJob {
            id: id.into(),
            name: format!("job-{id}"),
            enabled: true,
            delete_after_run: false,
            schedule: CronSchedule::At { at_ms: 1000 },
            payload: CronPayload::SystemEvent { text: "hi".into() },
            session_target: SessionTarget::Main,
            state: CronJobState::default(),
            sandbox: CronSandboxConfig::default(),
            wake_mode: CronWakeMode::default(),
            system: false,
            created_at_ms: 1000,
            updated_at_ms: 1000,
        }
    }

    fn make_run(job_id: &str, started_at_ms: u64, session_key: Option<String>) -> CronRunRecord {
        CronRunRecord {
            job_id: job_id.into(),
            started_at_ms,
            finished_at_ms: started_at_ms + 500,
            status: RunStatus::Ok,
            error: None,
            duration_ms: 500,
            output: None,
            input_tokens: None,
            output_tokens: None,
            session_key,
        }
    }

    async fn roundtrip_body(store: &DbStore) {
        store.save_job(&make_job("1")).await.unwrap();
        store.save_job(&make_job("2")).await.unwrap();

        let jobs = store.load_jobs().await.unwrap();
        assert_eq!(jobs.len(), 2);
    }

    async fn upsert_body(store: &DbStore) {
        store.save_job(&make_job("1")).await.unwrap();

        let mut job = make_job("1");
        job.name = "updated".into();
        store.save_job(&job).await.unwrap();

        let jobs = store.load_jobs().await.unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].name, "updated");
    }

    async fn delete_body(store: &DbStore) {
        store.save_job(&make_job("1")).await.unwrap();
        store.delete_job("1").await.unwrap();
        assert!(store.load_jobs().await.unwrap().is_empty());
    }

    async fn delete_not_found_body(store: &DbStore) {
        assert!(store.delete_job("nope").await.is_err());
    }

    async fn update_body(store: &DbStore) {
        store.save_job(&make_job("1")).await.unwrap();

        let mut job = make_job("1");
        job.name = "patched".into();
        store.update_job(&job).await.unwrap();

        let jobs = store.load_jobs().await.unwrap();
        assert_eq!(jobs[0].name, "patched");
    }

    async fn runs_body(store: &DbStore) {
        store.save_job(&make_job("j1")).await.unwrap();

        for i in 0..5 {
            store
                .append_run("j1", &make_run("j1", i * 1000, None))
                .await
                .unwrap();
        }

        let runs = store.get_runs("j1", 3).await.unwrap();
        assert_eq!(runs.len(), 3);
        // Should be the last 3, in chronological order
        assert_eq!(runs[0].started_at_ms, 2000);
        assert_eq!(runs[2].started_at_ms, 4000);
    }

    async fn runs_empty_body(store: &DbStore) {
        let runs = store.get_runs("none", 10).await.unwrap();
        assert!(runs.is_empty());
    }

    async fn prune_runs_before_body(store: &DbStore) {
        store.save_job(&make_job("j1")).await.unwrap();

        for i in 0..5 {
            store
                .append_run("j1", &make_run("j1", i * 1000, Some(format!("cron:{i}"))))
                .await
                .unwrap();
        }

        // Prune runs started before 3000ms — should remove runs at 0, 1000, 2000.
        let pruned = store.prune_runs_before(3000).await.unwrap();
        assert_eq!(pruned, 3);

        let remaining = store.get_runs("j1", 10).await.unwrap();
        assert_eq!(remaining.len(), 2);
        assert_eq!(remaining[0].started_at_ms, 3000);
    }

    async fn list_session_keys_before_body(store: &DbStore) {
        store.save_job(&make_job("j1")).await.unwrap();

        for i in 0..4 {
            let key = (i < 3).then(|| format!("cron:sess-{i}"));
            store
                .append_run("j1", &make_run("j1", i * 1000, key))
                .await
                .unwrap();
        }

        let keys = store.list_session_keys_before(2000).await.unwrap();
        assert_eq!(keys.len(), 2);
        assert!(keys.contains(&"cron:sess-0".to_string()));
        assert!(keys.contains(&"cron:sess-1".to_string()));
    }

    async fn run_record_with_session_key_body(store: &DbStore) {
        store.save_job(&make_job("j1")).await.unwrap();

        store
            .append_run("j1", &make_run("j1", 1000, Some("cron:abc-123".into())))
            .await
            .unwrap();

        let runs = store.get_runs("j1", 10).await.unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].session_key.as_deref(), Some("cron:abc-123"));
    }

    #[tokio::test]
    async fn test_sqlite_roundtrip() {
        roundtrip_body(&sqlite_store().await).await;
    }

    #[tokio::test]
    async fn test_postgres_roundtrip() {
        let Some((_guard, store)) = pg_store().await else {
            return;
        };
        roundtrip_body(&store).await;
    }

    #[tokio::test]
    async fn test_sqlite_upsert() {
        upsert_body(&sqlite_store().await).await;
    }

    #[tokio::test]
    async fn test_postgres_upsert() {
        let Some((_guard, store)) = pg_store().await else {
            return;
        };
        upsert_body(&store).await;
    }

    #[tokio::test]
    async fn test_sqlite_delete() {
        delete_body(&sqlite_store().await).await;
    }

    #[tokio::test]
    async fn test_postgres_delete() {
        let Some((_guard, store)) = pg_store().await else {
            return;
        };
        delete_body(&store).await;
    }

    #[tokio::test]
    async fn test_sqlite_delete_not_found() {
        delete_not_found_body(&sqlite_store().await).await;
    }

    #[tokio::test]
    async fn test_postgres_delete_not_found() {
        let Some((_guard, store)) = pg_store().await else {
            return;
        };
        delete_not_found_body(&store).await;
    }

    #[tokio::test]
    async fn test_sqlite_update() {
        update_body(&sqlite_store().await).await;
    }

    #[tokio::test]
    async fn test_postgres_update() {
        let Some((_guard, store)) = pg_store().await else {
            return;
        };
        update_body(&store).await;
    }

    #[tokio::test]
    async fn test_sqlite_runs() {
        runs_body(&sqlite_store().await).await;
    }

    #[tokio::test]
    async fn test_postgres_runs() {
        let Some((_guard, store)) = pg_store().await else {
            return;
        };
        runs_body(&store).await;
    }

    #[tokio::test]
    async fn test_sqlite_runs_empty() {
        runs_empty_body(&sqlite_store().await).await;
    }

    #[tokio::test]
    async fn test_postgres_runs_empty() {
        let Some((_guard, store)) = pg_store().await else {
            return;
        };
        runs_empty_body(&store).await;
    }

    #[tokio::test]
    async fn test_sqlite_prune_runs_before() {
        prune_runs_before_body(&sqlite_store().await).await;
    }

    #[tokio::test]
    async fn test_postgres_prune_runs_before() {
        let Some((_guard, store)) = pg_store().await else {
            return;
        };
        prune_runs_before_body(&store).await;
    }

    #[tokio::test]
    async fn test_sqlite_list_session_keys_before() {
        list_session_keys_before_body(&sqlite_store().await).await;
    }

    #[tokio::test]
    async fn test_postgres_list_session_keys_before() {
        let Some((_guard, store)) = pg_store().await else {
            return;
        };
        list_session_keys_before_body(&store).await;
    }

    #[tokio::test]
    async fn test_sqlite_run_record_with_session_key() {
        run_record_with_session_key_body(&sqlite_store().await).await;
    }

    #[tokio::test]
    async fn test_postgres_run_record_with_session_key() {
        let Some((_guard, store)) = pg_store().await else {
            return;
        };
        run_record_with_session_key_body(&store).await;
    }
}
