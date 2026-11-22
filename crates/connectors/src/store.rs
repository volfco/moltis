use std::collections::HashSet;

use {
    async_trait::async_trait,
    moltis_db::{Db, DbTx, Dialect},
    sqlx::{Decode, Row, Type},
    time::OffsetDateTime,
    uuid::Uuid,
};

use crate::{
    Account, AccountCreate, AccountUpdate, ConnectorError, ConnectorItem, ConnectorItemInput,
    ConnectorKind, ConnectorReader, Dataset, DatasetCreate, DatasetUpdate, ItemQuery,
    ProjectionConfig, Result, SnapshotResult, SourceObservation, SourceStateMap, SyncRun,
    SyncRunStatus,
    search::{fts_query, websearch_query},
};

pub const MAX_QUERY_LIMIT: u64 = 500;
const MAX_SOURCE_KEY_LENGTH: usize = 1024;

/// `connector_accounts` row decoded by column name on either backend.
struct AccountRow {
    id: String,
    kind: String,
    source_key: Option<String>,
    name: String,
    config: String,
    enabled: bool,
    created_at: i64,
    updated_at: i64,
}

impl<'r, R: Row> sqlx::FromRow<'r, R> for AccountRow
where
    &'r str: sqlx::ColumnIndex<R>,
    String: Decode<'r, R::Database> + Type<R::Database>,
    Option<String>: Decode<'r, R::Database> + Type<R::Database>,
    bool: Decode<'r, R::Database> + Type<R::Database>,
    i64: Decode<'r, R::Database> + Type<R::Database>,
{
    fn from_row(row: &'r R) -> std::result::Result<Self, sqlx::Error> {
        Ok(Self {
            id: row.try_get("id")?,
            kind: row.try_get("kind")?,
            source_key: row.try_get("source_key")?,
            name: row.try_get("name")?,
            config: row.try_get("config")?,
            enabled: row.try_get("enabled")?,
            created_at: row.try_get("created_at")?,
            updated_at: row.try_get("updated_at")?,
        })
    }
}

/// `connector_datasets` row decoded by column name on either backend.
struct DatasetRow {
    id: String,
    account_id: String,
    name: String,
    instruction: Option<String>,
    config: String,
    plan_revision: i64,
    synced_plan_revision: Option<i64>,
    schedule_minutes: Option<i64>,
    projection_jsonl: bool,
    projection_md: bool,
    enabled: bool,
    last_sync_at: Option<i64>,
    next_sync_at: Option<i64>,
    last_error: Option<String>,
    item_count: i64,
    created_at: i64,
    updated_at: i64,
}

impl<'r, R: Row> sqlx::FromRow<'r, R> for DatasetRow
where
    &'r str: sqlx::ColumnIndex<R>,
    String: Decode<'r, R::Database> + Type<R::Database>,
    Option<String>: Decode<'r, R::Database> + Type<R::Database>,
    bool: Decode<'r, R::Database> + Type<R::Database>,
    i64: Decode<'r, R::Database> + Type<R::Database>,
    Option<i64>: Decode<'r, R::Database> + Type<R::Database>,
{
    fn from_row(row: &'r R) -> std::result::Result<Self, sqlx::Error> {
        Ok(Self {
            id: row.try_get("id")?,
            account_id: row.try_get("account_id")?,
            name: row.try_get("name")?,
            instruction: row.try_get("instruction")?,
            config: row.try_get("config")?,
            plan_revision: row.try_get("plan_revision")?,
            synced_plan_revision: row.try_get("synced_plan_revision")?,
            schedule_minutes: row.try_get("schedule_minutes")?,
            projection_jsonl: row.try_get("projection_jsonl")?,
            projection_md: row.try_get("projection_md")?,
            enabled: row.try_get("enabled")?,
            last_sync_at: row.try_get("last_sync_at")?,
            next_sync_at: row.try_get("next_sync_at")?,
            last_error: row.try_get("last_error")?,
            item_count: row.try_get("item_count")?,
            created_at: row.try_get("created_at")?,
            updated_at: row.try_get("updated_at")?,
        })
    }
}

/// `connector_items` row decoded by column name on either backend.
struct ItemRow {
    id: String,
    dataset_id: String,
    remote_id: String,
    kind: String,
    remote_version: Option<String>,
    occurred_at: Option<String>,
    updated_at: Option<String>,
    body_json: String,
    search_text: String,
    content_hash: String,
    created_at: i64,
    stored_at: i64,
    deleted_at: Option<i64>,
}

impl<'r, R: Row> sqlx::FromRow<'r, R> for ItemRow
where
    &'r str: sqlx::ColumnIndex<R>,
    String: Decode<'r, R::Database> + Type<R::Database>,
    Option<String>: Decode<'r, R::Database> + Type<R::Database>,
    i64: Decode<'r, R::Database> + Type<R::Database>,
    Option<i64>: Decode<'r, R::Database> + Type<R::Database>,
{
    fn from_row(row: &'r R) -> std::result::Result<Self, sqlx::Error> {
        Ok(Self {
            id: row.try_get("id")?,
            dataset_id: row.try_get("dataset_id")?,
            remote_id: row.try_get("remote_id")?,
            kind: row.try_get("kind")?,
            remote_version: row.try_get("remote_version")?,
            occurred_at: row.try_get("occurred_at")?,
            updated_at: row.try_get("updated_at")?,
            body_json: row.try_get("body_json")?,
            search_text: row.try_get("search_text")?,
            content_hash: row.try_get("content_hash")?,
            created_at: row.try_get("created_at")?,
            stored_at: row.try_get("stored_at")?,
            deleted_at: row.try_get("deleted_at")?,
        })
    }
}

/// `connector_sync_runs` row decoded by column name on either backend.
struct SyncRunRow {
    id: String,
    dataset_id: String,
    status: String,
    started_at: i64,
    finished_at: Option<i64>,
    upserted: i64,
    deleted: i64,
    active: i64,
    error: Option<String>,
}

impl<'r, R: Row> sqlx::FromRow<'r, R> for SyncRunRow
where
    &'r str: sqlx::ColumnIndex<R>,
    String: Decode<'r, R::Database> + Type<R::Database>,
    Option<String>: Decode<'r, R::Database> + Type<R::Database>,
    i64: Decode<'r, R::Database> + Type<R::Database>,
    Option<i64>: Decode<'r, R::Database> + Type<R::Database>,
{
    fn from_row(row: &'r R) -> std::result::Result<Self, sqlx::Error> {
        Ok(Self {
            id: row.try_get("id")?,
            dataset_id: row.try_get("dataset_id")?,
            status: row.try_get("status")?,
            started_at: row.try_get("started_at")?,
            finished_at: row.try_get("finished_at")?,
            upserted: row.try_get("upserted")?,
            deleted: row.try_get("deleted")?,
            active: row.try_get("active")?,
            error: row.try_get("error")?,
        })
    }
}

/// Connector store backed by SQLite or PostgreSQL.
pub struct DbConnectorStore {
    db: Db,
}

impl DbConnectorStore {
    #[must_use]
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    #[must_use]
    pub fn pool(&self) -> &Db {
        &self.db
    }

    async fn dataset_has_kind(&self, dataset_id: &str, kind: ConnectorKind) -> Result<bool> {
        // EXISTS decodes as 0/1 on SQLite and BOOLEAN on PostgreSQL.
        let exists: bool = moltis_db::scalar!(
            &self.db,
            bool,
            "SELECT EXISTS(SELECT 1 FROM connector_datasets AS dataset \
             JOIN connector_accounts AS account ON account.id = dataset.account_id \
             WHERE dataset.id = ? AND account.kind = ?)",
            dataset_id,
            kind.as_str(),
        )?;
        Ok(exists)
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, input), fields(kind = %input.kind)))]
    pub async fn create_account(&self, input: AccountCreate) -> Result<Account> {
        let id = Uuid::new_v4().to_string();
        self.create_account_with_id(&id, input).await
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, input), fields(kind = %input.kind)))]
    pub async fn create_account_with_id(&self, id: &str, input: AccountCreate) -> Result<Account> {
        require_name(&input.name, "account")?;
        validate_optional_source_key(input.source_key.as_deref())?;
        if id.trim().is_empty() {
            return Err(ConnectorError::InvalidInput(
                "account id must not be empty".to_owned(),
            ));
        }
        let now = now_millis();
        let config = serde_json::to_string(&input.config)?;
        moltis_db::execute!(
            &self.db,
            "INSERT INTO connector_accounts \
             (id, kind, source_key, name, config, enabled, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            id,
            input.kind.as_str(),
            &input.source_key,
            &input.name,
            config,
            input.enabled,
            now,
            now,
        )?;
        record_write("account.create");
        self.get_account_required(id).await
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, input)))]
    pub async fn update_account(&self, id: &str, input: AccountUpdate) -> Result<Account> {
        require_name(&input.name, "account")?;
        let config = serde_json::to_string(&input.config)?;
        let mut transaction = self.db.begin().await?;
        let previous_config: String = moltis_db::tx_fetch_optional!(
            &mut transaction,
            (String,),
            "SELECT config FROM connector_accounts WHERE id = ?",
            id,
        )?
        .ok_or_else(|| not_found("account", id))?
        .0;
        let config_changed = previous_config != config;
        let affected = moltis_db::tx_execute!(
            &mut transaction,
            "UPDATE connector_accounts SET name = ?, config = ?, enabled = ?, updated_at = ? \
             WHERE id = ?",
            input.name,
            config,
            input.enabled,
            now_millis(),
            id,
        )?;
        require_affected(affected, "account", id)?;
        if config_changed {
            moltis_db::tx_execute!(
                &mut transaction,
                "UPDATE connector_datasets SET plan_revision = plan_revision + 1 \
                 WHERE account_id = ?",
                id,
            )?;
        }
        transaction.commit().await?;
        record_write("account.update");
        self.get_account_required(id).await
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self)))]
    pub async fn remove_account(&self, id: &str) -> Result<bool> {
        let removed =
            moltis_db::execute!(&self.db, "DELETE FROM connector_accounts WHERE id = ?", id)? != 0;
        if removed {
            record_write("account.remove");
        }
        Ok(removed)
    }

    pub async fn get_account(&self, id: &str) -> Result<Option<Account>> {
        let row: Option<AccountRow> = moltis_db::fetch_optional!(
            &self.db,
            AccountRow,
            "SELECT * FROM connector_accounts WHERE id = ?",
            id,
        )?;
        row.map(account_from_row).transpose()
    }

    pub async fn get_account_by_source_key(
        &self,
        kind: ConnectorKind,
        source_key: &str,
    ) -> Result<Option<Account>> {
        validate_source_key(source_key)?;
        let row: Option<AccountRow> = moltis_db::fetch_optional!(
            &self.db,
            AccountRow,
            "SELECT * FROM connector_accounts WHERE kind = ? AND source_key = ?",
            kind.as_str(),
            source_key,
        )?;
        row.map(account_from_row).transpose()
    }

    pub async fn adopt_account(
        &self,
        id: &str,
        source_key: &str,
        input: AccountUpdate,
    ) -> Result<Account> {
        require_name(&input.name, "account")?;
        validate_source_key(source_key)?;
        let config = serde_json::to_string(&input.config)?;
        let mut transaction = self.db.begin().await?;
        let previous_config: String = moltis_db::tx_fetch_optional!(
            &mut transaction,
            (String,),
            "SELECT config FROM connector_accounts WHERE id = ? AND source_key IS NULL",
            id,
        )?
        .ok_or_else(|| {
            ConnectorError::InvalidInput(format!("account {id} is not available for adoption"))
        })?
        .0;
        let config_changed = previous_config != config;
        let affected = moltis_db::tx_execute!(
            &mut transaction,
            "UPDATE connector_accounts SET source_key = ?, name = ?, config = ?, enabled = ?, \
             updated_at = ? WHERE id = ? AND source_key IS NULL",
            source_key,
            input.name,
            config,
            input.enabled,
            now_millis(),
            id,
        )?;
        if affected != 1 {
            return Err(ConnectorError::InvalidInput(format!(
                "account {id} is not available for adoption"
            )));
        }
        if config_changed {
            moltis_db::tx_execute!(
                &mut transaction,
                "UPDATE connector_datasets SET plan_revision = plan_revision + 1 \
                 WHERE account_id = ?",
                id,
            )?;
        }
        transaction.commit().await?;
        record_write("account.adopt");
        self.get_account_required(id).await
    }

    pub async fn list_accounts(&self) -> Result<Vec<Account>> {
        let rows: Vec<AccountRow> = moltis_db::fetch_all!(
            &self.db,
            AccountRow,
            "SELECT * FROM connector_accounts ORDER BY name, id",
        )?;
        rows.into_iter().map(account_from_row).collect()
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, input)))]
    pub async fn create_dataset(&self, input: DatasetCreate) -> Result<Dataset> {
        require_name(&input.name, "dataset")?;
        validate_schedule(input.schedule_minutes)?;
        let id = Uuid::new_v4().to_string();
        let now = now_millis();
        let config = serde_json::to_string(&input.config)?;
        moltis_db::execute!(
            &self.db,
            "INSERT INTO connector_datasets \
             (id, account_id, name, instruction, config, schedule_minutes, projection_jsonl, \
              projection_md, enabled, next_sync_at, created_at, updated_at) \
              VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            &id,
            &input.account_id,
            &input.name,
            &input.instruction,
            config,
            input.schedule_minutes.map(u64_to_i64).transpose()?,
            input.projections.jsonl,
            input.projections.markdown,
            input.enabled,
            input.next_sync_at.map(datetime_to_millis).transpose()?,
            now,
            now,
        )?;
        record_write("dataset.create");
        self.get_dataset_required(&id).await
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, input)))]
    pub async fn update_dataset(&self, id: &str, input: DatasetUpdate) -> Result<Dataset> {
        require_name(&input.name, "dataset")?;
        validate_schedule(input.schedule_minutes)?;
        let config = serde_json::to_string(&input.config)?;
        let affected = moltis_db::execute!(
            &self.db,
            "UPDATE connector_datasets SET name = ?, instruction = ?, \
             plan_revision = plan_revision + CASE WHEN config <> ? THEN 1 ELSE 0 END, \
             config = ?, schedule_minutes = ?, projection_jsonl = ?, projection_md = ?, \
             enabled = ?, next_sync_at = ?, updated_at = ? WHERE id = ?",
            input.name,
            input.instruction,
            &config,
            &config,
            input.schedule_minutes.map(u64_to_i64).transpose()?,
            input.projections.jsonl,
            input.projections.markdown,
            input.enabled,
            input.next_sync_at.map(datetime_to_millis).transpose()?,
            now_millis(),
            id,
        )?;
        require_affected(affected, "dataset", id)?;
        record_write("dataset.update");
        self.get_dataset_required(id).await
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self)))]
    pub async fn remove_dataset(&self, id: &str) -> Result<bool> {
        let removed =
            moltis_db::execute!(&self.db, "DELETE FROM connector_datasets WHERE id = ?", id)? != 0;
        if removed {
            record_write("dataset.remove");
        }
        Ok(removed)
    }

    pub async fn set_dataset_next_sync_at(
        &self,
        id: &str,
        next_sync_at: Option<OffsetDateTime>,
    ) -> Result<()> {
        let affected = moltis_db::execute!(
            &self.db,
            "UPDATE connector_datasets SET next_sync_at = ?, updated_at = ? WHERE id = ?",
            next_sync_at.map(datetime_to_millis).transpose()?,
            now_millis(),
            id,
        )?;
        require_affected(affected, "dataset", id)
    }

    pub async fn set_dataset_last_error(&self, id: &str, error: Option<&str>) -> Result<()> {
        let affected = moltis_db::execute!(
            &self.db,
            "UPDATE connector_datasets SET last_error = ?, updated_at = ? WHERE id = ?",
            error,
            now_millis(),
            id,
        )?;
        require_affected(affected, "dataset", id)
    }

    pub async fn get_dataset(&self, id: &str) -> Result<Option<Dataset>> {
        let row: Option<DatasetRow> = moltis_db::fetch_optional!(
            &self.db,
            DatasetRow,
            "SELECT * FROM connector_datasets WHERE id = ?",
            id,
        )?;
        row.map(dataset_from_row).transpose()
    }

    pub async fn list_datasets(&self, account_id: &str) -> Result<Vec<Dataset>> {
        let rows: Vec<DatasetRow> = moltis_db::fetch_all!(
            &self.db,
            DatasetRow,
            "SELECT * FROM connector_datasets WHERE account_id = ? ORDER BY name, id",
            account_id,
        )?;
        rows.into_iter().map(dataset_from_row).collect()
    }

    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(skip(self, items, observed_remote_ids), fields(item_count = items.len(), complete))
    )]
    pub async fn apply_snapshot(
        &self,
        dataset_id: &str,
        items: &[ConnectorItemInput],
        observed_remote_ids: &[String],
        complete: bool,
    ) -> Result<SnapshotResult> {
        validate_snapshot(items)?;
        let now = now_millis();
        let mut transaction = self.db.begin().await?;
        let result = apply_snapshot_transaction(
            &mut transaction,
            dataset_id,
            items,
            observed_remote_ids,
            complete,
            now,
        )
        .await?;
        transaction.commit().await?;
        record_snapshot(result);
        Ok(result)
    }

    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(skip(self, items, observations), fields(item_count = items.len()))
    )]
    pub async fn commit_sync_snapshot(
        &self,
        run_id: &str,
        dataset_id: &str,
        items: &[ConnectorItemInput],
        observations: &[SourceObservation],
        next_sync_at: Option<OffsetDateTime>,
    ) -> Result<(SnapshotResult, SyncRun)> {
        validate_snapshot(items)?;
        let now = now_millis();
        let mut transaction = self.db.begin().await?;
        let stats = crate::source::commit_observations(
            &mut transaction,
            run_id,
            dataset_id,
            items,
            observations,
            now,
        )
        .await?;
        let run_affected = moltis_db::tx_execute!(
            &mut transaction,
            "UPDATE connector_sync_runs SET status = ?, finished_at = ?, upserted = ?, \
             deleted = ?, active = ?, error = NULL \
             WHERE id = ? AND dataset_id = ? AND status = 'running'",
            SyncRunStatus::Succeeded.as_str(),
            now,
            u64_to_i64(stats.upserted)?,
            u64_to_i64(stats.deleted)?,
            u64_to_i64(stats.active)?,
            run_id,
            dataset_id,
        )?;
        require_affected(run_affected, "running sync run", run_id)?;
        let dataset_affected = moltis_db::tx_execute!(
            &mut transaction,
            "UPDATE connector_datasets SET last_sync_at = ?, last_error = NULL, item_count = ?, \
              next_sync_at = ?, synced_plan_revision = plan_revision, updated_at = ? WHERE id = ?",
            now,
            u64_to_i64(stats.active)?,
            next_sync_at.map(datetime_to_millis).transpose()?,
            now,
            dataset_id,
        )?;
        require_affected(dataset_affected, "dataset", dataset_id)?;
        let run = sync_run_in_transaction(&mut transaction, run_id).await?;
        transaction.commit().await?;
        record_snapshot(stats);
        record_write("sync.finish");
        Ok((stats, run))
    }

    pub async fn source_states(&self, dataset_id: &str) -> Result<SourceStateMap> {
        crate::source::read_states(&self.db, dataset_id).await
    }

    pub async fn query_items(
        &self,
        dataset_id: &str,
        query: ItemQuery,
    ) -> Result<Vec<ConnectorItem>> {
        validate_limit(query.limit)?;
        let fts = match self.db.dialect() {
            Dialect::Sqlite => query.text.as_deref().map(fts_query).transpose()?.flatten(),
            Dialect::Postgres => query
                .text
                .as_deref()
                .map(websearch_query)
                .transpose()?
                .flatten(),
        };
        if query.text.is_some() && fts.is_none() {
            return Ok(Vec::new());
        }
        let limit = u64_to_i64(query.limit)?;
        let offset = u64_to_i64(query.offset)?;
        let rows: Vec<ItemRow> = match (query.include_deleted, fts, self.db.dialect()) {
            (false, None, _) => moltis_db::fetch_all!(
                &self.db,
                ItemRow,
                "SELECT item.* FROM connector_items AS item WHERE item.dataset_id = ? \
                     AND item.deleted_at IS NULL \
                     ORDER BY item.occurred_at IS NULL, item.occurred_at DESC, item.remote_id \
                     LIMIT ? OFFSET ?",
                dataset_id,
                limit,
                offset,
            )?,
            (true, None, _) => moltis_db::fetch_all!(
                &self.db,
                ItemRow,
                "SELECT item.* FROM connector_items AS item WHERE item.dataset_id = ? \
                     ORDER BY item.occurred_at IS NULL, item.occurred_at DESC, item.remote_id \
                     LIMIT ? OFFSET ?",
                dataset_id,
                limit,
                offset,
            )?,
            (false, Some(fts), Dialect::Sqlite) => moltis_db::fetch_all!(
                &self.db,
                ItemRow,
                "SELECT item.* FROM connector_items AS item \
                     JOIN connector_items_fts ON connector_items_fts.rowid = item.rowid \
                     WHERE item.dataset_id = ? AND item.deleted_at IS NULL \
                     AND connector_items_fts MATCH ? \
                     ORDER BY bm25(connector_items_fts), item.occurred_at IS NULL, \
                     item.occurred_at DESC, item.remote_id \
                     LIMIT ? OFFSET ?",
                dataset_id,
                &fts,
                limit,
                offset,
            )?,
            (true, Some(fts), Dialect::Sqlite) => moltis_db::fetch_all!(
                &self.db,
                ItemRow,
                "SELECT item.* FROM connector_items AS item \
                     JOIN connector_items_fts ON connector_items_fts.rowid = item.rowid \
                     WHERE item.dataset_id = ? AND connector_items_fts MATCH ? \
                     ORDER BY bm25(connector_items_fts), item.occurred_at IS NULL, \
                     item.occurred_at DESC, item.remote_id \
                     LIMIT ? OFFSET ?",
                dataset_id,
                &fts,
                limit,
                offset,
            )?,
            (false, Some(fts), Dialect::Postgres) => {
                // The tsquery is built once in FROM so ts_rank can reuse it in
                // ORDER BY while keeping one bind per placeholder.
                moltis_db::fetch_all!(
                    &self.db,
                    ItemRow,
                    "SELECT item.* FROM connector_items AS item, \
                     websearch_to_tsquery('simple', ?) AS q \
                     WHERE item.dataset_id = ? AND item.deleted_at IS NULL \
                     AND item.tsv @@ q \
                     ORDER BY ts_rank(item.tsv, q)::float8 DESC, item.occurred_at IS NULL, \
                     item.occurred_at DESC, item.remote_id LIMIT ? OFFSET ?",
                    &fts,
                    dataset_id,
                    limit,
                    offset,
                )?
            },
            (true, Some(fts), Dialect::Postgres) => moltis_db::fetch_all!(
                &self.db,
                ItemRow,
                "SELECT item.* FROM connector_items AS item, \
                     websearch_to_tsquery('simple', ?) AS q \
                     WHERE item.dataset_id = ? AND item.tsv @@ q \
                     ORDER BY ts_rank(item.tsv, q)::float8 DESC, item.occurred_at IS NULL, \
                     item.occurred_at DESC, item.remote_id LIMIT ? OFFSET ?",
                &fts,
                dataset_id,
                limit,
                offset,
            )?,
        };
        rows.into_iter().map(item_from_row).collect()
    }

    pub async fn get_item(
        &self,
        dataset_id: &str,
        item_id: &str,
        include_deleted: bool,
    ) -> Result<Option<ConnectorItem>> {
        let row: Option<ItemRow> = if include_deleted {
            moltis_db::fetch_optional!(
                &self.db,
                ItemRow,
                "SELECT * FROM connector_items WHERE dataset_id = ? AND id = ?",
                dataset_id,
                item_id,
            )?
        } else {
            moltis_db::fetch_optional!(
                &self.db,
                ItemRow,
                "SELECT * FROM connector_items WHERE dataset_id = ? AND id = ? \
                 AND deleted_at IS NULL",
                dataset_id,
                item_id,
            )?
        };
        row.map(item_from_row).transpose()
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self)))]
    pub async fn start_sync_run(&self, dataset_id: &str) -> Result<SyncRun> {
        let id = Uuid::new_v4().to_string();
        let mut transaction = self.db.begin().await?;
        moltis_db::tx_execute!(
            &mut transaction,
            "INSERT INTO connector_sync_runs (id, dataset_id, status, started_at) VALUES (?, ?, ?, ?)",
            &id,
            dataset_id,
            SyncRunStatus::Running.as_str(),
            now_millis(),
        )?;
        let run = sync_run_in_transaction(&mut transaction, &id).await?;
        transaction.commit().await?;
        record_write("sync.start");
        Ok(run)
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self)))]
    pub async fn finish_sync_run(&self, id: &str, stats: SnapshotResult) -> Result<SyncRun> {
        let now = now_millis();
        let mut transaction = self.db.begin().await?;
        let affected = moltis_db::tx_execute!(
            &mut transaction,
            "UPDATE connector_sync_runs SET status = ?, finished_at = ?, upserted = ?, \
             deleted = ?, active = ?, error = NULL WHERE id = ? AND status = 'running'",
            SyncRunStatus::Succeeded.as_str(),
            now,
            u64_to_i64(stats.upserted)?,
            u64_to_i64(stats.deleted)?,
            u64_to_i64(stats.active)?,
            id,
        )?;
        require_affected(affected, "running sync run", id)?;
        moltis_db::tx_execute!(
            &mut transaction,
            "UPDATE connector_datasets SET last_sync_at = ?, last_error = NULL, item_count = ?, \
             updated_at = ? WHERE id = (SELECT dataset_id FROM connector_sync_runs WHERE id = ?)",
            now,
            u64_to_i64(stats.active)?,
            now,
            id,
        )?;
        let run = sync_run_in_transaction(&mut transaction, id).await?;
        transaction.commit().await?;
        record_write("sync.finish");
        Ok(run)
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, error)))]
    pub async fn fail_sync_run(&self, id: &str, error: &str) -> Result<SyncRun> {
        if error.trim().is_empty() {
            return Err(ConnectorError::InvalidInput(
                "sync failure error must not be empty".to_owned(),
            ));
        }
        let now = now_millis();
        let mut transaction = self.db.begin().await?;
        let affected = moltis_db::tx_execute!(
            &mut transaction,
            "UPDATE connector_sync_runs SET status = ?, finished_at = ?, error = ? \
             WHERE id = ? AND status = 'running'",
            SyncRunStatus::Failed.as_str(),
            now,
            error,
            id,
        )?;
        require_affected(affected, "running sync run", id)?;
        moltis_db::tx_execute!(
            &mut transaction,
            "UPDATE connector_datasets SET last_error = ?, updated_at = ? \
             WHERE id = (SELECT dataset_id FROM connector_sync_runs WHERE id = ?)",
            error,
            now,
            id,
        )?;
        let run = sync_run_in_transaction(&mut transaction, id).await?;
        transaction.commit().await?;
        record_write("sync.fail");
        Ok(run)
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, error)))]
    pub async fn fail_sync_run_and_reschedule(
        &self,
        id: &str,
        error: &str,
        next_sync_at: Option<OffsetDateTime>,
    ) -> Result<SyncRun> {
        if error.trim().is_empty() {
            return Err(ConnectorError::InvalidInput(
                "sync failure error must not be empty".to_owned(),
            ));
        }
        let now = now_millis();
        let mut transaction = self.db.begin().await?;
        let affected = moltis_db::tx_execute!(
            &mut transaction,
            "UPDATE connector_sync_runs SET status = ?, finished_at = ?, error = ? \
             WHERE id = ? AND status = 'running'",
            SyncRunStatus::Failed.as_str(),
            now,
            error,
            id,
        )?;
        require_affected(affected, "running sync run", id)?;
        let dataset_affected = moltis_db::tx_execute!(
            &mut transaction,
            "UPDATE connector_datasets SET last_error = ?, next_sync_at = ?, updated_at = ? \
             WHERE id = (SELECT dataset_id FROM connector_sync_runs WHERE id = ?)",
            error,
            next_sync_at.map(datetime_to_millis).transpose()?,
            now,
            id,
        )?;
        require_affected(dataset_affected, "sync run dataset", id)?;
        let run = sync_run_in_transaction(&mut transaction, id).await?;
        transaction.commit().await?;
        record_write("sync.fail");
        Ok(run)
    }

    pub async fn fail_running_sync_runs(&self, error: &str) -> Result<u64> {
        if error.trim().is_empty() {
            return Err(ConnectorError::InvalidInput(
                "sync failure error must not be empty".to_owned(),
            ));
        }
        let now = now_millis();
        let mut transaction = self.db.begin().await?;
        moltis_db::tx_execute!(
            &mut transaction,
            "UPDATE connector_datasets SET last_error = ?, updated_at = ? \
             WHERE id IN (SELECT dataset_id FROM connector_sync_runs WHERE status = 'running')",
            error,
            now,
        )?;
        let failed = moltis_db::tx_execute!(
            &mut transaction,
            "UPDATE connector_sync_runs SET status = 'failed', finished_at = ?, error = ? \
             WHERE status = 'running'",
            now,
            error,
        )?;
        transaction.commit().await?;
        if failed != 0 {
            record_write("sync.recover");
        }
        Ok(failed)
    }

    pub async fn list_recent_sync_runs(
        &self,
        dataset_id: &str,
        limit: u64,
    ) -> Result<Vec<SyncRun>> {
        validate_limit(limit)?;
        let rows: Vec<SyncRunRow> = moltis_db::fetch_all!(
            &self.db,
            SyncRunRow,
            "SELECT * FROM connector_sync_runs WHERE dataset_id = ? \
             ORDER BY started_at DESC, id DESC LIMIT ?",
            dataset_id,
            u64_to_i64(limit)?,
        )?;
        rows.into_iter().map(sync_run_from_row).collect()
    }

    async fn get_account_required(&self, id: &str) -> Result<Account> {
        self.get_account(id)
            .await?
            .ok_or_else(|| not_found("account", id))
    }

    async fn get_dataset_required(&self, id: &str) -> Result<Dataset> {
        self.get_dataset(id)
            .await?
            .ok_or_else(|| not_found("dataset", id))
    }
}

#[async_trait]
impl ConnectorReader for DbConnectorStore {
    async fn list_datasets_for_kind(&self, kind: ConnectorKind) -> Result<Vec<Dataset>> {
        let rows: Vec<DatasetRow> = moltis_db::fetch_all!(
            &self.db,
            DatasetRow,
            "SELECT dataset.* FROM connector_datasets AS dataset \
             JOIN connector_accounts AS account ON account.id = dataset.account_id \
             WHERE account.kind = ? ORDER BY dataset.name, dataset.id",
            kind.as_str(),
        )?;
        rows.into_iter().map(dataset_from_row).collect()
    }

    async fn query_items_for_kind(
        &self,
        kind: ConnectorKind,
        dataset_id: &str,
        query: ItemQuery,
    ) -> Result<Vec<ConnectorItem>> {
        if !self.dataset_has_kind(dataset_id, kind).await? {
            return Ok(Vec::new());
        }
        self.query_items(dataset_id, query).await
    }

    async fn get_item_for_kind(
        &self,
        kind: ConnectorKind,
        dataset_id: &str,
        item_id: &str,
    ) -> Result<Option<ConnectorItem>> {
        if !self.dataset_has_kind(dataset_id, kind).await? {
            return Ok(None);
        }
        self.get_item(dataset_id, item_id, false).await
    }
}

async fn sync_run_in_transaction(transaction: &mut DbTx, id: &str) -> Result<SyncRun> {
    let row: Option<SyncRunRow> = moltis_db::tx_fetch_optional!(
        transaction,
        SyncRunRow,
        "SELECT * FROM connector_sync_runs WHERE id = ?",
        id,
    )?;
    row.map(sync_run_from_row)
        .transpose()?
        .ok_or_else(|| not_found("sync run", id))
}

async fn apply_snapshot_transaction(
    transaction: &mut DbTx,
    dataset_id: &str,
    items: &[ConnectorItemInput],
    observed_remote_ids: &[String],
    complete: bool,
    now: i64,
) -> Result<SnapshotResult> {
    for item in items {
        let body = serde_json::to_string(&item.body_json)?;
        moltis_db::tx_execute!(
            transaction,
            "INSERT INTO connector_items \
             (id, dataset_id, remote_id, kind, remote_version, occurred_at, updated_at, \
               body_json, search_text, content_hash, created_at, stored_at, deleted_at) \
              VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, NULL) \
              ON CONFLICT(dataset_id, remote_id) DO UPDATE SET \
                kind = excluded.kind, remote_version = excluded.remote_version, \
                occurred_at = excluded.occurred_at, updated_at = excluded.updated_at, \
                body_json = excluded.body_json, search_text = excluded.search_text, \
                content_hash = excluded.content_hash, stored_at = excluded.stored_at, \
                deleted_at = NULL",
            Uuid::new_v4().to_string(),
            dataset_id,
            &item.remote_id,
            &item.kind,
            &item.remote_version,
            &item.occurred_at,
            &item.updated_at,
            body,
            &item.search_text,
            &item.content_hash,
            now,
            now,
        )?;
    }

    let deleted = if complete {
        tombstone_unobserved(transaction, dataset_id, observed_remote_ids, now).await?
    } else {
        0
    };
    let active = active_count(transaction, dataset_id).await?;
    let result = SnapshotResult {
        upserted: usize_to_u64(items.len())?,
        deleted,
        active,
    };
    let dataset_affected = moltis_db::tx_execute!(
        transaction,
        "UPDATE connector_datasets SET item_count = ?, updated_at = ? WHERE id = ?",
        u64_to_i64(active)?,
        now,
        dataset_id,
    )?;
    require_affected(dataset_affected, "dataset", dataset_id)?;
    Ok(result)
}

async fn tombstone_unobserved(
    transaction: &mut DbTx,
    dataset_id: &str,
    observed_remote_ids: &[String],
    now: i64,
) -> Result<u64> {
    let unique: HashSet<&str> = observed_remote_ids.iter().map(String::as_str).collect();
    let observed = serde_json::to_string(&unique)?;
    // SQLite expands the observed JSON array with json_each; PostgreSQL uses
    // jsonb_array_elements_text. Same shape, same binds.
    let sql = match transaction.dialect() {
        Dialect::Sqlite => {
            "UPDATE connector_items AS item SET deleted_at = ? \
             WHERE item.dataset_id = ? AND item.deleted_at IS NULL \
               AND NOT EXISTS (SELECT 1 FROM json_each(?) WHERE value = item.remote_id)"
        },
        Dialect::Postgres => {
            "UPDATE connector_items AS item SET deleted_at = ? \
             WHERE item.dataset_id = ? AND item.deleted_at IS NULL \
               AND NOT EXISTS (SELECT 1 FROM jsonb_array_elements_text(?::jsonb) AS observed \
                 WHERE observed.value = item.remote_id)"
        },
    };
    Ok(moltis_db::tx_execute!(
        transaction,
        sql,
        now,
        dataset_id,
        observed,
    )?)
}

async fn active_count(transaction: &mut DbTx, dataset_id: &str) -> Result<u64> {
    let count: i64 = moltis_db::tx_fetch_one!(
        transaction,
        (i64,),
        "SELECT COUNT(*) FROM connector_items WHERE dataset_id = ? AND deleted_at IS NULL",
        dataset_id,
    )?
    .0;
    i64_to_u64(count, "active item count")
}

fn account_from_row(row: AccountRow) -> Result<Account> {
    let AccountRow {
        id,
        kind,
        source_key,
        name,
        config,
        enabled,
        created_at,
        updated_at,
    } = row;
    Ok(Account {
        id,
        kind: ConnectorKind::try_from(kind)?,
        source_key,
        name,
        config: serde_json::from_str(&config)?,
        enabled,
        created_at: millis_to_datetime(created_at)?,
        updated_at: millis_to_datetime(updated_at)?,
    })
}

fn dataset_from_row(row: DatasetRow) -> Result<Dataset> {
    let DatasetRow {
        id,
        account_id,
        name,
        instruction,
        config,
        plan_revision,
        synced_plan_revision,
        schedule_minutes,
        projection_jsonl,
        projection_md,
        enabled,
        last_sync_at,
        next_sync_at,
        last_error,
        item_count,
        created_at,
        updated_at,
    } = row;
    Ok(Dataset {
        id,
        account_id,
        name,
        instruction,
        config: serde_json::from_str(&config)?,
        plan_revision: i64_to_u64(plan_revision, "plan_revision")?,
        synced_plan_revision: synced_plan_revision
            .map(|revision| i64_to_u64(revision, "synced_plan_revision"))
            .transpose()?,
        schedule_minutes: schedule_minutes
            .map(|value| i64_to_u64(value, "schedule_minutes"))
            .transpose()?,
        projections: ProjectionConfig {
            jsonl: projection_jsonl,
            markdown: projection_md,
        },
        enabled,
        last_sync_at: optional_datetime(last_sync_at)?,
        next_sync_at: optional_datetime(next_sync_at)?,
        last_error,
        item_count: i64_to_u64(item_count, "item_count")?,
        created_at: millis_to_datetime(created_at)?,
        updated_at: millis_to_datetime(updated_at)?,
    })
}

fn item_from_row(row: ItemRow) -> Result<ConnectorItem> {
    let ItemRow {
        id,
        dataset_id,
        remote_id,
        kind,
        remote_version,
        occurred_at,
        updated_at,
        body_json,
        search_text,
        content_hash,
        created_at,
        stored_at,
        deleted_at,
    } = row;
    Ok(ConnectorItem {
        id,
        dataset_id,
        remote_id,
        kind,
        remote_version,
        occurred_at,
        updated_at,
        body_json: serde_json::from_str(&body_json)?,
        search_text,
        content_hash,
        created_at: millis_to_datetime(created_at)?,
        stored_at: millis_to_datetime(stored_at)?,
        deleted_at: optional_datetime(deleted_at)?,
    })
}

fn sync_run_from_row(row: SyncRunRow) -> Result<SyncRun> {
    let SyncRunRow {
        id,
        dataset_id,
        status,
        started_at,
        finished_at,
        upserted,
        deleted,
        active,
        error,
    } = row;
    Ok(SyncRun {
        id,
        dataset_id,
        status: SyncRunStatus::try_from(status.as_str())?,
        started_at: millis_to_datetime(started_at)?,
        finished_at: optional_datetime(finished_at)?,
        upserted: i64_to_u64(upserted, "sync upserted")?,
        deleted: i64_to_u64(deleted, "sync deleted")?,
        active: i64_to_u64(active, "sync active")?,
        error,
    })
}

fn validate_snapshot(items: &[ConnectorItemInput]) -> Result<()> {
    let mut supplied = HashSet::with_capacity(items.len());
    for item in items {
        if item.remote_id.trim().is_empty()
            || item.kind.trim().is_empty()
            || item.content_hash.is_empty()
        {
            return Err(ConnectorError::InvalidInput(
                "snapshot remote_id, kind, and content_hash must not be empty".to_owned(),
            ));
        }
        if !supplied.insert(item.remote_id.as_str()) {
            return Err(ConnectorError::InvalidInput(format!(
                "duplicate snapshot remote_id: {}",
                item.remote_id
            )));
        }
    }
    Ok(())
}

fn require_name(name: &str, entity: &str) -> Result<()> {
    if name.trim().is_empty() {
        return Err(ConnectorError::InvalidInput(format!(
            "{entity} name must not be empty"
        )));
    }
    Ok(())
}

fn validate_optional_source_key(source_key: Option<&str>) -> Result<()> {
    source_key.map(validate_source_key).transpose().map(drop)
}

fn validate_source_key(source_key: &str) -> Result<()> {
    if source_key.trim().is_empty() {
        return Err(ConnectorError::InvalidInput(
            "account source_key must not be empty".to_owned(),
        ));
    }
    if source_key.chars().count() > MAX_SOURCE_KEY_LENGTH {
        return Err(ConnectorError::InvalidInput(format!(
            "account source_key must not exceed {MAX_SOURCE_KEY_LENGTH} characters"
        )));
    }
    Ok(())
}

fn validate_schedule(schedule: Option<u64>) -> Result<()> {
    if schedule == Some(0) {
        return Err(ConnectorError::InvalidInput(
            "schedule_minutes must be greater than zero".to_owned(),
        ));
    }
    if let Some(value) = schedule {
        u64_to_i64(value)?;
    }
    Ok(())
}

fn validate_limit(limit: u64) -> Result<()> {
    if limit == 0 || limit > MAX_QUERY_LIMIT {
        return Err(ConnectorError::QueryLimit {
            requested: limit,
            max: MAX_QUERY_LIMIT,
        });
    }
    Ok(())
}

fn require_affected(rows: u64, entity: &'static str, id: &str) -> Result<()> {
    if rows == 0 {
        return Err(not_found(entity, id));
    }
    Ok(())
}

fn not_found(entity: &'static str, id: &str) -> ConnectorError {
    ConnectorError::NotFound {
        entity,
        id: id.to_owned(),
    }
}

fn now_millis() -> i64 {
    let millis = OffsetDateTime::now_utc()
        .unix_timestamp_nanos()
        .div_euclid(1_000_000);
    i64::try_from(millis).unwrap_or(i64::MAX)
}

fn datetime_to_millis(value: OffsetDateTime) -> Result<i64> {
    i64::try_from(value.unix_timestamp_nanos().div_euclid(1_000_000)).map_err(|_| {
        ConnectorError::InvalidInput("timestamp is outside SQLite integer range".to_owned())
    })
}

fn millis_to_datetime(value: i64) -> Result<OffsetDateTime> {
    OffsetDateTime::from_unix_timestamp_nanos(i128::from(value) * 1_000_000)
        .map_err(|_| ConnectorError::InvalidTimestamp(value))
}

fn optional_datetime(value: Option<i64>) -> Result<Option<OffsetDateTime>> {
    value.map(millis_to_datetime).transpose()
}

fn u64_to_i64(value: u64) -> Result<i64> {
    i64::try_from(value).map_err(|_| {
        ConnectorError::InvalidInput("numeric value exceeds SQLite integer range".to_owned())
    })
}

fn i64_to_u64(value: i64, field: &str) -> Result<u64> {
    u64::try_from(value)
        .map_err(|_| ConnectorError::InvalidInput(format!("stored {field} must not be negative")))
}

fn usize_to_u64(value: usize) -> Result<u64> {
    u64::try_from(value)
        .map_err(|_| ConnectorError::InvalidInput("item count exceeds u64".to_owned()))
}

#[cfg(feature = "metrics")]
fn record_write(operation: &'static str) {
    metrics::counter!("moltis_connector_store_writes_total", "operation" => operation).increment(1);
}

#[cfg(not(feature = "metrics"))]
fn record_write(_operation: &'static str) {}

#[cfg(feature = "metrics")]
fn record_snapshot(result: SnapshotResult) {
    metrics::counter!("moltis_connector_snapshot_items_total", "action" => "upserted")
        .increment(result.upserted);
    metrics::counter!("moltis_connector_snapshot_items_total", "action" => "deleted")
        .increment(result.deleted);
}

#[cfg(not(feature = "metrics"))]
fn record_snapshot(_result: SnapshotResult) {}
