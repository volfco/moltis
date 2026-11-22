//! In-memory pairing state (kept for backward compat during transition).

use std::time::Duration;

use time::OffsetDateTime;

use super::{
    expiry::{current_epoch_ms, format_datetime, is_expired_at},
    types::{DeviceToken, Error, PairRequest, PairStatus, Result},
};

// ── In-memory pairing state (kept for backward compat during transition) ────

/// In-memory pairing state for use when no database is available (tests).
pub struct PairingState {
    pending: std::collections::HashMap<String, PairRequest>,
    devices: std::collections::HashMap<String, DeviceToken>,
    pair_ttl: Duration,
}

impl Default for PairingState {
    fn default() -> Self {
        Self::new()
    }
}

impl PairingState {
    pub fn new() -> Self {
        Self {
            pending: std::collections::HashMap::new(),
            devices: std::collections::HashMap::new(),
            pair_ttl: Duration::from_secs(300),
        }
    }

    pub fn request_pair(
        &mut self,
        device_id: &str,
        display_name: Option<&str>,
        platform: &str,
        public_key: Option<&str>,
    ) -> PairRequest {
        let id = uuid::Uuid::new_v4().to_string();
        let nonce = uuid::Uuid::new_v4().to_string();
        let now = OffsetDateTime::now_utc();
        let expires_at = now + self.pair_ttl;
        let req = PairRequest {
            id: id.clone(),
            device_id: device_id.to_string(),
            display_name: display_name.map(|s| s.to_string()),
            platform: platform.to_string(),
            public_key: public_key.map(|s| s.to_string()),
            nonce,
            status: PairStatus::Pending,
            created_at: format_datetime(now),
            expires_at: format_datetime(expires_at),
        };
        self.pending.insert(id, req.clone());
        req
    }

    pub fn list_pending(&self) -> Vec<&PairRequest> {
        let now = OffsetDateTime::now_utc();
        self.pending
            .values()
            .filter(|r| r.status == PairStatus::Pending && !is_expired_at(&r.expires_at, now))
            .collect()
    }

    pub fn approve(&mut self, pair_id: &str) -> Result<DeviceToken> {
        self.approve_at(pair_id, OffsetDateTime::now_utc())
    }

    /// pub(crate) so tests can pin the expiry-boundary semantics with a fixed
    /// `now` (mirrors [`super::PairingStore::list_pending_at`]).
    pub(crate) fn approve_at(&mut self, pair_id: &str, now: OffsetDateTime) -> Result<DeviceToken> {
        let req = self
            .pending
            .get_mut(pair_id)
            .ok_or(Error::PairRequestNotFound)?;
        if req.status != PairStatus::Pending {
            return Err(Error::PairRequestNotPending(req.status));
        }
        if is_expired_at(&req.expires_at, now) {
            req.status = PairStatus::Expired;
            return Err(Error::PairRequestExpired);
        }
        req.status = PairStatus::Approved;

        let token = DeviceToken {
            token: uuid::Uuid::new_v4().to_string(),
            device_id: req.device_id.clone(),
            scopes: vec![
                "operator.read".into(),
                "operator.write".into(),
                "operator.approvals".into(),
            ],
            issued_at_ms: current_epoch_ms(),
            revoked: false,
        };
        self.devices.insert(req.device_id.clone(), token.clone());
        Ok(token)
    }

    pub fn reject(&mut self, pair_id: &str) -> Result<()> {
        let req = self
            .pending
            .get_mut(pair_id)
            .ok_or(Error::PairRequestNotFound)?;
        if req.status != PairStatus::Pending {
            return Err(Error::PairRequestNotPending(req.status));
        }
        req.status = PairStatus::Rejected;
        Ok(())
    }

    pub fn list_devices(&self) -> Vec<&DeviceToken> {
        self.devices.values().filter(|d| !d.revoked).collect()
    }

    pub fn rotate_token(&mut self, device_id: &str) -> Result<DeviceToken> {
        let existing = self
            .devices
            .get_mut(device_id)
            .ok_or(Error::DeviceNotFound)?;
        existing.revoked = true;

        let new_token = DeviceToken {
            token: uuid::Uuid::new_v4().to_string(),
            device_id: device_id.to_string(),
            scopes: existing.scopes.clone(),
            issued_at_ms: current_epoch_ms(),
            revoked: false,
        };
        self.devices
            .insert(device_id.to_string(), new_token.clone());
        Ok(new_token)
    }

    pub fn revoke_token(&mut self, device_id: &str) -> Result<()> {
        let existing = self
            .devices
            .get_mut(device_id)
            .ok_or(Error::DeviceNotFound)?;
        existing.revoked = true;
        Ok(())
    }

    /// Create a pre-authorized device and issue a token directly (no pairing handshake).
    pub fn create_device_token(
        &mut self,
        display_name: Option<&str>,
        platform: &str,
    ) -> DeviceToken {
        let device_id = uuid::Uuid::new_v4().to_string();
        let _ = display_name;
        let _ = platform;
        let token = DeviceToken {
            token: uuid::Uuid::new_v4().to_string(),
            device_id: device_id.clone(),
            scopes: vec![
                "operator.read".into(),
                "operator.write".into(),
                "operator.approvals".into(),
            ],
            issued_at_ms: current_epoch_ms(),
            revoked: false,
        };
        self.devices.insert(device_id, token.clone());
        token
    }

    pub fn evict_expired(&mut self) {
        let now = OffsetDateTime::now_utc();
        self.pending
            .retain(|_, r| !(r.status == PairStatus::Pending && is_expired_at(&r.expires_at, now)));
    }
}
