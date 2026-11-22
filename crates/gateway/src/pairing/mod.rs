//! Device pairing state machine and device token management.
//!
//! Pairing state is persisted to SQLite so it survives gateway restarts.
//! An in-memory cache provides fast reads; all mutations write through to DB.

mod crypto;
mod expiry;
mod state;
mod store;
mod types;

#[cfg(test)]
mod tests;

pub use {
    crypto::{generate_challenge_nonce, public_key_fingerprint, verify_ed25519_challenge},
    state::PairingState,
    store::PairingStore,
    types::{
        DeviceToken, DeviceTokenEntry, DeviceTokenVerification, Error, KeyPinningResult,
        PairRequest, PairStatus, PairedDevice, PinPublicKeyResult, Result,
    },
};
