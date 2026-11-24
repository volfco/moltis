# Follow-ups

Short summaries only; details live in `continue.md`, `decisions.md`, and the
tracker (`bd ready`).

## Test suite findings (full workspace run, one crate per process, 2026-11-22)

- [ ] `crates/skills/src/clawhub.rs:1334` `live_scan_returns_security_data` fails
      reproducibly (2/2): it hits the real ClawHub API and asserts
      `scanners.vt.is_some()`, but the live payload no longer carries
      VirusTotal results. Network errors are tolerated by the test, an
      API-shape change is not. Either tolerate a missing `vt` field or gate
      the test behind an env flag like the other live tests.
- [ ] `crates/swift-bridge/src/ffi_core.rs:883`
      `chat_stream_sends_error_for_no_provider` is load-flaky: it passed 3/3
      in isolation but failed inside the full-crate run, where it races its
      own stream-event timeout against parallel tests. Await the event with a
      deterministic signal instead of a fixed budget.

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

- [ ] Cold-load e2e coverage exists only for the Tools section
      (`e2e/specs/settings-cold-load.spec.js`). The import/voice/phone/
      instrumentation surfaces were migrated to `sendRpcWhenReady` but have no
      deterministic cold-load spec; the identity fallback chain
      (`agents.identity.get` -> `agent.identity.get`) is unasserted.
- [ ] `VoiceSection` mixes a plain `sendRpc` provider fetch with
      `sendRpcWhenReady` in the same `Promise.all`; pick one style per load
      (behavior-identical today because the effect is gated on `connected`).

## Follow-ups from the integration wave

- [ ] Settings UI for the `[database]` engine (config-file only today; the
      Memory settings page no longer offers it).
- [ ] `moltis db reset` for PostgreSQL behind a moltis-ownership marker (today
      it refuses and tells the operator to drop/recreate the database).
- [ ] Clean up the inert `cron/runs/` directory left by the old JSON store
      once the `jobs.json.bak` import has aged.
- [ ] `moltis db reset` (SQLite) leaves `code-index/index.db` behind despite
      "Delete all database files completely" (pre-existing drift).
- [ ] Portable export/import archives the SQLite files only; a PostgreSQL
      deployment gets an empty DB section (pg_dump is the Postgres backup
      story — document it, or add a pg_dump hook).

## Memory backend (hardening landed; residual gaps)

- [ ] A skipped sync (another instance holds the advisory lock) is never
      retried on the headless profile: `init_memory.rs` runs one pass and the
      periodic loop only starts for non-headless profiles, so a headless
      gateway can end up with zero indexing for its whole lifetime.
- [ ] `server.db_pool_max_connections` has no minimum, but a Postgres memory
      sync pins one connection for the lock lifetime — a pool of 1 deadlocks
      the pass. Validate `>= 2` for `backend = "postgres"` (documented only).
- [ ] `sync_path`/`remove_path` (watcher and memory-write paths) take only the
      in-process semaphore; a second instance's full pass can reconcile away
      rows they are upserting. Commented as a known scope boundary.
- [ ] `migrations_pg/20240205100004_init.sql` still carries the now-stale
      "add a dimension and an HNSW index when sub-linear search is needed"
      comment; applied migrations must not be edited (sqlx checksums), so the
      correction belongs in the module docs.

## Pairing (post-hardening follow-ups)

- [ ] `device_tokens.expires_at` is declared in the migration but never
      written or checked — device tokens are immortal until revoked. Enforce a
      lifetime or drop the column to remove the false promise.
- [ ] Out-of-contract timestamp formats diverge between the Rust expiry
      fallback (second-truncated) and the SQL predicates (fractional-aware):
      a fractional legacy `'YYYY-MM-DD HH:MM:SS.5'` can be pending in Rust and
      expired in SQL. Malformed values are deliberately fail-closed.
- [ ] `approve`'s device upsert uses `ON CONFLICT(device_id) DO UPDATE` and
      rewrites a pinned public key / un-revokes; reachable only from the
      operator-gated RPC path (trust anchor), so left as-is deliberately.
