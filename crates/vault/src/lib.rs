//! Encryption-at-rest vault using XChaCha20-Poly1305.
//!
//! A random DEK (Data Encryption Key) is wrapped with a password-derived KEK
//! via Argon2id. The vault must be password-unlocked once per process start.
//! Trait-based [`Cipher`] design allows swapping the encryption backend.

pub mod error;
pub mod kdf;
pub mod key_wrap;
pub mod migration;
pub mod recovery;
#[cfg(test)]
pub(crate) mod test_support;
pub mod traits;
pub mod vault;
pub mod xchacha20;

pub use {
    error::VaultError,
    recovery::RecoveryKey,
    traits::Cipher,
    vault::{Vault, VaultStatus},
    xchacha20::XChaCha20Poly1305Cipher,
};

/// Run database migrations for the vault crate.
///
/// Creates the `vault_metadata` table on either backend. Should be called at
/// application startup after cron migrations and before gateway migrations.
pub async fn run_migrations(db: &moltis_db::Db) -> anyhow::Result<()> {
    moltis_db::run_migrations(
        db,
        sqlx::migrate!("./migrations"),
        sqlx::migrate!("./migrations_pg"),
    )
    .await?;
    Ok(())
}
