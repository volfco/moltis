# Feature: Multi-Agent Routing

## Overview
Route messages to different agent configurations based on rules. Currently Moltis has a single agent runner.

## What to build
- Agent registry: named agents with own provider/prompt/tools config
- Routing rules: channel-based, keyword-based, explicit `/agent` command
- Session isolation per agent workspace
- Default agent fallback

## References
- OpenClaw routes messages to isolated agent workspaces
- Current runner in `crates/agents/src/runner.rs`
