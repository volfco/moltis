use std::{borrow::Cow, str::FromStr, time::Duration};

use {
    sqlx::{
        Postgres, Sqlite,
        postgres::{PgConnectOptions, PgPool},
        sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqliteSynchronous},
    },
    tracing::debug,
};

use crate::{
    dialect::{Dialect, rewrite_placeholders},
    error::Result,
};

/// Connection settings shared by every database moltis opens.
#[derive(Debug, Clone)]
pub struct DbOptions {
    /// `sqlite:<path>`, `sqlite::memory:`, `postgres://…` or `postgresql://…`.
    pub url: String,
    pub max_connections: u32,
    /// Enable `foreign_keys` (SQLite only; PostgreSQL always enforces them).
    pub foreign_keys: bool,
}

impl DbOptions {
    #[must_use]
    pub fn sqlite(path: impl Into<String>, max_connections: u32) -> Self {
        let path = path.into();
        let url = if path.starts_with("sqlite:") {
            path
        } else {
            format!("sqlite:{path}")
        };
        Self {
            url,
            max_connections,
            foreign_keys: true,
        }
    }

    #[must_use]
    pub fn postgres(url: impl Into<String>, max_connections: u32) -> Self {
        Self {
            url: url.into(),
            max_connections,
            foreign_keys: true,
        }
    }

    /// Pick the dialect from the URL scheme.
    #[must_use]
    pub fn dialect(&self) -> Dialect {
        if is_postgres_url(&self.url) {
            Dialect::Postgres
        } else {
            Dialect::Sqlite
        }
    }
}

/// Whether a connection URL targets PostgreSQL.
#[must_use]
pub fn is_postgres_url(url: &str) -> bool {
    let url = url.trim();
    url.starts_with("postgres://") || url.starts_with("postgresql://")
}

/// An open connection pool for either supported backend.
///
/// Pools are cheap to clone (they share the underlying connections), so the
/// enum is passed by reference into stores.
#[derive(Clone)]
pub enum Db {
    Sqlite(SqlitePool),
    Postgres(PgPool),
}

impl std::fmt::Debug for Db {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Db")
            .field("dialect", &self.dialect())
            .finish_non_exhaustive()
    }
}

impl Db {
    pub async fn connect(options: &DbOptions) -> Result<Self> {
        match options.dialect() {
            Dialect::Sqlite => Self::connect_sqlite(options).await,
            Dialect::Postgres => Self::connect_postgres(options).await,
        }
    }

    async fn connect_sqlite(options: &DbOptions) -> Result<Self> {
        // Accept both `sqlite:<path>` and a bare filesystem path.
        let url = if options.url.starts_with("sqlite:") {
            options.url.clone()
        } else {
            format!("sqlite:{}", options.url)
        };
        let raw = url.strip_prefix("sqlite:").unwrap_or(&url);
        // Only switch a fresh database file to WAL: converting an existing
        // SQLite database is not something a config change should do silently.
        let existed = raw != ":memory:" && std::path::Path::new(raw).exists();
        let mut opts = SqliteConnectOptions::from_str(&url)?
            .create_if_missing(true)
            .foreign_keys(options.foreign_keys)
            .synchronous(SqliteSynchronous::Normal)
            .busy_timeout(Duration::from_secs(5));
        if !existed {
            opts = opts.journal_mode(SqliteJournalMode::Wal);
        }
        let pool = sqlx::pool::PoolOptions::new()
            .max_connections(options.max_connections)
            .connect_with(opts)
            .await?;
        Ok(Self::Sqlite(pool))
    }

    async fn connect_postgres(options: &DbOptions) -> Result<Self> {
        let opts = PgConnectOptions::from_str(&options.url)?.application_name("moltis");
        let host = opts.get_host().to_string();
        let database = opts.get_database().unwrap_or_default().to_string();
        let pool = sqlx::pool::PoolOptions::new()
            .max_connections(options.max_connections)
            .acquire_timeout(Duration::from_secs(10))
            .connect_with(opts)
            .await?;
        debug!(%host, %database, "postgres pool connected");
        Ok(Self::Postgres(pool))
    }

    #[must_use]
    pub fn dialect(&self) -> Dialect {
        match self {
            Self::Sqlite(_) => Dialect::Sqlite,
            Self::Postgres(_) => Dialect::Postgres,
        }
    }

    #[must_use]
    pub fn is_postgres(&self) -> bool {
        matches!(self, Self::Postgres(_))
    }

    #[must_use]
    pub fn sqlite(&self) -> Option<&SqlitePool> {
        match self {
            Self::Sqlite(pool) => Some(pool),
            Self::Postgres(_) => None,
        }
    }

    #[must_use]
    pub fn postgres(&self) -> Option<&PgPool> {
        match self {
            Self::Postgres(pool) => Some(pool),
            Self::Sqlite(_) => None,
        }
    }

    /// Rewrite a `?`-style query for this backend.
    #[must_use]
    pub fn sql<'a>(&self, sql: &'a str) -> Cow<'a, str> {
        rewrite_placeholders(self.dialect(), sql)
    }

    pub async fn begin(&self) -> Result<DbTx> {
        Ok(match self {
            Self::Sqlite(pool) => DbTx::Sqlite(pool.begin().await?),
            Self::Postgres(pool) => DbTx::Postgres(pool.begin().await?),
        })
    }

    pub async fn close(&self) {
        match self {
            Self::Sqlite(pool) => pool.close().await,
            Self::Postgres(pool) => pool.close().await,
        }
    }

    /// Report whether the backend answers a trivial query.
    pub async fn ping(&self) -> Result<()> {
        match self {
            Self::Sqlite(pool) => {
                sqlx::query("SELECT 1").execute(pool).await?;
            },
            Self::Postgres(pool) => {
                sqlx::query("SELECT 1").execute(pool).await?;
            },
        }
        Ok(())
    }
}

impl From<SqlitePool> for Db {
    fn from(pool: SqlitePool) -> Self {
        Self::Sqlite(pool)
    }
}

impl From<PgPool> for Db {
    fn from(pool: PgPool) -> Self {
        Self::Postgres(pool)
    }
}

/// A transaction on either backend. Use the `tx_*` macros for queries.
pub enum DbTx {
    Sqlite(sqlx::Transaction<'static, Sqlite>),
    Postgres(sqlx::Transaction<'static, Postgres>),
}

impl DbTx {
    #[must_use]
    pub fn dialect(&self) -> Dialect {
        match self {
            Self::Sqlite(_) => Dialect::Sqlite,
            Self::Postgres(_) => Dialect::Postgres,
        }
    }

    pub async fn commit(self) -> Result<()> {
        match self {
            Self::Sqlite(tx) => tx.commit().await?,
            Self::Postgres(tx) => tx.commit().await?,
        }
        Ok(())
    }

    pub async fn rollback(self) -> Result<()> {
        match self {
            Self::Sqlite(tx) => tx.rollback().await?,
            Self::Postgres(tx) => tx.rollback().await?,
        }
        Ok(())
    }
}

/// Run the backend-appropriate migrator.
///
/// Each crate keeps its SQLite migrations in `migrations/` and the PostgreSQL
/// equivalents in `migrations_pg/`; both are embedded at compile time by
/// `sqlx::migrate!`.
pub async fn run_migrations(
    db: &Db,
    sqlite: sqlx::migrate::Migrator,
    postgres: sqlx::migrate::Migrator,
) -> Result<()> {
    let mut sqlite = sqlite;
    let mut postgres = postgres;
    match db {
        Db::Sqlite(pool) => {
            sqlite.set_ignore_missing(true).run(pool).await?;
        },
        Db::Postgres(pool) => {
            postgres.set_ignore_missing(true).run(pool).await?;
        },
    }
    Ok(())
}
