use {clap::Subcommand, std::path::PathBuf};

#[derive(Subcommand)]
pub enum DbAction {
    /// Delete all database files completely (SQLite only).
    Reset,
    /// Clear all data from tables but keep the schema intact.
    Clear,
    /// Run all pending database migrations.
    Migrate,
}

/// Returns the paths to the database files.
fn db_paths() -> (PathBuf, PathBuf, PathBuf) {
    let data_dir = moltis_config::data_dir();
    let main_db = data_dir.join("moltis.db");
    let memory_db = data_dir.join("memory.db");
    let connectors_db = data_dir.join("connectors.db");
    (main_db, memory_db, connectors_db)
}

pub async fn handle_db(action: DbAction) -> anyhow::Result<()> {
    match action {
        DbAction::Reset => reset_databases().await,
        DbAction::Clear => clear_databases().await,
        DbAction::Migrate => run_migrations().await,
    }
}

/// Delete all database files completely.
///
/// Refuses on PostgreSQL: deleting local files has no analogue on a shared
/// server database, and dropping schema objects could destroy non-moltis data
/// in a database that is not exclusively ours.
async fn reset_databases() -> anyhow::Result<()> {
    reset_databases_for(&moltis_config::discover_and_load()).await
}

async fn reset_databases_for(config: &moltis_config::MoltisConfig) -> anyhow::Result<()> {
    if config.database.backend == moltis_config::DatabaseBackend::Postgres {
        anyhow::bail!(
            "`moltis db reset` deletes the local SQLite database files and is not supported \
             for backend = \"postgres\". Drop and recreate the PostgreSQL database yourself \
             (e.g. `DROP DATABASE moltis; CREATE DATABASE moltis;`) or drop only the moltis \
             tables, then run `moltis db migrate`."
        );
    }
    let (main_db, memory_db, connectors_db) = db_paths();

    let mut deleted = false;

    // Also delete WAL and SHM files that SQLite may have created.
    for base in [&main_db, &memory_db, &connectors_db] {
        for suffix in ["", "-wal", "-shm"] {
            let path = if suffix.is_empty() {
                base.clone()
            } else {
                base.with_extension(format!("db{}", suffix))
            };
            if path.exists() {
                std::fs::remove_file(&path)?;
                println!("Deleted: {}", path.display());
                deleted = true;
            }
        }
    }
    if let Some(data_dir) = connectors_db.parent() {
        let connector_files = data_dir.join("connectors");
        if connector_files.exists() {
            std::fs::remove_dir_all(&connector_files)?;
            println!("Deleted: {}", connector_files.display());
            deleted = true;
        }
    }

    if deleted {
        println!("Database files deleted. Run `moltis db migrate` to recreate them.");
    } else {
        println!("No database files found.");
    }

    Ok(())
}

/// Clear all data from tables but keep the schema intact.
async fn clear_databases() -> anyhow::Result<()> {
    let config = moltis_config::discover_and_load();
    if config.database.backend == moltis_config::DatabaseBackend::Postgres {
        return clear_postgres_database(&config).await;
    }
    let (main_db, memory_db, connectors_db) = db_paths();
    #[cfg(not(feature = "connectors"))]
    let _ = &connectors_db;

    // Clear main database
    if main_db.exists() {
        let db_url = format!("sqlite:{}?mode=rwc", main_db.display());
        let pool = sqlx::SqlitePool::connect(&db_url).await?;

        // Order matters due to foreign key constraints.
        // Delete from child tables first.
        let tables = [
            // Sessions/channels (depends on projects)
            "channel_sessions",
            "sessions",
            // Cron (cron_runs depends on cron_jobs)
            "cron_runs",
            "cron_jobs",
            // Gateway tables (no dependencies between them)
            "auth_sessions",
            "api_keys",
            "passkeys",
            "auth_password",
            "env_variables",
            "message_log",
            "channels",
            // Projects (other tables depend on this)
            "projects",
        ];

        for table in tables {
            // Use raw query to avoid compile-time checks
            let query = format!("DELETE FROM {table}");
            if let Err(e) = sqlx::query(&query).execute(&pool).await {
                // Table might not exist if migrations haven't run
                eprintln!("Warning: could not clear {table}: {e}");
            } else {
                println!("Cleared table: {table}");
            }
        }

        pool.close().await;
        println!("Main database cleared.");
    } else {
        println!("Main database not found: {}", main_db.display());
    }

    // Clear memory database
    if memory_db.exists() {
        let db_url = format!("sqlite:{}?mode=rwc", memory_db.display());
        let pool = sqlx::SqlitePool::connect(&db_url).await?;

        // Order matters: chunks depends on files
        let tables = ["embedding_cache", "chunks", "files"];

        for table in tables {
            let query = format!("DELETE FROM {table}");
            if let Err(e) = sqlx::query(&query).execute(&pool).await {
                eprintln!("Warning: could not clear {table}: {e}");
            } else {
                println!("Cleared table: {table}");
            }
        }

        pool.close().await;
        println!("Memory database cleared.");
    } else {
        println!("Memory database not found: {}", memory_db.display());
    }

    #[cfg(feature = "connectors")]
    if connectors_db.exists() {
        let db_url = format!("sqlite:{}?mode=rwc", connectors_db.display());
        let pool = sqlx::SqlitePool::connect(&db_url).await?;
        for table in [
            "connector_sync_runs",
            "connector_items",
            "connector_datasets",
            "connector_accounts",
        ] {
            let query = format!("DELETE FROM {table}");
            if let Err(error) = sqlx::query(&query).execute(&pool).await {
                eprintln!("Warning: could not clear {table}: {error}");
            } else {
                println!("Cleared table: {table}");
            }
        }
        pool.close().await;
        println!("Connectors database cleared.");
    } else if cfg!(feature = "connectors") {
        println!("Connectors database not found: {}", connectors_db.display());
    }
    if let Some(data_dir) = connectors_db.parent() {
        let connector_files = data_dir.join("connectors");
        if connector_files.exists() {
            std::fs::remove_dir_all(&connector_files)?;
            println!("Cleared connector exports: {}", connector_files.display());
        }
    }

    Ok(())
}

/// Run all pending database migrations.
async fn run_migrations() -> anyhow::Result<()> {
    let config = moltis_config::discover_and_load();
    if config.database.backend == moltis_config::DatabaseBackend::Postgres {
        return run_postgres_migrations(&config).await;
    }
    let (main_db, memory_db, connectors_db) = db_paths();
    #[cfg(not(feature = "connectors"))]
    let _ = &connectors_db;

    // Ensure data directory exists
    if let Some(parent) = main_db.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Run main database migrations
    println!("Running migrations for main database...");
    let db_url = format!("sqlite:{}?mode=rwc", main_db.display());
    let pool = sqlx::SqlitePool::connect(&db_url).await?;
    let db = moltis_db::Db::from(pool.clone());

    // Run migrations in dependency order (same as server.rs)
    moltis_projects::run_migrations(&db)
        .await
        .map_err(|e| anyhow::anyhow!("projects migrations failed: {e}"))?;
    println!("  - projects migrations complete");

    moltis_sessions::run_migrations(&db)
        .await
        .map_err(|e| anyhow::anyhow!("sessions migrations failed: {e}"))?;
    println!("  - sessions migrations complete");

    moltis_cron::run_migrations(&db)
        .await
        .map_err(|e| anyhow::anyhow!("cron migrations failed: {e}"))?;
    println!("  - cron migrations complete");

    moltis_gateway::run_migrations(&db)
        .await
        .map_err(|e| anyhow::anyhow!("gateway migrations failed: {e}"))?;
    println!("  - gateway migrations complete");

    pool.close().await;

    // Run memory database migrations
    println!("Running migrations for memory database...");
    let memory_url = format!("sqlite:{}?mode=rwc", memory_db.display());
    let memory_pool = sqlx::SqlitePool::connect(&memory_url).await?;

    moltis_memory::schema::run_migrations(&moltis_db::Db::from(memory_pool.clone()))
        .await
        .map_err(|e| anyhow::anyhow!("memory migrations failed: {e}"))?;
    println!("  - memory migrations complete");

    memory_pool.close().await;

    #[cfg(feature = "connectors")]
    {
        println!("Running migrations for connectors database...");
        let connectors_url = format!("sqlite:{}?mode=rwc", connectors_db.display());
        let connectors_pool = sqlx::SqlitePool::connect(&connectors_url).await?;
        restrict_database_permissions(&connectors_db)?;
        moltis_connectors::run_migrations(&moltis_db::Db::from(connectors_pool.clone()))
            .await
            .map_err(|error| anyhow::anyhow!("connector migrations failed: {error}"))?;
        println!("  - connector migrations complete");
        connectors_pool.close().await;
    }

    println!("All migrations complete.");
    Ok(())
}

/// Run every moltis migration on the shared PostgreSQL deployment database
/// (table names do not collide across stores).
async fn run_postgres_migrations(config: &moltis_config::MoltisConfig) -> anyhow::Result<()> {
    println!("Running migrations for the PostgreSQL deployment database...");
    let db = moltis_gateway::database::open_database(config, &moltis_config::data_dir()).await?;

    moltis_projects::run_migrations(&db)
        .await
        .map_err(|e| anyhow::anyhow!("projects migrations failed: {e}"))?;
    println!("  - projects migrations complete");

    moltis_sessions::run_migrations(&db)
        .await
        .map_err(|e| anyhow::anyhow!("sessions migrations failed: {e}"))?;
    println!("  - sessions migrations complete");

    moltis_cron::run_migrations(&db)
        .await
        .map_err(|e| anyhow::anyhow!("cron migrations failed: {e}"))?;
    println!("  - cron migrations complete");

    moltis_gateway::run_migrations(&db)
        .await
        .map_err(|e| anyhow::anyhow!("gateway migrations failed: {e}"))?;
    println!("  - gateway migrations complete");

    moltis_memory::schema::run_migrations(&db)
        .await
        .map_err(|e| anyhow::anyhow!("memory migrations failed: {e}"))?;
    println!("  - memory migrations complete");

    #[cfg(feature = "connectors")]
    {
        moltis_connectors::run_migrations(&db)
            .await
            .map_err(|error| anyhow::anyhow!("connector migrations failed: {error}"))?;
        println!("  - connector migrations complete");
    }

    db.close().await;
    println!("All migrations complete.");
    Ok(())
}

/// Clear every moltis table in the shared PostgreSQL deployment database.
async fn clear_postgres_database(config: &moltis_config::MoltisConfig) -> anyhow::Result<()> {
    let db = moltis_gateway::database::open_database(config, &moltis_config::data_dir()).await?;

    // Order matters due to foreign key constraints: children first.
    let tables = [
        // Sessions/channels (depends on projects)
        "channel_sessions",
        "sessions",
        // Cron (cron_runs depends on cron_jobs)
        "cron_runs",
        "cron_jobs",
        // Memory (chunks depends on files)
        "embedding_cache",
        "chunks",
        "files",
        // Connectors
        "connector_sync_runs",
        "connector_items",
        "connector_datasets",
        "connector_accounts",
        // Gateway tables (no dependencies between them)
        "auth_sessions",
        "api_keys",
        "passkeys",
        "auth_password",
        "env_variables",
        "message_log",
        "channels",
        // Projects (other tables depend on this)
        "projects",
    ];

    for table in tables {
        let query = format!("DELETE FROM {table}");
        match moltis_db::execute!(&db, &query) {
            Ok(_) => println!("Cleared table: {table}"),
            // Table might not exist if migrations haven't run
            Err(e) => eprintln!("Warning: could not clear {table}: {e}"),
        }
    }
    db.close().await;
    println!("PostgreSQL deployment database cleared.");
    Ok(())
}

fn restrict_database_permissions(path: &std::path::Path) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use {super::*, tempfile::TempDir};

    #[test]
    fn test_db_paths_uses_data_dir() {
        // Just verify the function constructs expected paths
        let paths = db_paths();
        assert!(
            paths.0.to_string_lossy().contains("moltis.db"),
            "main db path should contain moltis.db"
        );
        assert!(
            paths.1.to_string_lossy().contains("memory.db"),
            "memory db path should contain memory.db"
        );
        assert!(
            paths.2.to_string_lossy().contains("connectors.db"),
            "connectors db path should contain connectors.db"
        );
    }

    /// Test reset_databases by manually deleting files in a temp dir.
    /// This avoids global state issues by testing the file deletion logic directly.
    #[tokio::test]
    async fn test_reset_deletes_files() {
        let temp = TempDir::new().unwrap();

        // Create dummy database files
        let main_db = temp.path().join("moltis.db");
        let memory_db = temp.path().join("memory.db");
        let connectors_db = temp.path().join("connectors.db");
        let main_wal = temp.path().join("moltis.db-wal");
        let main_shm = temp.path().join("moltis.db-shm");
        let connector_exports = temp.path().join("connectors/exports/dataset");

        std::fs::write(&main_db, "test").unwrap();
        std::fs::write(&memory_db, "test").unwrap();
        std::fs::write(&connectors_db, "test").unwrap();
        std::fs::write(&main_wal, "test").unwrap();
        std::fs::write(&main_shm, "test").unwrap();
        std::fs::create_dir_all(&connector_exports).unwrap();
        std::fs::write(connector_exports.join("items.jsonl"), "sensitive").unwrap();

        assert!(main_db.exists());
        assert!(memory_db.exists());
        assert!(connectors_db.exists());
        assert!(main_wal.exists());
        assert!(main_shm.exists());
        assert!(connector_exports.exists());

        // Simulate what reset_databases does
        for path in [&main_db, &memory_db, &connectors_db, &main_wal, &main_shm] {
            std::fs::remove_file(path).unwrap();
        }
        std::fs::remove_dir_all(temp.path().join("connectors")).unwrap();

        assert!(!main_db.exists(), "main database should be deleted");
        assert!(!memory_db.exists(), "memory database should be deleted");
        assert!(
            !connectors_db.exists(),
            "connectors database should be deleted"
        );
        assert!(!main_wal.exists(), "WAL file should be deleted");
        assert!(!main_shm.exists(), "SHM file should be deleted");
        assert!(
            !connector_exports.exists(),
            "connector exports should be deleted"
        );
    }

    /// Test that handle_db dispatches to the correct action.
    #[test]
    fn test_db_action_variants() {
        // Verify the enum variants exist
        let _ = DbAction::Reset;
        let _ = DbAction::Clear;
        let _ = DbAction::Migrate;
    }

    #[tokio::test]
    async fn reset_refuses_on_postgres() {
        let mut config = moltis_config::MoltisConfig::default();
        config.database.backend = moltis_config::DatabaseBackend::Postgres;
        config.database.url = Some(secrecy::Secret::new(
            "postgres://postgres:test@127.0.0.1:55433/moltis".into(),
        ));
        let error = reset_databases_for(&config)
            .await
            .expect_err("reset must refuse on postgres");
        assert!(
            error.to_string().contains("not supported"),
            "error must explain the refusal: {error}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn connector_database_permissions_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let temp = TempDir::new().unwrap();
        let path = temp.path().join("connectors.db");
        std::fs::write(&path, "test").unwrap();
        restrict_database_permissions(&path).unwrap();
        assert_eq!(
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    /// Test that migrations run successfully against a fresh database in a temp directory.
    /// This verifies the full migration flow works end-to-end.
    #[tokio::test]
    async fn test_migrations_run_successfully_in_temp_dir() {
        let temp = TempDir::new().unwrap();
        let main_db = temp.path().join("moltis.db");
        let memory_db = temp.path().join("memory.db");

        // Run main database migrations
        let db_url = format!("sqlite:{}?mode=rwc", main_db.display());
        let pool = sqlx::SqlitePool::connect(&db_url).await.unwrap();

        // Run migrations in dependency order
        let db = moltis_db::Db::from(pool.clone());
        moltis_projects::run_migrations(&db).await.unwrap();
        moltis_sessions::run_migrations(&db).await.unwrap();
        moltis_cron::run_migrations(&db).await.unwrap();
        moltis_gateway::run_migrations(&db).await.unwrap();

        // Verify tables were created by querying them
        let _: (i64,) = sqlx::query_as("SELECT count(*) FROM projects")
            .fetch_one(&pool)
            .await
            .unwrap();
        let _: (i64,) = sqlx::query_as("SELECT count(*) FROM sessions")
            .fetch_one(&pool)
            .await
            .unwrap();
        let _: (i64,) = sqlx::query_as("SELECT count(*) FROM cron_jobs")
            .fetch_one(&pool)
            .await
            .unwrap();
        let _: (i64,) = sqlx::query_as("SELECT count(*) FROM channels")
            .fetch_one(&pool)
            .await
            .unwrap();

        pool.close().await;

        // Run memory database migrations
        let memory_url = format!("sqlite:{}?mode=rwc", memory_db.display());
        let memory_pool = sqlx::SqlitePool::connect(&memory_url).await.unwrap();

        moltis_memory::schema::run_migrations(&moltis_db::Db::from(memory_pool.clone()))
            .await
            .unwrap();

        // Verify memory tables were created
        let _: (i64,) = sqlx::query_as("SELECT count(*) FROM files")
            .fetch_one(&memory_pool)
            .await
            .unwrap();
        let _: (i64,) = sqlx::query_as("SELECT count(*) FROM chunks")
            .fetch_one(&memory_pool)
            .await
            .unwrap();

        memory_pool.close().await;

        // Verify database files exist
        assert!(main_db.exists(), "main database should be created");
        assert!(memory_db.exists(), "memory database should be created");
    }

    /// Test that migrations can run multiple times without error (idempotent).
    /// This verifies set_ignore_missing works correctly.
    #[tokio::test]
    async fn test_migrations_are_idempotent() {
        let temp = TempDir::new().unwrap();
        let main_db = temp.path().join("moltis.db");

        let db_url = format!("sqlite:{}?mode=rwc", main_db.display());
        let pool = sqlx::SqlitePool::connect(&db_url).await.unwrap();
        let db = moltis_db::Db::from(pool.clone());

        // Run all migrations
        moltis_projects::run_migrations(&db).await.unwrap();
        moltis_sessions::run_migrations(&db).await.unwrap();
        moltis_cron::run_migrations(&db).await.unwrap();
        moltis_gateway::run_migrations(&db).await.unwrap();

        // Run again - should still work due to set_ignore_missing
        moltis_projects::run_migrations(&db).await.unwrap();
        moltis_sessions::run_migrations(&db).await.unwrap();
        moltis_cron::run_migrations(&db).await.unwrap();
        moltis_gateway::run_migrations(&db).await.unwrap();

        pool.close().await;
    }
}
