# Moltis vs OpenClaw: Analysis & Feature Roadmap

## Architecture Comparison

### OpenClaw (TypeScript)

OpenClaw is a multi-platform **messaging bridge** for AI agents. Its core is a
WebSocket gateway (`localhost:18789`) that connects communication surfaces
(WhatsApp via Baileys, Telegram via grammy, Discord, Slack, Signal, iMessage)
to LLM-backed agents. Key architectural pieces:

- **Gateway daemon** — validates JSON Schema frames, emits agent/chat/presence/
  health/heartbeat/cron events.
- **Messaging providers** — a single gateway manages all channel connections.
- **Clients** — control-plane clients (macOS SwiftUI app, CLI, web UI,
  automations) connect via WebSocket.
- **Nodes** — devices (macOS/iOS/Android/headless) connect with `role: node`,
  declaring capabilities (camera, canvas, screen recording, location).
- **Device pairing** — new devices require approval; local connects are
  auto-approved.
- **Protocol** — TypeBox schemas generate both JSON Schema and Swift models.

### Moltis (Rust)

Moltis has diverged from a pure messaging bridge into a **personal AI
development gateway**. It is organized as a Cargo workspace with ~22 crates
(~44K LOC Rust):

| Crate | LOC | Purpose |
|-------|-----|---------|
| gateway | 14K | Axum HTTP/WS server, auth, web UI |
| agents | 5.4K | LLM providers, runner, tool registry |
| tools | 3K | exec, sandbox, policy |
| cron | 2.1K | scheduled tasks |
| skills | 2.1K | Agent Skills standard |
| telegram | 2K | Telegram channel via teloxide |
| memory | 2K | embeddings + hybrid search |
| projects | 1.5K | project context, git worktrees |
| sessions | 1.5K | JSONL persistence |
| oauth | 1K | device flow, PKCE, token storage |

**Key design principles:**
- Trait-based extensibility (`LlmProvider`, `ChannelPlugin`, `AgentTool`)
- Async all the way (Tokio, no `block_on`)
- Streaming-first for all LLM calls
- `secrecy::Secret<String>` for all sensitive data
- Multi-layer tool policy (global / per-agent / per-provider / per-sender)

### Where Moltis is ahead

- **Provider breadth** — 8+ providers with priority-based registry, feature-flag
  gating, and native tool-calling detection.
- **Security** — argon2 passwords, WebAuthn passkeys, `secrecy::Secret<T>`,
  multi-layer tool approval.
- **Memory** — embeddings-based hybrid search (keyword + vector) in SQLite.
- **Projects** — git worktree isolation, CLAUDE.md/AGENTS.md context loading.
- **TLS** — auto-generated self-signed certificates.
- **Type safety** — Rust's ownership model, `Send + Sync` guarantees.

---

## Missing Features (vs OpenClaw)

### Tier 1 — High Impact

#### 1. MCP Tool Integration ✅ (complete)

Moltis acts as an **MCP client** — it spawns local MCP server processes
(via npm/uvx) or connects to remote servers over HTTP/SSE. Each server
exposes tools that the agent can call during conversations.

**Done:** `crates/mcp/` crate with stdio + SSE transport, client, tool bridge,
manager, registry. Web UI for adding/managing/editing tools. Health polling
(30s) with real-time UI updates, auto-restart on crash with exponential
backoff, tool bridge auto-sync into agent ToolRegistry.

#### 2. More Channel Plugins

OpenClaw has WhatsApp, Discord, Slack, Signal, iMessage. Moltis only has
Telegram. The `ChannelPlugin` trait is ready.

**Priority order:**
1. Discord (discord-rs or serenity crate)
2. Slack (slack-morphism crate)
3. WhatsApp (hardest — no clean Rust Baileys equivalent, may need bridge)
4. Signal (signal-cli bridge or libsignal FFI)

#### 3. Node/Device System

OpenClaw's device pairing with capability declarations enables mobile/desktop
integration. Devices expose camera, screen recording, location, canvas.

**What to build:**
- WebSocket `role: node` handling in the gateway.
- Capability negotiation during handshake.
- Node invoke/result RPC methods.
- Device token issuance and pairing approval flow.

#### 4. Group Chat Support

OpenClaw handles group conversations with mention-based activation. Moltis is
single-user focused.

**What to build:**
- Mention detection in channel plugins (`@botname` triggers).
- Group session routing (one session per group, not per user).
- Reply-to-message support for threaded conversations.

### Tier 2 — Medium Impact

#### 5. Multi-Agent Routing

OpenClaw routes messages to isolated agent workspaces. Moltis has a single
agent runner.

**What to build:**
- Agent registry with named agents, each with its own provider/prompt/tools.
- Routing rules (channel-based, keyword-based, explicit `/agent` command).
- Session isolation per agent workspace.

#### 6. Webhook & Event Triggers

OpenClaw supports webhooks and Gmail Pub/Sub for event-driven activation.

**What to build:**
- `POST /api/webhooks/:id` endpoint that triggers agent runs.
- Webhook registration API with secret validation (HMAC).
- Gmail Pub/Sub integration (push notifications to webhook).

#### 7. Media Transcription Pipeline

OpenClaw has voice note transcription hooks. Moltis has `crates/media` but it
appears minimal.

**What to build:**
- Whisper API integration for audio transcription.
- Auto-transcribe voice messages from channels before passing to agent.
- Image description via vision models for non-text media.

#### 8. Presence & Health Broadcasting

OpenClaw broadcasts presence status across connected clients.

**What to build:**
- Track connected clients with last-seen timestamps.
- Broadcast presence events on connect/disconnect.
- Health status for channel connections and provider availability.

### Tier 3 — Nice to Have

#### 9. Native Apps (SwiftUI/Android)

OpenClaw has macOS SwiftUI, iOS, Android clients. Moltis is web-only. The
WebSocket protocol is already capable enough to support native clients.

#### 10. Subscription/Billing Support

OpenClaw has subscription authentication for hosted deployments. Only relevant
if moltis becomes a hosted service.

---

## Rust-Native Feature Ideas

These are features that play to Rust's strengths and would be difficult or
inferior in TypeScript.

### 1. Parallel Tool Execution

When the LLM requests multiple independent tool calls in a single turn,
execute them concurrently with `tokio::JoinSet`.

**Current state:** `runner.rs` executes tool calls sequentially.
**Improvement:** Detect independent calls, spawn concurrent tasks, collect
results. Rust's ownership model makes this safe — no shared mutable state
concerns.

**Impact:** Directly reduces agent turn latency. A turn with 3 tool calls
that each take 2s goes from 6s to 2s.

### 2. Typed Tool Schemas via Proc Macro

Instead of hand-writing JSON schemas for tools, derive them from Rust structs.

```rust
#[derive(ToolSchema)]
struct ExecParams {
    /// The command to execute
    command: String,
    /// Timeout in seconds
    #[schema(default = 30)]
    timeout: Option<u64>,
}
```

Compile-time validation of tool parameters. Eliminates a class of runtime
errors.

### 3. Actor Model for Channel Isolation

Each channel plugin runs as an independent Tokio task communicating via `mpsc`
channels. If Telegram panics, Discord keeps running.

```rust
// Each channel is a supervised actor
let (tx, rx) = mpsc::channel(256);
tokio::spawn(async move {
    if let Err(e) = telegram_actor(rx).await {
        tracing::error!("telegram crashed: {e}, restarting...");
        // auto-restart logic
    }
});
```

### 4. WASM Plugin Sandbox

Load skills/plugins as WASM modules via `wasmtime`. Sandboxed execution with
capability-based security, much safer than subprocess execution.

**Advantage over OpenClaw:** TypeScript plugins run in the same V8 isolate
(or child process). WASM gives true memory isolation with explicit capability
grants.

### 5. Zero-Copy Streaming

Use `bytes::Bytes` in the streaming path from provider to WebSocket client.
Avoid intermediate `String` allocations for each token delta.

### 6. Background Embedding Indexing

Spawn a dedicated Tokio task that continuously indexes project files into the
memory store without blocking agent interactions. Safe sharing via
`Arc<MemoryStore>`.

### 7. Provider Capability Types

Replace runtime `supports_tools() -> bool` with type-level distinctions:

```rust
trait ToolCapable: LlmProvider {
    async fn complete_with_tools(...) -> Result<...>;
}
```

The compiler enforces that only tool-capable providers are used for agentic
flows.

### 8. Connection Pool Per Provider

Use `reqwest::Client` connection pooling aggressively. Each provider maintains
a persistent HTTP/2 connection, reducing TLS handshake latency on repeated
calls.

### 9. Session Type State Machine

Encode session states in the type system:

```rust
struct Session<S: SessionState> { ... }
struct Created;
struct Active;
struct Compacting;

impl Session<Active> {
    fn compact(self) -> Session<Compacting> { ... }
}
// Session<Created> cannot call compact() — compile error
```

---

## Recommended Build Order

| # | Feature | Effort | Impact | Rust Leverage |
|---|---------|--------|--------|---------------|
| 1 | MCP client support | Medium | Very High | Tokio tasks, crash isolation |
| 2 | Parallel tool execution | Low | High | JoinSet, Send + Sync |
| 3 | Discord channel plugin | Medium | High | Trait already defined |
| 4 | Multi-agent routing | Medium | High | Actor model, isolation |
| 5 | `#[derive(ToolSchema)]` proc macro | Medium | Medium | Compile-time safety |
| 6 | Webhook triggers | Low | Medium | Axum routes |
| 7 | Group chat support | Low | Medium | Session routing |
| 8 | Media transcription | Low | Medium | Async pipeline |
| 9 | WhatsApp channel | High | High | May need FFI bridge |
| 10 | WASM plugin sandbox | High | Medium | wasmtime, unique advantage |
| 11 | Actor channel isolation | Low | Medium | Tokio spawn, mpsc |
| 12 | Background embedding indexer | Low | Low | Arc sharing |
| 13 | Node/device system | High | Medium | Protocol extension |
| 14 | Presence broadcasting | Low | Low | Broadcast channels |

**Suggested first sprint:** items 1 + 2 (MCP client + parallel tool execution).
MCP unblocks the entire tool ecosystem; parallel execution is low-effort and
immediately improves agent responsiveness.
