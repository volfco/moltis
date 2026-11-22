# Follow-ups

Short summaries only; details live in `continue.md`, `decisions.md`, and the
tracker (`bd ready`).

## Postgres test hygiene (verified 2026-03: workspace compiles; SqlitePool
## call-site inventory and pg_test_db error-swallowing fix are done)

- [ ] Cross-crate Postgres test races: per-process mutexes (`pg_lock`) cannot
      protect a shared database from `cargo test --workspace`; keep suites
      per-crate and document it, or give each crate's harness a database-level
      advisory lock.
- [ ] webhooks `tests/ingress_integration.rs` stays SQLite-only (store layer is
      mirrored; the ingress pipeline is engine-independent). Known gap.

## PostgreSQL backend rollout (port complete; remaining tooling)

- [ ] Add `moltis memory import-from-sqlite` so migration does not require a full
      re-embed (embeddings are portable; only the vector literal format differs).

## Settings pages

- [ ] Other `settings/sections/*` components fetch config in a `useEffect` with
      no WebSocket retry, so a cold page load can render defaults instead of the
      saved config (found via `MemorySection`, fixed there). Consider a shared
      `sendRpcWhenReady()` helper in `helpers.ts`.

## Follow-ups from the integration wave

- [ ] Settings UI for the `[database]` engine (config-file only today; the
      Memory settings page no longer offers it).
- [ ] `moltis db reset` for PostgreSQL behind a moltis-ownership marker (today
      it refuses and tells the operator to drop/recreate the database).
- [ ] Decompose `crates/gateway/src/pairing.rs` (1,526 lines, allowlisted in
      check-file-size.sh — grew past the 1,500 limit during the port).
- [ ] Clean up the inert `cron/runs/` directory left by the old JSON store
      once the `jobs.json.bak` import has aged.
- [ ] `moltis db reset` (SQLite) leaves `code-index/index.db` behind despite
      "Delete all database files completely" (pre-existing drift).
- [ ] Portable export/import archives the SQLite files only; a PostgreSQL
      deployment gets an empty DB section (pg_dump is the Postgres backup
      story — document it, or add a pg_dump hook).

## Memory backend hardening

- [ ] HNSW: `vector(N)` column + dimension recorded in a `memory_meta` table,
      explicit re-index on provider change (see `plans/postgres-pgvector-memory-backend.md`).
- [ ] Prefer a session-level advisory lock or `pg`-side job guard if several
      instances sync the same memory directory concurrently.
- [ ] Web UI: surface pgvector availability (probe `SELECT 1 FROM pg_extension`)
      so a missing extension shows an actionable message instead of a migration error.
