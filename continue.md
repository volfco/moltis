# Continue Here

Forward-looking state only. Remove entries as they are completed.

## Where the docs are

- `CLAUDE.md` / `AGENTS.md` — engineering rules (tests, style, commits, beads).
- `decisions.md` — why the database/pairing/memory designs are shaped as they are.
- `docs/src/memory.md` — memory backends, multi-instance sync, pgvector, reindex.
- `docs/src/metrics-and-tracing.md` — metric catalog (memory sync/reindex rows).
- `plans/postgres-pgvector-memory-backend.md` — original plan; superseded where
  `decisions.md` differs (feature gate, untyped vector, HNSW timing).

## Current state

PostgreSQL is a supported backend for **every** SQL store, selected by
`[database] { backend = "sqlite" | "postgres", url }` with hard failure on
outage (no SQLite fallback, no mixed engines). Built on the `moltis-db`
dual-backend abstraction (`Db`, `DbTx`, `moltis_db::*` macros) with per-crate
`migrations_pg/` directories.

Landed on the memory hardening wave:

- `crates/memory` — embedding identity in `memory_meta`, automatic re-index on
  provider/dimension change (`reindex.rs`), lazy typed `vector(N)` + HNSW,
  batched re-embedding, `MemoryManager::reindex(force)`.
- `crates/memory/src/sync_lock.rs` — Postgres session advisory lock around full
  sync passes (skipped passes log and count; the guard is armed across every
  await so a cancelled sync can never pool a locked session).
- `crates/memory/src/pgvector.rs` — startup preflight that attempts
  `CREATE EXTENSION vector` and only fails when creation is denied; the same
  probe feeds `memory.status` and the Memory settings banner.
- `crates/gateway/src/pairing/` — `pairing.rs` (1,526 lines) decomposed into
  types/store/state/crypto/expiry/tests, with atomic approve/reject and token
  rotation (single-winner transitions), request-time input validation, and
  redacted `Debug` for bearer tokens.
- `crates/web/ui` — `sendRpcWhenReady()` in `helpers.ts` (waits for the
  `hello-ok` handshake, bounded by one RPC budget) adopted for settings
  cold-load fetches; `MemorySection` uses it too.
- `crates/cli` — `moltis memory reindex [--force]` drives the in-place
  re-embed; the zvec collection migration it used to collide with is now
  `moltis memory migrate-collection`.

## Next items

1. Open the PR from `feat/postgres-memory-backend` and run
   `./scripts/local-validate.sh <PR>` (the CI gate).
2. Playwright needs `LD_LIBRARY_PATH` pointing at zvec's prebuilt library on
   machines where it is not on the loader path, or `target/debug/moltis` fails
   with `libzvec_c_api.so: cannot open shared object file` and every webServer
   start exits 127:
   `export LD_LIBRARY_PATH=$PWD/target/debug/build/zvec-rust-sys-*/out/zvec-prebuilt`
3. Follow-ups live in `todo.md`; the largest are `moltis memory import-from-sqlite`,
   retrying a skipped sync on headless profiles, and the Settings UI for the
   `[database]` engine.

## Verification recipes

```bash
# PostgreSQL-backed tests: skipped when the URL is unset, but they must FAIL
# loudly when the URL is set and unreachable.
docker run -d --name moltis-pg -e POSTGRES_PASSWORD=test -e POSTGRES_DB=moltis \
  -p 127.0.0.1:55433:5432 pgvector/pgvector:pg17
TEST_POSTGRES_URL=postgres://postgres:test@127.0.0.1:55433/moltis_t_memory \
  cargo test -p moltis-memory
# Postgres suites run one crate per process (per-process locks share
# _sqlx_migrations); see todo.md's follow-ups.
```
