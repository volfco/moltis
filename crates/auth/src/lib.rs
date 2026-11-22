//! Authentication and credential management.
//!
//! This crate provides:
//! - `CredentialStore`: database-backed storage (SQLite or PostgreSQL) for passwords, passkeys, API keys, and sessions
//! - `WebAuthnState`/`WebAuthnRegistry`: WebAuthn (passkey) challenge management
//! - Connection locality detection for auth decisions

pub mod credential_store;
pub mod error;
pub mod locality;
#[cfg(test)]
pub(crate) mod test_support;
pub mod webauthn;

pub use {
    credential_store::*,
    error::{Error, Result},
    locality::{has_proxy_headers, is_local_connection},
    webauthn::{WebAuthnRegistry, WebAuthnState, load_passkeys},
};

#[cfg(feature = "vault")]
pub use moltis_vault;
