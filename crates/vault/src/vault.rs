//! Vault state machine: initialization, seal/unseal, encrypt/decrypt.

use {
    base64::Engine,
    moltis_db::{Db, DbTx},
    tokio::sync::RwLock,
    zeroize::Zeroizing,
};

use crate::{
    error::VaultError,
    kdf::{self, KdfParams},
    key_wrap,
    recovery::{self, RecoveryKey},
    traits::Cipher,
    xchacha20::XChaCha20Poly1305Cipher,
};

/// Vault status exposed to the API / frontend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VaultStatus {
    /// No password has been set — vault doesn't exist yet.
    Uninitialized,
    /// Vault exists but the DEK is not in memory (needs password).
    Sealed,
    /// Vault is unlocked — DEK is held in memory.
    Unsealed,
}

/// Row from the `vault_metadata` table.
#[derive(Debug)]
struct VaultRow {
    kdf_salt: String,
    kdf_params: String,
    wrapped_dek: String,
    recovery_wrapped_dek: Option<String>,
    #[allow(dead_code)]
    recovery_key_hash: Option<String>,
}

/// Encryption-at-rest vault.
///
/// Generic over [`Cipher`] but defaults to [`XChaCha20Poly1305Cipher`].
/// The DEK is held in memory behind a `RwLock` — `None` means sealed.
pub struct Vault<C: Cipher = XChaCha20Poly1305Cipher> {
    db: Db,
    cipher: C,
    dek: RwLock<Option<Zeroizing<[u8; 32]>>>,
}

impl Vault<XChaCha20Poly1305Cipher> {
    /// Create a vault with the default XChaCha20-Poly1305 cipher.
    pub async fn new(db: Db) -> Result<Self, VaultError> {
        Self::with_cipher(db, XChaCha20Poly1305Cipher).await
    }
}

impl<C: Cipher> Vault<C> {
    /// Create a vault with a custom cipher.
    pub async fn with_cipher(db: Db, cipher: C) -> Result<Self, VaultError> {
        Ok(Self {
            db,
            cipher,
            dek: RwLock::new(None),
        })
    }

    /// Query the current vault status.
    pub async fn status(&self) -> Result<VaultStatus, VaultError> {
        let row = self.load_metadata().await?;
        match row {
            None => Ok(VaultStatus::Uninitialized),
            Some(_) => {
                let guard = self.dek.read().await;
                if guard.is_some() {
                    Ok(VaultStatus::Unsealed)
                } else {
                    Ok(VaultStatus::Sealed)
                }
            },
        }
    }

    /// Initialize the vault with a password.
    ///
    /// Generates a random DEK, wraps it with the password-derived KEK,
    /// generates a recovery key, and stores everything in the database.
    /// Returns the recovery key (shown to the user exactly once).
    pub async fn initialize(&self, password: &str) -> Result<RecoveryKey, VaultError> {
        let mut tx = self.db.begin().await?;
        let recovery_key = self.initialize_in_transaction(password, &mut tx).await?;
        tx.commit().await?;
        self.unseal(password).await?;

        #[cfg(feature = "tracing")]
        tracing::info!("vault initialized");

        Ok(recovery_key)
    }

    /// Initialize the vault as part of a caller-owned database transaction.
    ///
    /// The vault remains sealed until the caller commits and explicitly unseals
    /// it, so rolled-back transactions cannot leave an in-memory DEK live.
    pub async fn initialize_in_transaction(
        &self,
        password: &str,
        tx: &mut DbTx,
    ) -> Result<RecoveryKey, VaultError> {
        // Ensure vault doesn't already exist.
        if self.load_metadata_in_transaction(tx).await?.is_some() {
            return Err(VaultError::AlreadyInitialized);
        }

        // Generate random DEK.
        let mut dek = Zeroizing::new([0u8; 32]);
        rand::Rng::fill_bytes(&mut rand::rng(), dek.as_mut());

        // Derive KEK from password.
        let salt_b64 = kdf::generate_salt();
        let salt = kdf::decode_salt(&salt_b64)?;
        let params = KdfParams::default();
        let kek = kdf::derive_key(password.as_bytes(), &salt, &params)?;

        // Wrap DEK with password KEK.
        let wrapped_dek = key_wrap::wrap_dek(&self.cipher, &kek, &dek)?;

        // Generate recovery key and wrap DEK with it.
        let recovery_key = recovery::generate_recovery_key();
        let (recovery_wrapped, recovery_hash) =
            recovery::wrap_with_recovery(&self.cipher, &dek, recovery_key.phrase())?;

        // Store in database.
        let params_json = serde_json::to_string(&params)?;
        moltis_db::tx_execute!(
            tx,
            "INSERT INTO vault_metadata (id, kdf_salt, kdf_params, wrapped_dek, recovery_wrapped_dek, recovery_key_hash)
             VALUES (1, ?, ?, ?, ?, ?)",
            &salt_b64,
            &params_json,
            &wrapped_dek,
            &recovery_wrapped,
            &recovery_hash,
        )?;

        Ok(recovery_key)
    }

    /// Unseal the vault using a password.
    pub async fn unseal(&self, password: &str) -> Result<(), VaultError> {
        let row = self
            .load_metadata()
            .await?
            .ok_or(VaultError::NotInitialized)?;

        let salt = kdf::decode_salt(&row.kdf_salt)?;
        let params: KdfParams = serde_json::from_str(&row.kdf_params)?;
        let kek = kdf::derive_key(password.as_bytes(), &salt, &params)?;

        let dek = key_wrap::unwrap_dek(&self.cipher, &kek, &row.wrapped_dek)
            .map_err(|_| VaultError::BadCredential)?;

        *self.dek.write().await = Some(dek);

        #[cfg(feature = "tracing")]
        tracing::info!("vault unsealed");

        Ok(())
    }

    /// Unseal the vault using a recovery phrase.
    pub async fn unseal_with_recovery(&self, phrase: &str) -> Result<(), VaultError> {
        let row = self
            .load_metadata()
            .await?
            .ok_or(VaultError::NotInitialized)?;

        let recovery_wrapped = row.recovery_wrapped_dek.ok_or(VaultError::CipherError(
            "no recovery key configured".to_string(),
        ))?;

        let dek = recovery::unwrap_with_recovery(&self.cipher, &recovery_wrapped, phrase)
            .map_err(|_| VaultError::BadCredential)?;

        *self.dek.write().await = Some(dek);

        #[cfg(feature = "tracing")]
        tracing::info!("vault unsealed via recovery key");

        Ok(())
    }

    /// Seal the vault (clear the DEK from memory).
    pub async fn seal(&self) {
        *self.dek.write().await = None;

        #[cfg(feature = "tracing")]
        tracing::info!("vault sealed");
    }

    /// Change the password. Re-wraps the DEK with the new password's KEK.
    ///
    /// The vault must already be unsealed (DEK in memory).
    pub async fn change_password(&self, old: &str, new: &str) -> Result<(), VaultError> {
        let mut tx = self.db.begin().await?;
        self.change_password_in_transaction(old, new, &mut tx)
            .await?;
        tx.commit().await?;

        #[cfg(feature = "tracing")]
        tracing::info!("vault password changed (DEK re-wrapped)");

        Ok(())
    }

    /// Change the password as part of a caller-owned database transaction.
    pub async fn change_password_in_transaction(
        &self,
        old: &str,
        new: &str,
        tx: &mut DbTx,
    ) -> Result<(), VaultError> {
        let row = self
            .load_metadata_in_transaction(tx)
            .await?
            .ok_or(VaultError::NotInitialized)?;

        // Verify old password by unwrapping DEK.
        let salt = kdf::decode_salt(&row.kdf_salt)?;
        let params: KdfParams = serde_json::from_str(&row.kdf_params)?;
        let old_kek = kdf::derive_key(old.as_bytes(), &salt, &params)?;
        let _dek = key_wrap::unwrap_dek(&self.cipher, &old_kek, &row.wrapped_dek)
            .map_err(|_| VaultError::BadCredential)?;

        // Read current DEK from memory.
        let guard = self.dek.read().await;
        let dek = guard.as_ref().ok_or(VaultError::Sealed)?;

        // Derive new KEK with fresh salt.
        let new_salt_b64 = kdf::generate_salt();
        let new_salt = kdf::decode_salt(&new_salt_b64)?;
        let new_kek = kdf::derive_key(new.as_bytes(), &new_salt, &params)?;

        // Re-wrap DEK.
        let new_wrapped = key_wrap::wrap_dek(&self.cipher, &new_kek, dek)?;
        let params_json = serde_json::to_string(&params)?;

        drop(guard);

        self.update_wrapped_dek(tx, &new_salt_b64, &params_json, &new_wrapped)
            .await
    }

    /// Re-wrap the DEK with a new password without requiring the old password.
    ///
    /// This is only available while the vault is already unsealed, such as after
    /// recovery-key unlock, where possession of the in-memory DEK is the proof
    /// needed to rotate the password wrapper.
    pub async fn rewrap_unsealed(&self, new: &str) -> Result<(), VaultError> {
        let mut tx = self.db.begin().await?;
        self.rewrap_unsealed_in_transaction(new, &mut tx).await?;
        tx.commit().await?;

        #[cfg(feature = "tracing")]
        tracing::info!("vault password wrapper rekeyed from unsealed DEK");

        Ok(())
    }

    /// Re-wrap the DEK with a new password as part of a caller-owned database transaction.
    pub async fn rewrap_unsealed_in_transaction(
        &self,
        new: &str,
        tx: &mut DbTx,
    ) -> Result<(), VaultError> {
        let row = self
            .load_metadata_in_transaction(tx)
            .await?
            .ok_or(VaultError::NotInitialized)?;

        let params: KdfParams = serde_json::from_str(&row.kdf_params)?;
        let guard = self.dek.read().await;
        let dek = guard.as_ref().ok_or(VaultError::Sealed)?;

        let new_salt_b64 = kdf::generate_salt();
        let new_salt = kdf::decode_salt(&new_salt_b64)?;
        let new_kek = kdf::derive_key(new.as_bytes(), &new_salt, &params)?;
        let new_wrapped = key_wrap::wrap_dek(&self.cipher, &new_kek, dek)?;
        let params_json = serde_json::to_string(&params)?;

        drop(guard);

        self.update_wrapped_dek(tx, &new_salt_b64, &params_json, &new_wrapped)
            .await
    }

    /// Replace the password-wrapped DEK and KDF material inside `tx`.
    async fn update_wrapped_dek(
        &self,
        tx: &mut DbTx,
        salt_b64: &str,
        params_json: &str,
        wrapped_dek: &str,
    ) -> Result<(), VaultError> {
        let now = self.db.dialect().now_expr();
        moltis_db::tx_execute!(
            tx,
            &format!(
                "UPDATE vault_metadata SET kdf_salt = ?, kdf_params = ?, wrapped_dek = ?, \
                 updated_at = {now} WHERE id = 1"
            ),
            salt_b64,
            params_json,
            wrapped_dek,
        )?;
        Ok(())
    }

    /// Encrypt a string and return a versioned base64 blob.
    ///
    /// The AAD (additional authenticated data) should identify the context,
    /// e.g. `"env:MY_KEY"` or `"provider_keys"`.
    pub async fn encrypt_string(&self, plaintext: &str, aad: &str) -> Result<String, VaultError> {
        let guard = self.dek.read().await;
        let dek = guard.as_ref().ok_or(VaultError::Sealed)?;

        let encrypted = self
            .cipher
            .encrypt(dek, plaintext.as_bytes(), aad.as_bytes())?;

        // Prepend version tag.
        let mut blob = Vec::with_capacity(1 + encrypted.len());
        blob.push(self.cipher.version_tag());
        blob.extend_from_slice(&encrypted);

        Ok(base64::engine::general_purpose::STANDARD.encode(blob))
    }

    /// Decrypt a base64 blob back to a string.
    pub async fn decrypt_string(&self, b64: &str, aad: &str) -> Result<String, VaultError> {
        let guard = self.dek.read().await;
        let dek = guard.as_ref().ok_or(VaultError::Sealed)?;

        let blob = base64::engine::general_purpose::STANDARD.decode(b64)?;
        if blob.is_empty() {
            return Err(VaultError::CipherError("empty blob".to_string()));
        }

        let version = blob[0];
        if version != self.cipher.version_tag() {
            return Err(VaultError::CipherError(format!(
                "unsupported cipher version: {version:#04x}"
            )));
        }

        let plaintext = self.cipher.decrypt(dek, &blob[1..], aad.as_bytes())?;
        String::from_utf8(plaintext).map_err(|e| VaultError::CipherError(e.to_string()))
    }

    /// Whether the vault is currently unsealed (DEK in memory).
    pub async fn is_unsealed(&self) -> bool {
        self.dek.read().await.is_some()
    }

    /// Load vault metadata from the database.
    async fn load_metadata(&self) -> Result<Option<VaultRow>, VaultError> {
        let row: Option<(String, String, String, Option<String>, Option<String>)> = moltis_db::fetch_optional!(
            &self.db,
            (String, String, String, Option<String>, Option<String>),
            "SELECT kdf_salt, kdf_params, wrapped_dek, recovery_wrapped_dek, recovery_key_hash
                 FROM vault_metadata WHERE id = 1",
        )?;

        Ok(row.map(
            |(kdf_salt, kdf_params, wrapped_dek, recovery_wrapped_dek, recovery_key_hash)| {
                VaultRow {
                    kdf_salt,
                    kdf_params,
                    wrapped_dek,
                    recovery_wrapped_dek,
                    recovery_key_hash,
                }
            },
        ))
    }

    async fn load_metadata_in_transaction(
        &self,
        tx: &mut DbTx,
    ) -> Result<Option<VaultRow>, VaultError> {
        let row: Option<(String, String, String, Option<String>, Option<String>)> = moltis_db::tx_fetch_optional!(
            tx,
            (String, String, String, Option<String>, Option<String>),
            "SELECT kdf_salt, kdf_params, wrapped_dek, recovery_wrapped_dek, recovery_key_hash
                 FROM vault_metadata WHERE id = 1",
        )?;

        Ok(row.map(
            |(kdf_salt, kdf_params, wrapped_dek, recovery_wrapped_dek, recovery_key_hash)| {
                VaultRow {
                    kdf_salt,
                    kdf_params,
                    wrapped_dek,
                    recovery_wrapped_dek,
                    recovery_key_hash,
                }
            },
        ))
    }
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use {
        super::*,
        crate::{
            test_support::{Backend, test_db},
            xchacha20::XChaCha20Poly1305Cipher,
        },
    };

    fn test_password() -> String {
        let token = rand::random::<u64>();
        format!("vault-test-{token}")
    }

    async fn status_uninitialized_case(db: &Db) {
        let vault = Vault::with_cipher(db.clone(), XChaCha20Poly1305Cipher)
            .await
            .unwrap();
        assert_eq!(vault.status().await.unwrap(), VaultStatus::Uninitialized);
    }

    async fn initialize_and_unseal_case(db: &Db) {
        let vault = Vault::with_cipher(db.clone(), XChaCha20Poly1305Cipher)
            .await
            .unwrap();
        let password = test_password();

        let rk = vault.initialize(&password).await.unwrap();
        assert!(!rk.phrase().is_empty());
        assert_eq!(vault.status().await.unwrap(), VaultStatus::Unsealed);

        // Seal.
        vault.seal().await;
        assert_eq!(vault.status().await.unwrap(), VaultStatus::Sealed);

        // Unseal with password.
        vault.unseal(&password).await.unwrap();
        assert_eq!(vault.status().await.unwrap(), VaultStatus::Unsealed);
    }

    async fn wrong_password_fails_case(db: &Db) {
        let vault = Vault::with_cipher(db.clone(), XChaCha20Poly1305Cipher)
            .await
            .unwrap();
        let correct_password = test_password();
        let wrong_password = test_password();

        vault.initialize(&correct_password).await.unwrap();
        vault.seal().await;

        let result = vault.unseal(&wrong_password).await;
        assert!(matches!(result, Err(VaultError::BadCredential)));
    }

    async fn recovery_key_unseal_case(db: &Db) {
        let vault = Vault::with_cipher(db.clone(), XChaCha20Poly1305Cipher)
            .await
            .unwrap();
        let password = test_password();

        let rk = vault.initialize(&password).await.unwrap();
        let phrase = rk.phrase().to_string();
        vault.seal().await;

        vault.unseal_with_recovery(&phrase).await.unwrap();
        assert_eq!(vault.status().await.unwrap(), VaultStatus::Unsealed);
    }

    async fn encrypt_decrypt_string_case(db: &Db) {
        let vault = Vault::with_cipher(db.clone(), XChaCha20Poly1305Cipher)
            .await
            .unwrap();
        let password = test_password();

        vault.initialize(&password).await.unwrap();

        let encrypted = vault
            .encrypt_string("my secret api key", "env:OPENAI_API_KEY")
            .await
            .unwrap();
        let decrypted = vault
            .decrypt_string(&encrypted, "env:OPENAI_API_KEY")
            .await
            .unwrap();
        assert_eq!(decrypted, "my secret api key");
    }

    async fn encrypt_sealed_fails_case(db: &Db) {
        let vault = Vault::with_cipher(db.clone(), XChaCha20Poly1305Cipher)
            .await
            .unwrap();
        let password = test_password();

        vault.initialize(&password).await.unwrap();
        vault.seal().await;

        let result = vault.encrypt_string("data", "aad").await;
        assert!(matches!(result, Err(VaultError::Sealed)));
    }

    async fn wrong_aad_decrypt_fails_case(db: &Db) {
        let vault = Vault::with_cipher(db.clone(), XChaCha20Poly1305Cipher)
            .await
            .unwrap();
        let password = test_password();

        vault.initialize(&password).await.unwrap();

        let encrypted = vault.encrypt_string("secret", "env:KEY1").await.unwrap();
        let result = vault.decrypt_string(&encrypted, "env:KEY2").await;
        assert!(result.is_err());
    }

    async fn change_password_case(db: &Db) {
        let vault = Vault::with_cipher(db.clone(), XChaCha20Poly1305Cipher)
            .await
            .unwrap();
        let old_password = test_password();
        let new_password = test_password();

        vault.initialize(&old_password).await.unwrap();

        // Encrypt something with the old key.
        let encrypted = vault.encrypt_string("secret", "test").await.unwrap();

        // Change password.
        vault
            .change_password(&old_password, &new_password)
            .await
            .unwrap();

        // Seal and unseal with new password.
        vault.seal().await;
        vault.unseal(&new_password).await.unwrap();

        // Old data should still be decryptable (same DEK).
        let decrypted = vault.decrypt_string(&encrypted, "test").await.unwrap();
        assert_eq!(decrypted, "secret");

        // Old password should no longer work.
        vault.seal().await;
        let result = vault.unseal(&old_password).await;
        assert!(matches!(result, Err(VaultError::BadCredential)));
    }

    async fn rewrap_unsealed_changes_password_without_old_password_case(db: &Db) {
        let vault = Vault::with_cipher(db.clone(), XChaCha20Poly1305Cipher)
            .await
            .unwrap();
        let old_password = test_password();
        let new_password = test_password();

        let recovery_key = vault.initialize(&old_password).await.unwrap();
        let encrypted = vault.encrypt_string("secret", "test").await.unwrap();
        vault.seal().await;
        vault
            .unseal_with_recovery(recovery_key.phrase())
            .await
            .unwrap();

        vault.rewrap_unsealed(&new_password).await.unwrap();
        vault.seal().await;
        vault.unseal(&new_password).await.unwrap();
        let decrypted = vault.decrypt_string(&encrypted, "test").await.unwrap();
        assert_eq!(decrypted, "secret");

        vault.seal().await;
        let result = vault.unseal(&old_password).await;
        assert!(matches!(result, Err(VaultError::BadCredential)));
    }

    async fn rewrap_unsealed_fails_when_sealed_case(db: &Db) {
        let vault = Vault::with_cipher(db.clone(), XChaCha20Poly1305Cipher)
            .await
            .unwrap();
        let old_password = test_password();
        let new_password = test_password();

        vault.initialize(&old_password).await.unwrap();
        vault.seal().await;

        let result = vault.rewrap_unsealed(&new_password).await;
        assert!(matches!(result, Err(VaultError::Sealed)));
    }

    async fn double_initialize_fails_case(db: &Db) {
        let vault = Vault::with_cipher(db.clone(), XChaCha20Poly1305Cipher)
            .await
            .unwrap();
        let first_password = test_password();
        let second_password = test_password();

        vault.initialize(&first_password).await.unwrap();
        let result = vault.initialize(&second_password).await;
        assert!(matches!(result, Err(VaultError::AlreadyInitialized)));
    }

    async fn unseal_not_initialized_fails_case(db: &Db) {
        let vault = Vault::with_cipher(db.clone(), XChaCha20Poly1305Cipher)
            .await
            .unwrap();
        let password = test_password();

        let result = vault.unseal(&password).await;
        assert!(matches!(result, Err(VaultError::NotInitialized)));
    }

    #[tokio::test]
    async fn status_uninitialized() {
        status_uninitialized_case(&test_db(Backend::Sqlite).await.unwrap()).await;
    }

    #[tokio::test]
    async fn initialize_and_unseal() {
        initialize_and_unseal_case(&test_db(Backend::Sqlite).await.unwrap()).await;
    }

    #[tokio::test]
    async fn wrong_password_fails() {
        wrong_password_fails_case(&test_db(Backend::Sqlite).await.unwrap()).await;
    }

    #[tokio::test]
    async fn recovery_key_unseal() {
        recovery_key_unseal_case(&test_db(Backend::Sqlite).await.unwrap()).await;
    }

    #[tokio::test]
    async fn encrypt_decrypt_string() {
        encrypt_decrypt_string_case(&test_db(Backend::Sqlite).await.unwrap()).await;
    }

    #[tokio::test]
    async fn encrypt_sealed_fails() {
        encrypt_sealed_fails_case(&test_db(Backend::Sqlite).await.unwrap()).await;
    }

    #[tokio::test]
    async fn wrong_aad_decrypt_fails() {
        wrong_aad_decrypt_fails_case(&test_db(Backend::Sqlite).await.unwrap()).await;
    }

    #[tokio::test]
    async fn change_password() {
        change_password_case(&test_db(Backend::Sqlite).await.unwrap()).await;
    }

    #[tokio::test]
    async fn rewrap_unsealed_changes_password_without_old_password() {
        rewrap_unsealed_changes_password_without_old_password_case(
            &test_db(Backend::Sqlite).await.unwrap(),
        )
        .await;
    }

    #[tokio::test]
    async fn rewrap_unsealed_fails_when_sealed() {
        rewrap_unsealed_fails_when_sealed_case(&test_db(Backend::Sqlite).await.unwrap()).await;
    }

    #[tokio::test]
    async fn double_initialize_fails() {
        double_initialize_fails_case(&test_db(Backend::Sqlite).await.unwrap()).await;
    }

    #[tokio::test]
    async fn unseal_not_initialized_fails() {
        unseal_not_initialized_fails_case(&test_db(Backend::Sqlite).await.unwrap()).await;
    }

    /// Every vault case against PostgreSQL, one fresh schema per case.
    #[tokio::test]
    async fn postgres_suite() {
        if std::env::var("TEST_POSTGRES_URL").is_err() {
            eprintln!("skipping: TEST_POSTGRES_URL is not set");
            return;
        }
        let _guard = crate::test_support::pg_lock().lock().await;
        status_uninitialized_case(&test_db(Backend::Postgres).await.unwrap()).await;
        initialize_and_unseal_case(&test_db(Backend::Postgres).await.unwrap()).await;
        wrong_password_fails_case(&test_db(Backend::Postgres).await.unwrap()).await;
        recovery_key_unseal_case(&test_db(Backend::Postgres).await.unwrap()).await;
        encrypt_decrypt_string_case(&test_db(Backend::Postgres).await.unwrap()).await;
        encrypt_sealed_fails_case(&test_db(Backend::Postgres).await.unwrap()).await;
        wrong_aad_decrypt_fails_case(&test_db(Backend::Postgres).await.unwrap()).await;
        change_password_case(&test_db(Backend::Postgres).await.unwrap()).await;
        rewrap_unsealed_changes_password_without_old_password_case(
            &test_db(Backend::Postgres).await.unwrap(),
        )
        .await;
        rewrap_unsealed_fails_when_sealed_case(&test_db(Backend::Postgres).await.unwrap()).await;
        double_initialize_fails_case(&test_db(Backend::Postgres).await.unwrap()).await;
        unseal_not_initialized_fails_case(&test_db(Backend::Postgres).await.unwrap()).await;
    }
}
