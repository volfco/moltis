# Decision Log

Short entries recording *why* the code is the way it is. Newest first.

## 2026-11-22 — Postgres test harness must fail loudly when the URL is set

**Scope:** minor. `test_support.rs` in every ported crate.

**Decision:** `pg_test_db()` skips tests only when `TEST_POSTGRES_URL` is
*unset*. When it is set, connect/reset/migration failures panic so the test
fails.

**Why:** a silent skip (`Option`, `?` on errors) turns a typo'd URL into a
green run that tested nothing — the harness would defeat its own purpose. Found
by two independent review lanes.

## 2026-11-22 — moltis-auth is part of the database port

**Scope:** major. New port lane for `crates/auth` (81 query sites).

**Decision:** treat `moltis-auth` as a first-class store crate in the port.

**Why:** it was missed in the initial lane partition because it has no
`migrations/` directory — its tables live in `crates/gateway/migrations`. The
workspace compile surfaced it. Lesson recorded: partition by *query sites*, not
by migration directories.

## 2026-10-24 — Database backend abstraction (crates/db) instead of per-crate cfg duplication

**Scope:** major. New crate `moltis-db`, `crates/memory`, `crates/gateway`.

**Context:** Goal is PostgreSQL support in place of SQLite across moltis. The
codebase has ~420 `sqlx::query` call sites and 9 migration directories.

**Considered:**
1. `#[cfg(feature = "postgres")]` duplicate query code per crate — 2x code, drift.
2. `sqlx::Any` driver — unifies types but still needs placeholder rewriting and
   makes migrations/typed decoding awkward.
3. A thin abstraction crate: keep SQL text in SQLite form, rewrite `?` → `$n` at
   execution time, and route each query through macros that expand one branch
   per backend.

**Decision:** (3). `moltis_db::{fetch_all, fetch_optional, fetch_one, execute,
scalar}` plus `tx_*` variants take the same arguments as plain sqlx, so a call
site changes shape but not meaning:
`sqlx::query_as(...).bind(x).fetch_optional(pool)` →
`moltis_db::fetch_optional!(&db, T, sql, x)`.

**Why:** the macros keep sqlx's native encoding per backend (no custom param
enum), so type safety is unchanged, while one query string serves both dialects.
The generic-over-`Database` alternative does not work because bind closures and
`Row` are associated-type-bound to a single backend.

**Consequences:** every ported crate's queries must use `?` placeholders,
`BIGINT`/`BOOLEAN` in the PostgreSQL DDL, and `::float8` casts where PostgreSQL
would otherwise return `float4` (e.g. `ts_rank`).

## 2026-10-24 — PostgreSQL is always compiled in, no `postgres` cargo feature

**Scope:** major. `Cargo.toml`, `crates/gateway`.

**Considered:** gating the backend behind a `postgres` feature (as the original
plan proposed) versus always compiling the driver.

**Decision:** always compile it. `sqlx-postgres` is pure Rust and does not link
`libpq`, so the original concern (a native build dependency) does not apply.

**Why:** a feature gate would let a user configure `backend = "postgres"` in a
binary that silently falls back to SQLite — writing memory to the wrong place.
Always-on removes that failure mode.

## 2026-10-24 — Memory backend uses an untyped pgvector column (no HNSW yet)

**Scope:** minor. `crates/memory/migrations_pg`.

**Considered:** `vector(N)` + HNSW index with a dimension recorded in a meta
table and a mandatory re-index on model change; versus untyped `vector` with a
sequential scan ordered by `<=>`.

**Decision:** untyped `vector`, sequential scan. Similarity search still runs in
the database (`ORDER BY embedding <=> ?`), which is the main win over loading
every BLOB into Rust.

**Why:** any embedding dimension works without re-index bookkeeping, and no
migration-alter path is needed. HNSW is a later optimization (see `todo.md`).

## 2026-10-24 — PostgreSQL integration tests are env-gated, not `#[ignore]`

**Scope:** minor. `crates/db/tests`, `crates/memory/src/test_support.rs`.

**Decision:** tests run only when `TEST_POSTGRES_URL` is set, and skip by
returning early (not `#[ignore]`). CI provides a `pgvector/pgvector:pg17`
service container so they execute there. Tests that share one database take a
process-wide `tokio::sync::Mutex`.

**Why:** `cargo test` must work on machines without Docker or PostgreSQL, and
`#[ignore]`d tests are rarely run manually. The mutex avoids reset-schema races
between parallel test threads.
