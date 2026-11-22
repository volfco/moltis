-- Add deliver_only mode and template fields (PostgreSQL).
-- deliver_only is a flag column: BOOLEAN here, INTEGER 0/1 in SQLite; both
-- bind and decode as Rust bool.
ALTER TABLE webhooks ADD COLUMN deliver_only BOOLEAN NOT NULL DEFAULT false;
ALTER TABLE webhooks ADD COLUMN prompt_template TEXT;
ALTER TABLE webhooks ADD COLUMN deliver_to TEXT;
ALTER TABLE webhooks ADD COLUMN deliver_extra_json TEXT;
