//! Memory management: markdown files → chunked → embedded → hybrid search in
//! SQLite or PostgreSQL.

pub mod chunker;
pub mod config;
#[cfg(test)]
pub mod contract;
pub mod embeddings;
pub mod embeddings_batch;
pub mod embeddings_fallback;
#[cfg(feature = "local-embeddings")]
#[allow(unsafe_code)] // FFI wrappers for llama-cpp-2 require unsafe Send/Sync impls.
pub mod embeddings_local;
pub mod embeddings_openai;
pub mod error;
pub mod manager;
pub mod pgvector;
pub mod reindex;
#[cfg(test)]
mod reindex_tests;
pub mod reranking;
pub mod runtime;
pub mod schema;
pub mod search;
pub mod session_export;
pub mod store;
pub mod store_db;
pub mod sync_lock;
#[cfg(test)]
pub(crate) mod test_support;
pub mod tools;
#[cfg(feature = "file-watcher")]
pub mod watcher;
pub mod writer;

// Re-export run_migrations for consistency with other crates.
pub use {
    error::{Error, Result},
    schema::run_migrations,
};
