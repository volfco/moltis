# Continue Here

Forward-looking state only. Remove entries as they are completed.

## Where the docs are

- `CLAUDE.md` / `AGENTS.md` — engineering rules (tests, style, commits, beads).
- `decisions.md` — why PostgreSQL support is shaped the way it is.
- `docs/src/memory.md` — user-facing memory backends (built-in, zvec, postgres, qmd).
- `plans/postgres-pgvector-memory-backend.md` — original plan; its "feature-gate
  the backend" and "HNSW index" sections are superseded (see `decisions.md`).

## Current state

The PostgreSQL port is complete on `feat/postgres-memory-backend`: every
SQL-backed store (projects, sessions, cron, webhooks, vault, connectors,
code-index, auth, gateway, memory) runs on the `moltis-db` dual-backend
abstraction, and `[database]` selects the engine for the whole deployment
(`backend = "sqlite" | "postgres"`, `url`) with hard failure on outages — no
SQLite fallback, no mixed engines. Validated on both engines (per-crate test
suites + negative harness checks + E2E) and reviewed lane-by-lane; see
`decisions.md` for the design calls and `todo.md` for follow-ups.

PostgreSQL is a supported backend for the **memory index only**:

- `crates/db` — backend abstraction (`Db`, `DbTx`, `moltis_db::*` query macros).
- `crates/memory` — one `MemoryStore` (`store_db::DbMemoryStore`) serves SQLite
  and PostgreSQL; `migrations_pg/` holds the pgvector schema.
- Config: `[memory] backend = "postgres"` + `database_url`.
- Everything else (sessions, cron, projects, vault, webhooks, connectors,
  code-index, gateway auth) still opens `moltis.db` as SQLite.

## Next items

1. Open the PR from `feat/postgres-memory-backend` and run
   `./scripts/local-validate.sh <PR>` (the CI gate).
2. Follow-ups live in `todo.md` (Settings UI for the engine, postgres
   `db reset` with an ownership marker, pairing.rs decomposition, HNSW
   indexing, portable-export parity).

## Verification recipes

```bash
# PostgreSQL-backed tests (skipped when the URL is unset; must FAIL loudly
# when the URL is set but unreachable)
docker run -d --name moltis-pg -e POSTGRES_PASSWORD=test -e POSTGRES_DB=moltis \
  -p 127.0.0.1:55433:5432 pgvector/pgvector:pg17
TEST_POSTGRES_URL=postgres://postgres:test@127.0.0.1:55433/moltis_t_memory \
  cargo test -p moltis-memory
# Postgres suites run one crate per process (per-process locks share
# _sqlx_migrations); see todo.md's follow-ups.
```
