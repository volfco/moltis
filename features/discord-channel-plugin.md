# Feature: Discord Channel Plugin

## Overview
The `ChannelPlugin` trait is already defined. Discord has mature Rust crates (`serenity` or `twilight`).

## What to build
- `crates/discord/` — implement `ChannelPlugin` for Discord
- Bot token config in `moltis.toml`
- Message receive → agent run → reply flow
- Support text channels and DMs

## References
- Existing `ChannelPlugin` trait in `crates/channels/`
- Telegram implementation in `crates/telegram/` as reference
- OpenClaw Discord implementation for feature parity
