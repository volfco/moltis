//! Deployment-wide database connections driven by the `[database]` config.
//!
//! One `[database]` block decides the engine for every SQL store: a deployment
//! runs on PostgreSQL or SQLite, never both. A configured PostgreSQL backend
//! that cannot connect is a hard error — there is no SQLite fallback.

use std::path::Path;

/// Connect to the deployment database: `<data_dir>/moltis.db` on SQLite, the
/// `[database] url` on PostgreSQL.
pub async fn open_database(
    config: &moltis_config::MoltisConfig,
    data_dir: &Path,
) -> anyhow::Result<moltis_db::Db> {
    match config.database.backend {
        moltis_config::DatabaseBackend::Sqlite => {
            let path = data_dir.join("moltis.db");
            moltis_db::Db::connect(&moltis_db::DbOptions::sqlite(
                path.to_string_lossy(),
                config.server.db_pool_max_connections,
            ))
            .await
            .map_err(|error| anyhow::anyhow!("failed to open {}: {error}", path.display()))
        },
        moltis_config::DatabaseBackend::Postgres => {
            let url = postgres_url(config)?;
            let options =
                moltis_db::DbOptions::postgres(url, config.server.db_pool_max_connections);
            // The dialect is inferred from the URL scheme; the scheme was
            // validated in `postgres_url`, so this cannot open another engine.
            moltis_db::Db::connect(&options).await.map_err(|error| {
                anyhow::anyhow!("cannot connect to the [database] PostgreSQL server: {error}")
            })
        },
    }
}

/// Connect to the database backing the built-in memory store: its own
/// `<data_dir>/memory.db` on SQLite, the shared deployment database on
/// PostgreSQL.
pub async fn open_memory_database(
    config: &moltis_config::MoltisConfig,
    data_dir: &Path,
) -> anyhow::Result<moltis_db::Db> {
    match config.database.backend {
        moltis_config::DatabaseBackend::Sqlite => {
            let path = data_dir.join("memory.db");
            moltis_db::Db::connect(&moltis_db::DbOptions::sqlite(
                path.to_string_lossy(),
                config.server.db_pool_max_connections,
            ))
            .await
            .map_err(|error| anyhow::anyhow!("failed to open {}: {error}", path.display()))
        },
        moltis_config::DatabaseBackend::Postgres => open_database(config, data_dir).await,
    }
}

/// The `[database] url` of a PostgreSQL deployment.
///
/// Must be non-empty and use a `postgres://`/`postgresql://` scheme: the
/// dialect is inferred from the scheme, so a mismatched scheme would silently
/// open the wrong engine.
pub fn postgres_url(config: &moltis_config::MoltisConfig) -> anyhow::Result<String> {
    use secrecy::ExposeSecret;

    let url = config
        .database
        .url
        .as_ref()
        .map(|secret| secret.expose_secret().trim().to_owned())
        .filter(|url| !url.is_empty())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "[database] backend = \"postgres\" requires [database] url \
                 (e.g. postgres://user:pass@localhost/moltis)"
            )
        })?;
    if moltis_db::DbOptions::postgres(&url, 1).dialect() != moltis_db::Dialect::Postgres {
        anyhow::bail!(
            "[database] url must start with postgres:// or postgresql:// \
             when backend = \"postgres\""
        );
    }
    Ok(url)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn postgres_config(url: &str) -> moltis_config::MoltisConfig {
        let mut config = moltis_config::MoltisConfig::default();
        config.database.backend = moltis_config::DatabaseBackend::Postgres;
        config.database.url = Some(secrecy::Secret::new(url.to_owned()));
        config
    }

    /// postgres configured + unreachable must fail the startup connection —
    /// a deployment runs on one engine and must never fall back to SQLite.
    #[tokio::test]
    async fn unreachable_postgres_is_a_hard_error_without_sqlite_fallback() {
        // Port 1 on localhost: connection refused immediately.
        let config = postgres_config("postgres://postgres:test@127.0.0.1:1/moltis");
        let dir = tempfile::tempdir().unwrap();
        let result = open_database(&config, dir.path()).await;
        assert!(
            result.is_err(),
            "unreachable postgres must be a startup error, not a fallback"
        );
        assert!(
            !dir.path().join("moltis.db").exists(),
            "no SQLite database may be created when postgres is configured"
        );
    }

    #[tokio::test]
    async fn postgres_without_url_is_an_error() {
        let mut config = moltis_config::MoltisConfig::default();
        config.database.backend = moltis_config::DatabaseBackend::Postgres;
        let dir = tempfile::tempdir().unwrap();
        let error = open_database(&config, dir.path())
            .await
            .expect_err("missing url must fail");
        assert!(
            error.to_string().contains("[database] url"),
            "error must name the missing setting: {error}"
        );
    }

    #[tokio::test]
    async fn postgres_url_must_use_a_postgres_scheme() {
        let config = postgres_config("sqlite:/tmp/not-postgres.db");
        let error = postgres_url(&config).expect_err("sqlite scheme must fail");
        assert!(
            error.to_string().contains("postgres://"),
            "error must name the expected scheme: {error}"
        );
    }
}
