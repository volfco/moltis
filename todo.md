# Follow-ups

Short summaries only; details live in `continue.md`, `decisions.md`, and the
tracker (`bd ready`).

## Integration step (before this branch reaches main) — see continue.md for the executable spec

Call sites still passing `sqlx::SqlitePool` where `moltis_db::Db` is now required
(inventory from the ports-a-small review; workspace does not compile until done):

- crates/gateway/src/server/prepare_core.rs:469,475,478 (production startup)
- crates/gateway/src/session/title.rs:211
- crates/gateway/src/session/tests.rs:944
- crates/gateway/src/webhooks.rs:536
- crates/cli/src/db_commands.rs:191,362,364,423-432
- crates/swift-bridge/src/state.rs:79

Accepted review fixes to apply consistently across all ported crates:

- [ ] test_support `pg_test_db()` must not swallow connect errors: when
      TEST_POSTGRES_URL is set but unreachable, tests currently pass vacuously
      (`.ok()?` treats a typo'd URL as "skip"). Fail loudly instead.
- [ ] Cross-crate Postgres test races: per-process mutexes cannot protect a
      shared database from `cargo test --workspace`; keep suites per-crate and
      document it, or give each crate's harness a database-level advisory lock.
- [ ] webhooks `tests/ingress_integration.rs` stays SQLite-only (store layer is
      mirrored; the ingress pipeline is engine-independent). Known gap.

## PostgreSQL backend rollout (in progress)

- [ ] Port `crates/projects` (6 queries) to `moltis_db` macros + `migrations_pg/`.
- [ ] Port `crates/cron` (8), `crates/webhooks` (6), `crates/vault` (11).
- [ ] Port `crates/connectors` (8 + store), `crates/sessions` (~40), `crates/code-index` (8).
- [ ] Port `crates/gateway` (~110 queries: auth, pairing, channels, message_log).
- [ ] Add `[database] { url, max_connections }` and open the pool once at startup,
      once more than `moltis memory` can use PostgreSQL.
- [ ] Add `moltis memory import-from-sqlite` so migration does not require a full
      re-embed (embeddings are portable; only the vector literal format differs).

## Settings pages

- [ ] Other `settings/sections/*` components fetch config in a `useEffect` with
      no WebSocket retry, so a cold page load can render defaults instead of the
      saved config (found via `MemorySection`, fixed there). Consider a shared
      `sendRpcWhenReady()` helper in `helpers.ts`.

## Memory backend hardening

- [ ] HNSW: `vector(N)` column + dimension recorded in a `memory_meta` table,
      explicit re-index on provider change (see `plans/postgres-pgvector-memory-backend.md`).
- [ ] Prefer a session-level advisory lock or `pg`-side job guard if several
      instances sync the same memory directory concurrently.
- [ ] Web UI: surface pgvector availability (probe `SELECT 1 FROM pg_extension`)
      so a missing extension shows an actionable message instead of a migration error.
