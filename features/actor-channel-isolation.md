# Feature: Actor Model for Channel Isolation

## Overview
Each channel plugin runs as a supervised Tokio task. If one channel crashes, others keep running.

## What to build
- `mpsc` channel per plugin, crash isolation
- Auto-restart on panic with backoff
- Health reporting per channel

## Example
```rust
let (tx, rx) = mpsc::channel(256);
tokio::spawn(async move {
    if let Err(e) = telegram_actor(rx).await {
        tracing::error!("telegram crashed: {e}, restarting...");
    }
});
```
