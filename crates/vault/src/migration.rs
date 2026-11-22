//! Helpers for migrating plaintext data to encrypted vault storage.
//!
//! On the first vault unseal, plaintext secrets are encrypted in-place:
//! - Env vars: rows with `encrypted = 0` are encrypted and flagged.
//! - Managed SSH keys: rows with `encrypted = 0` are encrypted and flagged.
//! - `provider_keys.json` → encrypt → write `.enc` → rename `.json` to `.bak`.
//! - `oauth_tokens.json` → same pattern.

use std::path::Path;

use crate::{error::VaultError, traits::Cipher, vault::Vault};

/// Encrypt all plaintext env variable rows (where `encrypted = 0`).
///
/// Each value is encrypted with AAD `"env:<key>"` for domain separation.
pub async fn migrate_env_vars<C: Cipher>(
    vault: &Vault<C>,
    db: &moltis_db::Db,
) -> Result<usize, VaultError> {
    let rows: Vec<(i64, String, String)> = moltis_db::fetch_all!(
        db,
        (i64, String, String),
        "SELECT id, key, value FROM env_variables WHERE encrypted = 0",
    )?;

    let now = db.dialect().now_expr();
    let count = rows.len();
    for (id, key, plaintext) in rows {
        let aad = format!("env:{key}");
        let encrypted = vault.encrypt_string(&plaintext, &aad).await?;

        moltis_db::execute!(
            db,
            &format!(
                "UPDATE env_variables SET value = ?, encrypted = 1, updated_at = {now} WHERE id = ?"
            ),
            &encrypted,
            id,
        )?;
    }

    if count > 0 {
        #[cfg(feature = "tracing")]
        tracing::info!(count, "migrated env variables to encrypted storage");
    }

    Ok(count)
}

/// Encrypt all plaintext managed SSH private keys (where `encrypted = 0`).
///
/// Each private key is encrypted with AAD `"ssh-key:<name>"` for domain
/// separation.
pub async fn migrate_ssh_keys<C: Cipher>(
    vault: &Vault<C>,
    db: &moltis_db::Db,
) -> Result<usize, VaultError> {
    let rows: Vec<(i64, String, String)> = moltis_db::fetch_all!(
        db,
        (i64, String, String),
        "SELECT id, name, private_key FROM ssh_keys WHERE encrypted = 0",
    )?;

    let now = db.dialect().now_expr();
    let count = rows.len();
    for (id, name, plaintext) in rows {
        let aad = format!("ssh-key:{name}");
        let encrypted = vault.encrypt_string(&plaintext, &aad).await?;

        moltis_db::execute!(
            db,
            &format!(
                "UPDATE ssh_keys SET private_key = ?, encrypted = 1, updated_at = {now} WHERE id = ?"
            ),
            &encrypted,
            id,
        )?;
    }

    if count > 0 {
        #[cfg(feature = "tracing")]
        tracing::info!(count, "migrated ssh keys to encrypted storage");
    }

    Ok(count)
}

/// Encrypt a JSON file to an `.enc` file.
///
/// Reads `path`, encrypts the content, writes `path.enc`, renames `path` to `path.bak`.
/// If `path` doesn't exist or `path.enc` already exists, this is a no-op.
pub async fn migrate_json_file<C: Cipher>(
    vault: &Vault<C>,
    path: &Path,
    aad: &str,
) -> Result<bool, VaultError> {
    let enc_path = path.with_extension("json.enc");
    let bak_path = path.with_extension("json.bak");

    // Skip if plaintext file doesn't exist or encrypted already exists.
    if !path.exists() || enc_path.exists() {
        return Ok(false);
    }

    let plaintext = std::fs::read_to_string(path)
        .map_err(|e| VaultError::CipherError(format!("failed to read {}: {e}", path.display())))?;

    if plaintext.trim().is_empty() {
        return Ok(false);
    }

    let encrypted = vault.encrypt_string(&plaintext, aad).await?;

    std::fs::write(&enc_path, &encrypted).map_err(|e| {
        VaultError::CipherError(format!("failed to write {}: {e}", enc_path.display()))
    })?;

    // Set permissions on the encrypted file (Unix only).
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&enc_path, std::fs::Permissions::from_mode(0o600));
    }

    // Rename original to .bak.
    std::fs::rename(path, &bak_path)
        .map_err(|e| VaultError::CipherError(format!("failed to rename to .bak: {e}")))?;

    #[cfg(feature = "tracing")]
    tracing::info!(path = %path.display(), "migrated to encrypted storage");

    Ok(true)
}

/// Encrypt a JSON file to an `.enc` file without removing the original.
///
/// Unlike [`migrate_json_file`], this leaves the plaintext file in place so
/// sync consumers (e.g. `KeyStore`) can still read it.  The `.enc` copy is
/// authoritative when the vault is unsealed; the plaintext serves as a
/// fallback until all consumers are made vault-aware.
///
/// Re-encrypts on every call so the `.enc` file stays in sync with the
/// plaintext after key writes through sync code paths.
///
/// Returns `true` when the `.enc` file was (re-)written.
pub async fn encrypt_json_file<C: Cipher>(
    vault: &Vault<C>,
    path: &Path,
    aad: &str,
) -> Result<bool, VaultError> {
    let enc_path = path.with_extension("json.enc");

    if !path.exists() {
        return Ok(false);
    }

    let plaintext = std::fs::read_to_string(path)
        .map_err(|e| VaultError::CipherError(format!("failed to read {}: {e}", path.display())))?;

    if plaintext.trim().is_empty() {
        return Ok(false);
    }

    let encrypted = vault.encrypt_string(&plaintext, aad).await?;

    std::fs::write(&enc_path, &encrypted).map_err(|e| {
        VaultError::CipherError(format!("failed to write {}: {e}", enc_path.display()))
    })?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&enc_path, std::fs::Permissions::from_mode(0o600));
    }

    #[cfg(feature = "tracing")]
    tracing::info!(path = %path.display(), "encrypted copy created");

    Ok(true)
}

/// Decrypt an `.enc` file back to a string. Falls back to plaintext if `.enc` doesn't exist.
pub async fn load_encrypted_or_plaintext<C: Cipher>(
    vault: Option<&Vault<C>>,
    path: &Path,
    aad: &str,
) -> Result<Option<String>, VaultError> {
    let enc_path = path.with_extension("json.enc");

    // Try encrypted first.
    if enc_path.exists() {
        if let Some(vault) = vault {
            let content = std::fs::read_to_string(&enc_path).map_err(|e| {
                VaultError::CipherError(format!("failed to read {}: {e}", enc_path.display()))
            })?;
            let decrypted = vault.decrypt_string(&content, aad).await?;
            return Ok(Some(decrypted));
        }
        // Vault not available — can't decrypt.
        return Err(VaultError::Sealed);
    }

    // Fall back to plaintext.
    if path.exists() {
        let content = std::fs::read_to_string(path).map_err(|e| {
            VaultError::CipherError(format!("failed to read {}: {e}", path.display()))
        })?;
        return Ok(Some(content));
    }

    Ok(None)
}

/// Write content to an encrypted `.enc` file if the vault is available,
/// otherwise fall back to writing the plaintext `.json` file.
///
/// When the vault is available the `.enc` file is written but the plaintext
/// `.json` is **not** removed — sync consumers (e.g. `KeyStore`) still read
/// the plaintext until they are made vault-aware.
///
/// This is the write counterpart to [`load_encrypted_or_plaintext`].
pub async fn save_encrypted_or_plaintext<C: Cipher>(
    vault: Option<&Vault<C>>,
    path: &Path,
    aad: &str,
    content: &str,
) -> Result<(), VaultError> {
    let enc_path = path.with_extension("json.enc");

    if let Some(vault) = vault {
        let encrypted = vault.encrypt_string(content, aad).await?;
        std::fs::write(&enc_path, &encrypted).map_err(|e| {
            VaultError::CipherError(format!("failed to write {}: {e}", enc_path.display()))
        })?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&enc_path, std::fs::Permissions::from_mode(0o600));
        }
        // Plaintext is intentionally kept for sync consumers.
    } else {
        std::fs::write(path, content).map_err(|e| {
            VaultError::CipherError(format!("failed to write {}: {e}", path.display()))
        })?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        }
    }
    Ok(())
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
        moltis_db::Db,
    };

    async fn setup_vault(db: &Db) -> Vault<XChaCha20Poly1305Cipher> {
        let vault = Vault::with_cipher(db.clone(), XChaCha20Poly1305Cipher)
            .await
            .unwrap();
        vault.initialize("testpassword").await.unwrap();
        vault
    }

    async fn migrate_env_vars_encrypts_plaintext_case(db: &Db) {
        let vault = setup_vault(db).await;

        // Insert plaintext rows.
        moltis_db::execute!(
            db,
            "INSERT INTO env_variables (key, value) VALUES ('KEY1', 'secret1')",
        )
        .unwrap();
        moltis_db::execute!(
            db,
            "INSERT INTO env_variables (key, value) VALUES ('KEY2', 'secret2')",
        )
        .unwrap();

        let count = migrate_env_vars(&vault, db).await.unwrap();
        assert_eq!(count, 2);

        // Verify encrypted flag.
        let rows: Vec<(i64, String, i64)> = moltis_db::fetch_all!(
            db,
            (i64, String, i64),
            "SELECT id, value, encrypted FROM env_variables ORDER BY key",
        )
        .unwrap();
        for (_, value, encrypted) in &rows {
            assert_eq!(*encrypted, 1);
            assert_ne!(value, "secret1");
            assert_ne!(value, "secret2");
        }

        // Verify decryption works.
        let decrypted = vault.decrypt_string(&rows[0].1, "env:KEY1").await.unwrap();
        assert_eq!(decrypted, "secret1");

        // Running again is a no-op.
        let count2 = migrate_env_vars(&vault, db).await.unwrap();
        assert_eq!(count2, 0);
    }

    async fn migrate_ssh_keys_encrypts_plaintext_case(db: &Db) {
        let vault = setup_vault(db).await;

        moltis_db::execute!(
            db,
            "INSERT INTO ssh_keys (name, private_key, public_key, fingerprint)
             VALUES ('prod-box', 'PRIVATE KEY', 'ssh-ed25519 AAAA test', 'SHA256:test')",
        )
        .unwrap();

        let count = migrate_ssh_keys(&vault, db).await.unwrap();
        assert_eq!(count, 1);

        let row: (String, i64) = moltis_db::fetch_one!(
            db,
            (String, i64),
            "SELECT private_key, encrypted FROM ssh_keys WHERE name = 'prod-box'",
        )
        .unwrap();
        assert_eq!(row.1, 1);
        assert_ne!(row.0, "PRIVATE KEY");

        let decrypted = vault
            .decrypt_string(&row.0, "ssh-key:prod-box")
            .await
            .unwrap();
        assert_eq!(decrypted, "PRIVATE KEY");

        let count2 = migrate_ssh_keys(&vault, db).await.unwrap();
        assert_eq!(count2, 0);
    }

    async fn migrate_json_file_round_trip_case(db: &Db) {
        let vault = setup_vault(db).await;
        let tmp_dir = tempfile::tempdir().unwrap();
        let json_path = tmp_dir.path().join("provider_keys.json");
        let enc_path = tmp_dir.path().join("provider_keys.json.enc");
        let bak_path = tmp_dir.path().join("provider_keys.json.bak");

        // Write plaintext JSON.
        std::fs::write(&json_path, r#"{"openai":{"apiKey":"sk-test"}}"#).unwrap();

        let migrated = migrate_json_file(&vault, &json_path, "provider_keys")
            .await
            .unwrap();
        assert!(migrated);
        assert!(!json_path.exists()); // Renamed to .bak.
        assert!(enc_path.exists());
        assert!(bak_path.exists());

        // Load encrypted.
        let content = load_encrypted_or_plaintext(Some(&vault), &json_path, "provider_keys")
            .await
            .unwrap()
            .unwrap();
        assert!(content.contains("sk-test"));

        // Running again is a no-op (enc already exists).
        let migrated2 = migrate_json_file(&vault, &json_path, "provider_keys")
            .await
            .unwrap();
        assert!(!migrated2);
    }

    async fn encrypt_json_file_keeps_original_case(db: &Db) {
        let vault = setup_vault(db).await;
        let tmp_dir = tempfile::tempdir().unwrap();
        let json_path = tmp_dir.path().join("keys.json");
        let enc_path = tmp_dir.path().join("keys.json.enc");

        std::fs::write(&json_path, r#"{"groq":{"apiKey":"sk-groq"}}"#).unwrap();

        let created = encrypt_json_file(&vault, &json_path, "keys").await.unwrap();
        assert!(created);
        // Original must still exist (unlike migrate_json_file).
        assert!(json_path.exists());
        assert!(enc_path.exists());

        // Encrypted content should decrypt to the original.
        let content = load_encrypted_or_plaintext(Some(&vault), &json_path, "keys")
            .await
            .unwrap()
            .unwrap();
        assert!(content.contains("sk-groq"));

        // Update plaintext and re-encrypt — must pick up the new content.
        std::fs::write(&json_path, r#"{"groq":{"apiKey":"sk-groq-v2"}}"#).unwrap();
        let created2 = encrypt_json_file(&vault, &json_path, "keys").await.unwrap();
        assert!(created2);

        let content2 = load_encrypted_or_plaintext(Some(&vault), &json_path, "keys")
            .await
            .unwrap()
            .unwrap();
        assert!(content2.contains("sk-groq-v2"));
    }

    async fn save_encrypted_or_plaintext_round_trip_case(db: &Db) {
        let vault = setup_vault(db).await;
        let tmp_dir = tempfile::tempdir().unwrap();
        let json_path = tmp_dir.path().join("data.json");
        let enc_path = tmp_dir.path().join("data.json.enc");

        // Write a pre-existing plaintext file.
        std::fs::write(&json_path, "pre-existing").unwrap();

        // With vault: writes .enc but does NOT remove the plaintext.
        save_encrypted_or_plaintext(Some(&vault), &json_path, "data", r#"{"secret":"value"}"#)
            .await
            .unwrap();
        assert!(enc_path.exists());
        assert!(
            json_path.exists(),
            "plaintext must be preserved for sync consumers"
        );

        let decrypted = load_encrypted_or_plaintext(Some(&vault), &json_path, "data")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(decrypted, r#"{"secret":"value"}"#);

        // Without vault: writes plaintext .json
        let plain_path = tmp_dir.path().join("plain.json");
        save_encrypted_or_plaintext::<XChaCha20Poly1305Cipher>(
            None,
            &plain_path,
            "plain",
            r#"{"open":"data"}"#,
        )
        .await
        .unwrap();
        assert!(plain_path.exists());
        assert_eq!(
            std::fs::read_to_string(&plain_path).unwrap(),
            r#"{"open":"data"}"#
        );
    }

    async fn load_plaintext_fallback_case(_db: &Db) {
        let tmp_dir = tempfile::tempdir().unwrap();
        let json_path = tmp_dir.path().join("tokens.json");

        // No file at all.
        let result =
            load_encrypted_or_plaintext::<XChaCha20Poly1305Cipher>(None, &json_path, "tokens")
                .await
                .unwrap();
        assert!(result.is_none());

        // Plaintext file.
        std::fs::write(&json_path, r#"{"github":{}}"#).unwrap();
        let result =
            load_encrypted_or_plaintext::<XChaCha20Poly1305Cipher>(None, &json_path, "tokens")
                .await
                .unwrap();
        assert_eq!(result.unwrap(), r#"{"github":{}}"#);
    }

    #[tokio::test]
    async fn migrate_env_vars_encrypts_plaintext() {
        migrate_env_vars_encrypts_plaintext_case(&test_db(Backend::Sqlite).await.unwrap()).await;
    }

    #[tokio::test]
    async fn migrate_ssh_keys_encrypts_plaintext() {
        migrate_ssh_keys_encrypts_plaintext_case(&test_db(Backend::Sqlite).await.unwrap()).await;
    }

    #[tokio::test]
    async fn migrate_json_file_round_trip() {
        migrate_json_file_round_trip_case(&test_db(Backend::Sqlite).await.unwrap()).await;
    }

    #[tokio::test]
    async fn encrypt_json_file_keeps_original() {
        encrypt_json_file_keeps_original_case(&test_db(Backend::Sqlite).await.unwrap()).await;
    }

    #[tokio::test]
    async fn save_encrypted_or_plaintext_round_trip() {
        save_encrypted_or_plaintext_round_trip_case(&test_db(Backend::Sqlite).await.unwrap()).await;
    }

    #[tokio::test]
    async fn load_plaintext_fallback() {
        load_plaintext_fallback_case(&test_db(Backend::Sqlite).await.unwrap()).await;
    }

    /// Every migration-helper case against PostgreSQL, one fresh schema per
    /// case. File-based helpers ignore the database and run on SQLite only.
    #[tokio::test]
    async fn postgres_suite() {
        if std::env::var("TEST_POSTGRES_URL").is_err() {
            eprintln!("skipping: TEST_POSTGRES_URL is not set");
            return;
        }
        let _guard = crate::test_support::pg_lock().lock().await;
        migrate_env_vars_encrypts_plaintext_case(&test_db(Backend::Postgres).await.unwrap()).await;
        migrate_ssh_keys_encrypts_plaintext_case(&test_db(Backend::Postgres).await.unwrap()).await;
    }
}
