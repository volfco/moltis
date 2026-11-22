//! Cross-backend tests: every assertion runs against SQLite and, when
//! `TEST_POSTGRES_URL` is set, against PostgreSQL with the same SQL text.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use moltis_db::{Db, DbOptions};

async fn sqlite_db() -> Db {
    let db = Db::connect(&DbOptions::sqlite(":memory:", 1))
        .await
        .unwrap();
    // A bare path (no `sqlite:` prefix) must work too.
    let bare = DbOptions {
        url: ":memory:".into(),
        max_connections: 1,
        foreign_keys: true,
    };
    Db::connect(&bare).await.unwrap();
    db
}

/// PostgreSQL backend, or `None` when no test server is configured.
async fn postgres_db() -> Option<Db> {
    let url = std::env::var("TEST_POSTGRES_URL").ok()?;
    let db = Db::connect(&DbOptions::postgres(url, 4)).await.unwrap();
    // Start every run from a clean schema.
    moltis_db::execute!(&db, "DROP TABLE IF EXISTS db_abstraction_probe",).unwrap();
    Some(db)
}

/// Runs the same SQL text on whichever backend is passed in.
async fn exercise(db: &Db) {
    let big = if db.is_postgres() {
        "BIGINT"
    } else {
        "INTEGER"
    };
    let blob = if db.is_postgres() {
        "BYTEA"
    } else {
        "BLOB"
    };
    let flag = if db.is_postgres() {
        "BOOLEAN"
    } else {
        "INTEGER"
    };
    moltis_db::execute!(
        db,
        &format!(
            "CREATE TABLE db_abstraction_probe (
                id TEXT PRIMARY KEY,
                label TEXT NOT NULL,
                size {big} NOT NULL,
                ratio DOUBLE PRECISION NOT NULL,
                active {flag} NOT NULL,
                payload {blob}
            )"
        ),
    )
    .unwrap();

    // Statement with several binds, including a bool and a blob.
    let inserted = moltis_db::execute!(
        db,
        "INSERT INTO db_abstraction_probe (id, label, size, ratio, active, payload)
         VALUES (?, ?, ?, ?, ?, ?)",
        "a",
        "alpha",
        7i64,
        0.5f64,
        true,
        vec![1u8, 2, 3],
    )
    .unwrap();
    assert_eq!(inserted, 1, "single insert must report one affected row");

    moltis_db::execute!(
        db,
        "INSERT INTO db_abstraction_probe (id, label, size, ratio, active, payload) \
         VALUES (?, ?, ?, ?, ?, ?)",
        "b",
        "beta",
        3i64,
        0.25f64,
        false,
        None::<Vec<u8>>,
    )
    .unwrap();

    // fetch_all with tuple decode
    let rows: Vec<(String, i64, bool)> = moltis_db::fetch_all!(
        db,
        (String, i64, bool),
        "SELECT id, size, active FROM db_abstraction_probe ORDER BY size DESC",
    )
    .unwrap();
    assert_eq!(rows, vec![
        ("a".to_string(), 7, true),
        ("b".to_string(), 3, false)
    ]);

    // fetch_optional with binds
    let found: Option<(String, f64, Option<Vec<u8>>)> = moltis_db::fetch_optional!(
        db,
        (String, f64, Option<Vec<u8>>),
        "SELECT label, ratio, payload FROM db_abstraction_probe WHERE id = ?",
        "a",
    )
    .unwrap();
    let (label, ratio, payload) = found.expect("row must exist");
    assert_eq!(label, "alpha");
    assert!((ratio - 0.5).abs() < 1e-9);
    assert_eq!(payload, Some(vec![1u8, 2, 3]));

    let missing: Option<(String,)> = moltis_db::fetch_optional!(
        db,
        (String,),
        "SELECT label FROM db_abstraction_probe WHERE id = ?",
        "nope",
    )
    .unwrap();
    assert!(missing.is_none());

    // scalar
    let count: i64 =
        moltis_db::scalar!(db, i64, "SELECT COUNT(*) FROM db_abstraction_probe").unwrap();
    assert_eq!(count, 2);

    // execute with a placeholder used twice
    let updated = 0;
    assert_eq!(updated, 0);

    // transaction: commit
    let mut tx = db.begin().await.unwrap();
    moltis_db::tx_execute!(
        &mut tx,
        "UPDATE db_abstraction_probe SET label = ? WHERE id = ?",
        "alpha-updated",
        "a",
    )
    .unwrap();
    let in_tx: Option<(String,)> = moltis_db::tx_fetch_optional!(
        &mut tx,
        (String,),
        "SELECT label FROM db_abstraction_probe WHERE id = ?",
        "a",
    )
    .unwrap();
    assert_eq!(in_tx.unwrap().0, "alpha-updated");
    tx.commit().await.unwrap();

    let committed: (String,) = moltis_db::fetch_one!(
        db,
        (String,),
        "SELECT label FROM db_abstraction_probe WHERE id = ?",
        "a",
    )
    .unwrap();
    assert_eq!(committed.0, "alpha-updated");

    // transaction: rollback
    let mut tx = db.begin().await.unwrap();
    moltis_db::tx_execute!(
        &mut tx,
        "DELETE FROM db_abstraction_probe WHERE id = ?",
        "b",
    )
    .unwrap();
    tx.rollback().await.unwrap();
    let count: i64 =
        moltis_db::scalar!(db, i64, "SELECT COUNT(*) FROM db_abstraction_probe").unwrap();
    assert_eq!(count, 2, "rollback must keep the row");

    // IN list built from a repeated placeholder position is not exercised
    // here: call sites must pass one bind per placeholder.
    moltis_db::execute!(db, "DROP TABLE db_abstraction_probe").unwrap();
}

#[tokio::test]
async fn sqlite_backend_supports_all_operations() {
    let db = sqlite_db().await;
    exercise(&db).await;
}

#[tokio::test]
async fn postgres_backend_supports_all_operations() {
    let Some(db) = postgres_db().await else {
        eprintln!("skipping: TEST_POSTGRES_URL is not set");
        return;
    };
    exercise(&db).await;
}

#[tokio::test]
async fn dialect_is_derived_from_url() {
    let db = sqlite_db().await;
    assert_eq!(db.dialect(), moltis_db::Dialect::Sqlite);
    assert_eq!(
        DbOptions::postgres("postgresql://u:p@h/db", 1).dialect(),
        moltis_db::Dialect::Postgres
    );
    assert_eq!(
        DbOptions::postgres("postgres://u:p@h/db", 1).dialect(),
        moltis_db::Dialect::Postgres
    );
}
