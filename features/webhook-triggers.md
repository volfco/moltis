# Feature: Webhook Triggers

## Overview
Event-driven agent activation via HTTP webhooks.

## What to build
- `POST /api/webhooks/:id` endpoint
- Webhook registration API with HMAC secret validation
- Trigger agent runs with webhook payload as context
- Gmail Pub/Sub integration (push notifications to webhook)
