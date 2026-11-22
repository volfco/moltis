/// Storage abstraction for memory files, chunks, embedding cache, and hybrid search.
use async_trait::async_trait;

use crate::{
    embeddings::EmbeddingProvider,
    error::Result,
    pgvector::PgvectorStatus,
    schema::{ChunkRow, FileRow},
    search::SearchResult,
    sync_lock::SyncLockGuard,
};

/// Strategy for merging vector and keyword search results.
///
/// The per-source weights (`vector_weight` / `keyword_weight`) are passed
/// separately to [`MemoryStore::hybrid_search`] and apply to every variant,
/// so they are intentionally not duplicated here.
#[derive(Debug, Clone, Copy)]
pub enum MergeStrategy {
    /// Reciprocal Rank Fusion with rank-constant `k` (default k=60).
    Rrf { k: u32 },
    /// Weighted linear blend (uses the weights passed to `hybrid_search`).
    Weighted,
}

impl Default for MergeStrategy {
    fn default() -> Self {
        Self::Rrf { k: 60 }
    }
}

/// Identity of the embedding provider that produced the stored vectors, as
/// persisted in the engine-agnostic `memory_meta` table.
///
/// Vectors from different providers or dimensions are not comparable, so a
/// sync pass that sees a different identity re-embeds every chunk text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddingMeta {
    pub provider_key: String,
    pub model: String,
    pub dimensions: usize,
}

impl EmbeddingMeta {
    /// The identity of `provider`, as recorded after an embed.
    pub fn from_provider(provider: &dyn EmbeddingProvider) -> Self {
        Self {
            provider_key: provider.provider_key().to_string(),
            model: provider.model_name().to_string(),
            dimensions: provider.dimensions(),
        }
    }
}

/// A single embedding cache entry for batch inserts.
pub struct CacheEntry<'a> {
    pub provider: &'a str,
    pub model: &'a str,
    pub provider_key: &'a str,
    pub hash: &'a str,
    pub embedding: &'a [f32],
}

#[async_trait]
pub trait MemoryStore: Send + Sync {
    // ---- files ----
    async fn upsert_file(&self, file: &FileRow) -> Result<()>;
    async fn get_file(&self, path: &str) -> Result<Option<FileRow>>;
    async fn delete_file(&self, path: &str) -> Result<()>;
    async fn list_files(&self) -> Result<Vec<FileRow>>;

    // ---- chunks ----
    async fn upsert_chunks(&self, chunks: &[ChunkRow]) -> Result<()>;
    async fn get_chunks_for_file(&self, path: &str) -> Result<Vec<ChunkRow>>;
    async fn delete_chunks_for_file(&self, path: &str) -> Result<()>;
    async fn get_chunk_by_id(&self, id: &str) -> Result<Option<ChunkRow>>;

    // ---- embedding cache ----
    async fn get_cached_embedding(
        &self,
        provider: &str,
        model: &str,
        hash: &str,
    ) -> Result<Option<Vec<f32>>>;

    async fn put_cached_embedding(
        &self,
        provider: &str,
        model: &str,
        provider_key: &str,
        hash: &str,
        embedding: &[f32],
    ) -> Result<()>;

    /// Batch-insert multiple embedding cache entries in a single transaction.
    async fn put_cached_embeddings_batch(&self, entries: &[CacheEntry<'_>]) -> Result<()>;

    /// Count the number of rows in the embedding cache.
    async fn count_cached_embeddings(&self) -> Result<usize>;

    /// Evict the oldest cache rows, keeping at most `keep` entries.
    async fn evict_embedding_cache(&self, keep: usize) -> Result<usize>;

    // ---- search ----
    /// Perform hybrid search: vector similarity + keyword/FTS, merged by the
    /// chosen strategy. `vector_weight` and `keyword_weight` apply to every
    /// [`MergeStrategy`] variant. Pass an empty embedding slice for keyword-only
    /// search (only the FTS sub-query contributes); pass an empty query string
    /// for vector-only search.
    async fn hybrid_search(
        &self,
        embedding: &[f32],
        query: &str,
        vector_weight: f32,
        keyword_weight: f32,
        merge_strategy: MergeStrategy,
        limit: usize,
    ) -> Result<Vec<SearchResult>>;

    /// Backend type identifier: "sqlite" for the built-in SQLite store,
    /// "zvec" for the zvec HNSW store.
    fn store_type(&self) -> &'static str {
        "sqlite"
    }

    /// HNSW index build percentage (zvec only, returns None for SQLite).
    fn hnsw_percent(&self) -> Option<f64> {
        None
    }

    /// Approximate on-disk footprint of the store's data files in bytes.
    ///
    /// Backends that own their data files (zvec) override this so the memory
    /// status reports real disk usage. The built-in SQLite store reports 0
    /// here and lets the manager fall back to measuring its `db_path` file.
    /// Returns 0 for in-memory stores or when the files are unavailable.
    fn disk_size_bytes(&self) -> u64 {
        0
    }

    /// Try to acquire the cross-instance sync lock for a full sync pass.
    ///
    /// `None` means another instance currently holds the lock and the caller
    /// must skip the sync. Stores without a shared database default to an
    /// uncontended guard.
    async fn try_acquire_sync_lock(&self) -> Result<Option<SyncLockGuard>> {
        Ok(Some(SyncLockGuard::Uncontended))
    }

    /// pgvector extension availability for PostgreSQL stores.
    ///
    /// `None` where the probe does not apply (SQLite and non-SQL stores).
    async fn pgvector_status(&self) -> Result<Option<PgvectorStatus>> {
        Ok(None)
    }

    // ---- embedding identity ----

    /// The embedding identity recorded in `memory_meta`, if any.
    ///
    /// `None` means no embed has been recorded yet (fresh index) or the
    /// backend does not persist metadata.
    async fn read_embedding_meta(&self) -> Result<Option<EmbeddingMeta>> {
        Ok(None)
    }

    /// Record the embedding identity in `memory_meta`.
    async fn write_embedding_meta(&self, _meta: &EmbeddingMeta) -> Result<()> {
        Ok(())
    }

    /// Prepare the embedding storage for vectors of a different dimension.
    ///
    /// PostgreSQL drops the vector index and untypes `chunks.embedding` so a
    /// re-embed at the new dimension can land; a typed `vector(N)` column
    /// rejects the new values. Idempotent, and a no-op on backends without
    /// column typing.
    async fn relax_embedding_schema(&self) -> Result<()> {
        Ok(())
    }

    /// Make the embedding storage match `dimensions`: type the column as
    /// `vector(N)` and ensure its index (PostgreSQL only). Idempotent, so a
    /// re-index interrupted between the metadata write and this call resumes
    /// here on the next sync.
    async fn ensure_embedding_schema(&self, _dimensions: usize) -> Result<()> {
        Ok(())
    }
}
