//! Persistence for webhooks, deliveries, and response actions.

use {
    async_trait::async_trait,
    moltis_db::{Db, execute, fetch_all, fetch_one, fetch_optional, tx_execute},
    time::OffsetDateTime,
};

use crate::{
    Error, Result,
    types::{
        AuthMode, DeliveryStatus, EventFilter, SessionMode, ToolPolicy, Webhook, WebhookCreate,
        WebhookDelivery, WebhookPatch, WebhookResponseAction, generate_public_id,
    },
};

/// Persistence trait for webhook data.
#[async_trait]
pub trait WebhookStore: Send + Sync {
    async fn list_webhooks(&self) -> Result<Vec<Webhook>>;
    async fn get_webhook(&self, id: i64) -> Result<Webhook>;
    async fn get_webhook_by_public_id(&self, public_id: &str) -> Result<Webhook>;
    async fn create_webhook(&self, create: WebhookCreate) -> Result<Webhook>;
    async fn update_webhook(&self, id: i64, patch: WebhookPatch) -> Result<Webhook>;
    async fn delete_webhook(&self, id: i64) -> Result<()>;
    async fn increment_delivery_count(&self, id: i64, received_at: &str) -> Result<()>;

    async fn insert_delivery(&self, delivery: &NewDelivery) -> Result<i64>;
    async fn update_delivery_status(
        &self,
        id: i64,
        status: DeliveryStatus,
        update: DeliveryUpdate,
    ) -> Result<()>;
    async fn get_delivery(&self, id: i64) -> Result<WebhookDelivery>;
    async fn list_deliveries(
        &self,
        webhook_id: i64,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<WebhookDelivery>>;
    async fn get_delivery_body(&self, delivery_id: i64) -> Result<Option<Vec<u8>>>;
    async fn find_by_delivery_key(&self, webhook_id: i64, key: &str) -> Result<Option<i64>>;
    async fn list_unprocessed_deliveries(&self) -> Result<Vec<i64>>;

    async fn insert_response_action(&self, action: &NewResponseAction) -> Result<i64>;
    async fn list_response_actions(&self, delivery_id: i64) -> Result<Vec<WebhookResponseAction>>;

    async fn prune_deliveries_before(&self, before: &str) -> Result<u64>;
}

/// Input for inserting a new delivery.
pub struct NewDelivery {
    pub webhook_id: i64,
    pub received_at: String,
    pub status: DeliveryStatus,
    pub event_type: Option<String>,
    pub entity_key: Option<String>,
    pub delivery_key: Option<String>,
    pub http_method: Option<String>,
    pub content_type: Option<String>,
    pub remote_ip: Option<String>,
    pub headers_json: Option<String>,
    pub body_size: usize,
    pub body_blob: Option<Vec<u8>>,
    pub rejection_reason: Option<String>,
}

/// Fields to update on a delivery.
#[derive(Default)]
pub struct DeliveryUpdate {
    pub session_key: Option<String>,
    pub run_error: Option<String>,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub duration_ms: Option<i64>,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
}

/// Input for inserting a response action.
pub struct NewResponseAction {
    pub delivery_id: i64,
    pub tool_name: String,
    pub input_json: Option<String>,
    pub output_json: Option<String>,
    pub status: String,
    pub error_message: Option<String>,
}

/// Database-backed webhook store (SQLite or PostgreSQL).
pub struct DbWebhookStore {
    db: Db,
}

impl DbWebhookStore {
    /// Create a store using an existing pool (migrations must already be run).
    pub fn with_pool(db: Db) -> Self {
        Self { db }
    }
}

fn now_iso() -> String {
    OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".into())
}

/// Row type for `webhooks`; both backends decode the same Rust types.
#[derive(sqlx::FromRow)]
struct WebhookRow {
    id: i64,
    name: String,
    description: Option<String>,
    enabled: bool,
    public_id: String,
    agent_id: Option<String>,
    model: Option<String>,
    system_prompt_suffix: Option<String>,
    tool_policy_json: Option<String>,
    auth_mode: String,
    auth_config_json: Option<String>,
    source_profile: String,
    source_config_json: Option<String>,
    event_filter_json: Option<String>,
    session_mode: String,
    named_session_key: Option<String>,
    allowed_cidrs_json: Option<String>,
    max_body_bytes: i64,
    rate_limit_per_minute: i64,
    delivery_count: i64,
    last_delivery_at: Option<String>,
    created_at: String,
    updated_at: String,
    deliver_only: bool,
    prompt_template: Option<String>,
    deliver_to: Option<String>,
    deliver_extra_json: Option<String>,
}

/// Row type for `webhook_deliveries`; both backends decode the same Rust types.
#[derive(sqlx::FromRow)]
struct DeliveryRow {
    id: i64,
    webhook_id: i64,
    received_at: String,
    status: String,
    event_type: Option<String>,
    entity_key: Option<String>,
    delivery_key: Option<String>,
    http_method: Option<String>,
    content_type: Option<String>,
    remote_ip: Option<String>,
    headers_json: Option<String>,
    body_size: i64,
    session_key: Option<String>,
    rejection_reason: Option<String>,
    run_error: Option<String>,
    started_at: Option<String>,
    finished_at: Option<String>,
    duration_ms: Option<i64>,
    input_tokens: Option<i64>,
    output_tokens: Option<i64>,
}

/// Row type for `webhook_response_actions`; both backends decode the same Rust
/// types.
#[derive(sqlx::FromRow)]
struct ResponseActionRow {
    id: i64,
    delivery_id: i64,
    tool_name: String,
    input_json: Option<String>,
    output_json: Option<String>,
    status: String,
    error_message: Option<String>,
    created_at: String,
}

fn row_to_webhook(row: WebhookRow) -> Result<Webhook> {
    let auth_mode: AuthMode =
        serde_json::from_value(serde_json::Value::String(row.auth_mode)).unwrap_or_default();
    let session_mode: SessionMode =
        serde_json::from_value(serde_json::Value::String(row.session_mode)).unwrap_or_default();
    let tool_policy: Option<ToolPolicy> = row
        .tool_policy_json
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok());
    let event_filter: EventFilter = row
        .event_filter_json
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();
    let allowed_cidrs: Vec<String> = row
        .allowed_cidrs_json
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();
    let auth_config: Option<serde_json::Value> = row
        .auth_config_json
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok());
    let source_config: Option<serde_json::Value> = row
        .source_config_json
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok());
    let deliver_extra = row
        .deliver_extra_json
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok());

    Ok(Webhook {
        id: row.id,
        name: row.name,
        description: row.description,
        enabled: row.enabled,
        public_id: row.public_id,
        agent_id: row.agent_id,
        model: row.model,
        system_prompt_suffix: row.system_prompt_suffix,
        tool_policy,
        auth_mode,
        auth_config,
        source_profile: row.source_profile,
        source_config,
        event_filter,
        session_mode,
        named_session_key: row.named_session_key,
        allowed_cidrs,
        max_body_bytes: row.max_body_bytes as usize,
        rate_limit_per_minute: row.rate_limit_per_minute as u32,
        delivery_count: row.delivery_count as u64,
        last_delivery_at: row.last_delivery_at,
        created_at: row.created_at,
        updated_at: row.updated_at,
        deliver_only: row.deliver_only,
        prompt_template: row.prompt_template,
        deliver_to: row.deliver_to,
        deliver_extra,
    })
}

fn row_to_delivery(row: DeliveryRow) -> Result<WebhookDelivery> {
    let status: DeliveryStatus = serde_json::from_value(serde_json::Value::String(row.status))
        .unwrap_or(DeliveryStatus::Received);

    Ok(WebhookDelivery {
        id: row.id,
        webhook_id: row.webhook_id,
        received_at: row.received_at,
        status,
        event_type: row.event_type,
        entity_key: row.entity_key,
        delivery_key: row.delivery_key,
        http_method: row.http_method,
        content_type: row.content_type,
        remote_ip: row.remote_ip,
        headers_json: row.headers_json,
        body_size: row.body_size as usize,
        session_key: row.session_key,
        rejection_reason: row.rejection_reason,
        run_error: row.run_error,
        started_at: row.started_at,
        finished_at: row.finished_at,
        duration_ms: row.duration_ms,
        input_tokens: row.input_tokens,
        output_tokens: row.output_tokens,
    })
}

#[async_trait]
impl WebhookStore for DbWebhookStore {
    async fn list_webhooks(&self) -> Result<Vec<Webhook>> {
        let rows = fetch_all!(
            &self.db,
            WebhookRow,
            "SELECT * FROM webhooks ORDER BY created_at DESC"
        )?;
        rows.into_iter().map(row_to_webhook).collect()
    }

    async fn get_webhook(&self, id: i64) -> Result<Webhook> {
        let row = fetch_optional!(
            &self.db,
            WebhookRow,
            "SELECT * FROM webhooks WHERE id = ?",
            id
        )?
        .ok_or_else(|| Error::webhook_not_found(id.to_string()))?;
        row_to_webhook(row)
    }

    async fn get_webhook_by_public_id(&self, public_id: &str) -> Result<Webhook> {
        let row = fetch_optional!(
            &self.db,
            WebhookRow,
            "SELECT * FROM webhooks WHERE public_id = ?",
            public_id,
        )?
        .ok_or_else(|| Error::webhook_not_found(public_id))?;
        row_to_webhook(row)
    }

    async fn create_webhook(&self, create: WebhookCreate) -> Result<Webhook> {
        let public_id = generate_public_id();
        let now = now_iso();
        let auth_mode_str = serde_json::to_value(&create.auth_mode)?
            .as_str()
            .unwrap_or("static_header")
            .to_string();
        let session_mode_str = serde_json::to_value(&create.session_mode)?
            .as_str()
            .unwrap_or("per_delivery")
            .to_string();
        let tool_policy_json = create
            .tool_policy
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?;
        let event_filter_json = serde_json::to_string(&create.event_filter)?;
        let auth_config_json = create
            .auth_config
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?;
        let source_config_json = create
            .source_config
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?;
        let allowed_cidrs_json = serde_json::to_string(&create.allowed_cidrs)?;
        let deliver_extra_json = create
            .deliver_extra
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?;

        // RETURNING id instead of last_insert_rowid(): the latter is
        // SQLite-only, and RETURNING works on both engines (SQLite >= 3.35).
        let (id,) = fetch_one!(
            &self.db,
            (i64,),
            "INSERT INTO webhooks (name, description, public_id, agent_id, model, system_prompt_suffix, \
             tool_policy_json, auth_mode, auth_config_json, source_profile, source_config_json, \
             event_filter_json, session_mode, named_session_key, allowed_cidrs_json, \
             max_body_bytes, rate_limit_per_minute, deliver_only, prompt_template, deliver_to, \
             deliver_extra_json, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
             RETURNING id",
            &create.name,
            &create.description,
            &public_id,
            &create.agent_id,
            &create.model,
            &create.system_prompt_suffix,
            &tool_policy_json,
            &auth_mode_str,
            &auth_config_json,
            &create.source_profile,
            &source_config_json,
            &event_filter_json,
            &session_mode_str,
            &create.named_session_key,
            &allowed_cidrs_json,
            create.max_body_bytes as i64,
            create.rate_limit_per_minute as i64,
            create.deliver_only,
            &create.prompt_template,
            &create.deliver_to,
            &deliver_extra_json,
            &now,
            &now,
        )?;

        self.get_webhook(id).await
    }

    async fn update_webhook(&self, id: i64, patch: WebhookPatch) -> Result<Webhook> {
        let mut webhook = self.get_webhook(id).await?;
        let now = now_iso();

        if let Some(name) = patch.name {
            webhook.name = name;
        }
        if let Some(desc) = patch.description {
            webhook.description = desc;
        }
        if let Some(enabled) = patch.enabled {
            webhook.enabled = enabled;
        }
        if let Some(agent_id) = patch.agent_id {
            webhook.agent_id = agent_id;
        }
        if let Some(model) = patch.model {
            webhook.model = model;
        }
        if let Some(suffix) = patch.system_prompt_suffix {
            webhook.system_prompt_suffix = suffix;
        }
        if let Some(tp) = patch.tool_policy {
            webhook.tool_policy = tp;
        }
        if let Some(am) = patch.auth_mode {
            webhook.auth_mode = am;
        }
        if let Some(ac) = patch.auth_config {
            webhook.auth_config = ac;
        }
        if let Some(sc) = patch.source_config {
            webhook.source_config = sc;
        }
        if let Some(ef) = patch.event_filter {
            webhook.event_filter = ef;
        }
        if let Some(sm) = patch.session_mode {
            webhook.session_mode = sm;
        }
        if let Some(nsk) = patch.named_session_key {
            webhook.named_session_key = nsk;
        }
        if let Some(cidrs) = patch.allowed_cidrs {
            webhook.allowed_cidrs = cidrs;
        }
        if let Some(max) = patch.max_body_bytes {
            webhook.max_body_bytes = max;
        }
        if let Some(rl) = patch.rate_limit_per_minute {
            webhook.rate_limit_per_minute = rl;
        }
        if let Some(do_) = patch.deliver_only {
            webhook.deliver_only = do_;
        }
        if let Some(pt) = patch.prompt_template {
            webhook.prompt_template = pt;
        }
        if let Some(dt) = patch.deliver_to {
            webhook.deliver_to = dt;
        }
        if let Some(de) = patch.deliver_extra {
            webhook.deliver_extra = de;
        }

        let auth_mode_str = serde_json::to_value(&webhook.auth_mode)?
            .as_str()
            .unwrap_or("static_header")
            .to_string();
        let session_mode_str = serde_json::to_value(&webhook.session_mode)?
            .as_str()
            .unwrap_or("per_delivery")
            .to_string();
        let tool_policy_json = webhook
            .tool_policy
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?;
        let event_filter_json = serde_json::to_string(&webhook.event_filter)?;
        let auth_config_json = webhook
            .auth_config
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?;
        let source_config_json = webhook
            .source_config
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?;
        let allowed_cidrs_json = serde_json::to_string(&webhook.allowed_cidrs)?;
        let deliver_extra_json = webhook
            .deliver_extra
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?;

        execute!(
            &self.db,
            "UPDATE webhooks SET name = ?, description = ?, enabled = ?, agent_id = ?, model = ?, \
             system_prompt_suffix = ?, tool_policy_json = ?, auth_mode = ?, auth_config_json = ?, \
             source_config_json = ?, event_filter_json = ?, session_mode = ?, named_session_key = ?, \
             allowed_cidrs_json = ?, max_body_bytes = ?, rate_limit_per_minute = ?, \
             deliver_only = ?, prompt_template = ?, deliver_to = ?, deliver_extra_json = ?, \
             updated_at = ? \
             WHERE id = ?",
            &webhook.name,
            &webhook.description,
            webhook.enabled,
            &webhook.agent_id,
            &webhook.model,
            &webhook.system_prompt_suffix,
            &tool_policy_json,
            &auth_mode_str,
            &auth_config_json,
            &source_config_json,
            &event_filter_json,
            &session_mode_str,
            &webhook.named_session_key,
            &allowed_cidrs_json,
            webhook.max_body_bytes as i64,
            webhook.rate_limit_per_minute as i64,
            webhook.deliver_only,
            &webhook.prompt_template,
            &webhook.deliver_to,
            &deliver_extra_json,
            &now,
            id,
        )?;

        webhook.updated_at = now;
        Ok(webhook)
    }

    async fn delete_webhook(&self, id: i64) -> Result<()> {
        // Transaction prevents partial data loss if a later DELETE fails.
        let mut tx = self.db.begin().await?;
        tx_execute!(
            &mut tx,
            "DELETE FROM webhook_response_actions WHERE delivery_id IN \
             (SELECT id FROM webhook_deliveries WHERE webhook_id = ?)",
            id,
        )?;
        tx_execute!(
            &mut tx,
            "DELETE FROM webhook_deliveries WHERE webhook_id = ?",
            id
        )?;
        let affected = tx_execute!(&mut tx, "DELETE FROM webhooks WHERE id = ?", id)?;
        if affected == 0 {
            return Err(Error::webhook_not_found(id.to_string()));
        }
        tx.commit().await?;
        Ok(())
    }

    async fn increment_delivery_count(&self, id: i64, received_at: &str) -> Result<()> {
        execute!(
            &self.db,
            "UPDATE webhooks SET delivery_count = delivery_count + 1, last_delivery_at = ? WHERE id = ?",
            received_at,
            id,
        )?;
        Ok(())
    }

    async fn insert_delivery(&self, d: &NewDelivery) -> Result<i64> {
        let status_str = serde_json::to_value(&d.status)?
            .as_str()
            .unwrap_or("received")
            .to_string();
        // RETURNING id instead of last_insert_rowid(): the latter is
        // SQLite-only, and RETURNING works on both engines (SQLite >= 3.35).
        let (id,) = fetch_one!(
            &self.db,
            (i64,),
            "INSERT INTO webhook_deliveries (webhook_id, received_at, status, event_type, entity_key, \
             delivery_key, http_method, content_type, remote_ip, headers_json, body_size, body_blob, \
             rejection_reason) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
             RETURNING id",
            d.webhook_id,
            &d.received_at,
            &status_str,
            &d.event_type,
            &d.entity_key,
            &d.delivery_key,
            &d.http_method,
            &d.content_type,
            &d.remote_ip,
            &d.headers_json,
            d.body_size as i64,
            &d.body_blob,
            &d.rejection_reason,
        )?;
        Ok(id)
    }

    async fn update_delivery_status(
        &self,
        id: i64,
        status: DeliveryStatus,
        update: DeliveryUpdate,
    ) -> Result<()> {
        let status_str = serde_json::to_value(&status)?
            .as_str()
            .unwrap_or("received")
            .to_string();
        execute!(
            &self.db,
            "UPDATE webhook_deliveries SET status = ?, session_key = COALESCE(?, session_key), \
             run_error = COALESCE(?, run_error), started_at = COALESCE(?, started_at), \
             finished_at = COALESCE(?, finished_at), duration_ms = COALESCE(?, duration_ms), \
             input_tokens = COALESCE(?, input_tokens), output_tokens = COALESCE(?, output_tokens) \
             WHERE id = ?",
            &status_str,
            &update.session_key,
            &update.run_error,
            &update.started_at,
            &update.finished_at,
            update.duration_ms,
            update.input_tokens,
            update.output_tokens,
            id,
        )?;
        Ok(())
    }

    async fn get_delivery(&self, id: i64) -> Result<WebhookDelivery> {
        let row = fetch_optional!(
            &self.db,
            DeliveryRow,
            "SELECT * FROM webhook_deliveries WHERE id = ?",
            id,
        )?
        .ok_or(Error::DeliveryNotFound { delivery_id: id })?;
        row_to_delivery(row)
    }

    async fn list_deliveries(
        &self,
        webhook_id: i64,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<WebhookDelivery>> {
        let rows = fetch_all!(
            &self.db,
            DeliveryRow,
            "SELECT * FROM webhook_deliveries WHERE webhook_id = ? ORDER BY received_at DESC LIMIT ? OFFSET ?",
            webhook_id,
            limit as i64,
            offset as i64,
        )?;
        rows.into_iter().map(row_to_delivery).collect()
    }

    async fn get_delivery_body(&self, delivery_id: i64) -> Result<Option<Vec<u8>>> {
        let row = fetch_optional!(
            &self.db,
            (Option<Vec<u8>>,),
            "SELECT body_blob FROM webhook_deliveries WHERE id = ?",
            delivery_id,
        )?;
        Ok(row.and_then(|(blob,)| blob))
    }

    async fn find_by_delivery_key(&self, webhook_id: i64, key: &str) -> Result<Option<i64>> {
        let row = fetch_optional!(
            &self.db,
            (i64,),
            "SELECT id FROM webhook_deliveries WHERE webhook_id = ? AND delivery_key = ?",
            webhook_id,
            key,
        )?;
        Ok(row.map(|(id,)| id))
    }

    async fn list_unprocessed_deliveries(&self) -> Result<Vec<i64>> {
        let rows = fetch_all!(
            &self.db,
            (i64,),
            "SELECT id FROM webhook_deliveries WHERE status IN ('received', 'queued', 'processing') ORDER BY received_at ASC",
        )?;
        Ok(rows.into_iter().map(|(id,)| id).collect())
    }

    async fn insert_response_action(&self, a: &NewResponseAction) -> Result<i64> {
        let now = now_iso();
        // RETURNING id instead of last_insert_rowid(): the latter is
        // SQLite-only, and RETURNING works on both engines (SQLite >= 3.35).
        let (id,) = fetch_one!(
            &self.db,
            (i64,),
            "INSERT INTO webhook_response_actions (delivery_id, tool_name, input_json, output_json, status, error_message, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?) \
             RETURNING id",
            a.delivery_id,
            &a.tool_name,
            &a.input_json,
            &a.output_json,
            &a.status,
            &a.error_message,
            &now,
        )?;
        Ok(id)
    }

    async fn list_response_actions(&self, delivery_id: i64) -> Result<Vec<WebhookResponseAction>> {
        let rows = fetch_all!(
            &self.db,
            ResponseActionRow,
            "SELECT * FROM webhook_response_actions WHERE delivery_id = ? ORDER BY created_at ASC",
            delivery_id,
        )?;
        Ok(rows
            .into_iter()
            .map(|row| WebhookResponseAction {
                id: row.id,
                delivery_id: row.delivery_id,
                tool_name: row.tool_name,
                input_json: row.input_json,
                output_json: row.output_json,
                status: row.status,
                error_message: row.error_message,
                created_at: row.created_at,
            })
            .collect())
    }

    async fn prune_deliveries_before(&self, before: &str) -> Result<u64> {
        let mut tx = self.db.begin().await?;
        tx_execute!(
            &mut tx,
            "DELETE FROM webhook_response_actions WHERE delivery_id IN \
             (SELECT id FROM webhook_deliveries WHERE received_at < ?)",
            before,
        )?;
        let affected = tx_execute!(
            &mut tx,
            "DELETE FROM webhook_deliveries WHERE received_at < ?",
            before,
        )?;
        tx.commit().await?;
        Ok(affected)
    }
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use {super::*, crate::types::*};

    async fn sqlite_store() -> DbWebhookStore {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        let db = Db::from(pool);
        crate::run_migrations(&db).await.unwrap();
        DbWebhookStore::with_pool(db)
    }

    /// PostgreSQL store for one test, serialized and freshly migrated.
    /// The guard keeps the crate lock until the test ends.
    async fn pg_store() -> Option<(tokio::sync::MutexGuard<'static, ()>, DbWebhookStore)> {
        if std::env::var("TEST_POSTGRES_URL").is_err() {
            eprintln!("skipping: TEST_POSTGRES_URL is not set");
            return None;
        }
        let guard = crate::test_support::pg_lock().lock().await;
        let db = crate::test_support::pg_test_db().await?;
        Some((guard, DbWebhookStore::with_pool(db)))
    }

    fn make_create(name: &str) -> WebhookCreate {
        WebhookCreate {
            name: name.into(),
            description: None,
            agent_id: None,
            model: None,
            system_prompt_suffix: None,
            tool_policy: None,
            auth_mode: AuthMode::StaticHeader,
            auth_config: Some(serde_json::json!({ "header": "X-Secret", "value": "test123" })),
            source_profile: "generic".into(),
            source_config: None,
            event_filter: EventFilter::default(),
            session_mode: SessionMode::PerDelivery,
            named_session_key: None,
            allowed_cidrs: vec![],
            max_body_bytes: 1_048_576,
            rate_limit_per_minute: 60,
            deliver_only: false,
            prompt_template: None,
            deliver_to: None,
            deliver_extra: None,
        }
    }

    fn make_delivery(webhook_id: i64) -> NewDelivery {
        NewDelivery {
            webhook_id,
            received_at: "2026-04-07T00:00:00Z".into(),
            status: DeliveryStatus::Received,
            event_type: Some("push".into()),
            entity_key: None,
            delivery_key: Some("abc-123".into()),
            http_method: Some("POST".into()),
            content_type: Some("application/json".into()),
            remote_ip: Some("1.2.3.4".into()),
            headers_json: None,
            body_size: 100,
            body_blob: Some(b"{\"test\":true}".to_vec()),
            rejection_reason: None,
        }
    }

    async fn create_and_list_body(store: &DbWebhookStore) {
        store.create_webhook(make_create("hook1")).await.unwrap();
        store.create_webhook(make_create("hook2")).await.unwrap();
        let webhooks = store.list_webhooks().await.unwrap();
        assert_eq!(webhooks.len(), 2);
    }

    async fn get_by_public_id_body(store: &DbWebhookStore) {
        let wh = store.create_webhook(make_create("hook1")).await.unwrap();
        let fetched = store.get_webhook_by_public_id(&wh.public_id).await.unwrap();
        assert_eq!(fetched.id, wh.id);
        assert_eq!(fetched.name, "hook1");
    }

    async fn update_body(store: &DbWebhookStore) {
        let wh = store.create_webhook(make_create("hook1")).await.unwrap();
        let updated = store
            .update_webhook(wh.id, WebhookPatch {
                name: Some("renamed".into()),
                enabled: Some(false),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(updated.name, "renamed");
        assert!(!updated.enabled);
    }

    async fn delete_body(store: &DbWebhookStore) {
        let wh = store.create_webhook(make_create("hook1")).await.unwrap();
        store.delete_webhook(wh.id).await.unwrap();
        assert!(store.get_webhook(wh.id).await.is_err());
    }

    async fn delivery_roundtrip_body(store: &DbWebhookStore) {
        let wh = store.create_webhook(make_create("hook1")).await.unwrap();
        let delivery_id = store.insert_delivery(&make_delivery(wh.id)).await.unwrap();

        let delivery = store.get_delivery(delivery_id).await.unwrap();
        assert_eq!(delivery.event_type.as_deref(), Some("push"));
        assert_eq!(delivery.status, DeliveryStatus::Received);

        // Get body
        let body = store.get_delivery_body(delivery_id).await.unwrap();
        assert!(body.is_some());

        // Dedup check
        let dup = store.find_by_delivery_key(wh.id, "abc-123").await.unwrap();
        assert_eq!(dup, Some(delivery_id));
    }

    async fn delivery_status_update_body(store: &DbWebhookStore) {
        let wh = store.create_webhook(make_create("hook1")).await.unwrap();
        let mut delivery = make_delivery(wh.id);
        delivery.event_type = None;
        delivery.delivery_key = None;
        delivery.http_method = None;
        delivery.content_type = None;
        delivery.remote_ip = None;
        delivery.body_size = 0;
        delivery.body_blob = None;
        let delivery_id = store.insert_delivery(&delivery).await.unwrap();

        store
            .update_delivery_status(delivery_id, DeliveryStatus::Completed, DeliveryUpdate {
                session_key: Some("webhook:test:1".into()),
                duration_ms: Some(1500),
                input_tokens: Some(100),
                output_tokens: Some(200),
                ..Default::default()
            })
            .await
            .unwrap();

        let delivery = store.get_delivery(delivery_id).await.unwrap();
        assert_eq!(delivery.status, DeliveryStatus::Completed);
        assert_eq!(delivery.session_key.as_deref(), Some("webhook:test:1"));
    }

    async fn response_actions_body(store: &DbWebhookStore) {
        let wh = store.create_webhook(make_create("hook1")).await.unwrap();
        let mut delivery = make_delivery(wh.id);
        delivery.status = DeliveryStatus::Completed;
        delivery.event_type = None;
        delivery.delivery_key = None;
        delivery.http_method = None;
        delivery.content_type = None;
        delivery.remote_ip = None;
        delivery.body_size = 0;
        delivery.body_blob = None;
        let delivery_id = store.insert_delivery(&delivery).await.unwrap();

        store
            .insert_response_action(&NewResponseAction {
                delivery_id,
                tool_name: "github_post_comment".into(),
                input_json: Some("{\"body\":\"LGTM\"}".into()),
                output_json: Some("{\"id\":42}".into()),
                status: "success".into(),
                error_message: None,
            })
            .await
            .unwrap();

        let actions = store.list_response_actions(delivery_id).await.unwrap();
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].tool_name, "github_post_comment");
    }

    async fn cascade_delete_body(store: &DbWebhookStore) {
        let wh = store.create_webhook(make_create("hook1")).await.unwrap();
        let mut delivery = make_delivery(wh.id);
        delivery.event_type = None;
        delivery.delivery_key = None;
        delivery.http_method = None;
        delivery.content_type = None;
        delivery.remote_ip = None;
        delivery.body_size = 0;
        delivery.body_blob = None;
        let delivery_id = store.insert_delivery(&delivery).await.unwrap();
        store
            .insert_response_action(&NewResponseAction {
                delivery_id,
                tool_name: "test".into(),
                input_json: None,
                output_json: None,
                status: "success".into(),
                error_message: None,
            })
            .await
            .unwrap();

        // Deleting webhook should cascade to deliveries and actions
        store.delete_webhook(wh.id).await.unwrap();
        assert!(store.get_delivery(delivery_id).await.is_err());
    }

    #[tokio::test]
    async fn test_create_and_list() {
        create_and_list_body(&sqlite_store().await).await;
    }

    #[tokio::test]
    async fn test_postgres_create_and_list() {
        let Some((_guard, store)) = pg_store().await else {
            return;
        };
        create_and_list_body(&store).await;
    }

    #[tokio::test]
    async fn test_get_by_public_id() {
        get_by_public_id_body(&sqlite_store().await).await;
    }

    #[tokio::test]
    async fn test_postgres_get_by_public_id() {
        let Some((_guard, store)) = pg_store().await else {
            return;
        };
        get_by_public_id_body(&store).await;
    }

    #[tokio::test]
    async fn test_update() {
        update_body(&sqlite_store().await).await;
    }

    #[tokio::test]
    async fn test_postgres_update() {
        let Some((_guard, store)) = pg_store().await else {
            return;
        };
        update_body(&store).await;
    }

    #[tokio::test]
    async fn test_delete() {
        delete_body(&sqlite_store().await).await;
    }

    #[tokio::test]
    async fn test_postgres_delete() {
        let Some((_guard, store)) = pg_store().await else {
            return;
        };
        delete_body(&store).await;
    }

    #[tokio::test]
    async fn test_delivery_roundtrip() {
        delivery_roundtrip_body(&sqlite_store().await).await;
    }

    #[tokio::test]
    async fn test_postgres_delivery_roundtrip() {
        let Some((_guard, store)) = pg_store().await else {
            return;
        };
        delivery_roundtrip_body(&store).await;
    }

    #[tokio::test]
    async fn test_delivery_status_update() {
        delivery_status_update_body(&sqlite_store().await).await;
    }

    #[tokio::test]
    async fn test_postgres_delivery_status_update() {
        let Some((_guard, store)) = pg_store().await else {
            return;
        };
        delivery_status_update_body(&store).await;
    }

    #[tokio::test]
    async fn test_response_actions() {
        response_actions_body(&sqlite_store().await).await;
    }

    #[tokio::test]
    async fn test_postgres_response_actions() {
        let Some((_guard, store)) = pg_store().await else {
            return;
        };
        response_actions_body(&store).await;
    }

    #[tokio::test]
    async fn test_cascade_delete() {
        cascade_delete_body(&sqlite_store().await).await;
    }

    #[tokio::test]
    async fn test_postgres_cascade_delete() {
        let Some((_guard, store)) = pg_store().await else {
            return;
        };
        cascade_delete_body(&store).await;
    }
}
