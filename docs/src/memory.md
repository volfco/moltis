# Memory System

Moltis provides a powerful memory system that enables the agent to recall past conversations, notes, and context across sessions. This document explains the available backends, features, and configuration options.

If you are trying to understand the difference between short-term session
state, long-term memory files, and sandbox persistence, start with
[Memory Surfaces](memory-surfaces.md).

## Backends

Moltis supports three memory backends:

| Feature | Built-in | Zvec | QMD |
|---------|----------|------|-----|
| **Storage** | Database engine (SQLite `memory.db` or PostgreSQL) | Zvec collection directory | QMD index |
| **Search Type** | Hybrid (vector + FTS keyword) | Hybrid (HNSW vector + native FTS keyword) | Hybrid (BM25 + vector + LLM reranking) |
| **Vector Search** | Brute force in Rust (SQLite) or `ORDER BY embedding <=> query` (PostgreSQL) | HNSW index | HNSW |
| **Keyword Search** | FTS5 (SQLite) or `tsvector` + GIN (PostgreSQL) | Native FTS | BM25 |
| **Local Embeddings** | GGUF models via llama-cpp-2 | GGUF models via llama-cpp-2 | GGUF models |
| **Remote Embeddings** | OpenAI, Ollama, custom endpoints | OpenAI, Ollama, custom endpoints | Built-in |
| **Embedding Cache** | Database table with LRU eviction | Redb persistent K-V store | Built-in |
| **Batch API** | OpenAI batch (50% cost saving) | No | No |
| **Circuit Breaker** | Fallback chain with auto-recovery | Fallback chain with auto-recovery | No |
| **LLM Reranking** | Optional (configurable) | Optional (configurable) | Built-in with `query` command |
| **File Watching** | Real-time sync via notify | Real-time sync via notify | Built-in |
| **External Dependency** | None beyond the chosen database engine | Requires zvec C library (v0.5.0) | Requires QMD binary (Node.js/Bun) |
| **Offline Support** | Yes (with local embeddings) | Yes (with local embeddings) | Yes |

### Built-in Backend

The default backend uses SQLite for storage with FTS5 for keyword search and optional vector embeddings for semantic search. Key advantages:

- **Zero external dependencies**: Everything is embedded in the moltis binary
- **Fallback chain**: Automatically switches between embedding providers if one fails
- **Batch embedding**: Reduces OpenAI API costs by 50% for large sync operations
- **Embedding cache**: Avoids re-embedding unchanged content

### QMD Backend

QMD is an optional external sidecar that provides enhanced search capabilities:

- **BM25 keyword search**: Fast, instant results (similar to Elasticsearch)
- **Vector search**: Semantic similarity using local GGUF models
- **Hybrid search with LLM reranking**: Combines both methods with an LLM pass for optimal relevance

To use QMD:
1. Install the QMD CLI from [github.com/tobi/qmd](https://github.com/tobi/qmd): `npm install -g @tobilu/qmd` or `bun install -g @tobilu/qmd`
2. Verify the binary is on your `PATH`: `qmd --version`
3. Enable it in Settings > Memory > Backend

Moltis invokes the `qmd` CLI directly for indexing and search, so the memory
backend does not require a separate background daemon.

### Zvec Backend

Zvec is a native vector database backend that uses HNSW indexing for vector
similarity and native FTS for keyword search. It replaces SQLite as the storage
layer while keeping the same embedding provider chain, LLM reranking, and file
watching pipeline as the built-in backend.

**When to use zvec:**
- You have a large memory corpus and need faster vector search than brute-force
- You want persistent, zero-copy vector storage (collection directory on disk)
- You are comfortable building the zvec C library from source

**When to stick with built-in (SQLite):**
- You prefer a pure-Rust build with no C library dependencies
- Your memory corpus is small enough that brute-force search is fast
- You want the simplest possible setup with zero external build steps

**Setup:**

The Docker image and default source build include zvec. Standalone release
packages use the self-contained `portable` feature set and currently omit zvec
because its native runtime is not available for every supported target.

1. Build moltis with the `zvec` feature flag:
   ```bash
   cargo build --release --features zvec
   ```

2. Configure the backend in `moltis.toml`:
   ```toml
   [memory]
   backend = "zvec"
   db_path = "memory.zvec"     # collection directory (created if it doesn't exist)
   vector_weight = 0.7         # optional, default 0.7
   keyword_weight = 0.3        # optional, default 0.3
   search_merge_strategy = "rrf"   # "rrf" or "linear"
   ```

3. The zvec C library is built automatically from source by the `zvec-sys`
   crate's Cargo build script when you compile moltis with the `zvec` feature.


The `db_path` is resolved relative to Moltis' data directory
(`~/.moltis/memory.zvec`). The directory is created on first run with the
collection schema (chunk fields, FTS index, HNSW vector index with 768
dimensions).

**Required sidecar file:** zvec also creates a redb database next to the
collection directory at `<db_path>.cache` (e.g. `memory.zvec.cache`). This file
is **not a disposable cache** — it stores the embedding cache, the file
metadata index, and the per-path chunk index that `list_files`,
`get_file`, and `get_chunks_for_file` rely on. Treat the collection directory
*and* the `.cache` file as a unit: back them up together, and do not delete the
`.cache` file. If it is removed, the vectors remain in the collection but the
backend can no longer enumerate files or look up chunks by path until you
re-index.

### PostgreSQL as the database engine

The built-in backend stores its index in whichever database engine
`[database]` selects — SQLite (the default, zero-dependency) or PostgreSQL
with the [pgvector](https://github.com/pgvector/pgvector) extension. This is
a whole-deployment choice: every SQL-backed store (projects, sessions, cron,
vault, connectors, code-index, auth, memory) uses the same engine. A
deployment runs PostgreSQL or SQLite, never a mix, and there is no fallback:
if PostgreSQL is configured but unreachable, startup aborts with an error.

**When to choose PostgreSQL:**

- Your memory corpus is large enough that loading every embedding into the
  process for brute-force search is too slow
- Several moltis instances should share one database
- You already operate a PostgreSQL server

**When to stick with SQLite:**

- You want zero operational overhead and no external services
- Your data is small (a few thousand chunks or fewer)
- You want the simplest possible backup story (a few files on disk)

**Setup:**

1. Start a PostgreSQL server with pgvector available:
   ```bash
   docker run -d --name moltis-pg \
     -e POSTGRES_PASSWORD=moltis -e POSTGRES_DB=moltis \
     -p 5432:5432 pgvector/pgvector:pg17
   ```

2. Point moltis at it in `moltis.toml` (use a dedicated database):
   ```toml
   [database]
   backend = "postgres"
   url = "postgres://postgres:moltis@localhost/moltis"
   ```

3. Restart moltis. Migrations run automatically and create the `vector`
   extension, all store tables, and the GIN index used for memory keyword
   search. In SQLite mode the files stay as today (`moltis.db`, `memory.db`,
   `connectors.db`, …); in PostgreSQL mode everything lives in the one
   database.

Notes:

- The `vector` extension is created automatically when the connecting role
  may create extensions (e.g. the `postgres` superuser). Otherwise startup
  aborts before the memory migrations with the exact fix instead of a raw
  migration error: `pgvector extension is missing in PostgreSQL database
  'moltis': run as superuser: CREATE EXTENSION vector;`

- The Memory settings page shows a pgvector banner (`pgvector: available`, or
  the action from the `memory.status` RPC), and the `memory.status` RPC reports
  the same as `pgvector: { "available": bool, "action": "CREATE EXTENSION
  vector;" }` (omitted on SQLite). The probe runs once when the memory store is
  opened and the result is cached on it, so the banner is a confirmation rather
  than a fresh check: startup already blocks the missing-extension case (see
  above), which makes the banner belt-and-braces for a database whose extension
  was dropped while moltis was running.
- The `embedding` column is typed `vector(N)` with an HNSW index (cosine),
  where `N` comes from the recorded embedding identity. A dimension change is
  handled by the re-index pass, which relaxes the column and index first,
  re-embeds, then re-applies `vector(N)`. Until that pass runs, similarity
  search falls back to a sequential scan ordered by `<=>`. See
  [Re-indexing embeddings](#re-indexing-embeddings).
- Keyword search uses `websearch_to_tsquery`, which tolerates arbitrary user
  text (quoted phrases, `OR`, leading `-`) without syntax errors.
- `database.url` holds credentials: it is never returned by the settings API.
- `moltis db reset` deletes database *files* and is SQLite-only; on
  PostgreSQL, drop and recreate the database instead.

To move an existing SQLite memory index to PostgreSQL, stop moltis, switch
`[database]`, and start again — the memory index is derived data and re-syncs
from your memory files on the first sync (`moltis memory status` shows it).

### Several instances on one database

The sync lock is per database, not per process: every instance pointing at the
same PostgreSQL `database.url` shares one advisory lock key. A full sync pass
that starts while another instance is already syncing skips instead of
interleaving — it logs `memory: initial sync skipped; another instance holds
the memory sync lock` (or the `periodic sync skipped` variant) and does no
work at all, including no re-index of a shared qmd index. Skipped passes are
counted in `moltis_memory_sync_lock_skips_total`. The next pass tries again; a
skipping instance converges as soon as the holder finishes.

The lock holds one pooled connection for the length of the pass, so a
PostgreSQL pool of one connection deadlocks a sync that also needs the pool:
keep `server.db_pool_max_connections` at 2 or more.

Single-file updates (`sync_path`/`remove_path`, e.g. from the file watcher)
are serialized only inside one process; the advisory lock covers full passes
(`sync` and `reindex`, including their re-embedding work).

Skipping is a normal, non-retrying outcome for the headless startup profile
(`--no-background-tasks`): it syncs once at startup, so an instance that starts
while another one is mid-pass stays at its last synced state until the next
start.

## Features

### Citations

Citations append source file and line number information to search results:

```
Some important content from your notes.

Source: memory/notes.md#42
```

**Configuration options:**
- `auto` (default): Include citations when results come from multiple files
- `on`: Always include citations
- `off`: Never include citations

### Session Export

Session transcripts can be exported into searchable memory on `/new` and
`/reset`. This allows the agent to remember past conversations even after
restarts.

Exported sessions are:
- Stored in `memory/sessions/` as markdown files
- Sanitized to remove sensitive tool results and system messages
- Automatically cleaned up based on age/count limits

### LLM Reranking

LLM reranking uses the configured language model to re-score and reorder search results based on semantic relevance to the query. This provides better results than keyword or vector matching alone, at the cost of additional latency.

**How it works:**
1. Initial search returns candidate results
2. LLM evaluates each result's relevance (0.0-1.0 score)
3. Results are reordered by combined score (70% LLM, 30% original)

## Configuration

Memory settings can be configured in `moltis.toml`:

```toml
[memory]
# Orchestration style: "hybrid", "prompt-only", "search-only", or "off"
style = "hybrid"

# Agent-authored write target policy: "hybrid", "prompt-only", "search-only", or "off"
agent_write_mode = "hybrid"

# Managed USER.md write policy: "explicit-and-auto", "explicit-only", or "off"
user_profile_write_mode = "explicit-and-auto"

# Backend: "builtin" (default) or "qmd"
backend = "builtin"

# Embedding provider for the built-in backend: "local", "ollama", "openai", "custom", or auto-detect
# Ignored while backend = "qmd", but preserved for switching back later
# Omit this field for the real default, which is auto-detect
provider = "auto"

# Disable RAG embeddings and force keyword-only search
disable_rag = false

# Embedding API base URL (host, /v1, or full /embeddings endpoint)
base_url = "http://localhost:11434/v1"

# Citation mode: "on", "off", or "auto"
citations = "auto"

# Enable LLM reranking for hybrid search
llm_reranking = false

# Merge vector and keyword results with "rrf" or "linear"
search_merge_strategy = "rrf"

# Export sessions to memory for cross-run recall: "on-new-or-reset" or "off"
session_export = "on-new-or-reset"

# QMD-specific settings (only used when backend = "qmd")
[memory.qmd]
command = "qmd"
max_results = 10
timeout_ms = 30000
```

Real defaults, if you leave the fields unset:

- `style = "hybrid"`
- `agent_write_mode = "hybrid"`
- `user_profile_write_mode = "explicit-and-auto"`
- `backend = "builtin"`
- `provider = auto-detect` (unset, not hardcoded `local`)
- `disable_rag = false`
- `citations = "auto"`
- `llm_reranking = false`
- `search_merge_strategy = "rrf"`
- `session_export = "on-new-or-reset"`
- `[chat].prompt_memory_mode = "live-reload"`

`style` is separate from `[chat].prompt_memory_mode`. Style controls whether
`MEMORY.md` is injected and whether memory tools are exposed. Prompt memory
mode controls whether prompt-visible `MEMORY.md` is live-reloaded or frozen
per session.

The web settings page exposes both knobs in the Memory section so you can
experiment without hand-editing `moltis.toml`.

`agent_write_mode` is a separate axis again. It controls where agent-authored
memory writes may land:

- `hybrid` allows both `MEMORY.md` and `memory/*.md`
- `prompt-only` allows only `MEMORY.md`
- `search-only` allows only `memory/*.md`
- `off` disables agent-authored memory mutations, including `memory_save`,
  `memory_forget`, `memory_delete`, and the silent pre-compaction memory flush

`user_profile_write_mode` is about the managed `USER.md` surface, not agent
memory files:

- `explicit-and-auto` mirrors explicit settings saves to `USER.md` and also
  allows silent browser/channel timezone or location capture
- `explicit-only` mirrors explicit settings saves to `USER.md`, but disables
  silent browser/channel capture
- `off` stops Moltis from writing `USER.md`; the canonical user profile remains
  in `moltis.toml [user]`

`citations` and `search_merge_strategy` are typed config enums too:

- `citations = "auto" | "on" | "off"`
- `search_merge_strategy = "rrf" | "linear"`

Interaction rules that matter in practice:

- `provider`, `base_url`, `model`, and `api_key` only apply to
  `backend = "builtin"`. QMD ignores them.
- `[chat].prompt_memory_mode` only matters when `style` still allows prompt
  memory, `hybrid` or `prompt-only`.
- `llm_reranking` is only meaningful when RAG is enabled. If
  `disable_rag = true`, memory falls back to keyword search.
- `session_export` exports transcripts into searchable memory files. It does
  not inject those transcripts into the prompt directly.

Or via the web UI: **Settings > Memory**

## Recipes

Common combinations:

| Goal | Settings |
|------|----------|
| Default everyday setup | `style = "hybrid"`, `backend = "builtin"`, `prompt_memory_mode = "live-reload"` |
| Deterministic prompt memory | `style = "hybrid"`, `prompt_memory_mode = "frozen-at-session-start"` |
| Search-only long-term memory | `style = "search-only"` |
| Prompt-only memory, no recall tools | `style = "prompt-only"` |
| Disable agent memory writes | `agent_write_mode = "off"` |
| Keep `USER.md` from silent enrichment | `user_profile_write_mode = "explicit-only"` |
| Keep user profile only in config | `user_profile_write_mode = "off"` |
| Zvec backend experiment | `backend = "zvec"`, `db_path = "memory.zvec"` |
| Put everything on PostgreSQL | `[database]` with `backend = "postgres"`, `url = "postgres://…"` |
| QMD backend experiment | `backend = "qmd"` |

## Embedding Providers

The built-in backend supports multiple embedding providers:

| Provider | Model | Dimensions | Notes |
|----------|-------|------------|-------|
| Local (GGUF) | EmbeddingGemma-300M | 768 | Offline, ~300MB download, 2048-token context |
| Ollama | nomic-embed-text | 768 | Requires Ollama running |
| OpenAI | text-embedding-3-small | 1536 | Requires API key |
| Custom | Configurable | Varies | OpenAI-compatible endpoint |

The system auto-detects available providers and creates a fallback chain:
1. Try configured provider first
2. Fall back to other available providers if it fails
3. Use keyword-only search if no embedding provider is available

The local provider reads the context capacity from the GGUF model and sizes a
single encoder batch for each input. Inputs longer than the model context are
truncated at the token boundary so an oversized line or query cannot terminate
the Moltis process.

## Re-indexing embeddings

Changing the embedding provider, model or dimension makes the vectors already
in the index useless: they live in the old provider's embedding space and
cannot be converted. Chunk *text* is the source of truth, so the fix is to
re-embed every chunk from its text.

The index records its embedding identity (provider key, model, dimensions) in
`memory_meta`. A sync pass compares that against the configured provider and
**re-indexes automatically** before it touches any chunk, so a provider or
dimension change needs no manual step — the next sync (startup, file change,
periodic pass) re-embeds the whole index once and rewrites the identity. Log
lines say `memory: re-indexing chunks after an embedding identity change` with
the reason and the dimension transition.

When the recorded identity still matches, nothing is re-embedded. Use the CLI
when you cannot wait for a sync, when there is no sync to trigger, or when the
stored vectors are suspected bad:

```bash
moltis memory reindex            # re-embed only if the identity changed
moltis memory reindex --force    # re-embed unconditionally
```

Both `[database]` engines are supported — SQLite rewrites the chunk vectors,
PostgreSQL additionally retypes the `embedding` column to `vector(N)` and
rebuilds the HNSW index as part of the run.

**When to use it:**

- You switched `memory.provider` or `memory.model` and do not want to wait for
  the next sync (a change the identity records is also detected automatically)
- The stored vectors are suspect while the config is unchanged, e.g. after a
  crash mid-write, a restored or copied database, or an embedding endpoint
  that started returning different vectors — use `--force`, because nothing in
  the recorded identity tells those cases apart from a healthy index
- You changed an embedding setting the identity does not cover (a new
  `memory.api_key`, for example) and want the cache and vectors refreshed

Output reports what happened, so a run is auditable:

```
Memory re-index (memory.db):
  Re-embedded:     1842 chunks
  Dimensions:      1536 -> 768
  Reason:          dimensions changed
```

When the identity already matches, the run says so and points at `--force`:

```
Memory re-index (memory.db):
  Nothing to re-embed: the recorded embedding identity already matches the configured provider.
  Re-run with --force to re-embed the chunks anyway.
```

If another moltis instance holds the memory sync lock for the same database
(see [Several instances on one database](#several-instances-on-one-database)),
the run does nothing and **exits non-zero** with
`re-index skipped: another moltis instance holds the memory sync lock …`, so a
cron-driven re-index never looks like it succeeded. Stop the other instance or
re-run it later.

The command needs an explicit `memory.provider`; the gateway's provider
auto-detection and fallback chain are not replicated on the CLI, so set the
provider (and `memory.model` where relevant) to what the index was built with.

The other `moltis memory` subcommands are `memory search <query>`,
`memory status`, and `memory migrate-collection` (zvec — copies chunks from one
collection to another with re-embedding; rename of the old
`memory reindex --from/--to`, which the CLI now refuses with a pointer to the
new name).

## Memory Directories

By default, moltis indexes markdown files from:

- `~/.moltis/MEMORY.md` - Main long-term memory file
- `~/.moltis/memory/*.md` - Additional memory files
- `~/.moltis/memory/sessions/*.md` - Exported session transcripts

Prompt injection from `MEMORY.md` is controlled separately via
`[chat].prompt_memory_mode`. Use `live-reload` to reread `MEMORY.md` before
each turn, or `frozen-at-session-start` to keep a stable prompt-memory
snapshot for the lifetime of a session.

If sandboxing is enabled with the default `workspace_mount = "ro"`, sandboxed
commands may still read mounted memory files, but they cannot modify them
directly. Durable memory mutations should use `memory_save`,
`memory_forget`, or `memory_delete` rather than shell redirection or direct
file editing inside the sandbox.

## Tools

The memory system exposes five agent tools:

### memory_search

Search memory with a natural language query. Returns relevant chunks ranked
by hybrid (vector + keyword) similarity.

```json
{
  "query": "what did we discuss about the API design?",
  "limit": 5
}
```

### memory_get

Retrieve a specific memory chunk by ID. Useful for reading the full text of a
result found via `memory_search`.

```json
{
  "chunk_id": "memory/notes.md:42"
}
```

### memory_save

Save content to long-term memory files. The agent uses this tool when you ask
it to remember something ("remember that I prefer dark mode") or when it
decides certain information is worth persisting. This is the preferred
long-term write path even when memory files are visible through a read-only
sandbox mount.

```json
{
  "content": "User prefers dark mode and Vim keybindings.",
  "file": "MEMORY.md",
  "append": true
}
```

Successful writes also return a `checkpointId`, so the change can be rolled
back with `checkpoint_restore`.

**Parameters:**

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `content` | string | *(required)* | The content to save |
| `file` | string | `MEMORY.md` | Target file: `MEMORY.md`, `memory.md`, or `memory/<name>.md` |
| `append` | boolean | `true` | Append to existing file (`true`) or overwrite (`false`) |

If `memory.agent_write_mode = "search-only"` and `file` is omitted,
`memory_save` defaults to `memory/notes.md`. The write mode can also reject
targets that are otherwise valid paths.

**Path validation:** The tool enforces a strict allowlist of write targets
to prevent path traversal attacks. Only these patterns are accepted:

- `MEMORY.md` or `memory.md` (root memory files)
- `memory/<name>.md` (files in the memory subdirectory, one level deep)

Absolute paths, `..` traversal, non-`.md` extensions, spaces in filenames,
and nested subdirectories (`memory/a/b.md`) are all rejected. Content is
limited to 50 KB per write.

**Auto-reindex:** After writing, the memory system automatically re-indexes
the affected file so the new content is immediately searchable via
`memory_search`.

### memory_forget

Forget saved memory using natural language. This tool searches memory, asks the
configured LLM to choose which chunk or chunks match the forget request, then
deletes the exact stored text through the same deterministic file mutation path
used by `memory_delete`.

```json
{
  "request": "Forget that I prefer dark mode",
  "dry_run": true
}
```

If the request is ambiguous, stale, or the exact text is not uniquely
removable, `memory_forget` returns a preview with `needs_confirmation = true`
instead of mutating files.

Successful mutations return `checkpointIds`, because forgetting multiple chunks
may touch more than one file.

**Parameters:**

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `request` | string | *(required)* | Natural-language description of what saved memory to forget |
| `dry_run` | boolean | `false` | Preview planned deletions without mutating files |
| `limit` | integer | `6` | Maximum number of candidate chunks inspected before planning |

Use `memory_forget` for normal "forget X" requests. Use `memory_delete` only
when you already know the exact file and exact snippet to remove.

### memory_delete

Forget saved memory by removing an exact snippet from a memory file or deleting
the whole file. This mutates the backing Markdown file, not just the index.

```json
{
  "file": "MEMORY.md",
  "text": "User prefers dark mode and Vim keybindings.\n"
}
```

To delete an entire memory note instead:

```json
{
  "file": "memory/obsolete-note.md",
  "delete_file": true
}
```

Successful deletes also return a `checkpointId`, so the previous file state can
be restored with `checkpoint_restore`.

**Parameters:**

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `file` | string | *(required)* | Target file: `MEMORY.md`, `memory.md`, or `memory/<name>.md` |
| `text` | string | *(none)* | Exact text snippet to remove. Required unless `delete_file = true` |
| `delete_file` | boolean | `false` | Delete the whole file instead of removing exact text |
| `all_matches` | boolean | `false` | Remove every exact match of `text` instead of only the first |
| `delete_if_empty` | boolean | `true` | Delete the file if removing `text` leaves only whitespace |

`memory_delete` uses the same path validation rules as `memory_save`, updates
the search index immediately, and can clean up stale index entries when a file
is removed. It is the low-level exact-delete primitive that powers
`memory_forget`.

## Silent Memory Turn (Pre-Compaction Flush)

Before compacting a session (summarizing old messages to free context window
space), Moltis runs a **silent agentic turn** that reviews the conversation
and saves important information to memory files. This ensures durable memories
survive compaction.

**How it works:**

1. When a session approaches the model's context window limit, the gateway
   triggers compaction
2. Before summarizing, a hidden LLM turn runs with a special system prompt
   asking the agent to save noteworthy information
3. The agent writes to `MEMORY.md` and/or `memory/YYYY-MM-DD.md` using an
   internal `write_file` tool backed by the same `MemoryWriter` as
   `memory_save`
4. The LLM's response text is discarded (the user sees nothing)

This pre-compaction flush obeys `memory.agent_write_mode`. In `off` mode, the
flush is skipped entirely.
5. Written files are automatically re-indexed for future search

**What gets saved:**

- User preferences and working style
- Key decisions and their reasoning
- Project context, architecture choices, and conventions
- Important facts, names, dates, and relationships
- Technical setup details (tools, languages, frameworks)

This is the same approach used by OpenClaw. See the
[comparison page](memory-comparison.md) for a detailed analysis of both
systems.

## Architecture

```
┌──────────────────────────────────────────────────────────────────┐
│                       Memory Manager                             │
│               (implements MemoryWriter trait)                     │
├──────────────────────────────────────────────────────────────────┤
│                         Read Path                                │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────────────┐      │
│  │   Chunker   │  │   Search    │  │  Session Export     │      │
│  │ (markdown)  │  │  (hybrid)   │  │  (transcripts)      │      │
│  └─────────────┘  └─────────────┘  └─────────────────────┘      │
├──────────────────────────────────────────────────────────────────┤
│                        Write Path                                │
│  ┌─────────────────┐  ┌──────────────────┐  ┌────────────────┐  │
│  │ memory_save /   │  │  Silent Turn     │  │  Path          │  │
│  │ memory_delete   │  │  (pre-compact)   │  │  Validation    │  │
│  │  (agent tools)  │  │                  │  │                │  │
│  └─────────────────┘  └──────────────────┘  └────────────────┘  │
├──────────────────────────────────────────────────────────────────┤
│                      Storage Backend                             │
│  ┌────────────────────────┐  ┌────────────────────────┐         │
│  │   Built-in (SQLite)    │  │   QMD (sidecar)        │         │
│  │  - FTS5 keyword        │  │  - BM25 keyword        │         │
│  │  - Vector similarity   │  │  - Vector similarity   │         │
│  │  - Embedding cache     │  │  - LLM reranking       │         │
│  └────────────────────────┘  └────────────────────────┘         │
├──────────────────────────────────────────────────────────────────┤
│                    Embedding Providers                            │
│  ┌─────────┐  ┌─────────┐  ┌─────────┐  ┌───────────────┐      │
│  │  Local  │  │ Ollama  │  │ OpenAI  │  │ Batch/Fallback│      │
│  │  (GGUF) │  │         │  │         │  │               │      │
│  └─────────┘  └─────────┘  └─────────┘  └───────────────┘      │
└──────────────────────────────────────────────────────────────────┘
```

## Troubleshooting

### Memory not working

1. Check status in Settings > Memory
2. Ensure at least one embedding provider is available:
   - Local: Requires `local-embeddings` feature enabled at build
   - Ollama: Must be running at `localhost:11434`
   - OpenAI: Requires `OPENAI_API_KEY` environment variable

### Search returns no results

1. Check that memory files exist in the expected directories
2. Trigger a manual sync by restarting moltis
3. Check logs for sync errors

### Zvec not available

1. Ensure moltis was built with `--features zvec`
2. Verify the zvec C library built successfully (check for `libzvec_c_api.so` /
   `libzvec_c_api.dylib` / `zvec_c_api.dll` in the build output)
3. Check that `db_path` points to a writable directory
4. If the collection directory already exists but is corrupted, delete it and
   restart — moltis will recreate it on the next run

### PostgreSQL backend not available

1. Confirm `database.url` is set and the server is reachable
2. The pgvector extension is created automatically when the connecting role
   may create extensions; if startup fails with `pgvector extension is
   missing … run as superuser: CREATE EXTENSION vector;`, run that command as
   superuser and restart
3. Check the startup error: moltis aborts when the connection or a migration
   fails — it never silently falls back to SQLite

### QMD not available

1. Install QMD if needed: `npm install -g @tobilu/qmd` or `bun install -g @tobilu/qmd`
2. Verify QMD is installed: `qmd --version`
3. Check that the path is correct in settings
4. Ensure QMD can see its index and collections: `qmd status`
