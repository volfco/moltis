//! `MemoryStore` implementation for both supported backends (SQLite and
//! PostgreSQL).
//!
//! One implementation serves both dialects: queries are written with `?`
//! placeholders and run through the `moltis_db` macros, which rewrite them for
//! PostgreSQL. Only the statements whose *shape* differs per dialect branch on
//! [`Db::dialect`]:
//!
//! | Concern | SQLite | PostgreSQL |
//! |---------|--------|------------|
//! | Embedding column | BLOB of little-endian f32 | `vector` (pgvector) |
//! | Embedding column type | fixed by the BLOB layout | `vector(N)` + HNSW once `memory_meta` knows the dimension |
//! | Vector search | load all rows, cosine in Rust | `ORDER BY embedding <=> $1` |
//! | Keyword search | FTS5 `MATCH` | `tsvector` + `websearch_to_tsquery` |
//! | Cache eviction | `rowid IN (…)` | primary-key tuple `IN (…)` |

use {
    async_trait::async_trait,
    moltis_db::{Db, Dialect, execute, fetch_all, fetch_one, fetch_optional, scalar},
};

use crate::{
    pgvector::PgvectorStatus,
    schema::{ChunkRow, FileRow},
    search::{self, SearchResult},
    store::{CacheEntry, EmbeddingMeta, MemoryStore, MergeStrategy},
    sync_lock::SyncLockGuard,
};

/// `MemoryStore` backed by SQLite or PostgreSQL.
pub struct DbMemoryStore {
    db: Db,
    /// Probe result cached from the first [`MemoryStore::pgvector_status`] call.
    pgvector: tokio::sync::OnceCell<Option<PgvectorStatus>>,
}

impl DbMemoryStore {
    pub fn new(db: Db) -> Self {
        Self {
            db,
            pgvector: tokio::sync::OnceCell::new(),
        }
    }

    /// Wrap an existing SQLite pool (tests and SQLite-only call sites).
    pub fn from_sqlite_pool(pool: sqlx::SqlitePool) -> Self {
        Self::new(Db::from(pool))
    }

    pub fn db(&self) -> &Db {
        &self.db
    }
}

// ── embedding encoding ────────────────────────────────────────────────────

/// Pack an embedding into the byte representation used by the `ChunkRow`
/// trait contract (little-endian f32, matching the SQLite BLOB layout).
fn pack_embedding(values: &[f32]) -> Vec<u8> {
    values.iter().flat_map(|f| f.to_le_bytes()).collect()
}

/// Unpack the little-endian f32 byte representation back into floats.
fn unpack_embedding(blob: &[u8]) -> Vec<f32> {
    blob.chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

/// Render an embedding as a pgvector text literal (`[0.1,0.2,…]`).
fn vector_literal(values: &[f32]) -> String {
    let mut out = String::with_capacity(values.len() * 12 + 2);
    out.push('[');
    for (i, v) in values.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        if v.is_finite() {
            out.push_str(&v.to_string());
        } else {
            // pgvector rejects NaN/Infinity in text input.
            out.push('0');
        }
    }
    out.push(']');
    out
}

/// Parse a pgvector text literal back into floats.
fn parse_vector_literal(text: &str) -> Vec<f32> {
    text.trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .split(',')
        .filter_map(|part| part.trim().parse::<f32>().ok())
        .collect()
}

// ── vector search ─────────────────────────────────────────────────────────

/// Cosine similarity between two vectors.
fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let mut dot = 0.0f32;
    let mut norm_a = 0.0f32;
    let mut norm_b = 0.0f32;
    for (x, y) in a.iter().zip(b.iter()) {
        dot += x * y;
        norm_a += x * x;
        norm_b += y * y;
    }
    let denom = norm_a.sqrt() * norm_b.sqrt();
    if denom == 0.0 {
        0.0
    } else {
        dot / denom
    }
}

/// Vector search: scores are cosine similarity in `-1.0..=1.0`.
async fn vector_search(
    db: &Db,
    query_embedding: &[f32],
    limit: usize,
) -> crate::error::Result<Vec<SearchResult>> {
    match db.dialect() {
        Dialect::Sqlite => vector_search_scan(db, query_embedding, limit).await,
        Dialect::Postgres => vector_search_pg(db, query_embedding, limit).await,
    }
}

/// SQLite: no vector index, so stream every embedded chunk and keep the top K.
async fn vector_search_scan(
    db: &Db,
    query_embedding: &[f32],
    limit: usize,
) -> crate::error::Result<Vec<SearchResult>> {
    use std::collections::BinaryHeap;

    let rows: Vec<(String, String, String, i64, i64, Vec<u8>)> = fetch_all!(
        db,
        (String, String, String, i64, i64, Vec<u8>),
        "SELECT id, path, source, start_line, end_line, embedding FROM chunks \
         WHERE embedding IS NOT NULL",
    )?;

    let mut heap: BinaryHeap<ScoredResult> = BinaryHeap::with_capacity(limit + 1);
    for (id, path, source, start_line, end_line, blob) in rows {
        let embedding = unpack_embedding(&blob);
        let score = cosine_similarity(query_embedding, &embedding);
        let result = SearchResult {
            chunk_id: id,
            path,
            source,
            start_line,
            end_line,
            score,
            text: String::new(),
        };
        if heap.len() < limit {
            heap.push(ScoredResult { score, result });
        } else if let Some(min) = heap.peek()
            && score > min.score
        {
            heap.pop();
            heap.push(ScoredResult { score, result });
        }
    }

    let mut results: Vec<SearchResult> = heap.into_iter().map(|s| s.result).collect();
    results.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    Ok(results)
}

/// PostgreSQL: push the similarity search into pgvector.
async fn vector_search_pg(
    db: &Db,
    query_embedding: &[f32],
    limit: usize,
) -> crate::error::Result<Vec<SearchResult>> {
    let literal = vector_literal(query_embedding);
    let rows: Vec<(String, String, String, i64, i64, f64)> = fetch_all!(
        db,
        (String, String, String, i64, i64, f64),
        "SELECT id, path, source, start_line, end_line, embedding <=> ?::vector AS distance
         FROM chunks
         WHERE embedding IS NOT NULL
         ORDER BY distance
         LIMIT ?",
        literal,
        limit as i64,
    )?;

    Ok(rows
        .into_iter()
        .map(
            |(chunk_id, path, source, start_line, end_line, distance)| SearchResult {
                chunk_id,
                path,
                source,
                start_line,
                end_line,
                // `<=>` is cosine distance (1 - cosine similarity).
                score: (1.0 - distance) as f32,
                text: String::new(),
            },
        )
        .collect())
}

/// A scored result for the top-K min-heap. Ordered by score ascending so the
/// minimum score is at the top, allowing us to evict the worst candidate.
struct ScoredResult {
    score: f32,
    result: SearchResult,
}

impl PartialEq for ScoredResult {
    fn eq(&self, other: &Self) -> bool {
        self.score == other.score
    }
}

impl Eq for ScoredResult {}

impl PartialOrd for ScoredResult {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ScoredResult {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // Reverse order: smallest score at top of BinaryHeap (min-heap).
        other
            .score
            .partial_cmp(&self.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    }
}

// ── keyword search ────────────────────────────────────────────────────────

/// Sanitize a user query for FTS5 `MATCH`.
///
/// FTS5 has its own query grammar where characters like `.` (column filter),
/// `*` (prefix), `"` (phrase), `+`, `-`, `(`, `)`, `NEAR`, `OR`, `AND`, `NOT`
/// have special meaning.  Passing unsanitized user input (e.g. coordinates
/// like "37.759") causes `fts5: syntax error near "."`.
///
/// Strategy: split on whitespace, strip all non-alphanumeric characters from
/// each token, wrap every token in double quotes (so FTS5 treats it as a
/// literal), then join with spaces (implicit AND).
fn sanitize_fts5_query(query: &str) -> String {
    query
        .split_whitespace()
        .filter_map(|token| {
            let cleaned: String = token
                .chars()
                .filter(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            if cleaned.is_empty() {
                None
            } else {
                Some(format!("\"{cleaned}\""))
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Keyword search: scores are normalized to `0.0..=1.0`, higher is better.
async fn keyword_search(
    db: &Db,
    query: &str,
    limit: usize,
) -> crate::error::Result<Vec<SearchResult>> {
    match db.dialect() {
        Dialect::Sqlite => keyword_search_sqlite(db, query, limit).await,
        Dialect::Postgres => keyword_search_pg(db, query, limit).await,
    }
}

async fn keyword_search_sqlite(
    db: &Db,
    query: &str,
    limit: usize,
) -> crate::error::Result<Vec<SearchResult>> {
    let sanitized = sanitize_fts5_query(query);
    if sanitized.is_empty() {
        return Ok(Vec::new());
    }

    let rows: Vec<(String, String, String, i64, i64, f64)> = fetch_all!(
        db,
        (String, String, String, i64, i64, f64),
        "SELECT c.id, c.path, c.source, c.start_line, c.end_line, rank
         FROM chunks_fts f
         JOIN chunks c ON c.rowid = f.rowid
         WHERE chunks_fts MATCH ?
         ORDER BY rank
         LIMIT ?",
        sanitized,
        limit as i64,
    )?;

    // FTS5 rank is negative (lower = better). Normalize to 0..1 range.
    let min_rank = rows.iter().map(|r| r.5).fold(f64::INFINITY, f64::min);
    let max_rank = rows.iter().map(|r| r.5).fold(f64::NEG_INFINITY, f64::max);
    let range = max_rank - min_rank;

    Ok(rows
        .into_iter()
        .map(|(id, path, source, start_line, end_line, rank)| {
            let score = if range.abs() < 1e-9 {
                1.0
            } else {
                (1.0 - ((rank - min_rank) / range)) as f32
            };
            SearchResult {
                chunk_id: id,
                path,
                source,
                start_line,
                end_line,
                score,
                text: String::new(),
            }
        })
        .collect())
}

async fn keyword_search_pg(
    db: &Db,
    query: &str,
    limit: usize,
) -> crate::error::Result<Vec<SearchResult>> {
    if query.trim().is_empty() {
        return Ok(Vec::new());
    }

    // `websearch_to_tsquery` accepts free-form user text (quoted phrases, OR,
    // leading -) and never raises a syntax error, so no sanitizing is needed.
    // shortcut: English-only stemming, matching the 'english' regconfig in the
    // generated tsv column; make the regconfig configurable if memory content
    // is not predominantly English.
    let rows: Vec<(String, String, String, i64, i64, f64)> = fetch_all!(
        db,
        (String, String, String, i64, i64, f64),
        "SELECT c.id, c.path, c.source, c.start_line, c.end_line,
                ts_rank(c.tsv, q)::float8 AS rank
         FROM chunks c, websearch_to_tsquery('english', ?) AS q
         WHERE c.tsv @@ q
         ORDER BY rank DESC
         LIMIT ?",
        query,
        limit as i64,
    )?;

    // ts_rank is positive (higher = better); normalize the batch to 0..1.
    let min_rank = rows.iter().map(|r| r.5).fold(f64::INFINITY, f64::min);
    let max_rank = rows.iter().map(|r| r.5).fold(f64::NEG_INFINITY, f64::max);
    let range = max_rank - min_rank;

    Ok(rows
        .into_iter()
        .map(|(id, path, source, start_line, end_line, rank)| {
            let score = if range.abs() < 1e-9 {
                1.0
            } else {
                ((rank - min_rank) / range) as f32
            };
            SearchResult {
                chunk_id: id,
                path,
                source,
                start_line,
                end_line,
                score,
                text: String::new(),
            }
        })
        .collect())
}

// ── embedding identity (memory_meta) ──────────────────────────────────────

/// `memory_meta` key holding the provider key of the stored vectors.
const META_PROVIDER_KEY: &str = "embedding_provider_key";
/// `memory_meta` key holding the model name of the stored vectors.
const META_MODEL: &str = "embedding_model";
/// `memory_meta` key holding the dimension count of the stored vectors.
const META_DIMENSIONS: &str = "embedding_dims";

/// HNSW index over `chunks.embedding`, created once the dimension is known.
const CHUNKS_EMBEDDING_INDEX: &str = "chunks_embedding_hnsw";

/// Render `chunks.embedding`'s SQL type, e.g. `vector(1536)` or `vector`.
pub(crate) async fn embedding_column_type(db: &Db) -> crate::error::Result<String> {
    Ok(fetch_one!(
        db,
        (String,),
        "SELECT format_type(a.atttypid, a.atttypmod) FROM pg_attribute a \
         WHERE a.attrelid = 'chunks'::regclass AND a.attname = 'embedding' \
         AND NOT a.attisdropped"
    )?
    .0)
}

// ── row encoding ──────────────────────────────────────────────────────────

#[allow(clippy::type_complexity)]
type ChunkTuple = (
    String,
    String,
    String,
    i64,
    i64,
    String,
    String,
    String,
    Option<Vec<u8>>,
    String,
);

fn chunk_row_from_tuple(t: ChunkTuple) -> ChunkRow {
    let (id, path, source, start_line, end_line, hash, model, text, embedding, updated_at) = t;
    ChunkRow {
        id,
        path,
        source,
        start_line,
        end_line,
        hash,
        model,
        text,
        embedding,
        updated_at,
    }
}

/// Statement fragments that differ per dialect for embedding columns.
struct EmbeddingSql {
    /// Column expression for reads (bytes for SQLite, text for pgvector).
    read_expr: &'static str,
    /// Placeholder for writes, including the pgvector cast.
    write_placeholder: &'static str,
}

impl EmbeddingSql {
    fn for_dialect(dialect: Dialect) -> Self {
        match dialect {
            Dialect::Sqlite => Self {
                read_expr: "embedding",
                write_placeholder: "?",
            },
            Dialect::Postgres => Self {
                read_expr: "embedding::text",
                write_placeholder: "?::vector",
            },
        }
    }
}

#[async_trait]
impl MemoryStore for DbMemoryStore {
    async fn upsert_file(&self, file: &FileRow) -> crate::error::Result<()> {
        execute!(
            &self.db,
            "INSERT INTO files (path, source, hash, mtime, size) VALUES (?, ?, ?, ?, ?)
             ON CONFLICT(path) DO UPDATE SET source=excluded.source, hash=excluded.hash,
             mtime=excluded.mtime, size=excluded.size",
            &file.path,
            &file.source,
            &file.hash,
            file.mtime,
            file.size,
        )?;
        Ok(())
    }

    async fn get_file(&self, path: &str) -> crate::error::Result<Option<FileRow>> {
        let row: Option<(String, String, String, i64, i64)> = fetch_optional!(
            &self.db,
            (String, String, String, i64, i64),
            "SELECT path, source, hash, mtime, size FROM files WHERE path = ?",
            path,
        )?;
        Ok(row.map(|(path, source, hash, mtime, size)| FileRow {
            path,
            source,
            hash,
            mtime,
            size,
        }))
    }

    async fn delete_file(&self, path: &str) -> crate::error::Result<()> {
        execute!(&self.db, "DELETE FROM files WHERE path = ?", path)?;
        Ok(())
    }

    async fn list_files(&self) -> crate::error::Result<Vec<FileRow>> {
        let rows: Vec<(String, String, String, i64, i64)> = fetch_all!(
            &self.db,
            (String, String, String, i64, i64),
            "SELECT path, source, hash, mtime, size FROM files",
        )?;
        Ok(rows
            .into_iter()
            .map(|(path, source, hash, mtime, size)| FileRow {
                path,
                source,
                hash,
                mtime,
                size,
            })
            .collect())
    }

    async fn upsert_chunks(&self, chunks: &[ChunkRow]) -> crate::error::Result<()> {
        let embed = EmbeddingSql::for_dialect(self.db.dialect());
        let sql = format!(
            "INSERT INTO chunks (id, path, source, start_line, end_line, hash, model, text, \
             embedding, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, {embed_ph}, ?)
             ON CONFLICT(id) DO UPDATE SET
               path=excluded.path, source=excluded.source, start_line=excluded.start_line,
               end_line=excluded.end_line, hash=excluded.hash, model=excluded.model,
               text=excluded.text, embedding=excluded.embedding, updated_at=excluded.updated_at",
            embed_ph = embed.write_placeholder,
        );

        let mut tx = self.db.begin().await?;
        for chunk in chunks {
            match self.db.dialect() {
                Dialect::Sqlite => {
                    moltis_db::tx_execute!(
                        &mut tx,
                        &sql,
                        &chunk.id,
                        &chunk.path,
                        &chunk.source,
                        chunk.start_line,
                        chunk.end_line,
                        &chunk.hash,
                        &chunk.model,
                        &chunk.text,
                        chunk.embedding.as_deref(),
                        &chunk.updated_at,
                    )?;
                },
                Dialect::Postgres => {
                    let literal = chunk
                        .embedding
                        .as_deref()
                        .map(|b| vector_literal(&unpack_embedding(b)));
                    moltis_db::tx_execute!(
                        &mut tx,
                        &sql,
                        &chunk.id,
                        &chunk.path,
                        &chunk.source,
                        chunk.start_line,
                        chunk.end_line,
                        &chunk.hash,
                        &chunk.model,
                        &chunk.text,
                        &literal,
                        &chunk.updated_at,
                    )?;
                },
            }
        }
        tx.commit().await?;
        Ok(())
    }

    async fn get_chunks_for_file(&self, path: &str) -> crate::error::Result<Vec<ChunkRow>> {
        let embed = EmbeddingSql::for_dialect(self.db.dialect());
        let sql = format!(
            "SELECT id, path, source, start_line, end_line, hash, model, text, {}, \
             updated_at FROM chunks WHERE path = ? ORDER BY start_line",
            embed.read_expr
        );
        match self.db.dialect() {
            Dialect::Sqlite => {
                let rows: Vec<ChunkTuple> = fetch_all!(&self.db, ChunkTuple, &sql, path)?;
                Ok(rows.into_iter().map(chunk_row_from_tuple).collect())
            },
            Dialect::Postgres => {
                let rows: Vec<(
                    String,
                    String,
                    String,
                    i64,
                    i64,
                    String,
                    String,
                    String,
                    Option<String>,
                    String,
                )> = fetch_all!(
                    &self.db,
                    (
                        String,
                        String,
                        String,
                        i64,
                        i64,
                        String,
                        String,
                        String,
                        Option<String>,
                        String
                    ),
                    &sql,
                    path
                )?;
                Ok(rows
                    .into_iter()
                    .map(
                        |(
                            id,
                            path,
                            source,
                            start_line,
                            end_line,
                            hash,
                            model,
                            text,
                            embedding,
                            updated_at,
                        )| {
                            ChunkRow {
                                id,
                                path,
                                source,
                                start_line,
                                end_line,
                                hash,
                                model,
                                text,
                                embedding: embedding
                                    .as_deref()
                                    .map(|lit| pack_embedding(&parse_vector_literal(lit))),
                                updated_at,
                            }
                        },
                    )
                    .collect())
            },
        }
    }

    async fn delete_chunks_for_file(&self, path: &str) -> crate::error::Result<()> {
        execute!(&self.db, "DELETE FROM chunks WHERE path = ?", path)?;
        Ok(())
    }

    async fn get_chunk_by_id(&self, id: &str) -> crate::error::Result<Option<ChunkRow>> {
        let embed = EmbeddingSql::for_dialect(self.db.dialect());
        let sql = format!(
            "SELECT id, path, source, start_line, end_line, hash, model, text, {}, \
             updated_at FROM chunks WHERE id = ?",
            embed.read_expr
        );
        match self.db.dialect() {
            Dialect::Sqlite => {
                let row: Option<ChunkTuple> = fetch_optional!(&self.db, ChunkTuple, &sql, id)?;
                Ok(row.map(chunk_row_from_tuple))
            },
            Dialect::Postgres => {
                let row: Option<(
                    String,
                    String,
                    String,
                    i64,
                    i64,
                    String,
                    String,
                    String,
                    Option<String>,
                    String,
                )> = fetch_optional!(
                    &self.db,
                    (
                        String,
                        String,
                        String,
                        i64,
                        i64,
                        String,
                        String,
                        String,
                        Option<String>,
                        String
                    ),
                    &sql,
                    id
                )?;
                Ok(row.map(
                    |(
                        id,
                        path,
                        source,
                        start_line,
                        end_line,
                        hash,
                        model,
                        text,
                        embedding,
                        updated_at,
                    )| ChunkRow {
                        id,
                        path,
                        source,
                        start_line,
                        end_line,
                        hash,
                        model,
                        text,
                        embedding: embedding
                            .as_deref()
                            .map(|lit| pack_embedding(&parse_vector_literal(lit))),
                        updated_at,
                    },
                ))
            },
        }
    }

    async fn get_cached_embedding(
        &self,
        provider: &str,
        model: &str,
        hash: &str,
    ) -> crate::error::Result<Option<Vec<f32>>> {
        let embed = EmbeddingSql::for_dialect(self.db.dialect());
        let sql = format!(
            "SELECT {} FROM embedding_cache WHERE provider = ? AND model = ? AND hash = ?",
            embed.read_expr
        );
        match self.db.dialect() {
            Dialect::Sqlite => {
                let row: Option<(Vec<u8>,)> =
                    fetch_optional!(&self.db, (Vec<u8>,), &sql, provider, model, hash)?;
                Ok(row.map(|(blob,)| unpack_embedding(&blob)))
            },
            Dialect::Postgres => {
                let row: Option<(String,)> =
                    fetch_optional!(&self.db, (String,), &sql, provider, model, hash)?;
                Ok(row.map(|(lit,)| parse_vector_literal(&lit)))
            },
        }
    }

    async fn put_cached_embedding(
        &self,
        provider: &str,
        model: &str,
        provider_key: &str,
        hash: &str,
        embedding: &[f32],
    ) -> crate::error::Result<()> {
        let embed = EmbeddingSql::for_dialect(self.db.dialect());
        let now = self.db.dialect().now_expr();
        let sql = format!(
            "INSERT INTO embedding_cache (provider, model, provider_key, hash, embedding, dims, \
             updated_at) VALUES (?, ?, ?, ?, {embed_ph}, ?, {now})
             ON CONFLICT(provider, model, provider_key, hash) DO UPDATE SET
               embedding=excluded.embedding, dims=excluded.dims, updated_at=excluded.updated_at",
            embed_ph = embed.write_placeholder,
        );
        let dims = embedding.len() as i64;
        match self.db.dialect() {
            Dialect::Sqlite => {
                let blob = pack_embedding(embedding);
                execute!(
                    &self.db,
                    &sql,
                    provider,
                    model,
                    provider_key,
                    hash,
                    &blob,
                    dims
                )?;
            },
            Dialect::Postgres => {
                let literal = vector_literal(embedding);
                execute!(
                    &self.db,
                    &sql,
                    provider,
                    model,
                    provider_key,
                    hash,
                    &literal,
                    dims
                )?;
            },
        }
        Ok(())
    }

    async fn put_cached_embeddings_batch(
        &self,
        entries: &[CacheEntry<'_>],
    ) -> crate::error::Result<()> {
        let embed = EmbeddingSql::for_dialect(self.db.dialect());
        let now = self.db.dialect().now_expr();
        let sql = format!(
            "INSERT INTO embedding_cache (provider, model, provider_key, hash, embedding, dims, \
             updated_at) VALUES (?, ?, ?, ?, {embed_ph}, ?, {now})
             ON CONFLICT(provider, model, provider_key, hash) DO UPDATE SET
               embedding=excluded.embedding, dims=excluded.dims, updated_at=excluded.updated_at",
            embed_ph = embed.write_placeholder,
        );

        let mut tx = self.db.begin().await?;
        for entry in entries {
            let dims = entry.embedding.len() as i64;
            match self.db.dialect() {
                Dialect::Sqlite => {
                    let blob = pack_embedding(entry.embedding);
                    moltis_db::tx_execute!(
                        &mut tx,
                        &sql,
                        &entry.provider,
                        &entry.model,
                        &entry.provider_key,
                        &entry.hash,
                        blob,
                        dims,
                    )?;
                },
                Dialect::Postgres => {
                    let literal = vector_literal(entry.embedding);
                    moltis_db::tx_execute!(
                        &mut tx,
                        &sql,
                        &entry.provider,
                        &entry.model,
                        &entry.provider_key,
                        &entry.hash,
                        literal,
                        dims,
                    )?;
                },
            }
        }
        tx.commit().await?;
        Ok(())
    }

    async fn count_cached_embeddings(&self) -> crate::error::Result<usize> {
        let count: i64 = scalar!(&self.db, i64, "SELECT COUNT(*) FROM embedding_cache")?;
        Ok(count as usize)
    }

    async fn evict_embedding_cache(&self, keep: usize) -> crate::error::Result<usize> {
        let affected = match self.db.dialect() {
            Dialect::Sqlite => execute!(
                &self.db,
                "DELETE FROM embedding_cache WHERE rowid IN (
                    SELECT rowid FROM embedding_cache ORDER BY updated_at ASC
                    LIMIT MAX(0, (SELECT COUNT(*) FROM embedding_cache) - ?)
                 )",
                keep as i64,
            )?,
            Dialect::Postgres => execute!(
                &self.db,
                "DELETE FROM embedding_cache WHERE (provider, model, provider_key, hash) IN (
                    SELECT provider, model, provider_key, hash FROM embedding_cache
                    ORDER BY updated_at ASC
                    LIMIT GREATEST(0, (SELECT COUNT(*) FROM embedding_cache) - ?)
                 )",
                keep as i64,
            )?,
        };
        Ok(affected as usize)
    }

    #[tracing::instrument(skip(self, embedding, query), fields(query_len = query.len(), limit))]
    async fn hybrid_search(
        &self,
        embedding: &[f32],
        query: &str,
        vector_weight: f32,
        keyword_weight: f32,
        merge_strategy: MergeStrategy,
        limit: usize,
    ) -> crate::error::Result<Vec<SearchResult>> {
        let fetch_limit = limit * 3;

        let vector_results = if embedding.is_empty() {
            Vec::new()
        } else {
            vector_search(&self.db, embedding, fetch_limit).await?
        };

        let keyword_results = if query.is_empty() {
            Vec::new()
        } else {
            keyword_search(&self.db, query, fetch_limit).await?
        };

        let merged = match merge_strategy {
            MergeStrategy::Weighted => search::merge_weighted(
                &vector_results,
                &keyword_results,
                vector_weight,
                keyword_weight,
            ),
            MergeStrategy::Rrf { k } => search::merge_rrf(
                &vector_results,
                &keyword_results,
                vector_weight,
                keyword_weight,
                k,
            ),
        };

        let mut final_results: Vec<SearchResult> = merged.into_iter().take(limit).collect();
        search::fill_missing_text(self, &mut final_results).await?;
        Ok(final_results)
    }

    async fn try_acquire_sync_lock(&self) -> crate::error::Result<Option<SyncLockGuard>> {
        SyncLockGuard::try_acquire(&self.db).await
    }

    async fn pgvector_status(&self) -> crate::error::Result<Option<PgvectorStatus>> {
        // Cached: the startup preflight result is not reachable here and the
        // status RPC must not re-probe on every call.
        self.pgvector
            .get_or_try_init(|| crate::pgvector::probe(&self.db))
            .await
            .copied()
    }

    fn store_type(&self) -> &'static str {
        match self.db.dialect() {
            Dialect::Sqlite => "sqlite",
            Dialect::Postgres => "postgres",
        }
    }

    fn tracks_embedding_identity(&self) -> bool {
        true
    }

    async fn read_embedding_meta(&self) -> crate::error::Result<Option<EmbeddingMeta>> {
        let rows: Vec<(String, String)> = fetch_all!(
            &self.db,
            (String, String),
            "SELECT key, value FROM memory_meta"
        )?;
        let mut provider_key = None;
        let mut model = None;
        let mut dimensions = None;
        for (key, value) in rows {
            match key.as_str() {
                META_PROVIDER_KEY => provider_key = Some(value),
                META_MODEL => model = Some(value),
                META_DIMENSIONS => dimensions = value.parse::<usize>().ok(),
                _ => {},
            }
        }
        Ok(match (provider_key, model, dimensions) {
            (Some(provider_key), Some(model), Some(dimensions)) => Some(EmbeddingMeta {
                provider_key,
                model,
                dimensions,
            }),
            // A partially written identity is no identity: the next sync
            // re-embeds and rewrites all three keys.
            _ => None,
        })
    }

    async fn write_embedding_meta(&self, meta: &EmbeddingMeta) -> crate::error::Result<()> {
        let rows = [
            (META_PROVIDER_KEY, meta.provider_key.clone()),
            (META_MODEL, meta.model.clone()),
            (META_DIMENSIONS, meta.dimensions.to_string()),
        ];
        let mut tx = self.db.begin().await?;
        for (key, value) in rows {
            moltis_db::tx_execute!(
                &mut tx,
                "INSERT INTO memory_meta (key, value) VALUES (?, ?) \
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                key,
                value,
            )?;
        }
        tx.commit().await?;
        Ok(())
    }

    async fn relax_embedding_schema(&self) -> crate::error::Result<()> {
        if !self.db.is_postgres() {
            return Ok(());
        }
        // The HNSW index cannot span mixed dimensions, and it would be
        // rebuilt (then reject the new vectors) on the type change.
        execute!(
            &self.db,
            format!("DROP INDEX IF EXISTS {CHUNKS_EMBEDDING_INDEX}")
        )?;
        if embedding_column_type(&self.db).await? != "vector" {
            // Untyped vectors accept any dimension, so the rows can be mixed
            // between the old and the new provider while the re-embed runs.
            execute!(
                &self.db,
                "ALTER TABLE chunks ALTER COLUMN embedding TYPE vector USING embedding::vector"
            )?;
        }
        Ok(())
    }

    async fn ensure_embedding_schema(&self, dimensions: usize) -> crate::error::Result<()> {
        if !self.db.is_postgres() {
            return Ok(());
        }
        let expected = format!("vector({dimensions})");
        let (current, index_present): (String, bool) = fetch_one!(
            &self.db,
            (String, bool),
            format!(
                "SELECT format_type(a.atttypid, a.atttypmod), \
                 to_regclass('{CHUNKS_EMBEDDING_INDEX}') IS NOT NULL \
                 FROM pg_attribute a WHERE a.attrelid = 'chunks'::regclass \
                 AND a.attname = 'embedding' AND NOT a.attisdropped"
            ),
        )?;
        if current != expected {
            // `USING` is required: casting an untyped vector to a dimensioned
            // one is not an assignment cast. Every stored row already carries
            // `dimensions` values by now — the re-embed runs before this — so
            // the cast cannot fail; a mismatch surfaces as a loud error here
            // rather than silent mixed-dimension data.
            execute!(
                &self.db,
                format!(
                    "ALTER TABLE chunks ALTER COLUMN embedding TYPE vector({dimensions}) \
                     USING embedding::vector({dimensions})"
                )
            )?;
        }
        if !index_present {
            // HNSW with pgvector's default build parameters (m=16,
            // ef_construction=64): a fine default for memory-sized indexes and
            // no tuning knob to expose yet. Cosine distance matches both the
            // `<=>` search and how vectors are compared in SQLite.
            execute!(
                &self.db,
                format!(
                    "CREATE INDEX IF NOT EXISTS {CHUNKS_EMBEDDING_INDEX} \
                     ON chunks USING hnsw (embedding vector_cosine_ops)"
                )
            )?;
        }
        Ok(())
    }
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use {
        super::*,
        crate::schema::{FileRow, run_migrations},
    };

    /// Backend under test, plus re-embedding of `ChunkRow::embedding`.
    enum Backend {
        Sqlite,
        Postgres,
    }

    async fn store(backend: Backend) -> Option<DbMemoryStore> {
        let db = match backend {
            Backend::Sqlite => {
                let pool = sqlx::SqlitePool::connect(":memory:").await.unwrap();
                let db = Db::from(pool);
                run_migrations(&db).await.unwrap();
                db
            },
            Backend::Postgres => crate::test_support::pg_test_db().await?,
        };
        Some(DbMemoryStore::new(db))
    }

    /// Build a chunk whose `embedding` is in the trait's byte representation.
    fn chunk(id: &str, path: &str, text: &str, embedding: Option<Vec<f32>>) -> ChunkRow {
        ChunkRow {
            id: id.into(),
            path: path.into(),
            source: "s".into(),
            start_line: 1,
            end_line: 5,
            hash: format!("h-{id}"),
            model: "m".into(),
            text: text.into(),
            embedding: embedding.map(|v| pack_embedding(&v)),
            updated_at: "2024-01-01 00:00:00".into(),
        }
    }

    fn file(path: &str) -> FileRow {
        FileRow {
            path: path.into(),
            source: "s".into(),
            hash: "h".into(),
            mtime: 0,
            size: 0,
        }
    }

    async fn run_common_suite(backend: Backend) {
        let Some(store) = store(backend).await else {
            return;
        };

        // ---- file CRUD ----
        store.upsert_file(&file("t.md")).await.unwrap();
        assert_eq!(store.get_file("t.md").await.unwrap().unwrap().hash, "h");
        assert_eq!(store.list_files().await.unwrap().len(), 1);
        assert!(store.get_file("missing.md").await.unwrap().is_none());

        // ---- chunk CRUD + embedding round-trip through the database ----
        store
            .upsert_chunks(&[
                chunk("c1", "t.md", "first", Some(vec![1.0, 0.0, 0.0])),
                chunk("c2", "t.md", "second", Some(vec![0.0, 1.0, 0.0])),
                chunk("c3", "t.md", "third no embedding", None),
            ])
            .await
            .unwrap();

        let chunks = store.get_chunks_for_file("t.md").await.unwrap();
        assert_eq!(chunks.len(), 3);
        let c1 = store.get_chunk_by_id("c1").await.unwrap().unwrap();
        let decoded = unpack_embedding(c1.embedding.as_deref().unwrap());
        assert_eq!(
            decoded,
            vec![1.0, 0.0, 0.0],
            "embedding must survive the round-trip"
        );
        assert!(
            store
                .get_chunk_by_id("c3")
                .await
                .unwrap()
                .unwrap()
                .embedding
                .is_none()
        );

        // upsert of the same id must update, not duplicate
        store
            .upsert_chunks(&[chunk(
                "c1",
                "t.md",
                "first updated",
                Some(vec![1.0, 0.0, 0.0]),
            )])
            .await
            .unwrap();
        let chunks = store.get_chunks_for_file("t.md").await.unwrap();
        assert_eq!(chunks.len(), 3);
        assert_eq!(
            store.get_chunk_by_id("c1").await.unwrap().unwrap().text,
            "first updated"
        );

        // ---- vector search ranks by similarity ----
        let results = store
            .hybrid_search(&[1.0, 0.0, 0.0], "", 1.0, 0.0, MergeStrategy::Weighted, 10)
            .await
            .unwrap();
        assert_eq!(results.len(), 2, "only chunks with embeddings rank");
        assert_eq!(results[0].chunk_id, "c1");
        assert!(
            (results[0].score - 1.0).abs() < 1e-5,
            "exact match scores ~1"
        );
        assert!(results[1].score < results[0].score);

        // ---- keyword search ----
        let results = store
            .hybrid_search(&[], "second", 0.0, 1.0, MergeStrategy::Weighted, 10)
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].chunk_id, "c2");

        // ---- hybrid ----
        let results = store
            .hybrid_search(
                &[1.0, 0.0, 0.0],
                "second",
                0.5,
                0.5,
                MergeStrategy::Weighted,
                10,
            )
            .await
            .unwrap();
        assert!(results.len() >= 2);

        // ---- embedding cache ----
        store
            .put_cached_embedding("openai", "model", "key", "hash1", &[0.1, 0.2, 0.3])
            .await
            .unwrap();
        let cached = store
            .get_cached_embedding("openai", "model", "hash1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(cached.len(), 3);
        for (a, b) in cached.iter().zip([0.1, 0.2, 0.3].iter()) {
            assert!((a - b).abs() < 1e-6);
        }

        store
            .put_cached_embeddings_batch(&[
                CacheEntry {
                    provider: "openai",
                    model: "model",
                    provider_key: "key",
                    hash: "hash2",
                    embedding: &[1.0, 2.0],
                },
                CacheEntry {
                    provider: "openai",
                    model: "model",
                    provider_key: "key",
                    hash: "hash3",
                    embedding: &[3.0, 4.0],
                },
            ])
            .await
            .unwrap();
        assert_eq!(store.count_cached_embeddings().await.unwrap(), 3);

        let evicted = store.evict_embedding_cache(1).await.unwrap();
        assert_eq!(evicted, 2, "eviction keeps the newest entry");
        assert_eq!(store.count_cached_embeddings().await.unwrap(), 1);

        // ---- delete cascades ----
        store.delete_chunks_for_file("t.md").await.unwrap();
        assert!(store.get_chunks_for_file("t.md").await.unwrap().is_empty());
        store.delete_file("t.md").await.unwrap();
        assert!(store.list_files().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn sqlite_common_suite() {
        run_common_suite(Backend::Sqlite).await;
    }

    #[tokio::test]
    async fn postgres_common_suite() {
        if std::env::var("TEST_POSTGRES_URL").is_err() {
            eprintln!("skipping: TEST_POSTGRES_URL is not set");
            return;
        }
        let _guard = crate::test_support::pg_lock().lock().await;
        run_common_suite(Backend::Postgres).await;
    }

    #[tokio::test]
    async fn postgres_store_type_is_reported() {
        if std::env::var("TEST_POSTGRES_URL").is_err() {
            return;
        }
        let _guard = crate::test_support::pg_lock().lock().await;
        let Some(store) = store(Backend::Postgres).await else {
            return;
        };
        assert_eq!(MemoryStore::store_type(&store), "postgres");
        assert_eq!(DbMemoryStore::store_type(&store), "postgres");
    }

    #[test]
    fn vector_literal_round_trip() {
        let values = vec![0.5f32, -1.25, 0.0, 3.5];
        assert_eq!(vector_literal(&values), "[0.5,-1.25,0,3.5]");
        assert_eq!(parse_vector_literal(&vector_literal(&values)), values);
        assert!(parse_vector_literal("[]").is_empty());
        // Non-finite values are replaced so pgvector accepts the literal.
        assert_eq!(vector_literal(&[f32::NAN]), "[0]");
    }

    #[test]
    fn embedding_byte_round_trip() {
        let values = vec![0.1f32, 0.2, 0.3, -0.5];
        assert_eq!(unpack_embedding(&pack_embedding(&values)), values);
    }

    #[test]
    fn cosine_similarity_bounds() {
        assert!((cosine_similarity(&[1.0, 0.0], &[1.0, 0.0]) - 1.0).abs() < 1e-6);
        assert!((cosine_similarity(&[1.0, 0.0], &[0.0, 1.0])).abs() < 1e-6);
        assert!((cosine_similarity(&[1.0, 0.0], &[-1.0, 0.0]) + 1.0).abs() < 1e-6);
        assert_eq!(cosine_similarity(&[], &[]), 0.0);
        assert_eq!(cosine_similarity(&[1.0], &[1.0, 2.0]), 0.0);
    }

    #[test]
    fn sanitize_fts5_query_strips_grammar() {
        assert_eq!(sanitize_fts5_query("hello world"), "\"hello\" \"world\"");
        assert_eq!(sanitize_fts5_query("37.759"), "\"37759\"");
        assert_eq!(sanitize_fts5_query("..."), "");
        assert_eq!(sanitize_fts5_query("  "), "");
    }
}
