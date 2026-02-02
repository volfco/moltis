# Feature: WASM Plugin Sandbox

## Overview
Load skills/plugins as WASM modules via `wasmtime`. Sandboxed execution with capability-based security — much safer than subprocess execution.

## What to build
- `crates/wasm-sandbox/` — wasmtime-based plugin host
- Capability grants (filesystem, network, etc.)
- Skill → WASM compilation toolchain

## Advantage
TypeScript plugins run in the same V8 isolate or child process. WASM gives true memory isolation with explicit capability grants.
