//! PostgreSQL schema and CRUD smoke tests for the gateway stores.
//!
//! These do not mirror the SQLite test suite (a full mirror is out of scope):
//! they prove that `migrations_pg/` creates the gateway schema and that the
//! ported stores do round-trips against a real PostgreSQL server — plus one
//! deliberate mirror of the pinned pairing expiry-semantics test, because the
//! expiry dialect match is the riskiest decision in the port. Every test locks
//! [`test_support::pg_lock`] and resets the schema first, so the tests may
//! share one database.

#![allow(clippy::expect_used)]

use moltis_channels::{
    message_log::{MessageLog, MessageLogEntry},
    store::{ChannelStore, StoredChannel},
};

use crate::test_support::{pg_lock, pg_test_db};

/// Acquire the shared lock and open a freshly migrated database, or skip.
macro_rules! pg_db {
    () => {{
        let _guard = pg_lock().lock().await;
        let Some(db) = pg_test_db().await else {
            eprintln!("skipping: TEST_POSTGRES_URL not set");
            return;
        };
        (db, _guard)
    }};
}

#[tokio::test]
async fn postgres_schema_has_the_gateway_tables() {
    let (db, _guard) = pg_db!();

    let rows = moltis_db::fetch_all!(
        &db,
        (String,),
        "SELECT table_name FROM information_schema.tables WHERE table_schema = 'public'",
    )
    .expect("schema introspection");
    let present: std::collections::HashSet<String> = rows.into_iter().map(|(name,)| name).collect();

    for table in [
        "auth_password",
        "passkeys",
        "api_keys",
        "auth_sessions",
        "auth_audit_log",
        "env_variables",
        "message_log",
        "channels",
        "agents",
        "session_shares",
        "paired_devices",
        "pair_requests",
        "device_tokens",
        "ssh_keys",
        "ssh_targets",
        "voice_personas",
        "trace_links",
    ] {
        assert!(present.contains(table), "missing table {table}");
    }
}

#[tokio::test]
async fn postgres_pairing_store_round_trips_device_lifecycle() {
    let (db, _guard) = pg_db!();
    let store = crate::pairing::PairingStore::new(db.clone());

    let request = store
        .request_pair("pg-device", Some("PG Node"), "linux", None)
        .await
        .expect("request_pair");
    let pending = store.list_pending().await.expect("list_pending");
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].id, request.id);

    let token = store.approve(&request.id).await.expect("approve");
    let verified = store
        .verify_device_token(&token.token)
        .await
        .expect("verify")
        .expect("token verifies");
    assert_eq!(verified.device_id, "pg-device");

    // Rotation invalidates the old token; revoke kills the new one. Both
    // paths write device_tokens rows through the DEFAULT-based INSERT.
    let rotated = store.rotate_token("pg-device").await.expect("rotate");
    assert!(
        store
            .verify_device_token(&token.token)
            .await
            .expect("verify")
            .is_none()
    );
    assert!(
        store
            .verify_device_token(&rotated.token)
            .await
            .expect("verify")
            .is_some()
    );

    store.revoke_token("pg-device").await.expect("revoke");
    assert!(
        store
            .verify_device_token(&rotated.token)
            .await
            .expect("verify")
            .is_none()
    );
}

#[tokio::test]
async fn postgres_channel_store_round_trips_channel_config() {
    let (db, _guard) = pg_db!();
    let store = crate::channel_store::DbChannelStore::new(db.clone());

    let stamp = 1_700_000_000;
    store
        .upsert(StoredChannel {
            account_id: "bot-pg".into(),
            channel_type: "telegram".into(),
            config: serde_json::json!({ "token": "secret" }),
            created_at: stamp,
            updated_at: stamp,
        })
        .await
        .expect("upsert");

    let got = store
        .get("telegram", "bot-pg")
        .await
        .expect("get")
        .expect("channel stored");
    assert_eq!(got.config["token"], "secret");

    let all = store.list().await.expect("list");
    assert_eq!(all.len(), 1);

    // Second upsert replaces in place (composite-key ON CONFLICT).
    store
        .upsert(StoredChannel {
            account_id: "bot-pg".into(),
            channel_type: "telegram".into(),
            config: serde_json::json!({ "token": "rotated" }),
            created_at: stamp,
            updated_at: stamp + 1,
        })
        .await
        .expect("upsert again");
    let got = store
        .get("telegram", "bot-pg")
        .await
        .expect("get")
        .expect("channel still there");
    assert_eq!(got.config["token"], "rotated");

    store.delete("telegram", "bot-pg").await.expect("delete");
    assert!(
        store
            .get("telegram", "bot-pg")
            .await
            .expect("get")
            .is_none()
    );
}

#[tokio::test]
async fn postgres_message_log_round_trips_entries_and_senders() {
    let (db, _guard) = pg_db!();
    let store = crate::message_log_store::DbMessageLog::new(db.clone());

    let entry = |peer: &str, granted: bool| MessageLogEntry {
        id: 0,
        account_id: "bot-pg".into(),
        channel_type: "telegram".into(),
        peer_id: peer.into(),
        username: Some(peer.into()),
        sender_name: None,
        chat_id: "c1".into(),
        chat_type: "dm".into(),
        body: "hello".into(),
        access_granted: granted,
        created_at: 1_700_000_000,
    };
    store.log(entry("u1", true)).await.expect("log");
    store.log(entry("u1", false)).await.expect("log");
    store.log(entry("u2", false)).await.expect("log");

    let entries = store
        .list_by_account("telegram", "bot-pg", 10)
        .await
        .expect("list");
    assert_eq!(entries.len(), 3);
    assert!(entries.iter().any(|e| e.access_granted));

    // The bool aggregate decode (MAX(CASE ...) > 0) is dialect-sensitive.
    let senders = store
        .unique_senders("telegram", "bot-pg")
        .await
        .expect("senders");
    assert_eq!(senders.len(), 2);
    let u1 = senders.iter().find(|s| s.peer_id == "u1").expect("u1");
    assert_eq!(u1.message_count, 2);
    assert!(u1.last_access_granted);
    let u2 = senders.iter().find(|s| s.peer_id == "u2").expect("u2");
    assert!(!u2.last_access_granted);
}

/// Mirror of the SQLite test
/// `expiry_comparison_handles_fractional_and_legacy_timestamps`. The pinned
/// outcomes guard the riskiest dialect decision in the port: the Postgres
/// expiry predicate
/// `CASE WHEN pg_input_is_valid(expires_at,'timestamp') THEN expires_at::timestamp END …`
/// must match SQLite's `julianday` comparison for fractional RFC3339, legacy
/// naive, and malformed text (the last stays pending and is never evicted).
#[tokio::test]
async fn postgres_expiry_comparison_handles_fractional_and_legacy_timestamps() {
    let (db, _guard) = pg_db!();
    let store = crate::pairing::PairingStore::new(db.clone());

    let expiries = [
        ("exact-second", "2030-01-01T00:00:00Z"),
        ("fractional", "2030-01-01T00:00:00.900Z"),
        ("legacy", "2030-01-01 00:00:01"),
        ("malformed", "not-a-datetime"),
    ];
    let mut requests = Vec::new();
    for (device_id, expires_at) in expiries {
        let request = store
            .request_pair(device_id, None, "test", None)
            .await
            .expect("request_pair");
        moltis_db::execute!(
            &db,
            "UPDATE pair_requests SET expires_at = ? WHERE id = ?",
            expires_at,
            &request.id,
        )
        .expect("overwrite expires_at");
        requests.push(request);
    }

    let now = "2030-01-01T00:00:00.500Z";
    let pending = store.list_pending_at(now).await.expect("list_pending_at");
    let mut pending_devices: Vec<_> = pending
        .iter()
        .map(|request| request.device_id.as_str())
        .collect();
    pending_devices.sort_unstable();
    assert_eq!(pending_devices, ["fractional", "legacy"]);

    assert_eq!(
        store.evict_expired_at(now).await.expect("evict_expired_at"),
        1
    );
    assert_eq!(
        store
            .get_pair_status(&requests[0].id)
            .await
            .expect("status of exact-second"),
        Some(crate::pairing::PairStatus::Expired)
    );
    assert_eq!(
        store
            .get_pair_status(&requests[3].id)
            .await
            .expect("status of malformed"),
        Some(crate::pairing::PairStatus::Pending)
    );
}
