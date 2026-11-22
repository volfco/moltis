use std::collections::{BTreeMap, HashMap, HashSet};

use {
    moltis_db::DbTx,
    sqlx::{Decode, Row, Type},
    time::OffsetDateTime,
    uuid::Uuid,
};

use crate::{
    ConnectorError, ConnectorItemInput, Result, SnapshotResult, SourceDisposition,
    SourceObservation, SourceState, SourceStateMap,
};

/// `connector_source_states` row decoded by column name on either backend.
struct SourceStateRow {
    dataset_id: String,
    remote_id: String,
    remote_version: Option<String>,
    disposition: String,
    filter_reason: Option<String>,
    evaluated_plan_revision: i64,
    last_seen_run_id: String,
    observed_at: i64,
}

impl<'r, R: Row> sqlx::FromRow<'r, R> for SourceStateRow
where
    &'r str: sqlx::ColumnIndex<R>,
    String: Decode<'r, R::Database> + Type<R::Database>,
    Option<String>: Decode<'r, R::Database> + Type<R::Database>,
    i64: Decode<'r, R::Database> + Type<R::Database>,
{
    fn from_row(row: &'r R) -> std::result::Result<Self, sqlx::Error> {
        Ok(Self {
            dataset_id: row.try_get("dataset_id")?,
            remote_id: row.try_get("remote_id")?,
            remote_version: row.try_get("remote_version")?,
            disposition: row.try_get("disposition")?,
            filter_reason: row.try_get("filter_reason")?,
            evaluated_plan_revision: row.try_get("evaluated_plan_revision")?,
            last_seen_run_id: row.try_get("last_seen_run_id")?,
            observed_at: row.try_get("observed_at")?,
        })
    }
}

pub(crate) async fn read_states(db: &moltis_db::Db, dataset_id: &str) -> Result<SourceStateMap> {
    let rows: Vec<SourceStateRow> = moltis_db::fetch_all!(
        db,
        SourceStateRow,
        "SELECT * FROM connector_source_states WHERE dataset_id = ? ORDER BY remote_id",
        dataset_id,
    )?;
    rows.into_iter()
        .map(|row| {
            let state = source_state_from_row(row)?;
            Ok((state.remote_id.clone(), state))
        })
        .collect::<Result<BTreeMap<_, _>>>()
}

pub(crate) async fn commit_observations(
    transaction: &mut DbTx,
    run_id: &str,
    dataset_id: &str,
    items: &[ConnectorItemInput],
    observations: &[SourceObservation],
    now: i64,
) -> Result<SnapshotResult> {
    validate_observations(items, observations)?;
    for observation in observations {
        moltis_db::tx_execute!(
            transaction,
            "INSERT INTO connector_source_states \
             (dataset_id, remote_id, remote_version, disposition, filter_reason, \
              evaluated_plan_revision, last_seen_run_id, observed_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT(dataset_id, remote_id) DO UPDATE SET \
               remote_version = excluded.remote_version, disposition = excluded.disposition, \
               filter_reason = excluded.filter_reason, \
               evaluated_plan_revision = excluded.evaluated_plan_revision, \
               last_seen_run_id = excluded.last_seen_run_id, observed_at = excluded.observed_at",
            dataset_id,
            &observation.remote_id,
            &observation.remote_version,
            observation.disposition.as_str(),
            &observation.filter_reason,
            u64_to_i64(observation.evaluated_plan_revision)?,
            run_id,
            now,
        )?;
    }

    upsert_items(transaction, dataset_id, items, now).await?;
    let deleted = moltis_db::tx_execute!(
        transaction,
        "UPDATE connector_items AS item SET deleted_at = ? \
         WHERE item.dataset_id = ? AND item.deleted_at IS NULL \
           AND NOT EXISTS ( \
             SELECT 1 FROM connector_source_states AS source \
             WHERE source.dataset_id = item.dataset_id AND source.remote_id = item.remote_id \
               AND source.last_seen_run_id = ? AND source.disposition = 'included' \
           )",
        now,
        dataset_id,
        run_id,
    )?;

    moltis_db::tx_execute!(
        transaction,
        "DELETE FROM connector_source_states WHERE dataset_id = ? AND last_seen_run_id <> ?",
        dataset_id,
        run_id,
    )?;

    let active: i64 = moltis_db::tx_fetch_one!(
        transaction,
        (i64,),
        "SELECT COUNT(*) FROM connector_items WHERE dataset_id = ? AND deleted_at IS NULL",
        dataset_id,
    )?
    .0;
    Ok(SnapshotResult {
        upserted: usize_to_u64(items.len())?,
        deleted,
        active: i64_to_u64(active, "active item count")?,
    })
}

async fn upsert_items(
    transaction: &mut DbTx,
    dataset_id: &str,
    items: &[ConnectorItemInput],
    now: i64,
) -> Result<()> {
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
    Ok(())
}

fn validate_observations(
    items: &[ConnectorItemInput],
    observations: &[SourceObservation],
) -> Result<()> {
    let mut supplied = HashSet::with_capacity(observations.len());
    let by_remote_id = observations
        .iter()
        .map(|observation| (observation.remote_id.as_str(), observation.disposition))
        .collect::<HashMap<_, _>>();
    for observation in observations {
        if observation.remote_id.trim().is_empty() || observation.evaluated_plan_revision == 0 {
            return Err(ConnectorError::InvalidInput(
                "source observation remote_id must not be empty and plan revision must be positive"
                    .to_owned(),
            ));
        }
        if !supplied.insert(observation.remote_id.as_str()) {
            return Err(ConnectorError::InvalidInput(format!(
                "duplicate source observation remote_id: {}",
                observation.remote_id
            )));
        }
    }
    for item in items {
        if by_remote_id.get(item.remote_id.as_str()) != Some(&SourceDisposition::Included) {
            return Err(ConnectorError::InvalidInput(format!(
                "snapshot item has no included source observation: {}",
                item.remote_id
            )));
        }
    }
    Ok(())
}

fn source_state_from_row(row: SourceStateRow) -> Result<SourceState> {
    Ok(SourceState {
        dataset_id: row.dataset_id,
        remote_id: row.remote_id,
        remote_version: row.remote_version,
        disposition: SourceDisposition::try_from(row.disposition.as_str())?,
        filter_reason: row.filter_reason,
        evaluated_plan_revision: i64_to_u64(
            row.evaluated_plan_revision,
            "evaluated_plan_revision",
        )?,
        last_seen_run_id: row.last_seen_run_id,
        observed_at: millis_to_datetime(row.observed_at)?,
    })
}

fn millis_to_datetime(value: i64) -> Result<OffsetDateTime> {
    OffsetDateTime::from_unix_timestamp_nanos(i128::from(value) * 1_000_000)
        .map_err(|_| ConnectorError::InvalidTimestamp(value))
}

fn u64_to_i64(value: u64) -> Result<i64> {
    i64::try_from(value)
        .map_err(|_| ConnectorError::InvalidInput("numeric value exceeds i64 range".to_owned()))
}

fn i64_to_u64(value: i64, field: &str) -> Result<u64> {
    u64::try_from(value)
        .map_err(|_| ConnectorError::InvalidInput(format!("stored {field} must not be negative")))
}

fn usize_to_u64(value: usize) -> Result<u64> {
    u64::try_from(value)
        .map_err(|_| ConnectorError::InvalidInput("item count exceeds u64".to_owned()))
}
