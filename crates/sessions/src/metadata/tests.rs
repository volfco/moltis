#![allow(clippy::unwrap_used, clippy::expect_used)]

use {super::*, crate::test_support::backend_test};

#[test]
fn test_upsert_and_list() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("meta.json");
    let mut meta = SessionMetadata::load(path.clone()).unwrap();

    meta.upsert("main", None);
    meta.upsert("session:abc", Some("My Chat".to_string()));

    let list = meta.list();
    assert_eq!(list.len(), 2);
    let keys: Vec<&str> = list.iter().map(|e| e.key.as_str()).collect();
    assert!(keys.contains(&"main"));
    assert!(keys.contains(&"session:abc"));
    let abc = list.iter().find(|e| e.key == "session:abc").unwrap();
    assert_eq!(abc.label.as_deref(), Some("My Chat"));
}

#[test]
fn test_list_pins_main_then_sorts_by_recency() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("meta.json");
    let mut meta = SessionMetadata::load(path).unwrap();

    meta.upsert("main", None);
    meta.upsert("session:older", None);
    meta.upsert("session:newer", None);

    if let Some(entry) = meta.entries.get_mut("main") {
        entry.created_at = 1;
        entry.updated_at = 1;
    }
    if let Some(entry) = meta.entries.get_mut("session:older") {
        entry.created_at = 100;
        entry.updated_at = 100;
    }
    if let Some(entry) = meta.entries.get_mut("session:newer") {
        entry.created_at = 200;
        entry.updated_at = 200;
    }

    let keys: Vec<String> = meta.list().into_iter().map(|entry| entry.key).collect();
    assert_eq!(keys, vec!["main", "session:newer", "session:older"]);
}

#[test]
fn test_save_and_reload() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("meta.json");

    {
        let mut meta = SessionMetadata::load(path.clone()).unwrap();
        meta.upsert("main", Some("Main".to_string()));
        meta.save().unwrap();
    }

    let meta = SessionMetadata::load(path).unwrap();
    let entry = meta.get("main").unwrap();
    assert_eq!(entry.label.as_deref(), Some("Main"));
}

#[test]
fn test_remove() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("meta.json");
    let mut meta = SessionMetadata::load(path).unwrap();

    meta.upsert("main", None);
    assert!(meta.get("main").is_some());
    meta.remove("main");
    assert!(meta.get("main").is_none());
}

#[test]
fn external_agent_kind_parses_named_acp_variants() {
    for (raw, kind, display_name) in [
        ("acp-copilot", ExternalAgentKind::AcpCopilot, "ACP: Copilot"),
        ("acp-codex", ExternalAgentKind::AcpCodex, "ACP: Codex"),
        ("acp-claude", ExternalAgentKind::AcpClaude, "ACP: Claude"),
        ("acp-pi", ExternalAgentKind::AcpPi, "ACP: Pi"),
        (
            "acp-opencode",
            ExternalAgentKind::AcpOpencode,
            "ACP: opencode",
        ),
        ("acp-gemini", ExternalAgentKind::AcpGemini, "ACP: Gemini"),
        ("acp-augment", ExternalAgentKind::AcpAugment, "ACP: Augment"),
        ("acp-kiro", ExternalAgentKind::AcpKiro, "ACP: Kiro"),
        (
            "acp-openclaw",
            ExternalAgentKind::AcpOpenclaw,
            "ACP: OpenClaw",
        ),
        (
            "acp-openhands",
            ExternalAgentKind::AcpOpenhands,
            "ACP: OpenHands",
        ),
        ("acp-kimi", ExternalAgentKind::AcpKimi, "ACP: Kimi"),
        (
            "acp-minimax-code",
            ExternalAgentKind::AcpMinimaxCode,
            "ACP: MiniMax Code",
        ),
        ("acp-stakpak", ExternalAgentKind::AcpStakpak, "ACP: Stakpak"),
        (
            "acp-fast-agent",
            ExternalAgentKind::AcpFastAgent,
            "ACP: fast-agent",
        ),
    ] {
        assert_eq!(raw.parse::<ExternalAgentKind>(), Ok(kind));
        assert_eq!(kind.as_str(), raw);
        assert_eq!(kind.display_name(), display_name);
        assert!(kind.is_acp());
    }
}

// Store tests are written once as `run_*` bodies taking a `Db` and run on
// both engines via `backend_test!` below.

async fn run_upsert_and_list(db: &Db) {
    let meta = DbSessionMetadata::new(db.clone());

    meta.upsert("main", None).await.unwrap();
    meta.upsert("session:abc", Some("My Chat".to_string()))
        .await
        .unwrap();

    let list = meta.list().await;
    assert_eq!(list.len(), 2);
    let abc = list.iter().find(|e| e.key == "session:abc").unwrap();
    assert_eq!(abc.label.as_deref(), Some("My Chat"));
}

async fn run_list_pins_main_then_sorts_by_recency(db: &Db) {
    let meta = DbSessionMetadata::new(db.clone());

    meta.upsert("main", None).await.unwrap();
    meta.upsert("session:older", None).await.unwrap();
    meta.upsert("session:newer", None).await.unwrap();

    meta.set_timestamps_and_counts("main", 1, 1, 0, 0).await;
    meta.set_timestamps_and_counts("session:older", 100, 100, 0, 0)
        .await;
    meta.set_timestamps_and_counts("session:newer", 200, 200, 0, 0)
        .await;

    let keys: Vec<String> = meta
        .list()
        .await
        .into_iter()
        .map(|entry| entry.key)
        .collect();
    assert_eq!(keys, vec!["main", "session:newer", "session:older"]);
}

async fn run_remove(db: &Db) {
    let meta = DbSessionMetadata::new(db.clone());

    meta.upsert("main", None).await.unwrap();
    assert!(meta.get("main").await.is_some());
    meta.remove("main").await;
    assert!(meta.get("main").await.is_none());
}

async fn run_touch(db: &Db) {
    let meta = DbSessionMetadata::new(db.clone());

    meta.upsert("main", None).await.unwrap();
    meta.touch("main", 5).await;
    assert_eq!(meta.get("main").await.unwrap().message_count, 5);
}

async fn run_set_external_agent(db: &Db) {
    let meta = DbSessionMetadata::new(db.clone());

    meta.upsert("main", None).await.unwrap();
    meta.set_external_agent(
        "main",
        Some(ExternalAgentKind::Codex),
        Some("external-123".to_string()),
    )
    .await;

    let entry = meta.get("main").await.unwrap();
    assert_eq!(entry.external_agent_kind, Some(ExternalAgentKind::Codex));
    assert_eq!(entry.external_session_id.as_deref(), Some("external-123"));

    meta.set_external_agent("main", None, None).await;
    let entry = meta.get("main").await.unwrap();
    assert_eq!(entry.external_agent_kind, None);
    assert_eq!(entry.external_session_id, None);
}

async fn run_set_timestamps_and_counts(db: &Db) {
    let meta = DbSessionMetadata::new(db.clone());

    meta.upsert("main", None).await.unwrap();
    meta.set_timestamps_and_counts("main", 100, 200, 5, 3).await;

    let entry = meta.get("main").await.unwrap();
    assert_eq!(entry.created_at, 100);
    assert_eq!(entry.updated_at, 200);
    assert_eq!(entry.message_count, 5);
    assert_eq!(entry.last_seen_message_count, 3);
}

async fn run_mark_seen(db: &Db) {
    let meta = DbSessionMetadata::new(db.clone());

    meta.upsert("main", None).await.unwrap();
    // New session starts with last_seen_message_count = 0.
    assert_eq!(meta.get("main").await.unwrap().last_seen_message_count, 0);

    // Simulate receiving messages.
    meta.touch("main", 5).await;
    // touch does NOT change last_seen_message_count.
    assert_eq!(meta.get("main").await.unwrap().last_seen_message_count, 0);

    // Mark as seen.
    meta.mark_seen("main").await;
    let entry = meta.get("main").await.unwrap();
    assert_eq!(entry.last_seen_message_count, 5);
    assert_eq!(entry.message_count, 5);

    // More messages arrive — last_seen stays at previous value.
    meta.touch("main", 8).await;
    let entry = meta.get("main").await.unwrap();
    assert_eq!(entry.message_count, 8);
    assert_eq!(entry.last_seen_message_count, 5);
}

async fn run_mark_seen_emits_patched_event(db: &Db) {
    let bus = crate::session_events::SessionEventBus::new();
    let meta = DbSessionMetadata::with_event_bus(db.clone(), bus.clone());
    let mut rx = bus.subscribe();

    meta.upsert("main", None).await.unwrap();
    let created = rx.recv().await.unwrap();
    assert!(
        matches!(
            created,
            crate::session_events::SessionEvent::Created { session_key } if session_key == "main"
        ),
        "expected created event after upsert"
    );

    meta.mark_seen("main").await;
    let patched = rx.recv().await.unwrap();
    assert!(
        matches!(
            patched,
            crate::session_events::SessionEvent::Patched { session_key } if session_key == "main"
        ),
        "expected patched event after mark_seen"
    );
}

#[test]
fn test_touch() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("meta.json");
    let mut meta = SessionMetadata::load(path).unwrap();

    meta.upsert("main", None);
    meta.touch("main", 5);
    assert_eq!(meta.get("main").unwrap().message_count, 5);
}

#[test]
fn test_archived() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("meta.json");
    let mut meta = SessionMetadata::load(path.clone()).unwrap();

    meta.upsert("main", None);
    assert!(!meta.get("main").unwrap().archived);

    meta.set_archived("main", true);
    assert!(meta.get("main").unwrap().archived);

    meta.save().unwrap();
    let reloaded = SessionMetadata::load(path).unwrap();
    assert!(reloaded.get("main").unwrap().archived);
}

#[test]
fn test_sandbox_enabled() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("meta.json");
    let mut meta = SessionMetadata::load(path.clone()).unwrap();

    meta.upsert("main", None);
    assert!(meta.get("main").unwrap().sandbox_enabled.is_none());

    meta.set_sandbox_enabled("main", Some(true));
    assert_eq!(meta.get("main").unwrap().sandbox_enabled, Some(true));

    meta.set_sandbox_enabled("main", None);
    assert!(meta.get("main").unwrap().sandbox_enabled.is_none());

    // Verify it round-trips through save/load.
    meta.set_sandbox_enabled("main", Some(false));
    meta.save().unwrap();
    let reloaded = SessionMetadata::load(path).unwrap();
    assert_eq!(reloaded.get("main").unwrap().sandbox_enabled, Some(false));
}

#[test]
fn test_worktree_branch() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("meta.json");
    let mut meta = SessionMetadata::load(path.clone()).unwrap();

    meta.upsert("main", None);
    assert!(meta.get("main").unwrap().worktree_branch.is_none());

    meta.set_worktree_branch("main", Some("moltis/abc".to_string()));
    assert_eq!(
        meta.get("main").unwrap().worktree_branch.as_deref(),
        Some("moltis/abc")
    );

    meta.set_worktree_branch("main", None);
    assert!(meta.get("main").unwrap().worktree_branch.is_none());

    // Round-trip through save/load.
    meta.set_worktree_branch("main", Some("moltis/xyz".to_string()));
    meta.save().unwrap();
    let reloaded = SessionMetadata::load(path).unwrap();
    assert_eq!(
        reloaded.get("main").unwrap().worktree_branch.as_deref(),
        Some("moltis/xyz")
    );
}

async fn run_worktree_branch(db: &Db) {
    let meta = DbSessionMetadata::new(db.clone());

    meta.upsert("main", None).await.unwrap();
    assert!(meta.get("main").await.unwrap().worktree_branch.is_none());

    meta.set_worktree_branch("main", Some("moltis/abc".to_string()))
        .await;
    assert_eq!(
        meta.get("main").await.unwrap().worktree_branch.as_deref(),
        Some("moltis/abc")
    );

    meta.set_worktree_branch("main", None).await;
    assert!(meta.get("main").await.unwrap().worktree_branch.is_none());
}

async fn run_archived(db: &Db) {
    let meta = DbSessionMetadata::new(db.clone());

    meta.upsert("main", None).await.unwrap();
    assert!(!meta.get("main").await.unwrap().archived);

    meta.set_archived("main", true).await;
    assert!(meta.get("main").await.unwrap().archived);

    meta.set_archived("main", false).await;
    assert!(!meta.get("main").await.unwrap().archived);
}

#[test]
fn test_sandbox_image() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("meta.json");
    let mut meta = SessionMetadata::load(path.clone()).unwrap();

    meta.upsert("main", None);
    assert!(meta.get("main").unwrap().sandbox_image.is_none());

    meta.set_sandbox_image("main", Some("custom:latest".to_string()));
    assert_eq!(
        meta.get("main").unwrap().sandbox_image.as_deref(),
        Some("custom:latest")
    );

    meta.set_sandbox_image("main", None);
    assert!(meta.get("main").unwrap().sandbox_image.is_none());

    // Round-trip through save/load.
    meta.set_sandbox_image("main", Some("alpine:3.20".to_string()));
    meta.save().unwrap();
    let reloaded = SessionMetadata::load(path).unwrap();
    assert_eq!(
        reloaded.get("main").unwrap().sandbox_image.as_deref(),
        Some("alpine:3.20")
    );
}

async fn run_sandbox_image(db: &Db) {
    let meta = DbSessionMetadata::new(db.clone());

    meta.upsert("main", None).await.unwrap();
    assert!(meta.get("main").await.unwrap().sandbox_image.is_none());

    meta.set_sandbox_image("main", Some("custom:latest".to_string()))
        .await;
    assert_eq!(
        meta.get("main").await.unwrap().sandbox_image.as_deref(),
        Some("custom:latest")
    );

    meta.set_sandbox_image("main", None).await;
    assert!(meta.get("main").await.unwrap().sandbox_image.is_none());
}

#[test]
fn test_sandbox_enabled_serde_compat() {
    // Existing metadata without sandbox_enabled should deserialize fine.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("meta.json");
    fs::write(
        &path,
        r#"{"main":{"id":"1","key":"main","label":null,"created_at":0,"updated_at":0,"message_count":0}}"#,
    )
    .unwrap();
    let meta = SessionMetadata::load(path).unwrap();
    assert!(meta.get("main").unwrap().sandbox_enabled.is_none());
}

#[test]
fn test_channel_binding() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("meta.json");
    let mut meta = SessionMetadata::load(path.clone()).unwrap();

    meta.upsert("tg:bot1:123", None);
    assert!(meta.get("tg:bot1:123").unwrap().channel_binding.is_none());

    let binding = r#"{"channel_type":"telegram","account_id":"bot1","chat_id":"123"}"#;
    meta.set_channel_binding("tg:bot1:123", Some(binding.to_string()));
    assert_eq!(
        meta.get("tg:bot1:123").unwrap().channel_binding.as_deref(),
        Some(binding)
    );

    meta.set_channel_binding("tg:bot1:123", None);
    assert!(meta.get("tg:bot1:123").unwrap().channel_binding.is_none());

    // Round-trip through save/load.
    meta.set_channel_binding("tg:bot1:123", Some(binding.to_string()));
    meta.save().unwrap();
    let reloaded = SessionMetadata::load(path).unwrap();
    assert_eq!(
        reloaded
            .get("tg:bot1:123")
            .unwrap()
            .channel_binding
            .as_deref(),
        Some(binding)
    );
}

async fn run_active_session(db: &Db) {
    let meta = DbSessionMetadata::new(db.clone());

    // No active session initially.
    assert!(
        meta.get_active_session("telegram", "bot1", "123", None)
            .await
            .is_none()
    );

    // Set and get.
    meta.set_active_session("telegram", "bot1", "123", None, "session:abc")
        .await;
    assert_eq!(
        meta.get_active_session("telegram", "bot1", "123", None)
            .await
            .as_deref(),
        Some("session:abc")
    );

    // Overwrite.
    meta.set_active_session("telegram", "bot1", "123", None, "session:def")
        .await;
    assert_eq!(
        meta.get_active_session("telegram", "bot1", "123", None)
            .await
            .as_deref(),
        Some("session:def")
    );

    // Different chat_id is independent.
    assert!(
        meta.get_active_session("telegram", "bot1", "456", None)
            .await
            .is_none()
    );

    // Thread ID isolates sessions within the same chat.
    meta.set_active_session("telegram", "bot1", "123", Some("42"), "session:topic")
        .await;
    assert_eq!(
        meta.get_active_session("telegram", "bot1", "123", Some("42"))
            .await
            .as_deref(),
        Some("session:topic")
    );
    // Original chat without thread_id still has its own session.
    assert_eq!(
        meta.get_active_session("telegram", "bot1", "123", None)
            .await
            .as_deref(),
        Some("session:def")
    );
}

async fn run_list_channel_sessions(db: &Db) {
    let meta = DbSessionMetadata::new(db.clone());

    let binding = r#"{"channel_type":"telegram","account_id":"bot1","chat_id":"123"}"#.to_string();

    // Create two sessions with the same channel binding.
    meta.upsert("telegram:bot1:123", Some("Session 1".into()))
        .await
        .unwrap();
    meta.set_channel_binding("telegram:bot1:123", Some(binding.clone()))
        .await;

    meta.upsert("session:new1", Some("Session 2".into()))
        .await
        .unwrap();
    meta.set_channel_binding("session:new1", Some(binding.clone()))
        .await;

    let sessions = meta.list_channel_sessions("telegram", "bot1", "123").await;
    assert_eq!(sessions.len(), 2);
    let keys: Vec<&str> = sessions.iter().map(|s| s.key.as_str()).collect();
    assert!(keys.contains(&"telegram:bot1:123"));
    assert!(keys.contains(&"session:new1"));

    // Different chat should return empty.
    let other = meta.list_channel_sessions("telegram", "bot1", "999").await;
    assert!(other.is_empty());
}

async fn run_clear_active_session_mappings(db: &Db) {
    let meta = DbSessionMetadata::new(db.clone());

    meta.set_active_session("telegram", "bot1", "123", None, "session:abc")
        .await;
    meta.set_active_session("telegram", "bot1", "456", None, "session:abc")
        .await;
    meta.set_active_session("telegram", "bot1", "789", None, "session:def")
        .await;

    meta.clear_active_session_mappings("session:abc").await;

    assert!(
        meta.get_active_session("telegram", "bot1", "123", None)
            .await
            .is_none()
    );
    assert!(
        meta.get_active_session("telegram", "bot1", "456", None)
            .await
            .is_none()
    );
    assert_eq!(
        meta.get_active_session("telegram", "bot1", "789", None)
            .await
            .as_deref(),
        Some("session:def")
    );
}

async fn run_channel_binding(db: &Db) {
    let meta = DbSessionMetadata::new(db.clone());

    meta.upsert("tg:bot1:123", None).await.unwrap();
    assert!(
        meta.get("tg:bot1:123")
            .await
            .unwrap()
            .channel_binding
            .is_none()
    );

    let binding = r#"{"channel_type":"telegram","account_id":"bot1","chat_id":"123"}"#;
    meta.set_channel_binding("tg:bot1:123", Some(binding.to_string()))
        .await;
    assert_eq!(
        meta.get("tg:bot1:123")
            .await
            .unwrap()
            .channel_binding
            .as_deref(),
        Some(binding)
    );

    meta.set_channel_binding("tg:bot1:123", None).await;
    assert!(
        meta.get("tg:bot1:123")
            .await
            .unwrap()
            .channel_binding
            .is_none()
    );
}

#[test]
fn test_mcp_disabled() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("meta.json");
    let mut meta = SessionMetadata::load(path.clone()).unwrap();

    meta.upsert("main", None);
    assert!(meta.get("main").unwrap().mcp_disabled.is_none());

    meta.set_mcp_disabled("main", Some(true));
    assert_eq!(meta.get("main").unwrap().mcp_disabled, Some(true));

    meta.set_mcp_disabled("main", Some(false));
    assert_eq!(meta.get("main").unwrap().mcp_disabled, Some(false));

    meta.set_mcp_disabled("main", None);
    assert!(meta.get("main").unwrap().mcp_disabled.is_none());

    // Round-trip through save/load.
    meta.set_mcp_disabled("main", Some(true));
    meta.save().unwrap();
    let reloaded = SessionMetadata::load(path).unwrap();
    assert_eq!(reloaded.get("main").unwrap().mcp_disabled, Some(true));
}

async fn run_mcp_disabled(db: &Db) {
    let meta = DbSessionMetadata::new(db.clone());

    meta.upsert("main", None).await.unwrap();
    assert!(meta.get("main").await.unwrap().mcp_disabled.is_none());

    meta.set_mcp_disabled("main", Some(true)).await;
    assert_eq!(meta.get("main").await.unwrap().mcp_disabled, Some(true));

    meta.set_mcp_disabled("main", Some(false)).await;
    assert_eq!(meta.get("main").await.unwrap().mcp_disabled, Some(false));

    meta.set_mcp_disabled("main", None).await;
    assert!(meta.get("main").await.unwrap().mcp_disabled.is_none());
}

async fn run_version_starts_at_zero(db: &Db) {
    let meta = DbSessionMetadata::new(db.clone());

    let entry = meta.upsert("main", None).await.unwrap();
    assert_eq!(entry.version, 0);
}

async fn run_version_increments_on_mutation(db: &Db) {
    let meta = DbSessionMetadata::new(db.clone());

    meta.upsert("main", None).await.unwrap();
    assert_eq!(meta.get("main").await.unwrap().version, 0);

    meta.set_model("main", Some("gpt-4".to_string())).await;
    assert_eq!(meta.get("main").await.unwrap().version, 1);

    meta.touch("main", 5).await;
    assert_eq!(meta.get("main").await.unwrap().version, 2);

    // Insert a project row so the FK constraint is satisfied.
    execute!(&meta.db, "INSERT INTO projects (id) VALUES ('proj1')").unwrap();
    meta.set_project_id("main", Some("proj1".to_string())).await;
    assert_eq!(meta.get("main").await.unwrap().version, 3);

    meta.set_archived("main", true).await;
    assert_eq!(meta.get("main").await.unwrap().version, 4);

    meta.set_sandbox_enabled("main", Some(true)).await;
    assert_eq!(meta.get("main").await.unwrap().version, 5);

    meta.set_sandbox_image("main", Some("img:1".to_string()))
        .await;
    assert_eq!(meta.get("main").await.unwrap().version, 6);

    meta.set_worktree_branch("main", Some("branch".to_string()))
        .await;
    assert_eq!(meta.get("main").await.unwrap().version, 7);

    meta.set_mcp_disabled("main", Some(true)).await;
    assert_eq!(meta.get("main").await.unwrap().version, 8);

    meta.set_channel_binding("main", Some("{}".to_string()))
        .await;
    assert_eq!(meta.get("main").await.unwrap().version, 9);

    meta.set_parent("main", Some("parent".to_string()), Some(0))
        .await;
    assert_eq!(meta.get("main").await.unwrap().version, 10);

    meta.mark_seen("main").await;
    assert_eq!(meta.get("main").await.unwrap().version, 11);

    meta.set_preview("main", Some("hello")).await;
    assert_eq!(meta.get("main").await.unwrap().version, 12);

    meta.set_agent_id("main", Some("agent-1")).await.unwrap();
    assert_eq!(meta.get("main").await.unwrap().version, 13);
}

async fn run_version_increments_on_upsert_update(db: &Db) {
    let meta = DbSessionMetadata::new(db.clone());

    meta.upsert("main", Some("First".to_string()))
        .await
        .unwrap();
    assert_eq!(meta.get("main").await.unwrap().version, 0);

    // Upsert with existing key bumps version via ON CONFLICT.
    meta.upsert("main", Some("Second".to_string()))
        .await
        .unwrap();
    assert_eq!(meta.get("main").await.unwrap().version, 1);
}

async fn run_version_in_list(db: &Db) {
    let meta = DbSessionMetadata::new(db.clone());

    meta.upsert("main", None).await.unwrap();
    meta.touch("main", 3).await;

    let list = meta.list().await;
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].version, 1);
}

#[test]
fn test_json_backend_version() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("meta.json");
    let mut meta = SessionMetadata::load(path.clone()).unwrap();

    meta.upsert("main", None);
    assert_eq!(meta.get("main").unwrap().version, 0);

    meta.set_model("main", Some("gpt-4".to_string()));
    assert_eq!(meta.get("main").unwrap().version, 1);

    meta.touch("main", 5);
    assert_eq!(meta.get("main").unwrap().version, 2);

    // Upsert with label change bumps version.
    meta.upsert("main", Some("New Label".to_string()));
    assert_eq!(meta.get("main").unwrap().version, 3);

    // Upsert without change does not bump version.
    meta.upsert("main", Some("New Label".to_string()));
    assert_eq!(meta.get("main").unwrap().version, 3);

    // Round-trip through save/load preserves version.
    meta.save().unwrap();
    let reloaded = SessionMetadata::load(path).unwrap();
    assert_eq!(reloaded.get("main").unwrap().version, 3);
}

#[test]
fn test_agent_id() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("meta.json");
    let mut meta = SessionMetadata::load(path.clone()).unwrap();

    meta.upsert("main", None);
    assert!(meta.get("main").unwrap().agent_id.is_none());

    meta.set_agent_id("main", Some("agent-1".to_string()));
    assert_eq!(
        meta.get("main").unwrap().agent_id.as_deref(),
        Some("agent-1")
    );

    meta.set_agent_id("main", None);
    assert!(meta.get("main").unwrap().agent_id.is_none());

    // Round-trip through save/load.
    meta.set_agent_id("main", Some("agent-2".to_string()));
    meta.save().unwrap();
    let reloaded = SessionMetadata::load(path).unwrap();
    assert_eq!(
        reloaded.get("main").unwrap().agent_id.as_deref(),
        Some("agent-2")
    );
}

#[test]
fn test_mode_id() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("meta.json");
    let mut meta = SessionMetadata::load(path.clone()).unwrap();

    meta.upsert("main", None);
    assert!(meta.get("main").unwrap().mode_id.is_none());

    meta.set_mode_id("main", Some("review".to_string()));
    assert_eq!(meta.get("main").unwrap().mode_id.as_deref(), Some("review"));

    meta.set_mode_id("main", None);
    assert!(meta.get("main").unwrap().mode_id.is_none());

    meta.set_mode_id("main", Some("concise".to_string()));
    meta.save().unwrap();
    let reloaded = SessionMetadata::load(path).unwrap();
    assert_eq!(
        reloaded.get("main").unwrap().mode_id.as_deref(),
        Some("concise")
    );
}

#[test]
fn test_list_by_agent_id() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("meta.json");
    let mut meta = SessionMetadata::load(path).unwrap();

    meta.upsert("s1", Some("Session 1".to_string()));
    meta.upsert("s2", Some("Session 2".to_string()));
    meta.upsert("s3", Some("Session 3".to_string()));

    meta.set_agent_id("s1", Some("agent-a".to_string()));
    meta.set_agent_id("s2", Some("agent-a".to_string()));
    meta.set_agent_id("s3", Some("agent-b".to_string()));

    let agent_a = meta.list_by_agent_id("agent-a");
    assert_eq!(agent_a.len(), 2);
    let keys: Vec<&str> = agent_a.iter().map(|e| e.key.as_str()).collect();
    assert!(keys.contains(&"s1"));
    assert!(keys.contains(&"s2"));

    let agent_b = meta.list_by_agent_id("agent-b");
    assert_eq!(agent_b.len(), 1);
    assert_eq!(agent_b[0].key, "s3");

    let none = meta.list_by_agent_id("agent-missing");
    assert!(none.is_empty());
}

#[test]
fn test_delete_by_agent_id() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("meta.json");
    let mut meta = SessionMetadata::load(path).unwrap();

    meta.upsert("s1", None);
    meta.upsert("s2", None);
    meta.upsert("s3", None);

    meta.set_agent_id("s1", Some("agent-a".to_string()));
    meta.set_agent_id("s2", Some("agent-a".to_string()));
    meta.set_agent_id("s3", Some("agent-b".to_string()));

    let deleted = meta.delete_by_agent_id("agent-a");
    assert_eq!(deleted, 2);
    assert!(meta.get("s1").is_none());
    assert!(meta.get("s2").is_none());
    assert!(meta.get("s3").is_some());

    // Deleting a non-existent agent returns 0.
    let deleted = meta.delete_by_agent_id("agent-missing");
    assert_eq!(deleted, 0);
}

async fn run_agent_id(db: &Db) {
    let meta = DbSessionMetadata::new(db.clone());

    meta.upsert("main", None).await.unwrap();
    assert!(meta.get("main").await.unwrap().agent_id.is_none());

    meta.set_agent_id("main", Some("agent-1")).await.unwrap();
    assert_eq!(
        meta.get("main").await.unwrap().agent_id.as_deref(),
        Some("agent-1")
    );

    meta.set_agent_id("main", None).await.unwrap();
    assert!(meta.get("main").await.unwrap().agent_id.is_none());
}

async fn run_mode_id(db: &Db) {
    let meta = DbSessionMetadata::new(db.clone());

    meta.upsert("main", None).await.unwrap();
    assert!(meta.get("main").await.unwrap().mode_id.is_none());

    meta.set_mode_id("main", Some("review")).await.unwrap();
    assert_eq!(
        meta.get("main").await.unwrap().mode_id.as_deref(),
        Some("review")
    );

    meta.set_mode_id("main", None).await.unwrap();
    assert!(meta.get("main").await.unwrap().mode_id.is_none());
}

async fn run_list_by_agent_id(db: &Db) {
    let meta = DbSessionMetadata::new(db.clone());

    meta.upsert("s1", Some("Session 1".to_string()))
        .await
        .unwrap();
    meta.upsert("s2", Some("Session 2".to_string()))
        .await
        .unwrap();
    meta.upsert("s3", Some("Session 3".to_string()))
        .await
        .unwrap();

    meta.set_agent_id("s1", Some("agent-a")).await.unwrap();
    meta.set_agent_id("s2", Some("agent-a")).await.unwrap();
    meta.set_agent_id("s3", Some("agent-b")).await.unwrap();

    let agent_a = meta.list_by_agent_id("agent-a").await.unwrap();
    assert_eq!(agent_a.len(), 2);
    let keys: Vec<&str> = agent_a.iter().map(|e| e.key.as_str()).collect();
    assert!(keys.contains(&"s1"));
    assert!(keys.contains(&"s2"));

    let agent_b = meta.list_by_agent_id("agent-b").await.unwrap();
    assert_eq!(agent_b.len(), 1);
    assert_eq!(agent_b[0].key, "s3");

    let none = meta.list_by_agent_id("agent-missing").await.unwrap();
    assert!(none.is_empty());
}

async fn run_delete_by_agent_id(db: &Db) {
    let meta = DbSessionMetadata::new(db.clone());

    meta.upsert("s1", None).await.unwrap();
    meta.upsert("s2", None).await.unwrap();
    meta.upsert("s3", None).await.unwrap();

    meta.set_agent_id("s1", Some("agent-a")).await.unwrap();
    meta.set_agent_id("s2", Some("agent-a")).await.unwrap();
    meta.set_agent_id("s3", Some("agent-b")).await.unwrap();

    let deleted = meta.delete_by_agent_id("agent-a").await.unwrap();
    assert_eq!(deleted, 2);
    assert!(meta.get("s1").await.is_none());
    assert!(meta.get("s2").await.is_none());
    assert!(meta.get("s3").await.is_some());

    // Deleting a non-existent agent returns 0.
    let deleted = meta.delete_by_agent_id("agent-missing").await.unwrap();
    assert_eq!(deleted, 0);
}

#[test]
fn test_agent_id_serde_compat() {
    // Existing metadata without agent_id or mode_id should deserialize fine.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("meta.json");
    fs::write(
        &path,
        r#"{"main":{"id":"1","key":"main","label":null,"created_at":0,"updated_at":0,"message_count":0}}"#,
    )
    .unwrap();
    let meta = SessionMetadata::load(path).unwrap();
    assert!(meta.get("main").unwrap().agent_id.is_none());
    assert!(meta.get("main").unwrap().mode_id.is_none());
}

// Cross-backend runners: every store test above runs on SQLite and, when
// TEST_POSTGRES_URL is set, on PostgreSQL with the same assertions.
backend_test!(
    test_sqlite_upsert_and_list,
    test_postgres_upsert_and_list,
    run_upsert_and_list
);
backend_test!(
    test_sqlite_list_pins_main_then_sorts_by_recency,
    test_postgres_list_pins_main_then_sorts_by_recency,
    run_list_pins_main_then_sorts_by_recency
);
backend_test!(test_sqlite_remove, test_postgres_remove, run_remove);
backend_test!(test_sqlite_touch, test_postgres_touch, run_touch);
backend_test!(
    test_sqlite_set_external_agent,
    test_postgres_set_external_agent,
    run_set_external_agent
);
backend_test!(
    test_sqlite_set_timestamps_and_counts,
    test_postgres_set_timestamps_and_counts,
    run_set_timestamps_and_counts
);
backend_test!(
    test_sqlite_mark_seen,
    test_postgres_mark_seen,
    run_mark_seen
);
backend_test!(
    test_sqlite_mark_seen_emits_patched_event,
    test_postgres_mark_seen_emits_patched_event,
    run_mark_seen_emits_patched_event
);
backend_test!(
    test_sqlite_worktree_branch,
    test_postgres_worktree_branch,
    run_worktree_branch
);
backend_test!(test_sqlite_archived, test_postgres_archived, run_archived);
backend_test!(
    test_sqlite_sandbox_image,
    test_postgres_sandbox_image,
    run_sandbox_image
);
backend_test!(
    test_sqlite_active_session,
    test_postgres_active_session,
    run_active_session
);
backend_test!(
    test_sqlite_list_channel_sessions,
    test_postgres_list_channel_sessions,
    run_list_channel_sessions
);
backend_test!(
    test_sqlite_clear_active_session_mappings,
    test_postgres_clear_active_session_mappings,
    run_clear_active_session_mappings
);
backend_test!(
    test_sqlite_channel_binding,
    test_postgres_channel_binding,
    run_channel_binding
);
backend_test!(
    test_sqlite_mcp_disabled,
    test_postgres_mcp_disabled,
    run_mcp_disabled
);
backend_test!(
    test_version_starts_at_zero,
    test_postgres_version_starts_at_zero,
    run_version_starts_at_zero
);
backend_test!(
    test_version_increments_on_mutation,
    test_postgres_version_increments_on_mutation,
    run_version_increments_on_mutation
);
backend_test!(
    test_version_increments_on_upsert_update,
    test_postgres_version_increments_on_upsert_update,
    run_version_increments_on_upsert_update
);
backend_test!(
    test_version_in_list,
    test_postgres_version_in_list,
    run_version_in_list
);
backend_test!(test_sqlite_agent_id, test_postgres_agent_id, run_agent_id);
backend_test!(test_sqlite_mode_id, test_postgres_mode_id, run_mode_id);
backend_test!(
    test_sqlite_list_by_agent_id,
    test_postgres_list_by_agent_id,
    run_list_by_agent_id
);
backend_test!(
    test_sqlite_delete_by_agent_id,
    test_postgres_delete_by_agent_id,
    run_delete_by_agent_id
);
