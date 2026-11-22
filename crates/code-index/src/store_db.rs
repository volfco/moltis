//! SQLite + PostgreSQL storage backend for code-index.
//!
//! Stores chunks with quantized i8 embeddings. Keyword search runs through
//! FTS5 on SQLite and a generated `tsvector` column on PostgreSQL; vector
//! similarity is done in-memory (brute-force) following the memory system
//! pattern.

use {
    async_trait::async_trait,
    moltis_db::{Db, Dialect},
};

use crate::{
    error::{Error, Result},
    store::{CodeChunk, CodeIndexStore, dequantize, quantize},
    types::SearchResult,
};

/// Run database migrations for the code-index store (either backend).
pub async fn run_migrations(db: &Db) -> Result<()> {
    moltis_db::run_migrations(
        db,
        sqlx::migrate!("./migrations"),
        sqlx::migrate!("./migrations_pg"),
    )
    .await
    .map_err(|e| Error::IndexStore(format!("code-index migration failed: {e}")))
}

/// Escape a user query for safe use as an FTS5 MATCH expression.
///
/// FTS5 has its own query language (supports AND, OR, NOT, NEAR, phrase literals).
/// Passing raw input causes parse errors on unbalanced quotes or boolean operators.
/// We quote the entire query as a phrase literal, escaping internal double-quotes.
fn escape_fts5_query(query: &str) -> String {
    let escaped = query.replace('"', "\"\"");
    format!("\"{escaped}\"")
}

#[cfg(test)]
mod fts5_tests {
    use super::escape_fts5_query;

    #[test]
    fn normal_query() {
        assert_eq!(escape_fts5_query("hello world"), "\"hello world\"");
    }

    #[test]
    fn query_with_quotes() {
        assert_eq!(
            escape_fts5_query("find the \"auth\" middleware"),
            "\"find the \"\"auth\"\" middleware\""
        );
    }

    #[test]
    fn query_with_boolean_ops() {
        // Quoted as phrase literal — operators become literal text
        assert_eq!(
            escape_fts5_query("hello AND world OR NOT"),
            "\"hello AND world OR NOT\""
        );
    }

    #[test]
    fn empty_query() {
        assert_eq!(escape_fts5_query(""), "\"\"");
    }

    #[test]
    fn query_with_backslash() {
        // Backslashes pass through — FTS5 phrase literals do not escape them.
        let input = r"path	oile";
        let expected = r#""path	oile""#;
        assert_eq!(escape_fts5_query(input), expected);
    }
}

/// Dual-backend code index store (SQLite or PostgreSQL).
pub struct DbCodeIndexStore {
    db: Db,
}

impl DbCodeIndexStore {
    /// Create a store on a connected backend and run migrations.
    pub async fn new(db: Db) -> Result<Self> {
        let store = Self { db };
        store.initialize().await?;
        Ok(store)
    }
}

#[async_trait]
impl CodeIndexStore for DbCodeIndexStore {
    async fn initialize(&self) -> Result<()> {
        run_migrations(&self.db).await
    }

    async fn upsert_chunks(
        &self,
        project_id: &str,
        file_path: &str,
        chunks: &[CodeChunk],
    ) -> Result<()> {
        // Use a transaction for batch insert
        let mut tx = self
            .db
            .begin()
            .await
            .map_err(|e| Error::IndexStore(format!("failed to begin transaction: {e}")))?;

        // Delete existing chunks for this file first (full reindex of file)
        moltis_db::tx_execute!(
            &mut tx,
            "DELETE FROM code_chunks WHERE project_id = ? AND file_path = ?",
            project_id,
            file_path,
        )
        .map_err(|e| Error::IndexStore(format!("failed to delete existing chunks: {e}")))?;

        // Insert new chunks
        for chunk in chunks {
            let embedding_bytes = chunk
                .embedding
                .as_ref()
                .map(|e| quantize(e))
                .map(|q| q.into_iter().map(|v| v as u8).collect::<Vec<u8>>());

            moltis_db::tx_execute!(
                &mut tx,
                r#"
                INSERT INTO code_chunks
                    (project_id, file_path, chunk_index, content, embedding, start_line, end_line)
                VALUES
                    (?, ?, ?, ?, ?, ?, ?)
                "#,
                project_id,
                &chunk.file_path,
                chunk.chunk_index as i64,
                &chunk.content,
                embedding_bytes,
                chunk.start_line as i64,
                chunk.end_line as i64,
            )
            .map_err(|e| Error::IndexStore(format!("failed to insert chunk: {e}")))?;
        }

        tx.commit()
            .await
            .map_err(|e| Error::IndexStore(format!("failed to commit transaction: {e}")))?;

        Ok(())
    }

    async fn delete_file_chunks(&self, project_id: &str, file_path: &str) -> Result<()> {
        moltis_db::execute!(
            &self.db,
            "DELETE FROM code_chunks WHERE project_id = ? AND file_path = ?",
            project_id,
            file_path,
        )
        .map_err(|e| Error::IndexStore(format!("failed to delete file chunks: {e}")))?;

        Ok(())
    }

    async fn clear_project(&self, project_id: &str) -> Result<()> {
        moltis_db::execute!(
            &self.db,
            "DELETE FROM code_chunks WHERE project_id = ?",
            project_id,
        )
        .map_err(|e| Error::IndexStore(format!("failed to clear project: {e}")))?;

        Ok(())
    }

    async fn get_project_chunks(&self, project_id: &str) -> Result<Vec<CodeChunk>> {
        let rows: Vec<(String, i64, String, Option<Vec<u8>>, i64, i64)> = moltis_db::fetch_all!(
            &self.db,
            (String, i64, String, Option<Vec<u8>>, i64, i64),
            r#"
            SELECT
                file_path,
                chunk_index,
                content,
                embedding,
                start_line,
                end_line
            FROM code_chunks
            WHERE project_id = ?
            ORDER BY file_path, chunk_index
            "#,
            project_id,
        )
        .map_err(|e| Error::IndexStore(format!("failed to fetch chunks: {e}")))?;

        let mut chunks = Vec::with_capacity(rows.len());
        for (file_path, chunk_index, content, embedding_bytes, start_line, end_line) in rows {
            let embedding = embedding_bytes.map(|bytes| {
                let i8_vec: Vec<i8> = bytes.into_iter().map(|b| b as i8).collect();
                dequantize(&i8_vec)
            });

            chunks.push(CodeChunk {
                file_path,
                chunk_index: chunk_index as usize,
                content,
                embedding,
                start_line: start_line as usize,
                end_line: end_line as usize,
            });
        }

        Ok(chunks)
    }

    async fn search_keyword(
        &self,
        project_id: &str,
        query: &str,
        limit: usize,
    ) -> Result<Vec<SearchResult>> {
        // FTS5 query with project filter via JOIN on SQLite; generated
        // tsvector column on PostgreSQL. The tsquery is built once in FROM so
        // the bind list keeps one bind per placeholder.
        let rows: Vec<(String, String, i64, i64)> = match self.db.dialect() {
            Dialect::Sqlite => {
                // Query is escaped as a phrase literal to prevent FTS5 parse
                // errors on unbalanced quotes or boolean operators from
                // LLM-generated input.
                let safe_query = escape_fts5_query(query);
                moltis_db::fetch_all!(
                    &self.db,
                    (String, String, i64, i64),
                    r#"
            SELECT
                c.file_path,
                c.content,
                c.start_line,
                c.end_line
            FROM code_chunks c
            JOIN code_chunks_fts fts ON c.id = fts.rowid
            WHERE c.project_id = ?
                AND code_chunks_fts MATCH ?
            ORDER BY fts.rank
            LIMIT ?
            "#,
                    project_id,
                    &safe_query,
                    limit as i64,
                )
            },
            Dialect::Postgres => {
                // `websearch_to_tsquery` accepts free-form user text and never
                // raises a syntax error, so no phrase escaping is needed.
                moltis_db::fetch_all!(
                    &self.db,
                    (String, String, i64, i64),
                    r#"
            SELECT c.file_path, c.content, c.start_line, c.end_line
            FROM code_chunks c, websearch_to_tsquery('english', ?) AS q
            WHERE c.project_id = ? AND c.tsv @@ q
            ORDER BY ts_rank(c.tsv, q)::float8 DESC
            LIMIT ?
            "#,
                    query,
                    project_id,
                    limit as i64,
                )
            },
        }
        .map_err(|e| Error::IndexStore(format!("keyword search failed: {e}")))?;

        let results = rows
            .into_iter()
            .enumerate()
            .map(
                |(rank, (file_path, content, start_line, end_line))| SearchResult {
                    chunk_id: format!("{}:{}:{}", project_id, file_path, start_line),
                    path: file_path,
                    text: content,
                    start_line: start_line as usize,
                    end_line: end_line as usize,
                    score: 1.0 / (1.0 + rank as f32), // Map rank to (0, 1] range, higher is better
                    source: "builtin".to_string(),
                },
            )
            .collect();

        Ok(results)
    }

    async fn file_count(&self, project_id: &str) -> Result<usize> {
        let count: i64 = moltis_db::scalar!(
            &self.db,
            i64,
            "SELECT COUNT(DISTINCT file_path) FROM code_chunks WHERE project_id = ?",
            project_id,
        )
        .map_err(|e| Error::IndexStore(format!("failed to count files: {e}")))?;

        Ok(count as usize)
    }

    async fn chunk_count(&self, project_id: &str) -> Result<usize> {
        let count: i64 = moltis_db::scalar!(
            &self.db,
            i64,
            "SELECT COUNT(*) FROM code_chunks WHERE project_id = ?",
            project_id,
        )
        .map_err(|e| Error::IndexStore(format!("failed to count chunks: {e}")))?;

        Ok(count as usize)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use {super::*, moltis_db::DbOptions};

    /// Engine a test case runs against.
    enum Backend {
        Sqlite,
        Postgres,
    }

    /// Serializes PostgreSQL tests: they share one database, so they must not
    /// reset the schema while another test is mid-flight.
    fn pg_lock() -> &'static tokio::sync::Mutex<()> {
        static LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
        &LOCK
    }

    fn make_chunk(
        file_path: &str,
        index: usize,
        content: &str,
        start: usize,
        end: usize,
    ) -> CodeChunk {
        CodeChunk {
            file_path: file_path.to_string(),
            chunk_index: index,
            content: content.to_string(),
            embedding: None,
            start_line: start,
            end_line: end,
        }
    }

    /// Fresh store with an empty code-index schema. Returns `None` when
    /// PostgreSQL is requested but `TEST_POSTGRES_URL` is unset or setup
    /// fails. The caller must hold [`pg_lock`] for [`Backend::Postgres`].
    async fn store(backend: Backend) -> Option<DbCodeIndexStore> {
        let db = match backend {
            Backend::Sqlite => Db::connect(&DbOptions::sqlite(":memory:", 1)).await.ok()?,
            Backend::Postgres => {
                let url = std::env::var("TEST_POSTGRES_URL").ok()?;
                let db = Db::connect(&DbOptions::postgres(url, 2)).await.ok()?;
                // Dropping sqlx's history is required: with it in place the
                // migrator believes this version is already applied and
                // creates nothing.
                let reset = moltis_db::execute!(
                    &db,
                    "DROP TABLE IF EXISTS code_chunks_fts, code_chunks, _sqlx_migrations CASCADE"
                );
                if let Err(error) = reset {
                    eprintln!("postgres test reset failed: {error}");
                    return None;
                }
                db
            },
        };
        Some(DbCodeIndexStore::new(db).await.unwrap())
    }

    async fn initialize_idempotent_case(store: &DbCodeIndexStore) {
        // initialize() was already called by the constructor, call again
        store.initialize().await.unwrap();
        store.initialize().await.unwrap();
    }

    async fn upsert_and_get_chunks_case(store: &DbCodeIndexStore) {
        let chunks = vec![
            make_chunk("foo.rs", 0, "fn main() {}", 1, 1),
            make_chunk("foo.rs", 1, "fn helper() {}", 2, 2),
        ];
        store
            .upsert_chunks("proj", "foo.rs", &chunks)
            .await
            .unwrap();

        let got = store.get_project_chunks("proj").await.unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].file_path, "foo.rs");
        assert_eq!(got[0].chunk_index, 0);
        assert_eq!(got[0].content, "fn main() {}");
        assert_eq!(got[0].start_line, 1);
        assert_eq!(got[1].content, "fn helper() {}");
    }

    async fn upsert_replaces_file_case(store: &DbCodeIndexStore) {
        let v1 = vec![make_chunk("a.rs", 0, "version 1", 1, 1)];
        store.upsert_chunks("proj", "a.rs", &v1).await.unwrap();

        let v2 = vec![
            make_chunk("a.rs", 0, "version 2 line 1", 1, 1),
            make_chunk("a.rs", 1, "version 2 line 2", 2, 2),
        ];
        store.upsert_chunks("proj", "a.rs", &v2).await.unwrap();

        let got = store.get_project_chunks("proj").await.unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].content, "version 2 line 1");
        assert_eq!(got[1].content, "version 2 line 2");
    }

    async fn multi_file_isolation_case(store: &DbCodeIndexStore) {
        let chunks_a = vec![make_chunk("a.rs", 0, "file a", 1, 1)];
        let chunks_b = vec![make_chunk("b.rs", 0, "file b", 1, 1)];
        store
            .upsert_chunks("proj", "a.rs", &chunks_a)
            .await
            .unwrap();
        store
            .upsert_chunks("proj", "b.rs", &chunks_b)
            .await
            .unwrap();

        let got = store.get_project_chunks("proj").await.unwrap();
        assert_eq!(got.len(), 2);

        store.delete_file_chunks("proj", "a.rs").await.unwrap();
        let got = store.get_project_chunks("proj").await.unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].file_path, "b.rs");
    }

    async fn multi_project_isolation_case(store: &DbCodeIndexStore) {
        let chunks = vec![make_chunk("shared.rs", 0, "content", 1, 1)];
        store
            .upsert_chunks("X", "shared.rs", &chunks)
            .await
            .unwrap();
        store
            .upsert_chunks("Y", "shared.rs", &chunks)
            .await
            .unwrap();

        let got_x = store.get_project_chunks("X").await.unwrap();
        assert_eq!(got_x.len(), 1);

        let got_y = store.get_project_chunks("Y").await.unwrap();
        assert_eq!(got_y.len(), 1);
    }

    async fn delete_file_chunks_case(store: &DbCodeIndexStore) {
        store
            .upsert_chunks("proj", "a.rs", &[make_chunk("a.rs", 0, "a", 1, 1)])
            .await
            .unwrap();
        store
            .upsert_chunks("proj", "b.rs", &[make_chunk("b.rs", 0, "b", 1, 1)])
            .await
            .unwrap();

        store.delete_file_chunks("proj", "a.rs").await.unwrap();

        let got = store.get_project_chunks("proj").await.unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].file_path, "b.rs");
    }

    async fn clear_project_case(store: &DbCodeIndexStore) {
        let chunks = vec![make_chunk("f.rs", 0, "data", 1, 1)];
        store.upsert_chunks("X", "f.rs", &chunks).await.unwrap();
        store.upsert_chunks("Y", "f.rs", &chunks).await.unwrap();

        store.clear_project("X").await.unwrap();

        assert!(store.get_project_chunks("X").await.unwrap().is_empty());
        assert_eq!(store.get_project_chunks("Y").await.unwrap().len(), 1);
    }

    async fn embedding_roundtrip_case(store: &DbCodeIndexStore) {
        let original = vec![0.5, -0.3, 0.8, 0.1];
        let mut chunk = make_chunk("emb.rs", 0, "embedded code", 1, 5);
        chunk.embedding = Some(original.clone());

        store
            .upsert_chunks("proj", "emb.rs", &[chunk])
            .await
            .unwrap();

        let got = store.get_project_chunks("proj").await.unwrap();
        assert_eq!(got.len(), 1);
        let emb = got[0].embedding.as_ref().unwrap();
        // i8 quantization loses precision — tolerance of ~0.02
        for (orig, stored) in original.iter().zip(emb.iter()) {
            assert!(
                (orig - stored).abs() < 0.02,
                "original {orig} vs stored {stored}"
            );
        }
    }

    async fn keyword_search_basic_case(store: &DbCodeIndexStore) {
        store
            .upsert_chunks("proj", "main.rs", &[make_chunk(
                "main.rs",
                0,
                "fn main() { println!(\"hello\"); }",
                1,
                1,
            )])
            .await
            .unwrap();
        store
            .upsert_chunks("proj", "greet.py", &[make_chunk(
                "greet.py",
                0,
                "def greet(): pass",
                1,
                1,
            )])
            .await
            .unwrap();

        let results = store.search_keyword("proj", "main", 10).await.unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].path.contains("main.rs"));
    }

    async fn keyword_search_multi_result_case(store: &DbCodeIndexStore) {
        store
            .upsert_chunks("proj", "a.rs", &[make_chunk(
                "a.rs",
                0,
                "database connection pool",
                1,
                1,
            )])
            .await
            .unwrap();
        store
            .upsert_chunks("proj", "b.rs", &[make_chunk(
                "b.rs",
                0,
                "database query builder",
                1,
                1,
            )])
            .await
            .unwrap();
        store
            .upsert_chunks("proj", "c.rs", &[make_chunk(
                "c.rs",
                0,
                "http request handler",
                1,
                1,
            )])
            .await
            .unwrap();

        let results = store.search_keyword("proj", "database", 10).await.unwrap();
        assert_eq!(results.len(), 2);
    }

    async fn keyword_search_no_results_case(store: &DbCodeIndexStore) {
        store
            .upsert_chunks("proj", "a.rs", &[make_chunk(
                "a.rs",
                0,
                "hello world",
                1,
                1,
            )])
            .await
            .unwrap();

        let results = store
            .search_keyword("proj", "nonexistent_xyz", 10)
            .await
            .unwrap();
        assert!(results.is_empty());
    }

    async fn keyword_search_scores_valid_case(store: &DbCodeIndexStore) {
        store
            .upsert_chunks("proj", "a.rs", &[make_chunk(
                "a.rs",
                0,
                "rust programming language",
                1,
                1,
            )])
            .await
            .unwrap();
        store
            .upsert_chunks("proj", "b.rs", &[make_chunk(
                "b.rs",
                0,
                "rust memory safety",
                1,
                1,
            )])
            .await
            .unwrap();

        let results = store.search_keyword("proj", "rust", 10).await.unwrap();
        for r in &results {
            assert!(
                r.score > 0.0 && r.score <= 1.0,
                "score {} not in (0, 1]",
                r.score
            );
        }
    }

    async fn file_and_chunk_counts_case(store: &DbCodeIndexStore) {
        for file in ["a.rs", "b.rs", "c.rs"] {
            store
                .upsert_chunks("proj", file, &[
                    make_chunk(file, 0, "chunk0", 1, 5),
                    make_chunk(file, 1, "chunk1", 6, 10),
                ])
                .await
                .unwrap();
        }

        assert_eq!(store.file_count("proj").await.unwrap(), 3);
        assert_eq!(store.chunk_count("proj").await.unwrap(), 6);
    }

    async fn empty_project_case(store: &DbCodeIndexStore) {
        assert!(
            store
                .get_project_chunks("nonexistent")
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(store.file_count("nonexistent").await.unwrap(), 0);
        assert_eq!(store.chunk_count("nonexistent").await.unwrap(), 0);
    }

    #[tokio::test]
    async fn test_initialize_idempotent() {
        initialize_idempotent_case(&store(Backend::Sqlite).await.unwrap()).await;
    }

    #[tokio::test]
    async fn test_upsert_and_get_chunks() {
        upsert_and_get_chunks_case(&store(Backend::Sqlite).await.unwrap()).await;
    }

    #[tokio::test]
    async fn test_upsert_replaces_file() {
        upsert_replaces_file_case(&store(Backend::Sqlite).await.unwrap()).await;
    }

    #[tokio::test]
    async fn test_multi_file_isolation() {
        multi_file_isolation_case(&store(Backend::Sqlite).await.unwrap()).await;
    }

    #[tokio::test]
    async fn test_multi_project_isolation() {
        multi_project_isolation_case(&store(Backend::Sqlite).await.unwrap()).await;
    }

    #[tokio::test]
    async fn test_delete_file_chunks() {
        delete_file_chunks_case(&store(Backend::Sqlite).await.unwrap()).await;
    }

    #[tokio::test]
    async fn test_clear_project() {
        clear_project_case(&store(Backend::Sqlite).await.unwrap()).await;
    }

    #[tokio::test]
    async fn test_embedding_roundtrip() {
        embedding_roundtrip_case(&store(Backend::Sqlite).await.unwrap()).await;
    }

    #[tokio::test]
    async fn test_keyword_search_basic() {
        keyword_search_basic_case(&store(Backend::Sqlite).await.unwrap()).await;
    }

    #[tokio::test]
    async fn test_keyword_search_multi_result() {
        keyword_search_multi_result_case(&store(Backend::Sqlite).await.unwrap()).await;
    }

    #[tokio::test]
    async fn test_keyword_search_no_results() {
        keyword_search_no_results_case(&store(Backend::Sqlite).await.unwrap()).await;
    }

    #[tokio::test]
    async fn test_keyword_search_scores_valid() {
        keyword_search_scores_valid_case(&store(Backend::Sqlite).await.unwrap()).await;
    }

    #[tokio::test]
    async fn test_file_and_chunk_counts() {
        file_and_chunk_counts_case(&store(Backend::Sqlite).await.unwrap()).await;
    }

    #[tokio::test]
    async fn test_empty_project() {
        empty_project_case(&store(Backend::Sqlite).await.unwrap()).await;
    }

    /// Every store case against PostgreSQL, one fresh schema per case.
    #[tokio::test]
    async fn postgres_suite() {
        if std::env::var("TEST_POSTGRES_URL").is_err() {
            eprintln!("skipping: TEST_POSTGRES_URL is not set");
            return;
        }
        let _guard = pg_lock().lock().await;
        initialize_idempotent_case(&store(Backend::Postgres).await.unwrap()).await;
        upsert_and_get_chunks_case(&store(Backend::Postgres).await.unwrap()).await;
        upsert_replaces_file_case(&store(Backend::Postgres).await.unwrap()).await;
        multi_file_isolation_case(&store(Backend::Postgres).await.unwrap()).await;
        multi_project_isolation_case(&store(Backend::Postgres).await.unwrap()).await;
        delete_file_chunks_case(&store(Backend::Postgres).await.unwrap()).await;
        clear_project_case(&store(Backend::Postgres).await.unwrap()).await;
        embedding_roundtrip_case(&store(Backend::Postgres).await.unwrap()).await;
        keyword_search_basic_case(&store(Backend::Postgres).await.unwrap()).await;
        keyword_search_multi_result_case(&store(Backend::Postgres).await.unwrap()).await;
        keyword_search_no_results_case(&store(Backend::Postgres).await.unwrap()).await;
        keyword_search_scores_valid_case(&store(Backend::Postgres).await.unwrap()).await;
        file_and_chunk_counts_case(&store(Backend::Postgres).await.unwrap()).await;
        empty_project_case(&store(Backend::Postgres).await.unwrap()).await;
    }
}
