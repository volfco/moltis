//! Re-embedding of stored chunks with a different embedding provider.
//!
//! Chunk *text* is the source of truth: when the provider identity recorded in
//! `memory_meta` no longer matches the configured provider (or its dimension
//! changed), every chunk is re-embedded from its text. Stored vectors are
//! never transformed — an old vector cannot be converted into the new
//! provider's embedding space.
//!
//! Work is streamed file by file and embedded in batches, so re-indexing a
//! large index never holds all embeddings in memory at once.

use crate::{
    embeddings::EmbeddingProvider,
    error::Result,
    store::{CacheEntry, MemoryStore},
};

/// Chunk texts per [`EmbeddingProvider::embed_batch`] call.
const EMBED_BATCH: usize = 64;

/// Re-embed every chunk with `embedder` and refresh the embedding cache.
///
/// Returns the number of chunks re-embedded. Rows are rewritten per file: one
/// file's chunks are loaded, embedded in batches, then written back, so peak
/// memory is bounded by the largest file rather than by the index size.
pub async fn reembed_all_chunks(
    store: &dyn MemoryStore,
    embedder: &dyn EmbeddingProvider,
) -> Result<usize> {
    let provider = embedder.provider_key();
    let model = embedder.model_name();
    let mut reembedded = 0usize;

    for file in store.list_files().await? {
        let mut chunks = store.get_chunks_for_file(&file.path).await?;
        if chunks.is_empty() {
            continue;
        }

        let mut embeddings: Vec<Vec<f32>> = Vec::with_capacity(chunks.len());
        for batch in chunks.chunks(EMBED_BATCH) {
            let texts: Vec<String> = batch.iter().map(|chunk| chunk.text.clone()).collect();
            embeddings.extend(embedder.embed_batch(&texts).await?);
        }
        if embeddings.len() != chunks.len() {
            return Err(crate::error::Error::Embedding(format!(
                "embedder {} returned {} vectors for {} chunks of {}",
                model,
                embeddings.len(),
                chunks.len(),
                file.path
            )));
        }

        let cache_entries: Vec<CacheEntry<'_>> = embeddings
            .iter()
            .zip(&chunks)
            .map(|(embedding, chunk)| CacheEntry {
                provider,
                model,
                provider_key: provider,
                hash: &chunk.hash,
                embedding,
            })
            .collect();
        store.put_cached_embeddings_batch(&cache_entries).await?;

        for (chunk, embedding) in chunks.iter_mut().zip(&embeddings) {
            chunk.model = model.to_string();
            chunk.embedding = Some(
                embedding
                    .iter()
                    .flat_map(|value| value.to_le_bytes())
                    .collect(),
            );
        }
        reembedded += chunks.len();
        store.upsert_chunks(&chunks).await?;
    }

    Ok(reembedded)
}
