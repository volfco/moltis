use {
    async_trait::async_trait,
    moltis_channels::{
        Error as ChannelError, Result as ChannelResult,
        message_log::{MessageLog, MessageLogEntry, SenderSummary},
    },
    moltis_db::{Db, execute, fetch_all},
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

/// Database-backed message log (SQLite or PostgreSQL).
pub struct DbMessageLog {
    db: Db,
}

impl DbMessageLog {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    /// Initialize the message_log table schema.
    ///
    /// **Deprecated**: Schema is now managed by sqlx migrations.
    /// This method is retained for tests that use in-memory databases.
    #[doc(hidden)]
    pub async fn init(db: &Db) -> ChannelResult<()> {
        crate::run_migrations(db)
            .await
            .map_err(|e| channel_db_error("init message_log schema", e))
    }
}

#[async_trait]
impl MessageLog for DbMessageLog {
    async fn log(&self, entry: MessageLogEntry) -> ChannelResult<()> {
        execute!(
            &self.db,
            "INSERT INTO message_log
             (account_id, channel_type, peer_id, username, sender_name,
              chat_id, chat_type, body, access_granted, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            &entry.account_id,
            &entry.channel_type,
            &entry.peer_id,
            &entry.username,
            &entry.sender_name,
            &entry.chat_id,
            &entry.chat_type,
            &entry.body,
            entry.access_granted,
            entry.created_at,
        )
        .map_err(|e| channel_db_error("log channel message", e))?;
        Ok(())
    }

    async fn list_by_account(
        &self,
        channel_type: &str,
        account_id: &str,
        limit: u32,
    ) -> ChannelResult<Vec<MessageLogEntry>> {
        let rows = fetch_all!(
            &self.db,
            (
                i64,
                String,
                String,
                String,
                Option<String>,
                Option<String>,
                String,
                String,
                String,
                bool,
                i64,
            ),
            "SELECT id, account_id, channel_type, peer_id, username, sender_name,
                    chat_id, chat_type, body, access_granted, created_at
             FROM message_log
             WHERE channel_type = ? AND account_id = ?
             ORDER BY created_at DESC
             LIMIT ?",
            channel_type,
            account_id,
            i64::from(limit),
        )
        .map_err(|e| channel_db_error("list channel messages", e))?;

        Ok(rows
            .into_iter()
            .map(|r| MessageLogEntry {
                id: r.0,
                account_id: r.1,
                channel_type: r.2,
                peer_id: r.3,
                username: r.4,
                sender_name: r.5,
                chat_id: r.6,
                chat_type: r.7,
                body: r.8,
                access_granted: r.9,
                created_at: r.10,
            })
            .collect())
    }

    async fn unique_senders(
        &self,
        channel_type: &str,
        account_id: &str,
    ) -> ChannelResult<Vec<SenderSummary>> {
        let rows = fetch_all!(
            &self.db,
            (String, Option<String>, Option<String>, i64, i64, bool),
            "SELECT peer_id, username, sender_name,
                    COUNT(*) as message_count,
                    MAX(created_at) as last_seen,
                    MAX(CASE WHEN access_granted THEN 1 ELSE 0 END) > 0 as last_access_granted
             FROM message_log
             WHERE channel_type = ? AND account_id = ?
             GROUP BY peer_id
             ORDER BY last_seen DESC",
            channel_type,
            account_id,
        )
        .map_err(|e| channel_db_error("list channel senders", e))?;

        Ok(rows
            .into_iter()
            .map(|r| SenderSummary {
                peer_id: r.0,
                username: r.1,
                sender_name: r.2,
                message_count: r.3,
                last_seen: r.4,
                last_access_granted: r.5,
            })
            .collect())
    }
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use super::*;

    async fn test_db() -> Db {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        let db = Db::from(pool);
        DbMessageLog::init(&db).await.unwrap();
        db
    }

    fn sample_entry(account_id: &str, peer_id: &str, granted: bool) -> MessageLogEntry {
        MessageLogEntry {
            id: 0,
            account_id: account_id.into(),
            channel_type: "telegram".into(),
            peer_id: peer_id.into(),
            username: Some("testuser".into()),
            sender_name: Some("Test User".into()),
            chat_id: "12345".into(),
            chat_type: "dm".into(),
            body: "hello".into(),
            access_granted: granted,
            created_at: 1700000000,
        }
    }

    #[tokio::test]
    async fn log_and_list() {
        let db = test_db().await;
        let store = DbMessageLog::new(db);

        store
            .log(sample_entry("bot1", "user1", true))
            .await
            .unwrap();
        store
            .log(sample_entry("bot1", "user2", false))
            .await
            .unwrap();
        store
            .log(sample_entry("bot2", "user3", true))
            .await
            .unwrap();

        let entries = store.list_by_account("telegram", "bot1", 10).await.unwrap();
        assert_eq!(entries.len(), 2);

        let entries = store.list_by_account("telegram", "bot2", 10).await.unwrap();
        assert_eq!(entries.len(), 1);
    }

    #[tokio::test]
    async fn list_respects_limit() {
        let db = test_db().await;
        let store = DbMessageLog::new(db);

        for i in 0..5 {
            let mut e = sample_entry("bot1", "user1", true);
            e.created_at = 1700000000 + i;
            store.log(e).await.unwrap();
        }

        let entries = store.list_by_account("telegram", "bot1", 3).await.unwrap();
        assert_eq!(entries.len(), 3);
        // Most recent first
        assert!(entries[0].created_at > entries[1].created_at);
    }

    #[tokio::test]
    async fn unique_senders_groups() {
        let db = test_db().await;
        let store = DbMessageLog::new(db);

        store
            .log(sample_entry("bot1", "user1", true))
            .await
            .unwrap();
        store
            .log(sample_entry("bot1", "user1", false))
            .await
            .unwrap();
        store
            .log(sample_entry("bot1", "user2", true))
            .await
            .unwrap();

        let senders = store.unique_senders("telegram", "bot1").await.unwrap();
        assert_eq!(senders.len(), 2);
        let user1 = senders.iter().find(|s| s.peer_id == "user1").unwrap();
        assert_eq!(user1.message_count, 2);
    }

    #[tokio::test]
    async fn unique_senders_includes_denied_matrix_senders() {
        let db = test_db().await;
        let store = DbMessageLog::new(db);

        let entry = MessageLogEntry {
            id: 0,
            account_id: "matrix-bot".into(),
            channel_type: "matrix".into(),
            peer_id: "@alice:matrix.org".into(),
            username: Some("@alice:matrix.org".into()),
            sender_name: Some("Alice".into()),
            chat_id: "!room:matrix.org".into(),
            chat_type: "dm".into(),
            body: "hello".into(),
            access_granted: false,
            created_at: 1_700_000_000,
        };

        store.log(entry).await.unwrap();

        let senders = store.unique_senders("matrix", "matrix-bot").await.unwrap();
        assert_eq!(senders.len(), 1);
        assert_eq!(senders[0].peer_id, "@alice:matrix.org");
        assert!(!senders[0].last_access_granted);
    }
}
