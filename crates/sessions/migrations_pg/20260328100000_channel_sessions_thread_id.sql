-- Add thread_id to channel_sessions for Telegram forum-topic session isolation.
-- Unlike SQLite, PostgreSQL can alter a primary key in place, so no table
-- rebuild is needed. The constraint name is deterministic because migration
-- 20240205100001 declares it explicitly.
ALTER TABLE channel_sessions ADD COLUMN thread_id TEXT NOT NULL DEFAULT '';
ALTER TABLE channel_sessions DROP CONSTRAINT channel_sessions_pkey;
ALTER TABLE channel_sessions
    ADD PRIMARY KEY (channel_type, account_id, chat_id, thread_id);
