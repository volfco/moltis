//! Database-backed pairing store (SQLite or PostgreSQL).

use std::time::Duration;

use {
    moltis_db::{Db, execute, fetch_all, fetch_optional},
    time::OffsetDateTime,
};

use super::{
    crypto::{generate_token, sha256_hex},
    expiry::{
        current_epoch_ms, expired_expiry_sql, format_datetime, is_expired, pending_expiry_sql,
    },
    types::{
        DeviceToken, DeviceTokenEntry, DeviceTokenVerification, Error, KeyPinningResult,
        PairRequest, PairStatus, PairedDevice, PinPublicKeyResult, Result,
    },
};

// ── Pairing store ───────────────────────────────────────────────────────────

/// Database-backed pairing store (SQLite or PostgreSQL). Persists pair
/// requests, paired devices, and device tokens across gateway restarts.
pub struct PairingStore {
    db: Db,
    pair_ttl: Duration,
}

impl PairingStore {
    pub fn new(db: Db) -> Self {
        Self {
            db,
            pair_ttl: Duration::from_secs(300), // 5 min
        }
    }

    /// Submit a new pairing request. Returns the generated request with nonce.
    pub async fn request_pair(
        &self,
        device_id: &str,
        display_name: Option<&str>,
        platform: &str,
        public_key: Option<&str>,
    ) -> Result<PairRequest> {
        let id = uuid::Uuid::new_v4().to_string();
        let nonce = uuid::Uuid::new_v4().to_string();
        let now = OffsetDateTime::now_utc();
        let expires_at = now + self.pair_ttl;
        let created_str = format_datetime(now);
        let expires_str = format_datetime(expires_at);

        execute!(
            &self.db,
            "INSERT INTO pair_requests (id, device_id, display_name, platform, public_key, nonce, status, created_at, expires_at)
             VALUES (?, ?, ?, ?, ?, ?, 'pending', ?, ?)",
            &id,
            device_id,
            display_name,
            platform,
            public_key,
            &nonce,
            &created_str,
            &expires_str,
        )?;

        Ok(PairRequest {
            id,
            device_id: device_id.to_string(),
            display_name: display_name.map(|s| s.to_string()),
            platform: platform.to_string(),
            public_key: public_key.map(|s| s.to_string()),
            nonce,
            status: PairStatus::Pending,
            created_at: created_str,
            expires_at: expires_str,
        })
    }

    /// List all non-expired pending requests.
    pub async fn list_pending(&self) -> Result<Vec<PairRequest>> {
        let now = format_datetime(OffsetDateTime::now_utc());
        self.list_pending_at(&now).await
    }

    /// pub(crate) so the PostgreSQL smoke tests can pin the expiry-dialect
    /// semantics with a fixed `now` (mirrors the SQLite test of the same name).
    pub(crate) async fn list_pending_at(&self, now: &str) -> Result<Vec<PairRequest>> {
        let rows = fetch_all!(
            &self.db,
            (String, String, Option<String>, String, Option<String>, String, String, String),
            format!(
                "SELECT id, device_id, display_name, platform, public_key, nonce, created_at, expires_at
                 FROM pair_requests
                 WHERE status = 'pending' AND {}",
                pending_expiry_sql(self.db.dialect()),
            ),
            now,
        )?;

        Ok(rows
            .into_iter()
            .map(
                |(
                    id,
                    device_id,
                    display_name,
                    platform,
                    public_key,
                    nonce,
                    created_at,
                    expires_at,
                )| {
                    PairRequest {
                        id,
                        device_id,
                        display_name,
                        platform,
                        public_key,
                        nonce,
                        status: PairStatus::Pending,
                        created_at,
                        expires_at,
                    }
                },
            )
            .collect())
    }

    /// Approve a pending pair request. Issues a device token.
    pub async fn approve(&self, pair_id: &str) -> Result<DeviceToken> {
        // Load and validate the request.
        let row = fetch_optional!(
            &self.db,
            (
                String,
                Option<String>,
                String,
                Option<String>,
                String,
                String
            ),
            "SELECT device_id, display_name, platform, public_key, status, expires_at
                 FROM pair_requests WHERE id = ?",
            pair_id,
        )?;

        let (device_id, display_name, platform, public_key, status, expires_at) =
            row.ok_or(Error::PairRequestNotFound)?;

        let current_status = PairStatus::from_str(&status);
        if current_status != PairStatus::Pending {
            return Err(Error::PairRequestNotPending(current_status));
        }

        // Check expiry.
        if is_expired(&expires_at) {
            execute!(
                &self.db,
                "UPDATE pair_requests SET status = 'expired' WHERE id = ? AND status = 'pending'",
                pair_id,
            )?;
            return Err(Error::PairRequestExpired);
        }

        // Atomically consume the pending request so concurrent approve/reject
        // calls cannot both succeed and issue duplicate tokens.
        let consumed = execute!(
            &self.db,
            "UPDATE pair_requests SET status = 'approved' WHERE id = ? AND status = 'pending'",
            pair_id,
        )?;
        if consumed == 0 {
            return Err(self.consume_error(pair_id).await);
        }

        // Upsert the paired device.
        execute!(
            &self.db,
            "INSERT INTO paired_devices (device_id, display_name, platform, public_key, status)
             VALUES (?, ?, ?, ?, 'active')
             ON CONFLICT(device_id) DO UPDATE SET
                display_name = excluded.display_name,
                platform = excluded.platform,
                public_key = excluded.public_key,
                status = 'active',
                revoked_at = NULL",
            &device_id,
            &display_name,
            &platform,
            &public_key,
        )?;

        // Issue a device token.
        let raw_token = format!("mdt_{}", generate_token());
        let token_hash = sha256_hex(&raw_token);
        let token_prefix = &raw_token[..raw_token.len().min(12)];
        let scopes = vec![
            "operator.read".to_string(),
            "operator.write".to_string(),
            "operator.approvals".to_string(),
        ];
        let scopes_json = serde_json::to_string(&scopes).unwrap_or_default();
        let issued_at_ms = current_epoch_ms();

        execute!(
            &self.db,
            "INSERT INTO device_tokens (token_hash, token_prefix, device_id, scopes)
             VALUES (?, ?, ?, ?)",
            &token_hash,
            token_prefix,
            &device_id,
            &scopes_json,
        )?;

        Ok(DeviceToken {
            token: raw_token,
            device_id,
            scopes,
            issued_at_ms,
            revoked: false,
        })
    }

    /// Reject a pending pair request.
    pub async fn reject(&self, pair_id: &str) -> Result<()> {
        // Atomic check-and-consume: exactly one of concurrent approve/reject wins.
        let consumed = execute!(
            &self.db,
            "UPDATE pair_requests SET status = 'rejected' WHERE id = ? AND status = 'pending'",
            pair_id,
        )?;
        if consumed == 0 {
            return Err(self.consume_error(pair_id).await);
        }

        Ok(())
    }

    /// Error for a lost check-and-consume race: re-read the row so the caller
    /// sees the actual current status.
    async fn consume_error(&self, pair_id: &str) -> Error {
        match fetch_optional!(
            &self.db,
            (String,),
            "SELECT status FROM pair_requests WHERE id = ?",
            pair_id,
        ) {
            Ok(Some((status,))) => Error::PairRequestNotPending(PairStatus::from_str(&status)),
            Ok(None) => Error::PairRequestNotFound,
            Err(e) => Error::Db(e),
        }
    }

    /// List all approved (non-revoked) devices.
    pub async fn list_devices(&self) -> Result<Vec<PairedDevice>> {
        let rows = fetch_all!(
            &self.db,
            (String, Option<String>, String, Option<String>, String),
            "SELECT device_id, display_name, platform, public_key, created_at
             FROM paired_devices WHERE status = 'active'
             ORDER BY created_at DESC",
        )?;

        Ok(rows
            .into_iter()
            .map(
                |(device_id, display_name, platform, public_key, created_at)| PairedDevice {
                    device_id,
                    display_name,
                    platform,
                    public_key,
                    created_at,
                },
            )
            .collect())
    }

    /// List device tokens for a specific device (active only).
    pub async fn list_device_tokens(&self, device_id: &str) -> Result<Vec<DeviceTokenEntry>> {
        let rows = fetch_all!(
            &self.db,
            (String, String, String),
            "SELECT token_prefix, scopes, issued_at FROM device_tokens
             WHERE device_id = ? AND revoked = 0
             ORDER BY issued_at DESC",
            device_id,
        )?;

        Ok(rows
            .into_iter()
            .map(|(token_prefix, scopes_json, issued_at)| {
                let scopes = serde_json::from_str::<Vec<String>>(&scopes_json).unwrap_or_default();
                DeviceTokenEntry {
                    token_prefix,
                    device_id: device_id.to_string(),
                    scopes,
                    issued_at,
                }
            })
            .collect())
    }

    /// Rotate a device token: revoke all existing tokens, issue new.
    pub async fn rotate_token(&self, device_id: &str) -> Result<DeviceToken> {
        // Verify device exists and is active.
        let exists = fetch_optional!(
            &self.db,
            (String,),
            "SELECT status FROM paired_devices WHERE device_id = ?",
            device_id,
        )?;

        if exists.is_none() {
            return Err(Error::DeviceNotFound);
        }

        // Load current scopes from any active token.
        let scopes_row = fetch_optional!(
            &self.db,
            (String,),
            "SELECT scopes FROM device_tokens WHERE device_id = ? AND revoked = 0 LIMIT 1",
            device_id,
        )?;

        let scopes = scopes_row
            .and_then(|(s,)| serde_json::from_str::<Vec<String>>(&s).ok())
            .unwrap_or_else(|| {
                vec![
                    "operator.read".into(),
                    "operator.write".into(),
                    "operator.approvals".into(),
                ]
            });

        // Revoke all existing tokens.
        execute!(
            &self.db,
            "UPDATE device_tokens SET revoked = 1 WHERE device_id = ? AND revoked = 0",
            device_id,
        )?;

        // Issue new token.
        let raw_token = format!("mdt_{}", generate_token());
        let token_hash = sha256_hex(&raw_token);
        let token_prefix = &raw_token[..raw_token.len().min(12)];
        let scopes_json = serde_json::to_string(&scopes).unwrap_or_default();
        let issued_at_ms = current_epoch_ms();

        execute!(
            &self.db,
            "INSERT INTO device_tokens (token_hash, token_prefix, device_id, scopes)
             VALUES (?, ?, ?, ?)",
            &token_hash,
            token_prefix,
            device_id,
            &scopes_json,
        )?;

        Ok(DeviceToken {
            token: raw_token,
            device_id: device_id.to_string(),
            scopes,
            issued_at_ms,
            revoked: false,
        })
    }

    /// Revoke all tokens for a device and mark device as revoked.
    pub async fn revoke_token(&self, device_id: &str) -> Result<()> {
        let exists = fetch_optional!(
            &self.db,
            (String,),
            "SELECT status FROM paired_devices WHERE device_id = ?",
            device_id,
        )?;

        if exists.is_none() {
            return Err(Error::DeviceNotFound);
        }

        execute!(
            &self.db,
            "UPDATE device_tokens SET revoked = 1 WHERE device_id = ?",
            device_id,
        )?;

        execute!(
            &self.db,
            format!(
                "UPDATE paired_devices SET status = 'revoked', revoked_at = {} WHERE device_id = ?",
                self.db.dialect().now_expr(),
            ),
            device_id,
        )?;

        Ok(())
    }

    /// Verify a raw device token. Returns device identity and scopes if valid.
    pub async fn verify_device_token(
        &self,
        raw_token: &str,
    ) -> Result<Option<DeviceTokenVerification>> {
        let hash = sha256_hex(raw_token);

        let row = fetch_optional!(
            &self.db,
            (String, String),
            "SELECT device_id, scopes FROM device_tokens
             WHERE token_hash = ? AND revoked = 0",
            &hash,
        )?;

        match row {
            Some((device_id, scopes_json)) => {
                // Also verify the device is still active.
                let device_active = fetch_optional!(
                    &self.db,
                    (String,),
                    "SELECT status FROM paired_devices WHERE device_id = ? AND status = 'active'",
                    &device_id,
                )?;

                if device_active.is_none() {
                    return Ok(None);
                }

                let scopes = serde_json::from_str::<Vec<String>>(&scopes_json).unwrap_or_default();
                Ok(Some(DeviceTokenVerification { device_id, scopes }))
            },
            None => Ok(None),
        }
    }

    /// Create a pre-authorized device and issue a token directly (no pairing handshake).
    pub async fn create_device_token(
        &self,
        display_name: Option<&str>,
        platform: &str,
    ) -> Result<DeviceToken> {
        let device_id = uuid::Uuid::new_v4().to_string();

        // Insert as an active paired device.
        execute!(
            &self.db,
            "INSERT INTO paired_devices (device_id, display_name, platform, public_key, status)
             VALUES (?, ?, ?, NULL, 'active')",
            &device_id,
            display_name,
            platform,
        )?;

        // Issue a device token.
        let raw_token = format!("mdt_{}", generate_token());
        let token_hash = sha256_hex(&raw_token);
        let token_prefix = &raw_token[..raw_token.len().min(12)];
        let scopes = vec![
            "operator.read".to_string(),
            "operator.write".to_string(),
            "operator.approvals".to_string(),
        ];
        let scopes_json = serde_json::to_string(&scopes).unwrap_or_default();
        let issued_at_ms = current_epoch_ms();

        execute!(
            &self.db,
            "INSERT INTO device_tokens (token_hash, token_prefix, device_id, scopes)
             VALUES (?, ?, ?, ?)",
            &token_hash,
            token_prefix,
            &device_id,
            &scopes_json,
        )?;

        Ok(DeviceToken {
            token: raw_token,
            device_id,
            scopes,
            issued_at_ms,
            revoked: false,
        })
    }

    /// Look up an active paired device by its Ed25519 public key (base64).
    ///
    /// Returns `None` if no device is associated with this key, or if the
    /// device has been revoked.
    pub async fn find_device_by_public_key(
        &self,
        public_key: &str,
    ) -> Result<Option<PairedDevice>> {
        let row = fetch_optional!(
            &self.db,
            (String, Option<String>, String, Option<String>, String),
            "SELECT device_id, display_name, platform, public_key, created_at
                 FROM paired_devices
                 WHERE public_key = ? AND status = 'active'",
            public_key,
        )?;

        Ok(row.map(
            |(device_id, display_name, platform, public_key, created_at)| PairedDevice {
                device_id,
                display_name,
                platform,
                public_key,
                created_at,
            },
        ))
    }

    /// Check whether a public key has been revoked (not just absent).
    pub async fn is_public_key_revoked(&self, public_key: &str) -> Result<bool> {
        let row = fetch_optional!(
            &self.db,
            (String,),
            "SELECT status FROM paired_devices WHERE public_key = ? AND status = 'revoked'",
            public_key,
        )?;
        Ok(row.is_some())
    }

    /// Pin a public key to an existing device (used during token→key migration).
    /// Only sets the key if the device has no key pinned yet, and reports
    /// conflicts explicitly so callers do not mistake a zero-row update for a
    /// successful migration.
    pub async fn pin_public_key(
        &self,
        device_id: &str,
        public_key: &str,
    ) -> Result<PinPublicKeyResult> {
        let row = fetch_optional!(
            &self.db,
            (Option<String>, String),
            "SELECT public_key, status FROM paired_devices WHERE device_id = ?",
            device_id,
        )?;

        match row {
            None => Ok(PinPublicKeyResult::DeviceNotFound),
            Some((_, status)) if status == "revoked" => Ok(PinPublicKeyResult::Revoked),
            Some((Some(stored_key), status)) if status == "active" => {
                if stored_key == public_key {
                    Ok(PinPublicKeyResult::AlreadyPinned)
                } else {
                    Ok(PinPublicKeyResult::Mismatch {
                        expected: stored_key,
                    })
                }
            },
            Some((None, status)) if status == "active" => {
                let affected = execute!(
                    &self.db,
                    "UPDATE paired_devices SET public_key = ?
                     WHERE device_id = ? AND status = 'active' AND public_key IS NULL",
                    public_key,
                    device_id,
                )?;

                if affected > 0 {
                    Ok(PinPublicKeyResult::Pinned)
                } else {
                    let refreshed = fetch_optional!(
                        &self.db,
                        (Option<String>, String),
                        "SELECT public_key, status FROM paired_devices WHERE device_id = ?",
                        device_id,
                    )?;
                    match refreshed {
                        Some((Some(stored_key), status))
                            if status == "active" && stored_key == public_key =>
                        {
                            Ok(PinPublicKeyResult::AlreadyPinned)
                        },
                        Some((Some(stored_key), status)) if status == "active" => {
                            Ok(PinPublicKeyResult::Mismatch {
                                expected: stored_key,
                            })
                        },
                        Some((_, status)) if status == "revoked" => Ok(PinPublicKeyResult::Revoked),
                        None => Ok(PinPublicKeyResult::DeviceNotFound),
                        Some(..) => Ok(PinPublicKeyResult::DeviceNotFound),
                    }
                }
            },
            Some(..) => Ok(PinPublicKeyResult::DeviceNotFound),
        }
    }

    /// TOFU key pinning check: verify that a device's presented public key
    /// matches its stored key. Returns `Ok(true)` if the keys match or no key
    /// is pinned yet, `Ok(false)` if a different key is already pinned, and
    /// the expected fingerprint for the security warning.
    pub async fn check_key_pinning(
        &self,
        device_id: &str,
        presented_key: &str,
    ) -> Result<KeyPinningResult> {
        let row = fetch_optional!(
            &self.db,
            (Option<String>, String),
            "SELECT public_key, status FROM paired_devices WHERE device_id = ?",
            device_id,
        )?;

        match row {
            Some((Some(stored_key), status)) if status == "active" => {
                if stored_key == presented_key {
                    Ok(KeyPinningResult::Match)
                } else {
                    Ok(KeyPinningResult::Mismatch {
                        expected: stored_key,
                    })
                }
            },
            Some((None, status)) if status == "active" => Ok(KeyPinningResult::NoPinnedKey),
            Some((_, status)) if status == "revoked" => Ok(KeyPinningResult::Revoked),
            Some(..) => Ok(KeyPinningResult::NoPinnedKey),
            None => Ok(KeyPinningResult::NoPinnedKey), // unknown device
        }
    }

    /// Get the current status of a pair request by ID.
    pub async fn get_pair_status(&self, pair_id: &str) -> Result<Option<PairStatus>> {
        let row = fetch_optional!(
            &self.db,
            (String,),
            "SELECT status FROM pair_requests WHERE id = ?",
            pair_id,
        )?;
        Ok(row.map(|(s,)| PairStatus::from_str(&s)))
    }

    /// Evict expired pending requests.
    pub async fn evict_expired(&self) -> Result<u64> {
        let now = format_datetime(OffsetDateTime::now_utc());
        self.evict_expired_at(&now).await
    }

    /// pub(crate) for the PostgreSQL expiry-semantics smoke test, like
    /// [`PairingStore::list_pending_at`].
    pub(crate) async fn evict_expired_at(&self, now: &str) -> Result<u64> {
        let affected = execute!(
            &self.db,
            format!(
                "UPDATE pair_requests SET status = 'expired'
                 WHERE status = 'pending' AND {}",
                expired_expiry_sql(self.db.dialect()),
            ),
            now,
        )?;
        Ok(affected)
    }
}
