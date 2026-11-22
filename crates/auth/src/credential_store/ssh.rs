use std::convert::TryFrom;

use {
    moltis_db::{
        execute, fetch_all, fetch_one, fetch_optional, tx_execute, tx_fetch_one, tx_fetch_optional,
    },
    secrecy::Secret,
};

use crate::{
    Error, Result,
    credential_store::{
        CredentialStore, SshAuthMode, SshKeyEntry, SshResolvedTarget, SshTargetEntry,
        util::iso_timestamp,
    },
};

impl CredentialStore {
    pub async fn list_ssh_keys(&self) -> Result<Vec<SshKeyEntry>> {
        let rows: Vec<(i64, String, String, String, String, String, i64, i64)> = fetch_all!(
            &self.db,
            (i64, String, String, String, String, String, i64, i64),
            "SELECT
                k.id,
                k.name,
                k.public_key,
                k.fingerprint,
                k.created_at,
                k.updated_at,
                COALESCE(k.encrypted, 0),
                COUNT(t.id)
            FROM ssh_keys k
            LEFT JOIN ssh_targets t ON t.key_id = k.id
            GROUP BY k.id, k.name, k.public_key, k.fingerprint, k.created_at, k.updated_at, k.encrypted
            ORDER BY k.name ASC",
        )?;

        Ok(rows
            .into_iter()
            .map(
                |(
                    id,
                    name,
                    public_key,
                    fingerprint,
                    created_at,
                    updated_at,
                    encrypted,
                    target_count,
                )| SshKeyEntry {
                    id,
                    name,
                    public_key,
                    fingerprint,
                    created_at: iso_timestamp(&created_at),
                    updated_at: iso_timestamp(&updated_at),
                    encrypted: encrypted != 0,
                    target_count,
                },
            )
            .collect())
    }

    pub async fn create_ssh_key(
        &self,
        name: &str,
        private_key: &str,
        public_key: &str,
        fingerprint: &str,
    ) -> Result<i64> {
        let name = name.trim();
        if name.is_empty() {
            return Err(Error::Ssh("ssh key name is required".into()));
        }

        #[cfg(feature = "vault")]
        let (store_private_key, encrypted) = {
            if self.is_vault_encryption_enabled() {
                if let Some(ref vault) = self.vault
                    && vault.is_unsealed().await
                {
                    let aad = format!("ssh-key:{name}");
                    let enc = vault
                        .encrypt_string(private_key, &aad)
                        .await
                        .map_err(|e| Error::Crypto(e.to_string()))?;
                    (enc, 1_i64)
                } else {
                    (private_key.to_owned(), 0_i64)
                }
            } else {
                (private_key.to_owned(), 0_i64)
            }
        };
        #[cfg(not(feature = "vault"))]
        let (store_private_key, encrypted) = (private_key.to_owned(), 0_i64);

        let (id,) = fetch_one!(
            &self.db,
            (i64,),
            "INSERT INTO ssh_keys (name, private_key, public_key, fingerprint, encrypted)
             VALUES (?, ?, ?, ?, ?) RETURNING id",
            name,
            store_private_key,
            public_key.trim(),
            fingerprint.trim(),
            encrypted
        )?;

        Ok(id)
    }

    pub async fn delete_ssh_key(&self, id: i64) -> Result<()> {
        let deleted = execute!(
            &self.db,
            "DELETE FROM ssh_keys
             WHERE id = ?
               AND NOT EXISTS (SELECT 1 FROM ssh_targets WHERE key_id = ?)",
            id,
            id
        )?;

        if deleted == 0 {
            let in_use: Option<(i64,)> = fetch_optional!(
                &self.db,
                (i64,),
                "SELECT COUNT(1) FROM ssh_targets WHERE key_id = ?",
                id
            )?;
            if in_use.is_some_and(|(count,)| count > 0) {
                return Err(Error::Ssh(
                    "ssh key is still assigned to one or more targets".into(),
                ));
            }
        }
        Ok(())
    }

    pub async fn get_ssh_private_key(&self, key_id: i64) -> Result<Option<Secret<String>>> {
        let row: Option<(String, String, i64)> = fetch_optional!(
            &self.db,
            (String, String, i64),
            "SELECT name, private_key, COALESCE(encrypted, 0) FROM ssh_keys WHERE id = ?",
            key_id
        )?;

        let Some((name, private_key, encrypted)) = row else {
            return Ok(None);
        };

        #[cfg(feature = "vault")]
        {
            if encrypted != 0 {
                let Some(ref vault) = self.vault else {
                    return Err(Error::Crypto(
                        "vault not available for encrypted ssh key".into(),
                    ));
                };
                let aad = format!("ssh-key:{name}");
                let decrypted = vault
                    .decrypt_string(&private_key, &aad)
                    .await
                    .map_err(|e| Error::Crypto(e.to_string()))?;
                return Ok(Some(Secret::new(decrypted)));
            }
        }

        let _ = name;
        let _ = encrypted;
        Ok(Some(Secret::new(private_key)))
    }

    pub async fn list_ssh_targets(&self) -> Result<Vec<SshTargetEntry>> {
        let rows: Vec<(
            i64,
            String,
            String,
            Option<i64>,
            Option<String>,
            String,
            Option<i64>,
            Option<String>,
            i64,
            String,
            String,
        )> = fetch_all!(
            &self.db,
            (
                i64,
                String,
                String,
                Option<i64>,
                Option<String>,
                String,
                Option<i64>,
                Option<String>,
                i64,
                String,
                String,
            ),
            "SELECT
                t.id,
                t.label,
                t.target,
                t.port,
                t.known_host,
                t.auth_mode,
                t.key_id,
                k.name,
                COALESCE(t.is_default, 0),
                t.created_at,
                t.updated_at
            FROM ssh_targets t
            LEFT JOIN ssh_keys k ON k.id = t.key_id
            ORDER BY t.is_default DESC, t.label ASC",
        )?;

        rows.into_iter()
            .map(
                |(
                    id,
                    label,
                    target,
                    port,
                    known_host,
                    auth_mode,
                    key_id,
                    key_name,
                    is_default,
                    created_at,
                    updated_at,
                )| {
                    let port = port.and_then(|value| u16::try_from(value).ok());
                    Ok(SshTargetEntry {
                        id,
                        label,
                        target,
                        port,
                        known_host,
                        auth_mode: SshAuthMode::parse_db(&auth_mode)?,
                        key_id,
                        key_name,
                        is_default: is_default != 0,
                        created_at: iso_timestamp(&created_at),
                        updated_at: iso_timestamp(&updated_at),
                    })
                },
            )
            .collect()
    }

    pub async fn create_ssh_target(
        &self,
        label: &str,
        target: &str,
        port: Option<u16>,
        known_host: Option<&str>,
        auth_mode: SshAuthMode,
        key_id: Option<i64>,
        is_default: bool,
    ) -> Result<i64> {
        let label = label.trim();
        let target = target.trim();
        let known_host = known_host
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned);
        if label.is_empty() {
            return Err(Error::Ssh("ssh target label is required".into()));
        }
        if target.is_empty() {
            return Err(Error::Ssh("ssh target is required".into()));
        }

        let key_id = match auth_mode {
            SshAuthMode::System => None,
            SshAuthMode::Managed => {
                let Some(key_id) = key_id else {
                    return Err(Error::Ssh("managed ssh targets require a key".into()));
                };
                let exists: Option<(i64,)> = fetch_optional!(
                    &self.db,
                    (i64,),
                    "SELECT id FROM ssh_keys WHERE id = ?",
                    key_id
                )?;
                if exists.is_none() {
                    return Err(Error::Ssh("selected ssh key does not exist".into()));
                }
                Some(key_id)
            },
        };

        let mut tx = self.db.begin().await?;
        let has_default: Option<(i64,)> = tx_fetch_optional!(
            &mut tx,
            (i64,),
            "SELECT COUNT(1) FROM ssh_targets WHERE is_default = 1"
        )?;
        let should_be_default = is_default || has_default.unwrap_or((0,)).0 == 0;
        if should_be_default {
            tx_execute!(&mut tx, "UPDATE ssh_targets SET is_default = 0")?;
        }

        let (id,) = tx_fetch_one!(
            &mut tx,
            (i64,),
            "INSERT INTO ssh_targets (label, target, port, known_host, auth_mode, key_id, is_default)
             VALUES (?, ?, ?, ?, ?, ?, ?) RETURNING id",
            label,
            target,
            port.map(i64::from),
            known_host,
            auth_mode.as_db_str(),
            key_id,
            if should_be_default { 1_i64 } else { 0_i64 }
        )?;
        tx.commit().await?;

        Ok(id)
    }

    pub async fn delete_ssh_target(&self, id: i64) -> Result<()> {
        let mut tx = self.db.begin().await?;
        let was_default: Option<(i64,)> = tx_fetch_optional!(
            &mut tx,
            (i64,),
            "SELECT COALESCE(is_default, 0) FROM ssh_targets WHERE id = ?",
            id
        )?;

        tx_execute!(&mut tx, "DELETE FROM ssh_targets WHERE id = ?", id)?;

        if was_default.is_some_and(|(flag,)| flag != 0) {
            let replacement: Option<(i64,)> = tx_fetch_optional!(
                &mut tx,
                (i64,),
                "SELECT id FROM ssh_targets ORDER BY updated_at DESC, created_at DESC, id DESC LIMIT 1"
            )?;
            if let Some((replacement_id,)) = replacement {
                let now = self.db.dialect().now_expr();
                tx_execute!(
                    &mut tx,
                    &format!(
                        "UPDATE ssh_targets SET is_default = 1, updated_at = {now} WHERE id = ?"
                    ),
                    replacement_id
                )?;
            }
        }

        tx.commit().await?;
        Ok(())
    }

    pub async fn set_default_ssh_target(&self, id: i64) -> Result<()> {
        let mut tx = self.db.begin().await?;
        tx_execute!(&mut tx, "UPDATE ssh_targets SET is_default = 0")?;
        let now = self.db.dialect().now_expr();
        let updated = tx_execute!(
            &mut tx,
            &format!("UPDATE ssh_targets SET is_default = 1, updated_at = {now} WHERE id = ?"),
            id
        )?;
        if updated == 0 {
            return Err(Error::Ssh("ssh target not found".into()));
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn update_ssh_target_known_host(
        &self,
        id: i64,
        known_host: Option<&str>,
    ) -> Result<()> {
        let known_host = known_host
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned);
        let now = self.db.dialect().now_expr();
        let result = execute!(
            &self.db,
            &format!("UPDATE ssh_targets SET known_host = ?, updated_at = {now} WHERE id = ?"),
            known_host,
            id
        )?;
        if result == 0 {
            return Err(Error::Ssh("ssh target not found".into()));
        }
        Ok(())
    }

    pub async fn ssh_target_count(&self) -> Result<usize> {
        let row: Option<(i64,)> =
            fetch_optional!(&self.db, (i64,), "SELECT COUNT(1) FROM ssh_targets")?;
        let count = row.unwrap_or((0,)).0;
        Ok(usize::try_from(count).unwrap_or_default())
    }

    pub async fn get_default_ssh_target(&self) -> Result<Option<SshResolvedTarget>> {
        let row: Option<(
            i64,
            String,
            String,
            Option<i64>,
            Option<String>,
            String,
            Option<i64>,
            Option<String>,
        )> = fetch_optional!(
            &self.db,
            (
                i64,
                String,
                String,
                Option<i64>,
                Option<String>,
                String,
                Option<i64>,
                Option<String>,
            ),
            "SELECT
                    t.id,
                    t.label,
                    t.target,
                    t.port,
                    t.known_host,
                    t.auth_mode,
                    t.key_id,
                    k.name
                 FROM ssh_targets t
                 LEFT JOIN ssh_keys k ON k.id = t.key_id
                 WHERE t.is_default = 1
                 ORDER BY t.updated_at DESC
                 LIMIT 1",
        )?;

        let Some((id, label, target, port, known_host, auth_mode, key_id, key_name)) = row else {
            return Ok(None);
        };

        Ok(Some(SshResolvedTarget {
            id,
            node_id: format!("ssh:target:{id}"),
            label,
            target,
            port: port.and_then(|value| u16::try_from(value).ok()),
            known_host,
            auth_mode: SshAuthMode::parse_db(&auth_mode)?,
            key_id,
            key_name,
        }))
    }

    pub async fn resolve_ssh_target(&self, node_ref: &str) -> Result<Option<SshResolvedTarget>> {
        if let Some(id_str) = node_ref.strip_prefix("ssh:target:")
            && let Ok(id) = id_str.parse::<i64>()
        {
            return self.resolve_ssh_target_by_id(id).await;
        }

        let entries = self.list_ssh_targets().await?;
        let lower = node_ref.trim().to_lowercase();
        let matched = entries
            .into_iter()
            .find(|entry| entry.label.to_lowercase() == lower || entry.target == node_ref);
        let Some(entry) = matched else {
            return Ok(None);
        };

        Ok(Some(SshResolvedTarget {
            id: entry.id,
            node_id: format!("ssh:target:{}", entry.id),
            label: entry.label,
            target: entry.target,
            port: entry.port,
            known_host: entry.known_host,
            auth_mode: entry.auth_mode,
            key_id: entry.key_id,
            key_name: entry.key_name,
        }))
    }

    pub async fn resolve_ssh_target_by_id(&self, id: i64) -> Result<Option<SshResolvedTarget>> {
        let row: Option<(
            i64,
            String,
            String,
            Option<i64>,
            Option<String>,
            String,
            Option<i64>,
            Option<String>,
        )> = fetch_optional!(
            &self.db,
            (
                i64,
                String,
                String,
                Option<i64>,
                Option<String>,
                String,
                Option<i64>,
                Option<String>,
            ),
            "SELECT
                    t.id,
                    t.label,
                    t.target,
                    t.port,
                    t.known_host,
                    t.auth_mode,
                    t.key_id,
                    k.name
                 FROM ssh_targets t
                 LEFT JOIN ssh_keys k ON k.id = t.key_id
                 WHERE t.id = ?",
            id
        )?;

        let Some((id, label, target, port, known_host, auth_mode, key_id, key_name)) = row else {
            return Ok(None);
        };

        Ok(Some(SshResolvedTarget {
            id,
            node_id: format!("ssh:target:{id}"),
            label,
            target,
            port: port.and_then(|value| u16::try_from(value).ok()),
            known_host,
            auth_mode: SshAuthMode::parse_db(&auth_mode)?,
            key_id,
            key_name,
        }))
    }
}
