# MCP & Next Features Plan

## Status: What's Done

### Parallel Tool Execution (complete)
- `crates/agents/src/runner.rs` — tool calls run concurrently via `futures::future::join_all`
- `ToolCall` derives `Clone`; event ordering preserved (all starts before ends)
- 3 tests: parallel execution, one-fails, concurrency timing

### MCP Tool Integration (complete)
Moltis acts as an **MCP client** — it spawns external MCP server processes
(via npm/uvx) or connects to remote servers over HTTP/SSE. Each server
exposes tools that the agent can call during conversations.

- `crates/mcp/` — full crate with types, transport (stdio + SSE), client, tool_bridge, manager, registry
- JSON-RPC 2.0 over stdio or HTTP POST, 30s timeout, initialize/tools/call handshake
- `McpManager` lifecycle for multiple MCP server processes, persisted to `~/.moltis/mcp-servers.json`
- `LiveMcpService` in gateway with 9 RPC methods (list/add/remove/enable/disable/status/tools/restart/update)
- `GET /api/mcp` HTTP endpoint for page-load data
- Web UI page (`page-mcp.js` — "MCP Tools") with featured tools, configure flow (args + env vars), install box, transport selector (stdio/SSE), tool cards with expand/tools, enable/disable/restart/edit/remove, styled confirm dialog, toast notifications
- Health polling every 30s with real-time UI updates via `mcp.status` events
- Auto-restart on crash with exponential backoff (5 max attempts)
- Tool bridges auto-synced into agent `ToolRegistry` on start/stop/restart
- Stderr capture + detailed logging throughout transport/client
- Non-blocking add/enable/restart (server start spawned in background)
- WS pending callbacks flushed on disconnect

---

## MCP Tools: Completed

All MCP remaining work items are now implemented:

1. **SSE Transport** ✅ — `crates/mcp/src/sse_transport.rs`, `TransportType` enum (Stdio/Sse), UI transport selector
2. **Tool Bridge Registration** ✅ — `sync_mcp_tools()` wires `McpToolBridge` → `ToolRegistry` on start/stop/restart
3. **Health Polling** ✅ — `crates/gateway/src/mcp_health.rs`, 30s polling, broadcasts `mcp.status` events to UI
4. **Edit Server Config** ✅ — `mcp.update` RPC, inline edit form on server cards
5. **Auto-Restart on Crash** ✅ — exponential backoff (5s base, 300s cap, 5 max attempts) in health monitor

---

## Next Features (in priority order)

### Feature 3: Discord Channel Plugin
The `ChannelPlugin` trait is defined. Discord has mature Rust crates (`serenity` or `twilight`).

**What to build:**
- `crates/discord/` — implement `ChannelPlugin` for Discord
- Bot token config in `moltis.toml`
- Message receive → agent run → reply flow
- Support text channels and DMs

### Feature 4: Multi-Agent Routing
Route messages to different agent configurations based on rules.

**What to build:**
- Agent registry: named agents with own provider/prompt/tools config
- Routing rules: channel-based, keyword-based, explicit `/agent` command
- Session isolation per agent workspace
- Default agent fallback

### Feature 5: `#[derive(ToolSchema)]` Proc Macro
Derive JSON schemas from Rust structs for tool parameters.

**What to build:**
- `crates/tool-schema-derive/` proc macro crate
- Generate `tool_schema()` → `serde_json::Value` from struct fields + doc comments
- Compile-time validation, eliminate hand-written JSON schemas

### Feature 6: Webhook Triggers
Event-driven agent activation via HTTP webhooks.

**What to build:**
- `POST /api/webhooks/:id` endpoint
- Webhook registration API with HMAC secret validation
- Trigger agent runs with webhook payload as context

### Feature 7: Group Chat Support
Handle group conversations with mention-based activation.

**What to build:**
- `@botname` mention detection in channel plugins
- Group session routing (one session per group)
- Reply-to-message threading

### Feature 8: Actor Model for Channel Isolation
Each channel plugin as a supervised Tokio task.

**What to build:**
- `mpsc` channel per plugin, crash isolation
- Auto-restart on panic with backoff
- Health reporting per channel

### Feature 9: Media Transcription Pipeline
Voice/image processing before agent sees the message.

**What to build:**
- Whisper API integration for audio
- Vision model integration for images
- Auto-transcribe pipeline in channel message flow

### Feature 10: WASM Plugin Sandbox
Load skills as WASM modules via `wasmtime`.

**What to build:**
- `crates/wasm-sandbox/` — wasmtime-based plugin host
- Capability grants (filesystem, network, etc.)
- Skill → WASM compilation toolchain
