//! Tests for the embedding-identity tracking and auto re-index path:
//! `memory_meta` bookkeeping, chunk re-embedding, and — on PostgreSQL — the
//! lazily established `vector(N)` column and HNSW index.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use {
    async_trait::async_trait,
    moltis_db::Db,
    std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    tempfile::TempDir,
};

use crate::{
    config::MemoryConfig,
    embeddings::EmbeddingProvider,
    manager::{MemoryManager, ReindexReason},
    schema::run_migrations,
    store::MemoryStore,
    store_db::DbMemoryStore,
};

/// Topics the deterministic test embedder keys its dimensions on.
const KEYWORDS: [&str; 8] = [
    "rust", "python", "database", "memory", "search", "network", "cooking", "music",
];

/// Embedding call counters, held separately so they outlive the provider.
#[derive(Clone, Default)]
struct Counters {
    /// Calls into `embed`/`embed_batch` (one per batch).
    calls: Arc<AtomicUsize>,
    /// Texts embedded.
    texts: Arc<AtomicUsize>,
}

impl Counters {
    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    fn texts(&self) -> usize {
        self.texts.load(Ordering::SeqCst)
    }

    fn reset(&self) {
        self.calls.store(0, Ordering::SeqCst);
        self.texts.store(0, Ordering::SeqCst);
    }
}

/// Deterministic provider: a keyword bag padded to `dimensions`, rotated by the
/// provider key so a provider switch yields different vectors at equal size.
struct TestEmbedder {
    dimensions: usize,
    provider_key: &'static str,
    model: &'static str,
    counters: Counters,
}

impl TestEmbedder {
    fn new(dimensions: usize, provider_key: &'static str, model: &'static str) -> Self {
        Self {
            dimensions,
            provider_key,
            model,
            counters: Counters::default(),
        }
    }

    fn counters(&self) -> Counters {
        self.counters.clone()
    }

    fn vector(&self, text: &str) -> Vec<f32> {
        let lower = text.to_lowercase();
        let rotation = usize::from(!self.provider_key.ends_with('a'));
        (0..self.dimensions)
            .map(|i| {
                let keyword = KEYWORDS[(i + rotation) % KEYWORDS.len()];
                f32::from(u8::from(lower.contains(keyword)))
            })
            .collect()
    }
}

#[async_trait]
impl EmbeddingProvider for TestEmbedder {
    async fn embed(&self, text: &str) -> crate::Result<Vec<f32>> {
        self.counters.calls.fetch_add(1, Ordering::SeqCst);
        self.counters.texts.fetch_add(1, Ordering::SeqCst);
        Ok(self.vector(text))
    }

    async fn embed_batch(&self, texts: &[String]) -> crate::Result<Vec<Vec<f32>>> {
        self.counters.calls.fetch_add(1, Ordering::SeqCst);
        self.counters.texts.fetch_add(texts.len(), Ordering::SeqCst);
        Ok(texts.iter().map(|text| self.vector(text)).collect())
    }

    fn model_name(&self) -> &str {
        self.model
    }

    fn dimensions(&self) -> usize {
        self.dimensions
    }

    fn provider_key(&self) -> &str {
        self.provider_key
    }
}

fn config_for(dir: &TempDir) -> MemoryConfig {
    let memory_dir = dir.path().join("memory");
    std::fs::create_dir_all(&memory_dir).unwrap();
    MemoryConfig {
        db_path: ":memory:".into(),
        memory_dirs: vec![memory_dir],
        chunk_size: 50,
        chunk_overlap: 10,
        ..Default::default()
    }
}

/// Write one memory file into `dir` and return its absolute path.
fn write_memory_file(dir: &TempDir, name: &str, content: &str) -> String {
    let path = dir.path().join("memory").join(name);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, content).unwrap();
    path.to_string_lossy().into_owned()
}

async fn sqlite_db() -> Db {
    let pool = sqlx::SqlitePool::connect(":memory:").await.unwrap();
    let db = Db::from(pool);
    run_migrations(&db).await.unwrap();
    db
}

fn manager(
    dir: &TempDir,
    db: &Db,
    embedder: TestEmbedder,
) -> (MemoryManager, Counters, Box<dyn MemoryStore>) {
    let counters = embedder.counters();
    let manager = MemoryManager::new(
        config_for(dir),
        Box::new(DbMemoryStore::new(db.clone())),
        Box::new(embedder),
    );
    (manager, counters, Box::new(DbMemoryStore::new(db.clone())))
}

async fn recorded_meta(store: &dyn MemoryStore) -> crate::store::EmbeddingMeta {
    store
        .read_embedding_meta()
        .await
        .unwrap()
        .expect("embedding identity must be recorded after a sync")
}

/// First sync records the identity; a dimension change re-embeds every chunk,
/// rewrites the metadata and leaves search working.
#[tokio::test]
async fn sqlite_dimension_change_reembeds_and_records_identity() {
    let db = sqlite_db().await;
    let dir = TempDir::new().unwrap();
    let path = write_memory_file(&dir, "note.md", "rust memory search with database");

    let (first, first_counters, store) =
        manager(&dir, &db, TestEmbedder::new(8, "provider-a", "m"));
    let report = first.sync().await.unwrap();
    assert_eq!(report.files_updated, 1);
    // The identity is established on the first embed pass, not guessed from
    // configuration.
    let reindex = report.reindex.expect("first pass records the identity");
    assert_eq!(reindex.reason, Some(ReindexReason::NoRecordedIdentity));
    assert_eq!(reindex.chunks_reembedded, 0, "fresh index has no chunks");
    assert_eq!(first_counters.texts(), 1);

    let meta = recorded_meta(store.as_ref()).await;
    assert_eq!(meta.provider_key, "provider-a");
    assert_eq!(meta.model, "m");
    assert_eq!(meta.dimensions, 8);

    let (second, second_counters, _) = manager(&dir, &db, TestEmbedder::new(16, "provider-a", "m"));
    let report = second.sync().await.unwrap();
    let reindex = report.reindex.expect("dimension change must re-index");
    assert_eq!(reindex.reason, Some(ReindexReason::DimensionsChanged));
    assert_eq!(reindex.dimensions_before, Some(8));
    assert_eq!(reindex.dimensions_after, 16);
    assert_eq!(reindex.chunks_reembedded, 1);
    assert_eq!(second_counters.texts(), 1, "the chunk text is re-embedded");

    assert_eq!(recorded_meta(store.as_ref()).await.dimensions, 16);

    let chunks = store.get_chunks_for_file(&path).await.unwrap();
    assert_eq!(chunks[0].embedding.as_ref().unwrap().len(), 16 * 4);
    assert_eq!(chunks[0].model, "m");

    // Same dimension count as the query embed, so vector search still works.
    let results = second.search("rust", 5).await.unwrap();
    assert!(
        results.iter().any(|result| result.text.contains("rust")),
        "search must work after the dimension change: {results:?}"
    );
}

/// Same dimensions, different provider key/model: vectors are rebuilt and the
/// recorded identity is rewritten.
#[tokio::test]
async fn sqlite_provider_change_same_dimensions_reembeds() {
    let db = sqlite_db().await;
    let dir = TempDir::new().unwrap();
    let path = write_memory_file(&dir, "note.md", "rust memory search");

    let (first, _, store) = manager(&dir, &db, TestEmbedder::new(8, "provider-a", "m"));
    first.sync().await.unwrap();
    let before = store.get_chunks_for_file(&path).await.unwrap()[0]
        .embedding
        .clone();

    let (second, counters, _) = manager(&dir, &db, TestEmbedder::new(8, "provider-b", "m"));
    let report = second.sync().await.unwrap();
    let reindex = report.reindex.expect("provider change must re-index");
    assert_eq!(reindex.reason, Some(ReindexReason::ProviderChanged));
    assert_eq!(reindex.chunks_reembedded, 1);
    assert_eq!(counters.texts(), 1);

    let after = store.get_chunks_for_file(&path).await.unwrap()[0]
        .embedding
        .clone();
    assert_ne!(before, after, "the vectors must be regenerated");
    assert_eq!(after.unwrap().len(), 8 * 4);
    assert_eq!(
        recorded_meta(store.as_ref()).await.provider_key,
        "provider-b"
    );
}

/// A matching identity costs no embed calls at all.
#[tokio::test]
async fn sqlite_unchanged_identity_does_not_reembed() {
    let db = sqlite_db().await;
    let dir = TempDir::new().unwrap();
    write_memory_file(&dir, "note.md", "rust memory search");

    let (manager, counters, _) = manager(&dir, &db, TestEmbedder::new(8, "provider-a", "m"));
    manager.sync().await.unwrap();

    counters.reset();
    let report = manager.sync().await.unwrap();
    assert!(
        report.reindex.is_none(),
        "matching identity must not re-index"
    );
    assert_eq!(counters.calls(), 0, "no embed calls on an unchanged index");
}

/// `reindex(true)` rebuilds even when the identity is unchanged; `false` is a
/// no-op.
#[tokio::test]
async fn sqlite_force_reindexes_unchanged_identity() {
    let db = sqlite_db().await;
    let dir = TempDir::new().unwrap();
    write_memory_file(&dir, "note.md", "rust memory search");

    let (manager, counters, _) = manager(&dir, &db, TestEmbedder::new(8, "provider-a", "m"));
    manager.sync().await.unwrap();

    counters.reset();
    assert!(manager.reindex(false).await.unwrap().reason.is_none());
    assert_eq!(counters.calls(), 0);

    let report = manager.reindex(true).await.unwrap();
    assert_eq!(report.reason, Some(ReindexReason::Forced));
    assert_eq!(report.chunks_reembedded, 1);
    assert_eq!(counters.texts(), 1);
}

// ── PostgreSQL: typed column + HNSW index ─────────────────────────────────

fn pg_enabled() -> bool {
    if std::env::var("TEST_POSTGRES_URL").is_err() {
        eprintln!("skipping: TEST_POSTGRES_URL is not set");
        return false;
    }
    true
}

async fn embedding_type(db: &Db) -> String {
    crate::store_db::embedding_column_type(db).await.unwrap()
}

async fn hnsw_index_present(db: &Db) -> bool {
    moltis_db::scalar!(
        db,
        bool,
        "SELECT to_regclass('chunks_embedding_hnsw') IS NOT NULL"
    )
    .unwrap()
}

/// The first embed establishes the dimension, which types the column and
/// creates the HNSW index; a dimension change retypes both, and search still
/// works against the index.
#[tokio::test]
async fn pg_dimension_change_retypes_column_and_index() {
    if !pg_enabled() {
        return;
    }
    let _guard = crate::test_support::pg_lock().lock().await;
    let Some(db) = crate::test_support::pg_test_db().await else {
        return;
    };
    let dir = TempDir::new().unwrap();
    write_memory_file(&dir, "note.md", "rust memory search with database");

    let (first, ..) = manager(&dir, &db, TestEmbedder::new(8, "provider-a", "m"));
    first.sync().await.unwrap();
    assert_eq!(embedding_type(&db).await, "vector(8)");
    assert!(hnsw_index_present(&db).await, "HNSW index must be created");

    let (second, counters, _) = manager(&dir, &db, TestEmbedder::new(16, "provider-a", "m"));
    let report = second.sync().await.unwrap();
    let reindex = report.reindex.expect("dimension change must re-index");
    assert_eq!(reindex.reason, Some(ReindexReason::DimensionsChanged));
    assert_eq!(reindex.chunks_reembedded, 1);
    assert_eq!(counters.texts(), 1);

    assert_eq!(embedding_type(&db).await, "vector(16)");
    assert!(
        hnsw_index_present(&db).await,
        "HNSW index must be recreated"
    );

    let results = second.search("rust", 5).await.unwrap();
    assert!(
        results.iter().any(|result| result.text.contains("rust")),
        "search must work against the retyped column: {results:?}"
    );
}

/// Same dimensions across a provider switch: re-embeds, column stays typed.
#[tokio::test]
async fn pg_provider_change_keeps_typed_column() {
    if !pg_enabled() {
        return;
    }
    let _guard = crate::test_support::pg_lock().lock().await;
    let Some(db) = crate::test_support::pg_test_db().await else {
        return;
    };
    let dir = TempDir::new().unwrap();
    write_memory_file(&dir, "note.md", "rust memory search");

    let (first, _, store) = manager(&dir, &db, TestEmbedder::new(8, "provider-a", "m"));
    first.sync().await.unwrap();
    let before = store
        .get_chunks_for_file(&write_memory_path(&dir, "note.md"))
        .await
        .unwrap()[0]
        .embedding
        .clone();

    let (second, counters, _) = manager(&dir, &db, TestEmbedder::new(8, "provider-b", "m"));
    let report = second.sync().await.unwrap();
    assert_eq!(
        report
            .reindex
            .expect("provider change must re-index")
            .reason,
        Some(ReindexReason::ProviderChanged)
    );
    assert_eq!(counters.texts(), 1);
    assert_eq!(embedding_type(&db).await, "vector(8)");
    assert!(hnsw_index_present(&db).await);

    let after = store
        .get_chunks_for_file(&write_memory_path(&dir, "note.md"))
        .await
        .unwrap()[0]
        .embedding
        .clone();
    assert_ne!(
        before, after,
        "vectors must be regenerated for the new provider"
    );
}

/// A matching identity does no re-embedding and leaves the schema alone.
#[tokio::test]
async fn pg_unchanged_identity_does_no_work() {
    if !pg_enabled() {
        return;
    }
    let _guard = crate::test_support::pg_lock().lock().await;
    let Some(db) = crate::test_support::pg_test_db().await else {
        return;
    };
    let dir = TempDir::new().unwrap();
    write_memory_file(&dir, "note.md", "rust memory search");

    let (manager, counters, _) = manager(&dir, &db, TestEmbedder::new(8, "provider-a", "m"));
    manager.sync().await.unwrap();

    counters.reset();
    let report = manager.sync().await.unwrap();
    assert!(report.reindex.is_none());
    assert_eq!(counters.calls(), 0);
    assert_eq!(embedding_type(&db).await, "vector(8)");
    assert!(hnsw_index_present(&db).await);
}

/// A re-index interrupted after the column was relaxed (index dropped, column
/// untyped) resumes on the next sync: the identity still mismatches, so the
/// re-embed runs, then the type and index are re-applied. A matching identity
/// only needs the schema repair.
#[tokio::test]
async fn pg_interrupted_reindex_resumes_on_next_sync() {
    if !pg_enabled() {
        return;
    }
    let _guard = crate::test_support::pg_lock().lock().await;
    let Some(db) = crate::test_support::pg_test_db().await else {
        return;
    };
    let dir = TempDir::new().unwrap();
    write_memory_file(&dir, "note.md", "rust memory search");

    let (first, ..) = manager(&dir, &db, TestEmbedder::new(8, "provider-a", "m"));
    first.sync().await.unwrap();

    // Simulate a crash right after the dimension-change prelude: the index is
    // gone, the column untyped, and the metadata still records 8 dimensions.
    moltis_db::execute!(&db, "DROP INDEX IF EXISTS chunks_embedding_hnsw").unwrap();
    moltis_db::execute!(
        &db,
        "ALTER TABLE chunks ALTER COLUMN embedding TYPE vector USING embedding::vector"
    )
    .unwrap();
    assert_eq!(embedding_type(&db).await, "vector");
    assert!(!hnsw_index_present(&db).await);

    let (second, counters, _) = manager(&dir, &db, TestEmbedder::new(16, "provider-a", "m"));
    let report = second.sync().await.unwrap();
    assert_eq!(
        report.reindex.expect("resumed re-index").reason,
        Some(ReindexReason::DimensionsChanged)
    );
    assert_eq!(counters.texts(), 1);
    assert_eq!(embedding_type(&db).await, "vector(16)");
    assert!(hnsw_index_present(&db).await);

    // Second resume path: the metadata is already 16 but the schema was left
    // untyped by a crash between the metadata write and the schema change. The
    // matching identity must repair the schema without re-embedding.
    moltis_db::execute!(&db, "DROP INDEX IF EXISTS chunks_embedding_hnsw").unwrap();
    moltis_db::execute!(
        &db,
        "ALTER TABLE chunks ALTER COLUMN embedding TYPE vector USING embedding::vector"
    )
    .unwrap();
    counters.reset();
    let report = second.sync().await.unwrap();
    assert!(report.reindex.is_none(), "identity matches, no re-embed");
    assert_eq!(counters.calls(), 0);
    assert_eq!(embedding_type(&db).await, "vector(16)");
    assert!(hnsw_index_present(&db).await);

    // Idempotent: running the schema step again changes nothing.
    second.sync().await.unwrap();
    assert_eq!(embedding_type(&db).await, "vector(16)");
}

fn write_memory_path(dir: &TempDir, name: &str) -> String {
    dir.path()
        .join("memory")
        .join(name)
        .to_string_lossy()
        .into_owned()
}
