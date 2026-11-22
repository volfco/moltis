# Continue Here

Forward-looking state only. Remove entries as they are completed.

## Where the docs are

- `CLAUDE.md` / `AGENTS.md` — engineering rules (tests, style, commits, beads).
- `decisions.md` — why PostgreSQL support is shaped the way it is.
- `docs/src/memory.md` — user-facing memory backends (built-in, zvec, postgres, qmd).
- `plans/postgres-pgvector-memory-backend.md` — original plan; its "feature-gate
  the backend" and "HNSW index" sections are superseded (see `decisions.md`).

## Current state

PostgreSQL is a supported backend for the **memory index only**:

- `crates/db` — backend abstraction (`Db`, `DbTx`, `moltis_db::*` query macros).
- `crates/memory` — one `MemoryStore` (`store_db::DbMemoryStore`) serves SQLite
  and PostgreSQL; `migrations_pg/` holds the pgvector schema.
- Config: `[memory] backend = "postgres"` + `database_url`.
- Everything else (sessions, cron, projects, vault, webhooks, connectors,
  code-index, gateway auth) still opens `moltis.db` as SQLite.

## Next items

1. Port the remaining stores to `moltis_db` macros, one crate per PR, smallest
   first: `projects` (6 queries) → `cron` (8) → `webhooks` (6) → `vault` (11) →
   `connectors` → `sessions` (~40) → `gateway` (~110) → `code-index`.
   Each needs a `migrations_pg/` directory translated from `migrations/`
   (`AUTOINCREMENT` → `GENERATED … AS IDENTITY` or a `BIGINT` PK, `BLOB` →
   `BYTEA`, `INTEGER` → `BIGINT`, `CREATE VIRTUAL TABLE … fts5` → generated
   `tsvector` + GIN, FTS triggers dropped).
2. Add a shared `[database]` config section (`url`, `max_connections`) once more
   than the memory store can use PostgreSQL, and open the pool once at startup.
3. Optional: HNSW index for the memory backend (`vector(N)` + dimension in a
   meta table + explicit re-index on model change).
4. Optional: `moltis memory` subcommands that copy an SQLite index into
   PostgreSQL instead of relying on a full re-sync.

## Verification recipes

```bash
# PostgreSQL-backed tests (skipped when the URL is unset)
docker run -d --name moltis-pg -e POSTGRES_PASSWORD=test -e POSTGRES_DB=moltis \
  -p 127.0.0.1:55433:5432 pgvector/pgvector:pg17
TEST_POSTGRES_URL=postgres://postgres:test@127.0.0.1:55433/moltis \
  cargo test -p moltis-db -p moltis-memory
```
