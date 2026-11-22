//! Pairing domain types.

use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("pair request not found")]
    PairRequestNotFound,

    #[error("pair request already {0:?}")]
    PairRequestNotPending(PairStatus),

    #[error("pair request expired")]
    PairRequestExpired,

    #[error("device not found")]
    DeviceNotFound,

    #[error("invalid device token")]
    InvalidToken,

    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

impl From<moltis_db::DbError> for Error {
    fn from(e: moltis_db::DbError) -> Self {
        match e {
            moltis_db::DbError::Sqlx(e) => Error::Db(e),
            // Keep the public Error enum unchanged: surface the rare non-sqlx
            // variants through the existing Db variant.
            other => Error::Db(sqlx::Error::Protocol(other.to_string())),
        }
    }
}

// ── Types ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PairStatus {
    Pending,
    Approved,
    Rejected,
    Expired,
}

impl PairStatus {
    /// String representation used for DB storage and serialization.
    #[allow(dead_code)]
    fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Approved => "approved",
            Self::Rejected => "rejected",
            Self::Expired => "expired",
        }
    }

    pub(super) fn from_str(s: &str) -> Self {
        match s {
            "approved" => Self::Approved,
            "rejected" => Self::Rejected,
            "expired" => Self::Expired,
            _ => Self::Pending,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PairRequest {
    pub id: String,
    pub device_id: String,
    pub display_name: Option<String>,
    pub platform: String,
    pub public_key: Option<String>,
    pub nonce: String,
    pub status: PairStatus,
    pub created_at: String,
    pub expires_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceToken {
    pub token: String,
    pub device_id: String,
    pub scopes: Vec<String>,
    pub issued_at_ms: u64,
    pub revoked: bool,
}

/// Result of verifying a device token.
#[derive(Debug, Clone)]
pub struct DeviceTokenVerification {
    pub device_id: String,
    pub scopes: Vec<String>,
}

/// Result of a TOFU key pinning check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyPinningResult {
    /// Presented key matches the stored (pinned) key.
    Match,
    /// A different key is already pinned for this device.
    Mismatch { expected: String },
    /// No key is pinned yet (new device or no stored key).
    NoPinnedKey,
    /// The device has been revoked — reject regardless of key.
    Revoked,
}

/// Result of pinning a public key to an existing active device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PinPublicKeyResult {
    /// The device had no key and this call pinned it.
    Pinned,
    /// The same key was already pinned.
    AlreadyPinned,
    /// A different key is already pinned for this device.
    Mismatch { expected: String },
    /// The device has been revoked.
    Revoked,
    /// No device exists for the supplied ID.
    DeviceNotFound,
}

// ── Additional types ────────────────────────────────────────────────────────

/// A paired device (for listing).
#[derive(Debug, Clone, Serialize)]
pub struct PairedDevice {
    pub device_id: String,
    pub display_name: Option<String>,
    pub platform: String,
    pub public_key: Option<String>,
    pub created_at: String,
}

/// A device token entry (for listing — never exposes raw token).
#[derive(Debug, Clone, Serialize)]
pub struct DeviceTokenEntry {
    pub token_prefix: String,
    pub device_id: String,
    pub scopes: Vec<String>,
    pub issued_at: String,
}
