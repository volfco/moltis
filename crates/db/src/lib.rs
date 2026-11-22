//! Database backend abstraction.
//!
//! Moltis stores its data in SQLite by default and can be configured to use
//! PostgreSQL instead. SQL dialects differ in a few ways that matter here:
//!
//! - Placeholders: SQLite uses `?`, PostgreSQL uses `$1`, `$2`, …
//! - Types: PostgreSQL distinguishes `INT4`/`INT8` and needs `BYTEA` for blobs.
//!
//! This crate keeps query text in the SQLite (`?`) form and rewrites the
//! placeholders for PostgreSQL at execution time, so a single query string
//! works on both backends. Integer columns must be `BIGINT` in the PostgreSQL
//! schema so they decode into `i64`, matching SQLite's `INTEGER`.
//!
//! Queries go through the macros ([`fetch_all`], [`fetch_optional`],
//! [`fetch_one`], [`execute`]) because the sqlx query builder is generic over
//! one backend: the macros expand to both branches and pick one at runtime.

pub mod dialect;
pub mod error;
pub mod macros;
pub mod pool;

pub use {
    dialect::{Dialect, rewrite_placeholders},
    error::{DbError, Result},
    pool::{Db, DbOptions, DbTx, run_migrations},
};
