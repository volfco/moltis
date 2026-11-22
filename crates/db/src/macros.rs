//! Query macros that work on both backends.
//!
//! The sqlx query builder is generic over a single backend, so each macro
//! expands into one branch per backend and picks one at runtime. Call sites
//! therefore look identical to plain sqlx usage:
//!
//! ```ignore
//! let rows: Vec<(String, i64)> =
//!     moltis_db::fetch_all!(&db, (String, i64), "SELECT path, size FROM files",)?;
//! ```
//!
//! Invariants every call site must respect so both backends agree:
//!
//! - Query text uses `?` placeholders (rewritten to `$n` for PostgreSQL).
//! - Integer columns are `BIGINT` in the PostgreSQL schema so they decode as
//!   `i64`, matching SQLite `INTEGER`.
//! - Boolean columns are `BOOLEAN` in PostgreSQL and `INTEGER` in SQLite.
//! - Timestamps are stored as epoch milliseconds (`BIGINT` / `INTEGER`).
//!
//! `$db` must be an expression of type `&Db`; `$tx` must be `&mut DbTx`.

/// Fetch all rows as a tuple or `FromRow` type.
#[macro_export]
macro_rules! fetch_all {
    ($db:expr, $ty:ty, $sql:expr $(, $bind:expr)* $(,)?) => {{
        let __raw = $sql;
        let __dialect = $crate::Db::dialect($db);
        let __sql = $crate::rewrite_placeholders(
            __dialect,
            ::core::convert::AsRef::<str>::as_ref(&__raw),
        );
        match $db {
            $crate::Db::Sqlite(__pool) => ::sqlx::query_as::<_, $ty>(&__sql)
                $(.bind($bind))*
                .fetch_all(__pool)
                .await,
            $crate::Db::Postgres(__pool) => ::sqlx::query_as::<_, $ty>(&__sql)
                $(.bind($bind))*
                .fetch_all(__pool)
                .await,
        }
    }};
}

/// Fetch zero or one row.
#[macro_export]
macro_rules! fetch_optional {
    ($db:expr, $ty:ty, $sql:expr $(, $bind:expr)* $(,)?) => {{
        let __raw = $sql;
        let __dialect = $crate::Db::dialect($db);
        let __sql = $crate::rewrite_placeholders(
            __dialect,
            ::core::convert::AsRef::<str>::as_ref(&__raw),
        );
        match $db {
            $crate::Db::Sqlite(__pool) => ::sqlx::query_as::<_, $ty>(&__sql)
                $(.bind($bind))*
                .fetch_optional(__pool)
                .await,
            $crate::Db::Postgres(__pool) => ::sqlx::query_as::<_, $ty>(&__sql)
                $(.bind($bind))*
                .fetch_optional(__pool)
                .await,
        }
    }};
}

/// Fetch exactly one row, erroring when the query returns none.
#[macro_export]
macro_rules! fetch_one {
    ($db:expr, $ty:ty, $sql:expr $(, $bind:expr)* $(,)?) => {{
        let __raw = $sql;
        let __dialect = $crate::Db::dialect($db);
        let __sql = $crate::rewrite_placeholders(
            __dialect,
            ::core::convert::AsRef::<str>::as_ref(&__raw),
        );
        match $db {
            $crate::Db::Sqlite(__pool) => ::sqlx::query_as::<_, $ty>(&__sql)
                $(.bind($bind))*
                .fetch_one(__pool)
                .await,
            $crate::Db::Postgres(__pool) => ::sqlx::query_as::<_, $ty>(&__sql)
                $(.bind($bind))*
                .fetch_one(__pool)
                .await,
        }
    }};
}

/// Execute a statement and return the number of affected rows.
#[macro_export]
macro_rules! execute {
    ($db:expr, $sql:expr $(, $bind:expr)* $(,)?) => {{
        let __raw = $sql;
        let __dialect = $crate::Db::dialect($db);
        let __sql = $crate::rewrite_placeholders(
            __dialect,
            ::core::convert::AsRef::<str>::as_ref(&__raw),
        );
        match $db {
            $crate::Db::Sqlite(__pool) => ::sqlx::query(&__sql)
                $(.bind($bind))*
                .execute(__pool)
                .await
                .map(|r| r.rows_affected()),
            $crate::Db::Postgres(__pool) => ::sqlx::query(&__sql)
                $(.bind($bind))*
                .execute(__pool)
                .await
                .map(|r| r.rows_affected()),
        }
    }};
}

/// Fetch a single scalar value (e.g. `SELECT COUNT(*)`).
#[macro_export]
macro_rules! scalar {
    ($db:expr, $ty:ty, $sql:expr $(, $bind:expr)* $(,)?) => {{
        let __raw = $sql;
        let __dialect = $crate::Db::dialect($db);
        let __sql = $crate::rewrite_placeholders(
            __dialect,
            ::core::convert::AsRef::<str>::as_ref(&__raw),
        );
        match $db {
            $crate::Db::Sqlite(__pool) => ::sqlx::query_scalar::<_, $ty>(&__sql)
                $(.bind($bind))*
                .fetch_one(__pool)
                .await,
            $crate::Db::Postgres(__pool) => ::sqlx::query_scalar::<_, $ty>(&__sql)
                $(.bind($bind))*
                .fetch_one(__pool)
                .await,
        }
    }};
}

/// Fetch all rows inside a transaction.
#[macro_export]
macro_rules! tx_fetch_all {
    ($tx:expr, $ty:ty, $sql:expr $(, $bind:expr)* $(,)?) => {{
        let __raw = $sql;
        let __sql = $crate::rewrite_placeholders(
            $crate::DbTx::dialect($tx),
            ::core::convert::AsRef::<str>::as_ref(&__raw),
        );
        match $tx {
            $crate::DbTx::Sqlite(__tx) => ::sqlx::query_as::<_, $ty>(&__sql)
                $(.bind($bind))*
                .fetch_all(&mut **__tx)
                .await,
            $crate::DbTx::Postgres(__tx) => ::sqlx::query_as::<_, $ty>(&__sql)
                $(.bind($bind))*
                .fetch_all(&mut **__tx)
                .await,
        }
    }};
}

/// Fetch zero or one row inside a transaction.
#[macro_export]
macro_rules! tx_fetch_optional {
    ($tx:expr, $ty:ty, $sql:expr $(, $bind:expr)* $(,)?) => {{
        let __raw = $sql;
        let __sql = $crate::rewrite_placeholders(
            $crate::DbTx::dialect($tx),
            ::core::convert::AsRef::<str>::as_ref(&__raw),
        );
        match $tx {
            $crate::DbTx::Sqlite(__tx) => ::sqlx::query_as::<_, $ty>(&__sql)
                $(.bind($bind))*
                .fetch_optional(&mut **__tx)
                .await,
            $crate::DbTx::Postgres(__tx) => ::sqlx::query_as::<_, $ty>(&__sql)
                $(.bind($bind))*
                .fetch_optional(&mut **__tx)
                .await,
        }
    }};
}

/// Fetch exactly one row inside a transaction.
#[macro_export]
macro_rules! tx_fetch_one {
    ($tx:expr, $ty:ty, $sql:expr $(, $bind:expr)* $(,)?) => {{
        let __raw = $sql;
        let __sql = $crate::rewrite_placeholders(
            $crate::DbTx::dialect($tx),
            ::core::convert::AsRef::<str>::as_ref(&__raw),
        );
        match $tx {
            $crate::DbTx::Sqlite(__tx) => ::sqlx::query_as::<_, $ty>(&__sql)
                $(.bind($bind))*
                .fetch_one(&mut **__tx)
                .await,
            $crate::DbTx::Postgres(__tx) => ::sqlx::query_as::<_, $ty>(&__sql)
                $(.bind($bind))*
                .fetch_one(&mut **__tx)
                .await,
        }
    }};
}

/// Execute a statement inside a transaction, returning affected rows.
#[macro_export]
macro_rules! tx_execute {
    ($tx:expr, $sql:expr $(, $bind:expr)* $(,)?) => {{
        let __raw = $sql;
        let __sql = $crate::rewrite_placeholders(
            $crate::DbTx::dialect($tx),
            ::core::convert::AsRef::<str>::as_ref(&__raw),
        );
        match $tx {
            $crate::DbTx::Sqlite(__tx) => ::sqlx::query(&__sql)
                $(.bind($bind))*
                .execute(&mut **__tx)
                .await
                .map(|r| r.rows_affected()),
            $crate::DbTx::Postgres(__tx) => ::sqlx::query(&__sql)
                $(.bind($bind))*
                .execute(&mut **__tx)
                .await
                .map(|r| r.rows_affected()),
        }
    }};
}
