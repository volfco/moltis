use moltis_db::{execute, fetch_all, fetch_one};

use crate::{
    Result,
    credential_store::{
        ApiKeyEntry, ApiKeyVerification, CredentialStore,
        util::{generate_token, hmac_sha256_hex, iso_timestamp, sha256_hex},
    },
};

impl CredentialStore {
    /// Generate a new API key with optional scopes. Returns (id, raw_key).
    /// The raw key is only shown once, we store its HMAC-SHA256 hash with a per-key random salt.
    pub async fn create_api_key(
        &self,
        label: &str,
        scopes: Option<&[String]>,
    ) -> Result<(i64, String)> {
        let raw_key = format!("mk_{}", generate_token());
        let prefix = &raw_key[..raw_key.len().min(11)];
        let salt = generate_token();
        let hash = hmac_sha256_hex(&raw_key, &salt);

        let scopes_json = scopes
            .filter(|s| !s.is_empty())
            .map(|s| serde_json::to_string(s).unwrap_or_default());

        let (id,) = fetch_one!(
            &self.db,
            (i64,),
            "INSERT INTO api_keys (label, key_hash, key_prefix, scopes, key_salt) VALUES (?, ?, ?, ?, ?) RETURNING id",
            label,
            &hash,
            prefix,
            &scopes_json,
            &salt
        )?;
        Ok((id, raw_key))
    }

    /// List all API keys (active only, not revoked).
    pub async fn list_api_keys(&self) -> Result<Vec<ApiKeyEntry>> {
        let rows: Vec<(i64, String, String, String, Option<String>)> = fetch_all!(
            &self.db,
            (i64, String, String, String, Option<String>),
            "SELECT id, label, key_prefix, created_at, scopes FROM api_keys WHERE revoked_at IS NULL ORDER BY created_at DESC",
        )?;
        Ok(rows
            .into_iter()
            .map(|(id, label, key_prefix, created_at, scopes_json)| {
                let scopes = scopes_json
                    .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
                    .filter(|v| !v.is_empty());
                ApiKeyEntry {
                    id,
                    label,
                    key_prefix,
                    created_at: iso_timestamp(&created_at),
                    scopes,
                }
            })
            .collect())
    }

    /// Revoke an API key by id.
    pub async fn revoke_api_key(&self, key_id: i64) -> Result<()> {
        let now = self.db.dialect().now_expr();
        execute!(
            &self.db,
            &format!("UPDATE api_keys SET revoked_at = {now} WHERE id = ? AND revoked_at IS NULL"),
            key_id
        )?;
        Ok(())
    }

    /// Verify a raw API key. Returns `Some(ApiKeyVerification)` if valid,
    /// `None` if invalid or revoked.
    ///
    /// Supports both salted (HMAC-SHA256) and legacy unsalted (SHA-256) keys.
    pub async fn verify_api_key(&self, raw_key: &str) -> Result<Option<ApiKeyVerification>> {
        let rows: Vec<(i64, String, Option<String>, Option<String>)> = fetch_all!(
            &self.db,
            (i64, String, Option<String>, Option<String>),
            "SELECT id, key_hash, scopes, key_salt FROM api_keys WHERE revoked_at IS NULL",
        )?;

        for (key_id, stored_hash, scopes_json, salt) in rows {
            let matches = if let Some(ref s) = salt {
                hmac_sha256_hex(raw_key, s) == stored_hash
            } else {
                sha256_hex(raw_key) == stored_hash
            };
            if matches {
                let scopes = scopes_json
                    .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
                    .unwrap_or_default();
                return Ok(Some(ApiKeyVerification { key_id, scopes }));
            }
        }
        Ok(None)
    }
}
