use moltis_db::{execute, fetch_all, fetch_one, fetch_optional};

use crate::{
    Result,
    credential_store::{CredentialStore, PasskeyEntry, util::iso_timestamp},
};

impl CredentialStore {
    /// Store a new passkey credential.
    pub async fn store_passkey(
        &self,
        credential_id: &[u8],
        name: &str,
        passkey_data: &[u8],
    ) -> Result<i64> {
        let (id,) = fetch_one!(
            &self.db,
            (i64,),
            "INSERT INTO passkeys (credential_id, name, passkey_data) VALUES (?, ?, ?) RETURNING id",
            credential_id,
            name,
            passkey_data
        )?;
        self.mark_setup_complete().await?;
        Ok(id)
    }

    /// List all registered passkeys.
    pub async fn list_passkeys(&self) -> Result<Vec<PasskeyEntry>> {
        let rows: Vec<(i64, String, String)> = fetch_all!(
            &self.db,
            (i64, String, String),
            "SELECT id, name, created_at FROM passkeys ORDER BY created_at DESC",
        )?;
        Ok(rows
            .into_iter()
            .map(|(id, name, created_at)| PasskeyEntry {
                id,
                name,
                created_at: iso_timestamp(&created_at),
            })
            .collect())
    }

    /// Remove a passkey by id.
    pub async fn remove_passkey(&self, passkey_id: i64) -> Result<()> {
        execute!(&self.db, "DELETE FROM passkeys WHERE id = ?", passkey_id)?;
        self.recompute_setup_complete().await?;
        Ok(())
    }

    /// Rename a passkey.
    pub async fn rename_passkey(&self, passkey_id: i64, name: &str) -> Result<()> {
        execute!(
            &self.db,
            "UPDATE passkeys SET name = ? WHERE id = ?",
            name,
            passkey_id
        )?;
        Ok(())
    }

    /// Load all passkey data blobs (for WebAuthn authentication).
    pub async fn load_all_passkey_data(&self) -> Result<Vec<(i64, Vec<u8>)>> {
        let rows: Vec<(i64, Vec<u8>)> = fetch_all!(
            &self.db,
            (i64, Vec<u8>),
            "SELECT id, passkey_data FROM passkeys",
        )?;
        Ok(rows)
    }

    /// Check if any passkeys are registered (for login page UI).
    pub async fn has_passkeys(&self) -> Result<bool> {
        let row: Option<(i64,)> =
            fetch_optional!(&self.db, (i64,), "SELECT id FROM passkeys LIMIT 1")?;
        Ok(row.is_some())
    }
}
