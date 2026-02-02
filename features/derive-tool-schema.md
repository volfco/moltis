# Feature: `#[derive(ToolSchema)]` Proc Macro

## Overview
Derive JSON schemas from Rust structs for tool parameters instead of hand-writing JSON schemas.

## What to build
- `crates/tool-schema-derive/` proc macro crate
- Generate `tool_schema()` → `serde_json::Value` from struct fields + doc comments
- Compile-time validation, eliminate hand-written JSON schemas

## Example
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
