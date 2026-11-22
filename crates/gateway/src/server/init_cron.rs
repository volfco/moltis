//! Cron store wiring over the deployment database.
//!
//! The SQL store is the production home for cron jobs (`moltis_cron`'s
//! migrations run against the deployment database at startup). One-time
//! import: jobs written by the legacy JSON `FileStore` are moved into the SQL
//! store and `jobs.json` is renamed to `jobs.json.bak`, mirroring the
//! `projects.toml` import in `prepare_core`.

use std::{path::Path, sync::Arc};

use moltis_cron::store::CronStore;

/// Create the cron store on the deployment database, importing legacy
/// `FileStore` data once.
pub(crate) async fn init_cron_store(db: moltis_db::Db, data_dir: &Path) -> Arc<dyn CronStore> {
    let store: Arc<dyn CronStore> = Arc::new(moltis_cron::store_db::DbStore::with_pool(db));
    if let Err(error) = import_legacy_jobs(store.as_ref(), data_dir).await {
        // The legacy file stays in place so no job is lost; the next start
        // retries the import.
        tracing::warn!(%error, "cron: legacy jobs.json import failed; leaving the file in place");
    }
    store
}

/// Import jobs and their run history from the legacy JSON `FileStore`.
///
/// Renames `jobs.json` to `jobs.json.bak` after a complete import so a later
/// start does not re-import. On any failure the file is left untouched.
async fn import_legacy_jobs(target: &dyn CronStore, data_dir: &Path) -> anyhow::Result<()> {
    let base = data_dir.join("cron");
    let jobs_path = base.join("jobs.json");
    if !jobs_path.exists() {
        return Ok(());
    }
    let source = moltis_cron::store_file::FileStore::new(jobs_path.clone(), base.join("runs"));
    let jobs = source.load_jobs().await?;
    let mut imported = 0_usize;
    for job in &jobs {
        target.save_job(job).await?;
        // Re-imports (after a partial failure) must not duplicate run
        // history: clear this job's runs before re-appending them.
        target.delete_runs_for_job(&job.id).await?;
        for run in source.get_runs(&job.id, usize::MAX).await? {
            target.append_run(&job.id, &run).await?;
        }
        imported += 1;
    }
    std::fs::rename(&jobs_path, jobs_path.with_extension("json.bak"))?;
    tracing::info!(
        count = imported,
        "cron: imported legacy jobs.json into the database"
    );
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use moltis_cron::types::{
        CronJob, CronJobState, CronPayload, CronRunRecord, CronSandboxConfig, CronSchedule,
        CronWakeMode, RunStatus, SessionTarget,
    };

    use super::*;

    fn sample_job(id: &str) -> CronJob {
        CronJob {
            id: id.into(),
            name: "Nightly".into(),
            enabled: true,
            delete_after_run: false,
            schedule: CronSchedule::Cron {
                expr: "0 3 * * *".into(),
                tz: Some("UTC".into()),
            },
            payload: CronPayload::SystemEvent {
                text: "ping".into(),
            },
            session_target: SessionTarget::Named("heartbeat".into()),
            state: CronJobState {
                next_run_at_ms: Some(1_700_000_100_000),
                last_status: Some(RunStatus::Ok),
                last_duration_ms: Some(1_000),
                ..CronJobState::default()
            },
            sandbox: CronSandboxConfig::default(),
            wake_mode: CronWakeMode::Now,
            system: true,
            created_at_ms: 1_700_000_000_000,
            updated_at_ms: 1_700_000_000_001,
        }
    }

    fn sample_run(job_id: &str) -> CronRunRecord {
        CronRunRecord {
            job_id: job_id.into(),
            started_at_ms: 1_700_000_001_000,
            finished_at_ms: 1_700_000_002_000,
            status: RunStatus::Ok,
            error: None,
            duration_ms: 1_000,
            output: Some("done".into()),
            input_tokens: Some(3),
            output_tokens: Some(5),
            session_key: Some(format!("cron:{job_id}")),
        }
    }

    async fn sql_store() -> moltis_cron::store_db::DbStore {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        let db = moltis_db::Db::from(pool);
        moltis_cron::run_migrations(&db).await.unwrap();
        moltis_cron::store_db::DbStore::with_pool(db)
    }

    #[tokio::test]
    async fn import_moves_jobs_and_runs_and_renames_jobs_json() {
        let dir = tempfile::tempdir().unwrap();
        let source = moltis_cron::store_file::FileStore::new(
            dir.path().join("cron/jobs.json"),
            dir.path().join("cron/runs"),
        );
        let job = sample_job("job-1");
        let run = sample_run("job-1");
        source.save_job(&job).await.unwrap();
        source.append_run("job-1", &run).await.unwrap();

        let target = sql_store().await;
        import_legacy_jobs(&target, dir.path()).await.unwrap();

        assert_eq!(target.load_jobs().await.unwrap(), vec![job.clone()]);
        assert_eq!(target.get_runs("job-1", 10).await.unwrap(), vec![run]);
        assert!(!dir.path().join("cron/jobs.json").exists());
        assert!(dir.path().join("cron/jobs.json.bak").exists());
    }

    #[tokio::test]
    async fn import_retry_after_partial_failure_does_not_duplicate_runs() {
        let dir = tempfile::tempdir().unwrap();
        let source = moltis_cron::store_file::FileStore::new(
            dir.path().join("cron/jobs.json"),
            dir.path().join("cron/runs"),
        );
        let job = sample_job("job-1");
        let run = sample_run("job-1");
        source.save_job(&job).await.unwrap();
        source.append_run("job-1", &run).await.unwrap();

        let target = sql_store().await;
        import_legacy_jobs(&target, dir.path()).await.unwrap();

        // Simulate a partial failure: the rename never happened, so the next
        // startup finds jobs.json again and re-imports the same data.
        std::fs::rename(
            dir.path().join("cron/jobs.json.bak"),
            dir.path().join("cron/jobs.json"),
        )
        .unwrap();
        import_legacy_jobs(&target, dir.path()).await.unwrap();

        assert_eq!(target.load_jobs().await.unwrap(), vec![job.clone()]);
        assert_eq!(
            target.get_runs("job-1", 10).await.unwrap(),
            vec![run],
            "a re-import must not duplicate run history"
        );
    }

    #[tokio::test]
    async fn import_without_jobs_json_is_a_no_op() {
        let dir = tempfile::tempdir().unwrap();
        let target = sql_store().await;
        import_legacy_jobs(&target, dir.path()).await.unwrap();
        assert!(target.load_jobs().await.unwrap().is_empty());
    }
}
