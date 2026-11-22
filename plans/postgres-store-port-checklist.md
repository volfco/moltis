# PostgreSQL Store Port Checklist

Port one SQL-backed crate from SQLite-only to the dual-backend `moltis-db`
abstraction. This checklist is the contract for every port lane; the goal is
that a deployment runs on **one** database engine (SQLite or PostgreSQL), with
no cross-engine mixing and no silent fallback.

## Reference implementations (read these first)

| What | Where |
|------|-------|
| Query macros + type invariants | `crates/db/src/macros.rs` |
| Placeholder rewriting (`?` → `$n`) | `crates/db/src/dialect.rs` |
| `Db` / `DbTx` / `run_migrations` | `crates/db/src/pool.rs` |
| Completed store port (both dialects) | `crates/memory/src/store_db.rs` |
| Postgres DDL translation example | `crates/memory/migrations_pg/20240205100004_init.sql` |
| Env-gated Postgres test helper | `crates/memory/src/test_support.rs` |
| Cross-backend test pattern | `crates/db/tests/backends.rs` |

## Steps per crate

1. Add `moltis-db = { workspace = true }` to the crate's `[dependencies]`
   (and `[dev-dependencies]` only if tests need it and the lib does not).
2. `run_migrations`: change `pub async fn run_migrations(pool: &sqlx::SqlitePool)`
   to take `&moltis_db::Db` and call
   `moltis_db::run_migrations(db, sqlx::migrate!("./migrations"), sqlx::migrate!("./migrations_pg"))`.
3. Store types: hold `moltis_db::Db` (owned, it is cheap to clone) instead of
   `sqlx::SqlitePool`. Constructors take `moltis_db::Db`. Keep every other
   public name and signature identical — only the parameter type changes.
   Rename `*_sqlite` modules/types to `*_db` where the name would otherwise lie.
4. Rewrite every `sqlx::query*` call to the `moltis_db` macros
   (`fetch_all!`, `fetch_optional!`, `fetch_one!`, `execute!`, `scalar!`, and
   `tx_*` variants inside transactions). Keep `?` placeholders and the same
   binds. Bind by reference where the value is behind a borrow
   (`&row.field`, `row.field.as_deref()` for `Option<Vec<u8>>`).
5. Create `migrations_pg/` mirroring each `migrations/*.sql` with PostgreSQL
   DDL. Same migration file names (sqlx versions them by name).
6. Tests: keep every existing SQLite test green, then mirror the store tests as
   Postgres tests gated on `TEST_POSTGRES_URL` (return early with an
   `eprintln!("skipping: …")` when unset). Serialize them with a crate-local
   `tokio::sync::Mutex` and reset the schema first (see
   `crates/memory/src/test_support.rs`): dropping tables alone is not enough —
   `_sqlx_migrations` must go too or the migrator skips everything.

## DDL mapping (SQLite → PostgreSQL)

| SQLite | PostgreSQL | Rust type stays |
|--------|-----------|-----------------|
| `INTEGER` | `BIGINT` | `i64` |
| `INTEGER PRIMARY KEY AUTOINCREMENT` | `BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY` | `i64` |
| `TEXT` | `TEXT` | `String` |
| `BLOB` | `BYTEA` | `Vec<u8>` |
| `REAL` | `DOUBLE PRECISION` | `f64` |
| flag column read/written as `bool` | `BOOLEAN` | `bool` |
| `TEXT` timestamp `DEFAULT (datetime('now'))` | `DEFAULT (to_char(now() AT TIME ZONE 'utc', 'YYYY-MM-DD HH24:MI:SS'))` | `String` |
| epoch-millis `INTEGER` | `BIGINT` | `i64` |
| `CREATE VIRTUAL TABLE … USING fts5(…, content=…)` + sync triggers | generated `tsvector` column + `GIN` index; drop the triggers | — |
| `INSERT OR IGNORE` | `INSERT … ON CONFLICT … DO NOTHING` | — |
| `INSERT OR REPLACE` | `INSERT … ON CONFLICT … DO UPDATE SET …` | — |

Keep `FOREIGN KEY … ON DELETE CASCADE`, unique constraints and indexes
(identical names are fine). `AUTOINCREMENT` has no equivalent need in Postgres
(`IDENTITY` is enough).

## Query rewrite rules

- Placeholders stay `?`; `moltis-db` rewrites them to `$1…$n` per call.
- One bind per placeholder, in textual order.
- Type-check traps on Postgres: aggregate/float expressions come back as
  `float4` unless you cast `::float8` (e.g. `ts_rank(...)`); decode into `f64`.
- SQLite-only SQL (`datetime('now')`, `julianday`, `rowid`, `GLOB`, `PRAGMA`,
  `strftime`) needs a Postgres equivalent — use a small dialect match (see
  `EmbeddingSql` in `crates/memory/src/store_db.rs`) or move the computation
  into Rust where that is simpler.
- Never hand-roll parameter numbering; never interpolate user values into SQL.

## Boundaries

- Touch only your assigned crate's files (including its `Cargo.toml`,
  `migrations/`, `migrations_pg/`, tests).
- Do **not** touch `crates/gateway`, `crates/cli`, `crates/config`, the root
  `Cargo.toml`, or any other crate. Callers that break on your parameter-type
  change are fixed in a later integration step — that is expected, not a bug.
- No SQLite fallback paths, no feature flags gating Postgres.

## Validation (must pass before you report)

```bash
cargo fmt                       # pinned nightly via rust-toolchain.toml
cargo check -p <crate> --all-targets
cargo test -p <crate>           # SQLite, existing tests still green
TEST_POSTGRES_URL=<your-url> cargo test -p <crate>   # Postgres, run per crate
```

## Commit and report

- Commit in your worktree with a conventional message
  (`feat(<crate>): port store to moltis-db dual backend`) and a body naming the
  non-obvious DDL/query decisions. No `Co-Authored-By`, no assistant links.
- Report: files changed, query sites converted (count), DDL decisions,
  test results for both engines, and anything intentionally left undone.
