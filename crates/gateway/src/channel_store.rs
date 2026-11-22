use {
    async_trait::async_trait,
    moltis_db::{Db, execute, fetch_all, fetch_optional},
};

use moltis_channels::{
    Error as ChannelError, Result as ChannelResult,
    plugin::ChannelType,
    store::{ChannelStore, StoredChannel},
};

#[cfg(feature = "vault")]
use {
    moltis_secret_store::{
        decrypt_secret_fields, encrypt_secret_fields, has_encrypted_secret_fields,
        has_plaintext_secret_fields,
    },
    moltis_vault::VaultStatus,
    std::sync::Arc,
};

fn channel_db_error(
    context: &'static str,
    source: impl Into<Box<dyn std::error::Error + Send + Sync>>,
) -> ChannelError {
    ChannelError::External {
        context: context.into(),
        source: source.into(),
    }
}

/// Internal row type for sqlx mapping.
#[derive(sqlx::FromRow)]
struct ChannelRow {
    account_id: String,
    channel_type: String,
    config: String,
    created_at: i64,
    updated_at: i64,
}

impl TryFrom<ChannelRow> for StoredChannel {
    type Error = ChannelError;

    fn try_from(r: ChannelRow) -> ChannelResult<Self> {
        Ok(Self {
            account_id: r.account_id,
            channel_type: r.channel_type,
            config: serde_json::from_str(&r.config)?,
            created_at: r.created_at,
            updated_at: r.updated_at,
        })
    }
}

/// Database-backed channel store (SQLite or PostgreSQL).
pub struct DbChannelStore {
    db: Db,
}

impl DbChannelStore {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    /// Initialize the channels table schema.
    ///
    /// **Deprecated**: Schema is now managed by sqlx migrations.
    /// This method is retained for tests that use in-memory databases.
    #[doc(hidden)]
    pub async fn init(db: &Db) -> ChannelResult<()> {
        crate::run_migrations(db)
            .await
            .map_err(|e| channel_db_error("init channels schema", e))
    }
}

#[cfg(feature = "vault")]
fn channel_secret_fields(channel_type: &str) -> ChannelResult<&'static [&'static str]> {
    channel_type
        .parse::<ChannelType>()
        .map(|channel_type| channel_type.secret_fields())
}

#[cfg(feature = "vault")]
fn channel_secret_aad_scope(channel_type: &str, account_id: &str) -> String {
    format!("channel:{channel_type}:{account_id}")
}

#[cfg(feature = "vault")]
fn channel_secret_store_error(
    context: &'static str,
    source: moltis_secret_store::Error,
) -> ChannelError {
    ChannelError::external(context, source)
}

/// Vault-aware wrapper that encrypts declared secret fields in channel configs.
#[cfg(feature = "vault")]
pub struct VaultChannelStore {
    inner: Arc<dyn ChannelStore>,
    vault: Option<Arc<moltis_vault::Vault>>,
}

#[cfg(feature = "vault")]
impl VaultChannelStore {
    pub fn new(inner: Arc<dyn ChannelStore>, vault: Option<Arc<moltis_vault::Vault>>) -> Self {
        Self { inner, vault }
    }

    async fn prepare_for_storage(&self, channel: StoredChannel) -> ChannelResult<StoredChannel> {
        let mut config = channel.config;
        let secret_fields = channel_secret_fields(&channel.channel_type)?;
        if secret_fields.is_empty() {
            return Ok(StoredChannel { config, ..channel });
        }

        if !crate::vault_lifecycle::is_vault_encryption_runtime_enabled() {
            return Ok(StoredChannel { config, ..channel });
        }

        let Some(vault) = self.vault.as_ref() else {
            return Ok(StoredChannel { config, ..channel });
        };

        if vault.is_unsealed().await {
            encrypt_secret_fields(
                &mut config,
                secret_fields,
                &channel_secret_aad_scope(&channel.channel_type, &channel.account_id),
                vault.as_ref(),
            )
            .await
            .map_err(|error| channel_secret_store_error("encrypt channel secrets", error))?;
            return Ok(StoredChannel { config, ..channel });
        }

        let status = vault
            .status()
            .await
            .map_err(|error| ChannelError::external("check vault status", error))?;
        if matches!(status, VaultStatus::Uninitialized) {
            return Ok(StoredChannel { config, ..channel });
        }

        let has_plaintext =
            has_plaintext_secret_fields(&config, secret_fields).map_err(|error| {
                channel_secret_store_error("inspect plaintext channel secrets", error)
            })?;
        if has_plaintext {
            return Err(ChannelError::unavailable(
                "vault is sealed; channel secrets cannot be persisted",
            ));
        }

        Ok(StoredChannel { config, ..channel })
    }

    async fn prepare_for_runtime(&self, channel: StoredChannel) -> ChannelResult<StoredChannel> {
        let mut config = channel.config;
        let secret_fields = channel_secret_fields(&channel.channel_type)?;
        if secret_fields.is_empty() {
            return Ok(StoredChannel { config, ..channel });
        }

        let has_encrypted =
            has_encrypted_secret_fields(&config, secret_fields).map_err(|error| {
                channel_secret_store_error("inspect encrypted channel secrets", error)
            })?;
        if !has_encrypted {
            return Ok(StoredChannel { config, ..channel });
        }

        let Some(vault) = self.vault.as_ref() else {
            return Err(ChannelError::unavailable(
                "encrypted channel secrets require the vault",
            ));
        };

        if !vault.is_unsealed().await {
            return Err(ChannelError::unavailable(
                "vault is sealed; encrypted channel secrets are unavailable",
            ));
        }

        decrypt_secret_fields(
            &mut config,
            secret_fields,
            &channel_secret_aad_scope(&channel.channel_type, &channel.account_id),
            vault.as_ref(),
        )
        .await
        .map_err(|error| channel_secret_store_error("decrypt channel secrets", error))?;

        Ok(StoredChannel { config, ..channel })
    }
}

#[cfg(feature = "vault")]
#[async_trait]
impl ChannelStore for VaultChannelStore {
    async fn list(&self) -> ChannelResult<Vec<StoredChannel>> {
        let mut channels = Vec::new();
        for channel in self.inner.list().await? {
            channels.push(self.prepare_for_runtime(channel).await?);
        }
        Ok(channels)
    }

    async fn get(
        &self,
        channel_type: &str,
        account_id: &str,
    ) -> ChannelResult<Option<StoredChannel>> {
        let Some(channel) = self.inner.get(channel_type, account_id).await? else {
            return Ok(None);
        };

        self.prepare_for_runtime(channel).await.map(Some)
    }

    async fn upsert(&self, channel: StoredChannel) -> ChannelResult<()> {
        let channel = self.prepare_for_storage(channel).await?;
        self.inner.upsert(channel).await
    }

    async fn delete(&self, channel_type: &str, account_id: &str) -> ChannelResult<()> {
        self.inner.delete(channel_type, account_id).await
    }
}

#[async_trait]
impl ChannelStore for DbChannelStore {
    async fn list(&self) -> ChannelResult<Vec<StoredChannel>> {
        let rows = fetch_all!(
            &self.db,
            ChannelRow,
            "SELECT * FROM channels ORDER BY updated_at DESC"
        )
        .map_err(|e| channel_db_error("list channels", e))?;
        rows.into_iter().map(TryInto::try_into).collect()
    }

    async fn get(
        &self,
        channel_type: &str,
        account_id: &str,
    ) -> ChannelResult<Option<StoredChannel>> {
        let row = fetch_optional!(
            &self.db,
            ChannelRow,
            "SELECT * FROM channels WHERE channel_type = ? AND account_id = ?",
            channel_type,
            account_id,
        )
        .map_err(|e| channel_db_error("get channel", e))?;
        row.map(TryInto::try_into).transpose()
    }

    async fn upsert(&self, channel: StoredChannel) -> ChannelResult<()> {
        let config_json = serde_json::to_string(&channel.config)?;
        execute!(
            &self.db,
            r#"INSERT INTO channels (account_id, channel_type, config, created_at, updated_at)
               VALUES (?, ?, ?, ?, ?)
               ON CONFLICT(channel_type, account_id) DO UPDATE SET
                 channel_type = excluded.channel_type,
                 config = excluded.config,
                 updated_at = excluded.updated_at"#,
            &channel.account_id,
            &channel.channel_type,
            &config_json,
            channel.created_at,
            channel.updated_at,
        )
        .map_err(|e| channel_db_error("upsert channel", e))?;
        Ok(())
    }

    async fn delete(&self, channel_type: &str, account_id: &str) -> ChannelResult<()> {
        execute!(
            &self.db,
            "DELETE FROM channels WHERE channel_type = ? AND account_id = ?",
            channel_type,
            account_id,
        )
        .map_err(|e| channel_db_error("delete channel", e))?;
        Ok(())
    }
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "vault")]
    use std::sync::Arc;

    async fn test_pool() -> sqlx::SqlitePool {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        DbChannelStore::init(&Db::from(pool.clone())).await.unwrap();
        pool
    }

    #[cfg(feature = "vault")]
    async fn test_vault(pool: sqlx::SqlitePool) -> Arc<moltis_vault::Vault> {
        moltis_db::execute!(
            &Db::from(pool.clone()),
            "CREATE TABLE IF NOT EXISTS vault_metadata (
                id                   INTEGER PRIMARY KEY CHECK (id = 1),
                version              INTEGER NOT NULL DEFAULT 1,
                kdf_salt             TEXT NOT NULL,
                kdf_params           TEXT NOT NULL,
                wrapped_dek          TEXT NOT NULL,
                recovery_wrapped_dek TEXT,
                recovery_key_hash    TEXT,
                created_at           TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at           TEXT NOT NULL DEFAULT (datetime('now'))
            )",
        )
        .unwrap();

        let vault = Arc::new(moltis_vault::Vault::new(Db::from(pool)).await.unwrap());
        vault.initialize("test-password").await.unwrap();
        vault
    }

    fn now() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
    }

    #[tokio::test]
    async fn test_upsert_and_get() {
        let pool = test_pool().await;
        let store = DbChannelStore::new(Db::from(pool));

        let ch = StoredChannel {
            account_id: "bot1".into(),
            channel_type: "telegram".into(),
            config: serde_json::json!({"token": "abc"}),
            created_at: now(),
            updated_at: now(),
        };
        store.upsert(ch).await.unwrap();

        let got = store.get("telegram", "bot1").await.unwrap().unwrap();
        assert_eq!(got.account_id, "bot1");
        assert_eq!(got.config["token"], "abc");
    }

    #[tokio::test]
    async fn test_upsert_updates_existing() {
        let pool = test_pool().await;
        let store = DbChannelStore::new(Db::from(pool));
        let t = now();

        store
            .upsert(StoredChannel {
                account_id: "bot1".into(),
                channel_type: "telegram".into(),
                config: serde_json::json!({"token": "old"}),
                created_at: t,
                updated_at: t,
            })
            .await
            .unwrap();

        store
            .upsert(StoredChannel {
                account_id: "bot1".into(),
                channel_type: "telegram".into(),
                config: serde_json::json!({"token": "new"}),
                created_at: t,
                updated_at: t + 1,
            })
            .await
            .unwrap();

        let got = store.get("telegram", "bot1").await.unwrap().unwrap();
        assert_eq!(got.config["token"], "new");

        let all = store.list().await.unwrap();
        assert_eq!(all.len(), 1);
    }

    #[tokio::test]
    async fn test_delete() {
        let pool = test_pool().await;
        let store = DbChannelStore::new(Db::from(pool));

        store
            .upsert(StoredChannel {
                account_id: "bot1".into(),
                channel_type: "telegram".into(),
                config: serde_json::json!({}),
                created_at: now(),
                updated_at: now(),
            })
            .await
            .unwrap();

        store.delete("telegram", "bot1").await.unwrap();
        assert!(store.get("telegram", "bot1").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_list_order() {
        let pool = test_pool().await;
        let store = DbChannelStore::new(Db::from(pool));

        store
            .upsert(StoredChannel {
                account_id: "old".into(),
                channel_type: "telegram".into(),
                config: serde_json::json!({}),
                created_at: 100,
                updated_at: 100,
            })
            .await
            .unwrap();

        store
            .upsert(StoredChannel {
                account_id: "new".into(),
                channel_type: "telegram".into(),
                config: serde_json::json!({}),
                created_at: 200,
                updated_at: 200,
            })
            .await
            .unwrap();

        let all = store.list().await.unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].account_id, "new");
        assert_eq!(all[1].account_id, "old");
    }

    #[tokio::test]
    async fn test_get_nonexistent() {
        let pool = test_pool().await;
        let store = DbChannelStore::new(Db::from(pool));
        assert!(store.get("telegram", "nope").await.unwrap().is_none());
    }

    #[cfg(feature = "vault")]
    #[tokio::test]
    #[serial_test::serial(vault_runtime)]
    async fn vault_store_encrypts_and_decrypts_secret_fields() {
        crate::vault_lifecycle::set_vault_encryption_runtime_enabled(true);
        let pool = test_pool().await;
        let vault = test_vault(pool.clone()).await;
        let inner: Arc<dyn ChannelStore> = Arc::new(DbChannelStore::new(Db::from(pool.clone())));
        let store = VaultChannelStore::new(inner, Some(vault));
        let timestamp = now();

        store
            .upsert(StoredChannel {
                account_id: "bot1".into(),
                channel_type: "telegram".into(),
                config: serde_json::json!({"token": "abc"}),
                created_at: timestamp,
                updated_at: timestamp,
            })
            .await
            .unwrap();

        let raw: (String,) = moltis_db::fetch_one!(
            &Db::from(pool.clone()),
            (String,),
            "SELECT config FROM channels WHERE channel_type = 'telegram' AND account_id = 'bot1'",
        )
        .unwrap();
        assert!(!raw.0.contains("\"abc\""));
        assert!(raw.0.contains("vault_encrypted"));

        let got = store.get("telegram", "bot1").await.unwrap().unwrap();
        assert_eq!(got.config["token"], "abc");
    }

    #[cfg(feature = "vault")]
    #[tokio::test]
    async fn vault_store_falls_back_to_plaintext_when_uninitialized() {
        let pool = test_pool().await;
        moltis_db::execute!(
            &Db::from(pool.clone()),
            "CREATE TABLE IF NOT EXISTS vault_metadata (
                id                   INTEGER PRIMARY KEY CHECK (id = 1),
                version              INTEGER NOT NULL DEFAULT 1,
                kdf_salt             TEXT NOT NULL,
                kdf_params           TEXT NOT NULL,
                wrapped_dek          TEXT NOT NULL,
                recovery_wrapped_dek TEXT,
                recovery_key_hash    TEXT,
                created_at           TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at           TEXT NOT NULL DEFAULT (datetime('now'))
            )",
        )
        .unwrap();
        let vault = Arc::new(
            moltis_vault::Vault::new(Db::from(pool.clone()))
                .await
                .unwrap(),
        );
        let inner: Arc<dyn ChannelStore> = Arc::new(DbChannelStore::new(Db::from(pool.clone())));
        let store = VaultChannelStore::new(inner, Some(vault));
        let timestamp = now();

        store
            .upsert(StoredChannel {
                account_id: "bot1".into(),
                channel_type: "telegram".into(),
                config: serde_json::json!({"token": "abc"}),
                created_at: timestamp,
                updated_at: timestamp,
            })
            .await
            .unwrap();

        let raw: (String,) = moltis_db::fetch_one!(
            &Db::from(pool.clone()),
            (String,),
            "SELECT config FROM channels WHERE channel_type = 'telegram' AND account_id = 'bot1'",
        )
        .unwrap();
        assert!(raw.0.contains("\"abc\""));
    }

    #[cfg(feature = "vault")]
    #[tokio::test]
    #[serial_test::serial(vault_runtime)]
    async fn vault_store_rejects_plaintext_secret_updates_when_sealed() {
        crate::vault_lifecycle::set_vault_encryption_runtime_enabled(true);
        let pool = test_pool().await;
        let vault = test_vault(pool.clone()).await;
        vault.seal().await;
        let inner: Arc<dyn ChannelStore> = Arc::new(DbChannelStore::new(Db::from(pool)));
        let store = VaultChannelStore::new(inner, Some(vault));
        let timestamp = now();

        let error = store
            .upsert(StoredChannel {
                account_id: "bot1".into(),
                channel_type: "telegram".into(),
                config: serde_json::json!({"token": "abc"}),
                created_at: timestamp,
                updated_at: timestamp,
            })
            .await
            .unwrap_err();

        assert!(error.to_string().contains("vault is sealed"));
    }
}
