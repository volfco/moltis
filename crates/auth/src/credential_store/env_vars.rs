#[cfg(feature = "vault")]
use std::sync::Arc;

use moltis_db::{execute, fetch_all, fetch_one, fetch_optional};
#[cfg(feature = "vault")]
use moltis_vault::Vault;

#[cfg(feature = "vault")]
use crate::Error;
use crate::{
    Result,
    credential_store::{
        CredentialStore, EnvVarEntry,
        util::{days_from_now_sql, iso_timestamp},
    },
};

impl CredentialStore {
    /// List all environment variables (names only, no values).
    pub async fn list_env_vars(&self) -> Result<Vec<EnvVarEntry>> {
        let rows: Vec<(i64, String, String, String, i64)> = fetch_all!(
            &self.db,
            (i64, String, String, String, i64),
            "SELECT id, key, created_at, updated_at, COALESCE(encrypted, 0) FROM env_variables ORDER BY key ASC",
        )?;
        Ok(rows
            .into_iter()
            .map(|(id, key, created_at, updated_at, encrypted)| EnvVarEntry {
                id,
                key,
                created_at: iso_timestamp(&created_at),
                updated_at: iso_timestamp(&updated_at),
                encrypted: encrypted != 0,
            })
            .collect())
    }

    /// Set (upsert) an environment variable.
    ///
    /// When the vault feature is enabled and the vault is unsealed, the value is encrypted before storage.
    pub async fn set_env_var(&self, key: &str, value: &str) -> Result<i64> {
        #[cfg(feature = "vault")]
        let (store_value, encrypted) = {
            if self.is_vault_encryption_enabled() {
                if let Some(ref vault) = self.vault
                    && vault.is_unsealed().await
                {
                    let aad = format!("env:{key}");
                    let enc = vault
                        .encrypt_string(value, &aad)
                        .await
                        .map_err(|e| Error::Crypto(e.to_string()))?;
                    (enc, 1_i64)
                } else {
                    (value.to_owned(), 0_i64)
                }
            } else {
                (value.to_owned(), 0_i64)
            }
        };
        #[cfg(not(feature = "vault"))]
        let (store_value, encrypted) = (value.to_owned(), 0_i64);

        let now = self.db.dialect().now_expr();
        let (id,) = fetch_one!(
            &self.db,
            (i64,),
            &format!(
                "INSERT INTO env_variables (key, value, encrypted) VALUES (?, ?, ?)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value, encrypted = excluded.encrypted, updated_at = {now}
                 RETURNING id"
            ),
            key,
            &store_value,
            encrypted
        )?;
        Ok(id)
    }

    /// Delete an environment variable by id. Returns the key name if found.
    pub async fn delete_env_var(&self, id: i64) -> Result<Option<String>> {
        let key: Option<(String,)> = fetch_optional!(
            &self.db,
            (String,),
            "SELECT key FROM env_variables WHERE id = ?",
            id
        )?;
        execute!(&self.db, "DELETE FROM env_variables WHERE id = ?", id)?;
        Ok(key.map(|(k,)| k))
    }

    /// Get all environment variable key-value pairs (internal use for sandbox injection).
    pub async fn get_all_env_values(&self) -> Result<Vec<(String, String)>> {
        let rows: Vec<(String, String, i64)> = fetch_all!(
            &self.db,
            (String, String, i64),
            "SELECT key, value, COALESCE(encrypted, 0) FROM env_variables ORDER BY key ASC",
        )?;

        let mut result = Vec::with_capacity(rows.len());
        for (key, value, encrypted) in rows {
            #[cfg(feature = "vault")]
            let plaintext = {
                if encrypted != 0 {
                    if let Some(ref vault) = self.vault {
                        let aad = format!("env:{key}");
                        match vault.decrypt_string(&value, &aad).await {
                            Ok(v) => v,
                            Err(e) => {
                                tracing::warn!(key = %key, error = %e, "failed to decrypt env var, skipping");
                                continue;
                            },
                        }
                    } else {
                        tracing::warn!(key = %key, "encrypted env var but no vault available, skipping");
                        continue;
                    }
                } else {
                    value
                }
            };
            #[cfg(not(feature = "vault"))]
            let plaintext = {
                let _ = encrypted;
                value
            };

            result.push((key, plaintext));
        }
        Ok(result)
    }

    #[cfg(feature = "vault")]
    pub fn vault_for_env(&self) -> Option<&Arc<Vault>> {
        self.vault.as_ref()
    }

    pub async fn audit_log(&self, event_type: &str, client_ip: Option<&str>, detail: Option<&str>) {
        let result = execute!(
            &self.db,
            "INSERT INTO auth_audit_log (event_type, client_ip, detail) VALUES (?, ?, ?)",
            event_type,
            client_ip,
            detail
        );
        if let Err(e) = result {
            tracing::debug!(error = %e, "failed to write audit log");
        }

        let cutoff = days_from_now_sql(self.db.dialect(), -90);
        let _ = execute!(
            &self.db,
            &format!("DELETE FROM auth_audit_log WHERE created_at < {cutoff}")
        );
    }
}
