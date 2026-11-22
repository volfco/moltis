#![allow(clippy::unwrap_used, clippy::expect_used)]

use {super::*, moltis_db::Db};

async fn test_db() -> Db {
    let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
    let db = Db::from(pool);
    crate::run_migrations(&db).await.unwrap();
    db
}

#[tokio::test]
async fn pairing_lifecycle() {
    let db = test_db().await;
    let store = PairingStore::new(db);

    // Request pairing.
    let req = store
        .request_pair("dev-1", Some("My iPhone"), "ios", None)
        .await
        .unwrap();
    assert_eq!(req.device_id, "dev-1");
    assert_eq!(req.status, PairStatus::Pending);

    // List pending.
    let pending = store.list_pending().await.unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].id, req.id);

    // Approve.
    let token = store.approve(&req.id).await.unwrap();
    assert!(token.token.starts_with("mdt_"));
    assert_eq!(token.device_id, "dev-1");
    assert!(!token.scopes.is_empty());

    // Pending should be empty now.
    let pending = store.list_pending().await.unwrap();
    assert!(pending.is_empty());

    // Device should be listed.
    let devices = store.list_devices().await.unwrap();
    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0].device_id, "dev-1");

    // Verify token.
    let verification = store.verify_device_token(&token.token).await.unwrap();
    assert!(verification.is_some());
    let v = verification.unwrap();
    assert_eq!(v.device_id, "dev-1");
    assert_eq!(v.scopes, token.scopes);

    // Rotate token.
    let new_token = store.rotate_token("dev-1").await.unwrap();
    assert_ne!(new_token.token, token.token);

    // Old token should be invalid.
    let old_verify = store.verify_device_token(&token.token).await.unwrap();
    assert!(old_verify.is_none());

    // New token should be valid.
    let new_verify = store.verify_device_token(&new_token.token).await.unwrap();
    assert!(new_verify.is_some());

    // Revoke device.
    store.revoke_token("dev-1").await.unwrap();
    let revoked_verify = store.verify_device_token(&new_token.token).await.unwrap();
    assert!(revoked_verify.is_none());

    // Device should not be listed.
    let devices = store.list_devices().await.unwrap();
    assert!(devices.is_empty());
}

#[tokio::test]
async fn reject_pair_request() {
    let db = test_db().await;
    let store = PairingStore::new(db);

    let req = store
        .request_pair("dev-2", None, "android", None)
        .await
        .unwrap();
    store.reject(&req.id).await.unwrap();

    // Should not be in pending.
    let pending = store.list_pending().await.unwrap();
    assert!(pending.is_empty());

    // Reject again should fail.
    let result = store.reject(&req.id).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn expiry_comparison_handles_fractional_and_legacy_timestamps() {
    let db = test_db().await;
    let store = PairingStore::new(db.clone());
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
            .unwrap();
        moltis_db::execute!(
            &db,
            "UPDATE pair_requests SET expires_at = ? WHERE id = ?",
            expires_at,
            &request.id,
        )
        .unwrap();
        requests.push(request);
    }

    let now = "2030-01-01T00:00:00.500Z";
    let pending = store.list_pending_at(now).await.unwrap();
    let mut pending_devices: Vec<_> = pending
        .iter()
        .map(|request| request.device_id.as_str())
        .collect();
    pending_devices.sort_unstable();
    assert_eq!(pending_devices, ["fractional", "legacy"]);

    assert_eq!(store.evict_expired_at(now).await.unwrap(), 1);
    assert_eq!(
        store.get_pair_status(&requests[0].id).await.unwrap(),
        Some(PairStatus::Expired)
    );
    assert_eq!(
        store.get_pair_status(&requests[3].id).await.unwrap(),
        Some(PairStatus::Pending)
    );
}

#[tokio::test]
async fn approve_nonexistent_request() {
    let db = test_db().await;
    let store = PairingStore::new(db);

    let result = store.approve("nonexistent").await;
    assert!(matches!(result, Err(Error::PairRequestNotFound)));
}

#[tokio::test]
async fn verify_invalid_token() {
    let db = test_db().await;
    let store = PairingStore::new(db);

    let result = store.verify_device_token("invalid_token").await.unwrap();
    assert!(result.is_none());
}

#[tokio::test]
async fn create_device_token_directly() {
    let db = test_db().await;
    let store = PairingStore::new(db);

    // Create a device token without the pairing handshake.
    let token = store
        .create_device_token(Some("My Server"), "linux")
        .await
        .unwrap();
    assert!(token.token.starts_with("mdt_"));
    assert!(!token.scopes.is_empty());

    // Device should be listed.
    let devices = store.list_devices().await.unwrap();
    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0].device_id, token.device_id);
    assert_eq!(devices[0].display_name.as_deref(), Some("My Server"));
    assert_eq!(devices[0].platform, "linux");

    // Token should verify.
    let verification = store.verify_device_token(&token.token).await.unwrap();
    assert!(verification.is_some());
    let v = verification.unwrap();
    assert_eq!(v.device_id, token.device_id);

    // Can revoke it.
    store.revoke_token(&token.device_id).await.unwrap();
    let revoked_verify = store.verify_device_token(&token.token).await.unwrap();
    assert!(revoked_verify.is_none());
}

#[tokio::test]
async fn rotate_nonexistent_device() {
    let db = test_db().await;
    let store = PairingStore::new(db);

    let result = store.rotate_token("nonexistent").await;
    assert!(matches!(result, Err(Error::DeviceNotFound)));
}

// ── Ed25519 identity tests ──────────────────────────────────────────

fn make_test_keypair() -> (String, ed25519_dalek::SigningKey) {
    use {base64::Engine, rand::Rng};

    let mut seed = [0u8; 32];
    rand::rng().fill_bytes(&mut seed);
    let signing_key = ed25519_dalek::SigningKey::from_bytes(&seed);
    let pk_b64 =
        base64::engine::general_purpose::STANDARD.encode(signing_key.verifying_key().as_bytes());
    (pk_b64, signing_key)
}

#[tokio::test]
async fn find_device_by_public_key_lifecycle() {
    let db = test_db().await;
    let store = PairingStore::new(db);
    let (pk_b64, _) = make_test_keypair();

    // Not found before pairing.
    assert!(
        store
            .find_device_by_public_key(&pk_b64)
            .await
            .unwrap()
            .is_none()
    );

    // Pair with public key.
    let req = store
        .request_pair("dev-pk-1", Some("Key Node"), "linux", Some(&pk_b64))
        .await
        .unwrap();
    store.approve(&req.id).await.unwrap();

    // Now found.
    let device = store
        .find_device_by_public_key(&pk_b64)
        .await
        .unwrap()
        .expect("device should be found");
    assert_eq!(device.device_id, "dev-pk-1");
    assert_eq!(device.public_key.as_deref(), Some(pk_b64.as_str()));

    // Revoke and verify it's no longer found.
    store.revoke_token("dev-pk-1").await.unwrap();
    assert!(
        store
            .find_device_by_public_key(&pk_b64)
            .await
            .unwrap()
            .is_none()
    );

    // But it IS marked as revoked.
    assert!(store.is_public_key_revoked(&pk_b64).await.unwrap());
}

#[test]
fn ed25519_challenge_response_valid() {
    use {base64::Engine, ed25519_dalek::Signer};

    let (pk_b64, signing_key) = make_test_keypair();
    let (nonce, _nonce_b64) = generate_challenge_nonce();

    let signature = signing_key.sign(&nonce);
    let sig_b64 = base64::engine::general_purpose::STANDARD.encode(signature.to_bytes());

    assert!(verify_ed25519_challenge(&pk_b64, &nonce, &sig_b64).unwrap());
}

#[test]
fn ed25519_challenge_response_wrong_key() {
    use {base64::Engine, ed25519_dalek::Signer};

    let (_pk_b64, signing_key) = make_test_keypair();
    let (other_pk_b64, _) = make_test_keypair();
    let (nonce, _) = generate_challenge_nonce();

    let signature = signing_key.sign(&nonce);
    let sig_b64 = base64::engine::general_purpose::STANDARD.encode(signature.to_bytes());

    // Verify with the wrong public key should fail.
    assert!(!verify_ed25519_challenge(&other_pk_b64, &nonce, &sig_b64).unwrap());
}

#[test]
fn ed25519_challenge_response_wrong_nonce() {
    use {base64::Engine, ed25519_dalek::Signer};

    let (pk_b64, signing_key) = make_test_keypair();
    let (nonce, _) = generate_challenge_nonce();
    let (wrong_nonce, _) = generate_challenge_nonce();

    let signature = signing_key.sign(&nonce);
    let sig_b64 = base64::engine::general_purpose::STANDARD.encode(signature.to_bytes());

    // Verify with wrong nonce should fail.
    assert!(!verify_ed25519_challenge(&pk_b64, &wrong_nonce, &sig_b64).unwrap());
}

#[tokio::test]
async fn key_pinning_enforcement() {
    let db = test_db().await;
    let store = PairingStore::new(db);
    let (pk_b64, _) = make_test_keypair();
    let (other_pk_b64, _) = make_test_keypair();

    // No pinned key for unknown device.
    assert_eq!(
        store.check_key_pinning("dev-pin-1", &pk_b64).await.unwrap(),
        KeyPinningResult::NoPinnedKey
    );

    // Pair with first key.
    let req = store
        .request_pair("dev-pin-1", Some("Pinned Node"), "linux", Some(&pk_b64))
        .await
        .unwrap();
    store.approve(&req.id).await.unwrap();

    // Same key should match.
    assert_eq!(
        store.check_key_pinning("dev-pin-1", &pk_b64).await.unwrap(),
        KeyPinningResult::Match
    );

    // Different key should be rejected.
    match store
        .check_key_pinning("dev-pin-1", &other_pk_b64)
        .await
        .unwrap()
    {
        KeyPinningResult::Mismatch { expected } => {
            assert_eq!(expected, pk_b64);
        },
        other => panic!("expected Mismatch, got {other:?}"),
    }
}

#[tokio::test]
async fn pin_public_key_reports_actual_outcome() {
    let db = test_db().await;
    let store = PairingStore::new(db);
    let (pk_b64, _) = make_test_keypair();
    let (other_pk_b64, _) = make_test_keypair();

    assert_eq!(
        store
            .pin_public_key("missing-device", &pk_b64)
            .await
            .unwrap(),
        PinPublicKeyResult::DeviceNotFound
    );

    let token = store
        .create_device_token(Some("Migrating Node"), "linux")
        .await
        .unwrap();

    assert_eq!(
        store
            .pin_public_key(&token.device_id, &pk_b64)
            .await
            .unwrap(),
        PinPublicKeyResult::Pinned
    );
    assert_eq!(
        store
            .pin_public_key(&token.device_id, &pk_b64)
            .await
            .unwrap(),
        PinPublicKeyResult::AlreadyPinned
    );

    match store
        .pin_public_key(&token.device_id, &other_pk_b64)
        .await
        .unwrap()
    {
        PinPublicKeyResult::Mismatch { expected } => assert_eq!(expected, pk_b64),
        other => panic!("expected Mismatch, got {other:?}"),
    }

    store.revoke_token(&token.device_id).await.unwrap();
    assert_eq!(
        store
            .pin_public_key(&token.device_id, &pk_b64)
            .await
            .unwrap(),
        PinPublicKeyResult::Revoked
    );
}

#[tokio::test]
async fn key_pinning_reports_revoked_device() {
    let db = test_db().await;
    let store = PairingStore::new(db);
    let (pk_b64, _) = make_test_keypair();

    let req = store
        .request_pair(
            "dev-revoked-pin",
            Some("Pinned Node"),
            "linux",
            Some(&pk_b64),
        )
        .await
        .unwrap();
    store.approve(&req.id).await.unwrap();
    store.revoke_token("dev-revoked-pin").await.unwrap();

    assert_eq!(
        store
            .check_key_pinning("dev-revoked-pin", &pk_b64)
            .await
            .unwrap(),
        KeyPinningResult::Revoked
    );
}

#[test]
fn public_key_fingerprint_format() {
    let (pk_b64, _) = make_test_keypair();
    let fp = public_key_fingerprint(&pk_b64).unwrap();
    assert!(fp.starts_with("SHA256:"));
    assert_eq!(fp.len(), 7 + 44); // SHA256: prefix + 44 base64 chars
}

#[test]
fn public_key_fingerprint_rejects_malformed_keys() {
    use base64::Engine;

    assert!(public_key_fingerprint("not base64").is_err());
    assert!(public_key_fingerprint("").is_err());

    let too_short = base64::engine::general_purpose::STANDARD.encode([1u8; 31]);
    assert!(public_key_fingerprint(&too_short).is_err());
}

// ── Expiry boundary regression tests ────────────────────────────────────────

#[test]
fn expiry_boundary_counts_as_expired() {
    use super::expiry::is_expired_at;

    let boundary = "2030-01-01T00:00:00Z";
    let at = |s: &str| {
        time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).unwrap()
    };
    let boundary_at = at(boundary);

    // The exact expiry instant is expired; one nanosecond earlier is not.
    assert!(!is_expired_at(
        boundary,
        boundary_at - time::Duration::NANOSECOND
    ));
    assert!(is_expired_at(boundary, boundary_at));
    assert!(is_expired_at(
        boundary,
        boundary_at + time::Duration::NANOSECOND
    ));

    // Legacy DB timestamps share the same boundary at second granularity.
    assert!(!is_expired_at(
        "2030-01-01 00:00:00",
        boundary_at - time::Duration::SECOND
    ));
    assert!(is_expired_at("2030-01-01 00:00:00", boundary_at));
}

#[test]
fn expiry_boundary_handles_fractional_rfc3339() {
    use super::expiry::is_expired_at;

    // Lexicographic string comparison wrongly treats "…00Z" as later than
    // "…00.1Z"; instant comparison must not.
    let expires = "2030-01-01T00:05:00Z";
    let now = time::OffsetDateTime::parse(
        "2030-01-01T00:05:00.1Z",
        &time::format_description::well_known::Rfc3339,
    )
    .unwrap();
    assert!(is_expired_at(expires, now));
}

#[tokio::test]
async fn expired_request_cannot_be_approved() {
    let db = test_db().await;
    let store = PairingStore::new(db.clone());

    let req = store
        .request_pair("dev-expired-approve", None, "linux", None)
        .await
        .unwrap();
    moltis_db::execute!(
        &db,
        "UPDATE pair_requests SET expires_at = ? WHERE id = ?",
        "2000-01-01T00:00:00Z",
        &req.id,
    )
    .unwrap();

    let result = store.approve(&req.id).await;
    assert!(matches!(result, Err(Error::PairRequestExpired)));
    assert_eq!(
        store.get_pair_status(&req.id).await.unwrap(),
        Some(PairStatus::Expired)
    );
    assert!(store.list_devices().await.unwrap().is_empty());
    assert!(
        store
            .list_device_tokens("dev-expired-approve")
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn pending_and_evict_agree_on_expiry_boundary() {
    let db = test_db().await;
    let store = PairingStore::new(db.clone());

    let req = store
        .request_pair("dev-boundary", None, "test", None)
        .await
        .unwrap();
    let boundary = "2030-01-01T00:00:00Z";
    moltis_db::execute!(
        &db,
        "UPDATE pair_requests SET expires_at = ? WHERE id = ?",
        boundary,
        &req.id,
    )
    .unwrap();

    // One second before expiry the request is still pending.
    assert_eq!(
        store
            .list_pending_at("2029-12-31T23:59:59Z")
            .await
            .unwrap()
            .len(),
        1
    );

    // At the exact expiry instant it is already expired everywhere.
    assert!(store.list_pending_at(boundary).await.unwrap().is_empty());
    assert_eq!(store.evict_expired_at(boundary).await.unwrap(), 1);
    assert_eq!(
        store.get_pair_status(&req.id).await.unwrap(),
        Some(PairStatus::Expired)
    );
}

#[tokio::test]
async fn pairing_state_approve_enforces_expiry_boundary() {
    let mut state = PairingState::new();
    let at = |s: &str| {
        time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).unwrap()
    };

    // Exactly at expiry the request is rejected and consumed as expired.
    let req = state.request_pair("dev-state", None, "linux", None);
    let expires = at(&req.expires_at);
    assert!(matches!(
        state.approve_at(&req.id, expires),
        Err(Error::PairRequestExpired)
    ));
    assert!(matches!(
        state.approve_at(&req.id, expires),
        Err(Error::PairRequestNotPending(PairStatus::Expired))
    ));

    // One nanosecond before expiry it is still approvable.
    let req2 = state.request_pair("dev-state-2", None, "linux", None);
    let expires2 = at(&req2.expires_at);
    assert!(
        state
            .approve_at(&req2.id, expires2 - time::Duration::NANOSECOND)
            .is_ok()
    );
}

// ── Single-use / check-and-consume regression tests ─────────────────────────

/// One connection so concurrent queries interleave on a shared in-memory
/// database instead of fanning out across pool connections.
async fn race_db() -> Db {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    let db = Db::from(pool);
    crate::run_migrations(&db).await.unwrap();
    db
}

#[tokio::test]
async fn approve_is_single_use() {
    let db = test_db().await;
    let store = PairingStore::new(db.clone());

    let req = store
        .request_pair("dev-once", None, "linux", None)
        .await
        .unwrap();
    let first = store.approve(&req.id).await.unwrap();

    let replay = store.approve(&req.id).await;
    assert!(matches!(
        replay,
        Err(Error::PairRequestNotPending(PairStatus::Approved))
    ));

    // The replay attempt issued no second token.
    let rows =
        moltis_db::fetch_all!(&db, (String,), "SELECT token_hash FROM device_tokens",).unwrap();
    assert_eq!(rows.len(), 1);
    assert!(
        store
            .verify_device_token(&first.token)
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn rejected_request_cannot_be_approved_or_replayed() {
    let db = test_db().await;
    let store = PairingStore::new(db);

    let req = store
        .request_pair("dev-rej", None, "linux", None)
        .await
        .unwrap();
    store.reject(&req.id).await.unwrap();

    assert!(matches!(
        store.reject(&req.id).await,
        Err(Error::PairRequestNotPending(PairStatus::Rejected))
    ));
    assert!(matches!(
        store.approve(&req.id).await,
        Err(Error::PairRequestNotPending(PairStatus::Rejected))
    ));
    assert!(store.list_devices().await.unwrap().is_empty());
}

#[tokio::test]
async fn concurrent_approve_issues_exactly_one_token() {
    let db = race_db().await;
    let store = PairingStore::new(db.clone());

    let req = store
        .request_pair("dev-race", None, "linux", None)
        .await
        .unwrap();
    let (a, b) = tokio::join!(store.approve(&req.id), store.approve(&req.id));
    assert!(a.is_ok() != b.is_ok(), "exactly one approve must win");

    let rows =
        moltis_db::fetch_all!(&db, (String,), "SELECT token_hash FROM device_tokens",).unwrap();
    assert_eq!(rows.len(), 1);
}

#[tokio::test]
async fn concurrent_approve_and_reject_have_one_winner() {
    let db = race_db().await;
    let store = PairingStore::new(db.clone());

    let req = store
        .request_pair("dev-race-2", None, "linux", None)
        .await
        .unwrap();
    let (approved, rejected) = tokio::join!(store.approve(&req.id), store.reject(&req.id));
    assert!(approved.is_ok() != rejected.is_ok(), "exactly one must win");

    match (approved, rejected) {
        (Ok(token), Err(_)) => {
            assert_eq!(
                store.get_pair_status(&req.id).await.unwrap(),
                Some(PairStatus::Approved)
            );
            assert!(
                store
                    .verify_device_token(&token.token)
                    .await
                    .unwrap()
                    .is_some()
            );
        },
        (Err(_), Ok(())) => {
            assert_eq!(
                store.get_pair_status(&req.id).await.unwrap(),
                Some(PairStatus::Rejected)
            );
            assert!(store.list_devices().await.unwrap().is_empty());
        },
        other => panic!("expected exactly one winner, got {other:?}"),
    }
}

// ── Security-focused tests: token generation and redemption edge cases ──────

#[test]
fn generate_token_is_unique_and_url_safe() {
    use super::crypto::generate_token;

    let tokens: std::collections::HashSet<String> = (0..32).map(|_| generate_token()).collect();
    assert_eq!(tokens.len(), 32, "generated tokens must not collide");
    for token in &tokens {
        // 32 bytes of entropy, URL-safe base64 without padding.
        assert_eq!(token.len(), 43);
        assert!(
            token
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        );
    }
}

#[tokio::test]
async fn raw_device_token_is_never_persisted() {
    use super::crypto::sha256_hex;

    let db = test_db().await;
    let store = PairingStore::new(db.clone());

    let req = store
        .request_pair("dev-store", None, "linux", None)
        .await
        .unwrap();
    let token = store.approve(&req.id).await.unwrap();

    // Only the SHA-256 digest and a short non-secret prefix are stored;
    // verification compares digests, never the raw secret.
    let rows = moltis_db::fetch_all!(
        &db,
        (String, String),
        "SELECT token_hash, token_prefix FROM device_tokens",
    )
    .unwrap();
    assert_eq!(rows.len(), 1);
    let (hash, prefix) = &rows[0];
    assert_eq!(hash, &sha256_hex(&token.token));
    assert_eq!(prefix, &token.token[..12]);
    assert_ne!(hash, &token.token);
}

#[tokio::test]
async fn tampered_or_truncated_token_is_rejected() {
    let db = test_db().await;
    let store = PairingStore::new(db);

    let req = store
        .request_pair("dev-tamper", None, "linux", None)
        .await
        .unwrap();
    let token = store.approve(&req.id).await.unwrap();
    assert!(
        store
            .verify_device_token(&token.token)
            .await
            .unwrap()
            .is_some()
    );

    // One flipped character must invalidate the token.
    let mut tampered = token.token.clone().into_bytes();
    let last = tampered.len() - 1;
    tampered[last] = if tampered[last] == b'A' {
        b'B'
    } else {
        b'A'
    };
    let tampered = String::from_utf8(tampered).unwrap();
    assert!(
        store
            .verify_device_token(&tampered)
            .await
            .unwrap()
            .is_none()
    );

    // Truncation must invalidate the token.
    let truncated = token.token[..token.token.len() - 1].to_string();
    assert!(
        store
            .verify_device_token(&truncated)
            .await
            .unwrap()
            .is_none()
    );
}

// ── Expiry-guarded consume regression tests ─────────────────────────────────

/// Push a pair request's expiry into the past via SQL (test setup).
async fn expire_request(db: &Db, pair_id: &str) {
    moltis_db::execute!(
        db,
        "UPDATE pair_requests SET expires_at = ? WHERE id = ?",
        "2000-01-01T00:00:00Z",
        pair_id,
    )
    .unwrap();
}

#[tokio::test]
async fn expired_approve_races_match_sequential_errors() {
    let db = test_db().await;
    let store = PairingStore::new(db.clone());

    // Sequential error pair: first approve marks the request expired, the
    // second observes it.
    let req = store
        .request_pair("dev-exp-seq", None, "linux", None)
        .await
        .unwrap();
    expire_request(&db, &req.id).await;
    assert!(matches!(
        store.approve(&req.id).await,
        Err(Error::PairRequestExpired)
    ));
    assert!(matches!(
        store.approve(&req.id).await,
        Err(Error::PairRequestNotPending(PairStatus::Expired))
    ));

    // Racing approves on an expired request must produce exactly the same
    // errors, and never a token.
    let req2 = store
        .request_pair("dev-exp-race", None, "linux", None)
        .await
        .unwrap();
    expire_request(&db, &req2.id).await;
    let (a, b) = tokio::join!(store.approve(&req2.id), store.approve(&req2.id));
    let errs = [a.unwrap_err(), b.unwrap_err()];
    let expired = errs
        .iter()
        .filter(|e| matches!(e, Error::PairRequestExpired))
        .count();
    let not_pending = errs
        .iter()
        .filter(|e| matches!(e, Error::PairRequestNotPending(PairStatus::Expired)))
        .count();
    assert_eq!((expired, not_pending), (1, 1));
    assert!(store.list_devices().await.unwrap().is_empty());
}

#[tokio::test]
async fn concurrent_approve_reject_on_expired_request_matches_sequential() {
    let db = test_db().await;
    let store = PairingStore::new(db.clone());

    let req = store
        .request_pair("dev-exp-ar", None, "linux", None)
        .await
        .unwrap();
    expire_request(&db, &req.id).await;

    let (approved, rejected) = tokio::join!(store.approve(&req.id), store.reject(&req.id));

    // Whichever transition lands first, the outcome pair equals one of the two
    // sequential orders (approve-then-reject or reject-then-approve).
    match (approved, rejected) {
        (Err(Error::PairRequestExpired), Err(e)) => {
            assert!(matches!(
                e,
                Error::PairRequestNotPending(PairStatus::Expired)
            ));
        },
        (Err(e), Ok(())) => {
            assert!(matches!(
                e,
                Error::PairRequestNotPending(PairStatus::Rejected)
            ));
        },
        other => panic!("unexpected outcome: {other:?}"),
    }
    assert!(store.list_devices().await.unwrap().is_empty());
}

#[tokio::test]
async fn approve_fails_closed_on_malformed_expiry() {
    let db = test_db().await;
    let store = PairingStore::new(db.clone());

    let req = store
        .request_pair("dev-exp-malformed", None, "linux", None)
        .await
        .unwrap();
    moltis_db::execute!(
        &db,
        "UPDATE pair_requests SET expires_at = ? WHERE id = ?",
        "not-a-datetime",
        &req.id,
    )
    .unwrap();

    // The SQL expiry predicate treats unparsable timestamps as NULL, so the
    // consume refuses the row: garbage expiry must never mint a token.
    assert!(store.approve(&req.id).await.is_err());
    assert!(store.list_devices().await.unwrap().is_empty());
    assert!(
        store
            .list_device_tokens("dev-exp-malformed")
            .await
            .unwrap()
            .is_empty()
    );
}
