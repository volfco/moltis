//! `moltis memory reindex` — re-embed the stored chunks in place.
//!
//! Automatic re-indexing only runs inside a sync pass, so operators need a
//! manual entry point for a same-identity provider switch, a suspected
//! corrupted index, or any change the recorded identity does not cover.

use crate::memory_commands::{ActiveStore, open_store_for_backend};

/// Re-embed every chunk when the configured embedding identity differs from the
/// one recorded with the index, or unconditionally with `--force`.
///
/// Works on both `[database]` engines: the store/manager own the per-engine
/// work (the identity in `memory_meta` plus chunk vectors for SQLite, the typed
/// `vector(N)` column and HNSW index for PostgreSQL).
pub(crate) async fn reindex_memory(force: bool, to: Option<String>) -> anyhow::Result<()> {
    // `--to` used to belong to this command name; refusing it keeps an old
    // migration script from silently re-embedding instead of migrating.
    if to.is_some() {
        anyhow::bail!(
            "`--to` belongs to `moltis memory migrate-collection` (renamed from \
             `memory reindex`). Run that command for collection migration."
        );
    }

    let config = moltis_config::discover_and_load();
    let data_dir = moltis_config::data_dir();
    let (store, location) = match open_store_for_backend(&config, &data_dir, true).await? {
        ActiveStore::Open { store, location } => (store, location),
        ActiveStore::NotFound { message } => anyhow::bail!(message),
    };
    // `reindex` is a no-op on stores without `memory_meta` (they never recorded
    // an identity), so say that instead of reporting a successful nothing.
    if !store.tracks_embedding_identity() {
        anyhow::bail!(
            "the {} backend does not record an embedding identity, so there is nothing to \
             re-embed here. Use memory.backend = \"builtin\" to re-index the chunk vectors.",
            store.store_type()
        );
    }

    let embedder = configured_embedder(&config).await?;
    let manager = moltis_memory::manager::MemoryManager::new(
        moltis_memory::config::MemoryConfig {
            db_path: location.clone(),
            ..Default::default()
        },
        store,
        embedder,
    );
    for line in reindex_summary(&manager.reindex(force).await?, &location)? {
        println!("{line}");
    }
    Ok(())
}

/// Build the embedding provider the gateway uses for the configured
/// `[memory]` provider.
///
/// The identity (provider key, model, dimensions) has to match what the gateway
/// recorded in `memory_meta` — otherwise every re-index would look like a
/// provider change — so this mirrors the gateway's construction rather than
/// inventing its own. Provider auto-detection and the fallback chain stay
/// gateway-only: they need the gateway's provider config and env overrides.
async fn configured_embedder(
    config: &moltis_config::MoltisConfig,
) -> anyhow::Result<Box<dyn moltis_memory::embeddings::EmbeddingProvider>> {
    use {moltis_config::MemoryProvider, secrecy::ExposeSecret};

    let mem_cfg = &config.memory;
    if mem_cfg.disable_rag {
        anyhow::bail!("RAG is disabled (memory.disable_rag = true); there is nothing to re-embed.");
    }
    let Some(provider) = mem_cfg.provider else {
        anyhow::bail!(
            "no embedding provider configured: set memory.provider and memory.model to the \
             provider the index was built with."
        );
    };

    match provider {
        MemoryProvider::Local => {
            #[cfg(feature = "local-embeddings")]
            {
                let cache_dir = mem_cfg
                    .base_url
                    .as_ref()
                    .map(std::path::PathBuf::from)
                    .unwrap_or_else(
                    moltis_memory::embeddings_local::LocalGgufEmbeddingProvider::default_cache_dir,
                );
                let model_path =
                    moltis_memory::embeddings_local::LocalGgufEmbeddingProvider::ensure_model(
                        cache_dir,
                    )
                    .await?;
                Ok(Box::new(
                    moltis_memory::embeddings_local::LocalGgufEmbeddingProvider::new(model_path)?,
                ))
            }
            #[cfg(not(feature = "local-embeddings"))]
            anyhow::bail!(
                "memory.provider = \"local\" needs a build with the `local-embeddings` feature"
            )
        },
        MemoryProvider::Ollama | MemoryProvider::Custom | MemoryProvider::OpenAi => {
            let base_url = mem_cfg.base_url.clone().unwrap_or_else(|| match provider {
                MemoryProvider::Ollama => "http://localhost:11434".into(),
                _ => "https://api.openai.com".into(),
            });
            let api_key = mem_cfg
                .api_key
                .as_ref()
                .map(|key| key.expose_secret().clone())
                .or_else(|| std::env::var("OPENAI_API_KEY").ok())
                .unwrap_or_default();
            let mut embedder =
                moltis_memory::embeddings_openai::OpenAiEmbeddingProvider::new(api_key);
            if base_url != "https://api.openai.com" {
                embedder = embedder.with_base_url(base_url);
            }
            if let Some(model) = &mem_cfg.model {
                // The gateway records 1536 dimensions for every explicitly
                // configured OpenAI-compatible provider it builds; matching it
                // keeps the identity comparable with `memory_meta`.
                embedder = embedder.with_model(model.clone(), 1536);
            }
            Ok(Box::new(embedder))
        },
    }
}

/// Operator-facing lines for a re-index pass.
///
/// A skipped pass is an error, not a success: the re-embed the operator asked
/// for did not run because another instance holds the memory sync lock. It
/// becomes an `Err` so the command exits non-zero (anyhow prints it as
/// `Error: ...`), matching how the other CLI commands report a failed outcome.
fn reindex_summary(
    report: &moltis_memory::manager::ReindexReport,
    location: &str,
) -> anyhow::Result<Vec<String>> {
    use moltis_memory::manager::ReindexReason;

    if report.skipped {
        anyhow::bail!(
            "re-index skipped: another moltis instance holds the memory sync lock for \
             {location}, so nothing was re-embedded. Re-run when it finishes."
        );
    }

    let mut lines = vec![format!("Memory re-index ({location}):")];
    let Some(reason) = report.reason else {
        lines.push(
            "  Nothing to re-embed: the recorded embedding identity already matches the \
             configured provider."
                .to_string(),
        );
        lines.push("  Re-run with --force to re-embed the chunks anyway.".to_string());
        return Ok(lines);
    };

    let reason = match reason {
        ReindexReason::NoRecordedIdentity => "initial index (no embedding identity recorded yet)",
        ReindexReason::DimensionsChanged => "dimensions changed",
        ReindexReason::ProviderChanged => "provider changed",
        ReindexReason::Forced => "forced",
    };
    let dimensions = match report.dimensions_before {
        Some(before) => format!("{before} -> {}", report.dimensions_after),
        None => format!("unknown -> {}", report.dimensions_after),
    };
    lines.push(format!(
        "  Re-embedded:     {} chunks",
        report.chunks_reembedded
    ));
    lines.push(format!("  Dimensions:      {dimensions}"));
    lines.push(format!("  Reason:          {reason}"));
    Ok(lines)
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use {super::*, crate::memory_commands::open_store_for_backend};

    #[test]
    fn test_reindex_summary_reports_count_dimensions_and_reason() {
        let report = moltis_memory::manager::ReindexReport {
            skipped: false,
            reason: Some(moltis_memory::manager::ReindexReason::DimensionsChanged),
            chunks_reembedded: 12,
            dimensions_before: Some(1536),
            dimensions_after: 768,
        };

        let summary = reindex_summary(&report, "memory.db")
            .expect("a completed pass is not an error")
            .join("\n");
        assert!(summary.contains("memory.db"), "summary names the store");
        assert!(summary.contains("12 chunks"), "summary reports the count");
        assert!(
            summary.contains("1536 -> 768"),
            "summary reports the transition"
        );
        assert!(
            summary.contains("dimensions changed"),
            "summary reports the reason"
        );
    }

    #[test]
    fn test_reindex_summary_unchanged_identity_points_at_force() {
        let report = moltis_memory::manager::ReindexReport {
            dimensions_after: 768,
            ..Default::default()
        };

        let summary = reindex_summary(&report, "memory.db")
            .expect("an unchanged identity is not an error")
            .join("\n");
        assert!(summary.contains("Nothing to re-embed"));
        assert!(summary.contains("--force"));
    }

    #[test]
    fn test_reindex_summary_skip_is_an_error() {
        let report = moltis_memory::manager::ReindexReport {
            skipped: true,
            ..Default::default()
        };

        let error = reindex_summary(&report, "memory.db")
            .err()
            .expect("a skipped pass must not report success")
            .to_string();
        assert!(error.contains("skipped"), "got: {error}");
        assert!(error.contains("sync lock"), "got: {error}");
    }

    #[tokio::test]
    async fn test_reindex_rejects_collection_migration_flag() {
        let error = reindex_memory(false, Some("target.zvec".into()))
            .await
            .err()
            .expect("`--to` must be refused")
            .to_string();
        assert!(error.contains("migrate-collection"), "got: {error}");
    }

    /// Deterministic embedder: re-indexing only needs a provider with a stable
    /// identity, not real vectors.
    struct StubEmbedder;

    #[async_trait::async_trait]
    impl moltis_memory::embeddings::EmbeddingProvider for StubEmbedder {
        async fn embed(&self, _text: &str) -> moltis_memory::error::Result<Vec<f32>> {
            Ok(vec![0.1, 0.2])
        }

        fn model_name(&self) -> &str {
            "stub"
        }

        fn dimensions(&self) -> usize {
            2
        }

        fn provider_key(&self) -> &str {
            "stub:local"
        }
    }

    /// Runs the same store/manager/summary path as `reindex_memory`, with a
    /// stub embedder instead of the config-derived one (no network).
    #[tokio::test]
    #[serial_test::serial(data_dir)]
    async fn test_reindex_reembeds_chunks_against_temp_data_dir() {
        let tmp = tempfile::TempDir::new().unwrap();
        let db_path = tmp.path().join("memory.db");
        let db_url = format!("sqlite:{}?mode=rwc", db_path.display());

        let pool = sqlx::SqlitePool::connect(&db_url).await.unwrap();
        moltis_memory::schema::run_migrations(&moltis_db::Db::from(pool.clone()))
            .await
            .unwrap();
        sqlx::query("INSERT INTO files (path, source, hash, mtime, size) VALUES (?, ?, ?, ?, ?)")
            .bind("notes.md")
            .bind("daily")
            .bind("hash1")
            .bind(2000_i64)
            .bind(100_i64)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO chunks (id, path, source, start_line, end_line, hash, model, text, updated_at)\n             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind("c1")
        .bind("notes.md")
        .bind("daily")
        .bind(1_i64)
        .bind(5_i64)
        .bind("h1")
        .bind("none")
        .bind("some content")
        .bind("now")
        .execute(&pool)
        .await
        .unwrap();
        pool.close().await;

        // A read-only pool cannot re-embed: reindex opens the database for write.
        let (store, location) =
            match open_store_for_backend(&moltis_config::MoltisConfig::default(), tmp.path(), true)
                .await
                .unwrap()
            {
                ActiveStore::Open { store, location } => (store, location),
                ActiveStore::NotFound { message } => panic!("{message}"),
            };
        let manager = moltis_memory::manager::MemoryManager::new(
            moltis_memory::config::MemoryConfig {
                db_path: location.clone(),
                ..Default::default()
            },
            store,
            Box::new(StubEmbedder),
        );

        let report = manager.reindex(true).await.unwrap();
        assert_eq!(
            report.chunks_reembedded, 1,
            "the single chunk is re-embedded"
        );
        assert!(!report.skipped);

        let summary = reindex_summary(&report, &location).unwrap().join("\n");
        assert!(summary.contains("1 chunks"), "got: {summary}");
        assert!(summary.contains("initial index"), "got: {summary}");
        assert!(
            summary.contains("unknown -> 2"),
            "no identity was recorded before the pass: {summary}"
        );
    }
}
