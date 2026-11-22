//! pgvector availability probe for the PostgreSQL memory schema.
//!
//! The memory migrations require the `vector` extension and create it
//! themselves, but their raw error hides the fix when the connecting role
//! may not create extensions. A preflight runs before the migrations:
//! missing extension → attempt `CREATE EXTENSION IF NOT EXISTS vector` (a
//! privileged role needs no manual step) → only when that is denied, fail
//! with the exact remediation instead of the raw error. The same probe feeds
//! the `pgvector` field of the `memory.status` RPC.

use {moltis_db::Db, serde::Serialize};

use crate::error::{Error, Result};

/// SQL command an operator must run to fix a missing pgvector install.
pub const PGVECTOR_ACTION: &str = "CREATE EXTENSION vector;";

/// pgvector availability in one database, as reported by `memory.status`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct PgvectorStatus {
    pub available: bool,
    /// Remediation to run when `available` is false ([`PGVECTOR_ACTION`]).
    pub action: &'static str,
}

/// Probe pgvector availability in `db`.
///
/// Returns `None` where the probe does not apply: SQLite (pgvector is a
/// PostgreSQL-only extension) and exotic setups without the `pg_extension`
/// catalog, where extension state cannot be queried at all.
pub async fn probe(db: &Db) -> Result<Option<PgvectorStatus>> {
    if !db.is_postgres() {
        return Ok(None);
    }
    // Guard for exotic setups without the pg_extension catalog: extension
    // state is unknowable there, so skip the probe and leave the raw
    // migration error as the only signal.
    let catalog_present: bool = moltis_db::scalar!(
        db,
        bool,
        "SELECT to_regclass('pg_catalog.pg_extension') IS NOT NULL"
    )?;
    if !catalog_present {
        return Ok(None);
    }
    // An installed extension is not enough: the `vector` type must also
    // resolve on this connection, or a role whose search_path hides the
    // extension's schema gets "available" and then fails in the migrations.
    let available: bool = moltis_db::scalar!(
        db,
        bool,
        "SELECT EXISTS (SELECT 1 FROM pg_catalog.pg_extension WHERE extname = 'vector') \
         AND to_regtype('vector') IS NOT NULL"
    )?;
    Ok(Some(PgvectorStatus {
        available,
        action: PGVECTOR_ACTION,
    }))
}

/// Ensure pgvector is installed before the PostgreSQL memory migrations run.
///
/// When the extension is missing this attempts `CREATE EXTENSION IF NOT
/// EXISTS vector` first — the same statement the migrations run — so a
/// privileged role (e.g. superuser) needs no manual step. Only when that
/// attempt fails (insufficient privileges, missing extension package) does
/// the error surface, naming the database, the exact remediation, and the
/// cause of the failed creation. SQLite and exotic setups without
/// `pg_extension` pass through unchanged.
pub async fn preflight(db: &Db) -> Result<()> {
    let Some(status) = probe(db).await? else {
        return Ok(());
    };
    if status.available {
        return Ok(());
    }
    let cause = match moltis_db::execute!(db, "CREATE EXTENSION IF NOT EXISTS vector") {
        Ok(_) => return Ok(()),
        Err(error) => error,
    };
    // `IF NOT EXISTS` races with a concurrent first-boot install: the loser
    // can see a duplicate-extension error while the extension is in fact
    // there. Re-probe before reporting a privilege problem.
    if probe(db).await?.is_some_and(|status| status.available) {
        return Ok(());
    }
    let database: String = moltis_db::scalar!(db, String, "SELECT current_database()::text")
        .unwrap_or_else(|_| "unknown".to_string());
    Err(Error::PgvectorMissing(format!(
        "pgvector extension is missing in PostgreSQL database '{database}': \
         run as superuser: {PGVECTOR_ACTION} (automatic creation failed: {cause})"
    )))
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pgvector_status_serializes_available_and_action() {
        let value = serde_json::to_value(PgvectorStatus {
            available: false,
            action: PGVECTOR_ACTION,
        })
        .unwrap();
        assert_eq!(
            value,
            serde_json::json!({"available": false, "action": "CREATE EXTENSION vector;"})
        );
    }

    #[tokio::test]
    async fn probe_and_preflight_skip_sqlite() {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        let db = Db::from(pool);
        assert_eq!(probe(&db).await.unwrap(), None);
        preflight(&db).await.unwrap();
    }

    #[tokio::test]
    async fn pg_preflight_creates_pgvector_without_manual_step() {
        let _guard = crate::test_support::pg_lock().lock().await;
        let Some(db) = crate::test_support::pg_test_db_pre_migration().await else {
            return;
        };
        reset_extension_state(&db).await;

        let status = probe(&db)
            .await
            .unwrap()
            .expect("probe applies on PostgreSQL");
        assert!(
            !status.available,
            "fresh database must report pgvector missing"
        );

        // Documented zero-touch flow: a superuser run must not need a manual
        // CREATE EXTENSION before the first start.
        preflight(&db)
            .await
            .expect("privileged preflight must auto-create pgvector");
        crate::schema::run_migrations(&db).await.unwrap();

        let status = probe(&db)
            .await
            .unwrap()
            .expect("probe applies on PostgreSQL");
        assert!(
            status.available,
            "preflight must leave pgvector installed for the migrations"
        );
    }

    #[tokio::test]
    async fn pg_preflight_names_remediation_when_extension_creation_denied() {
        let _guard = crate::test_support::pg_lock().lock().await;
        let Some(db) = crate::test_support::pg_test_db_pre_migration().await else {
            return;
        };
        reset_extension_state(&db).await;

        // A non-superuser role without create rights cannot install pgvector,
        // so the preflight's create attempt fails and the actionable error
        // must replace the raw one.
        moltis_db::execute!(&db, format!("DROP ROLE IF EXISTS {RESTRICTED_ROLE}"))
            .unwrap_or_else(|error| panic!("drop test role failed: {error}"));
        moltis_db::execute!(
            &db,
            format!("CREATE ROLE {RESTRICTED_ROLE} LOGIN PASSWORD 'restricted'")
        )
        .unwrap_or_else(|error| panic!("create test role failed: {error}"));
        let restricted = Db::connect(&moltis_db::DbOptions::postgres(restricted_url(), 2))
            .await
            .unwrap_or_else(|error| panic!("restricted test role must connect: {error}"));

        let error = preflight(&restricted)
            .await
            .expect_err("denied creation must fail the preflight");
        let message = error.to_string();
        assert!(
            message.contains("run as superuser: CREATE EXTENSION vector;"),
            "error must name the remediation: {message}"
        );
        let database: String =
            moltis_db::scalar!(&db, String, "SELECT current_database()::text").unwrap();
        assert!(
            message.contains(&database),
            "error must name the database: {message}"
        );

        // Leave no role behind: reruns and later tests must see a clean cluster.
        let restricted_role = RESTRICTED_ROLE;
        moltis_db::execute!(&db, format!("DROP OWNED BY {restricted_role}"))
            .unwrap_or_else(|error| panic!("drop restricted role objects failed: {error}"));
        moltis_db::execute!(&db, format!("DROP ROLE IF EXISTS {restricted_role}"))
            .unwrap_or_else(|error| panic!("drop test role failed: {error}"));
    }

    /// Non-superuser role used by the creation-denied test.
    const RESTRICTED_ROLE: &str = "moltis_pgvector_restricted";

    /// Reruns must see the pre-install state; the schema reset already left
    /// no object that depends on the extension.
    async fn reset_extension_state(db: &Db) {
        moltis_db::execute!(db, "DROP EXTENSION IF EXISTS vector")
            .unwrap_or_else(|error| panic!("drop test extension failed: {error}"));
    }

    /// Connect URL for [`RESTRICTED_ROLE`], derived from `TEST_POSTGRES_URL`.
    fn restricted_url() -> String {
        let url = std::env::var("TEST_POSTGRES_URL").unwrap();
        let (_, after_scheme) = url.split_once("://").unwrap();
        // `rsplit_once`: passwords may contain '@'.
        let (_, host_and_db) = after_scheme.rsplit_once('@').unwrap();
        format!("postgres://{RESTRICTED_ROLE}:restricted@{host_and_db}")
    }
}
