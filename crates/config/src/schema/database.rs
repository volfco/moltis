use {
    secrecy::Secret,
    serde::{Deserialize, Serialize},
};

/// Deployment-wide database engine selection.
///
/// One `[database]` block decides the engine for every SQL store: a deployment
/// runs on PostgreSQL or SQLite, never both, with no fallback. The dialect is
/// inferred from the URL scheme and pool sizing comes from
/// `server.db_pool_max_connections`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DatabaseConfig {
    /// Database engine for every SQL store: `"sqlite"` or `"postgres"`.
    pub backend: DatabaseBackend,
    /// PostgreSQL connection URL, e.g. `"postgres://user:pass@localhost/moltis"`.
    /// Required when `backend = "postgres"`; ignored for `"sqlite"`, which
    /// keeps using the database files under the data directory.
    #[serde(
        default,
        serialize_with = "crate::schema::serialize_option_secret",
        skip_serializing_if = "Option::is_none"
    )]
    pub url: Option<Secret<String>>,
}

/// Database engine selected by `[database] backend`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DatabaseBackend {
    /// SQLite database files under the data directory (default).
    #[default]
    Sqlite,
    /// One shared PostgreSQL database given by `[database] url`.
    Postgres,
}
