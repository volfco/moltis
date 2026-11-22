//! pgvector availability probe for the PostgreSQL memory schema.
//!
//! The memory migrations require the `vector` extension. When it is missing
//! the raw migration error hides the fix, so a preflight probe runs before the
//! migrations and fails startup with the exact remediation instead. The same
//! probe feeds the `pgvector` field of the `memory.status` RPC.

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
    let available: bool = moltis_db::scalar!(
        db,
        bool,
        "SELECT EXISTS (SELECT 1 FROM pg_catalog.pg_extension WHERE extname = 'vector')"
    )?;
    Ok(Some(PgvectorStatus {
        available,
        action: PGVECTOR_ACTION,
    }))
}

/// Fail when PostgreSQL memory migrations would run without pgvector.
///
/// The error names the database and the exact remediation. SQLite and exotic
/// setups without `pg_extension` pass through unchanged.
pub async fn preflight(db: &Db) -> Result<()> {
    match probe(db).await? {
        Some(status) if !status.available => {
            let database: String =
                moltis_db::scalar!(db, String, "SELECT current_database()::text")
                    .unwrap_or_else(|_| "unknown".to_string());
            Err(Error::PgvectorMissing(format!(
                "pgvector extension is missing in PostgreSQL database '{database}': \
                 run as superuser: {PGVECTOR_ACTION}"
            )))
        },
        _ => Ok(()),
    }
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
    async fn pg_preflight_names_remediation_when_pgvector_missing_then_passes_after_migrations() {
        let _guard = crate::test_support::pg_lock().lock().await;
        let Some(db) = crate::test_support::pg_test_db_pre_migration().await else {
            return;
        };
        // Reruns must see the pre-install state; the schema reset above left
        // no object that depends on the extension.
        moltis_db::execute!(&db, "DROP EXTENSION IF EXISTS vector")
            .unwrap_or_else(|error| panic!("drop test extension failed: {error}"));

        let status = probe(&db)
            .await
            .unwrap()
            .expect("probe applies on PostgreSQL");
        assert!(
            !status.available,
            "fresh database must report pgvector missing"
        );

        let error = preflight(&db)
            .await
            .expect_err("missing pgvector must fail the preflight");
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

        // The migrations create the extension (CREATE EXTENSION IF NOT EXISTS).
        crate::schema::run_migrations(&db).await.unwrap();
        let status = probe(&db)
            .await
            .unwrap()
            .expect("probe applies on PostgreSQL");
        assert!(
            status.available,
            "migrated database must report pgvector available"
        );
        preflight(&db).await.unwrap();
    }
}
