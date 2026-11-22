-- Managed outbound SSH identities and named targets for remote exec.
-- Mirrors migrations/20260328093000_ssh_keys_and_targets.sql. Flag columns
-- stay numeric (BIGINT) because the Rust boundary binds/reads them as i64.
CREATE TABLE IF NOT EXISTS ssh_keys (
    id          BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    name        TEXT   NOT NULL UNIQUE,
    private_key TEXT   NOT NULL,
    public_key  TEXT   NOT NULL,
    fingerprint TEXT   NOT NULL,
    encrypted   BIGINT NOT NULL DEFAULT 0,
    created_at  TEXT   NOT NULL DEFAULT (to_char(now() AT TIME ZONE 'utc', 'YYYY-MM-DD HH24:MI:SS')),
    updated_at  TEXT   NOT NULL DEFAULT (to_char(now() AT TIME ZONE 'utc', 'YYYY-MM-DD HH24:MI:SS'))
);

CREATE TABLE IF NOT EXISTS ssh_targets (
    id          BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    label       TEXT   NOT NULL UNIQUE,
    target      TEXT   NOT NULL,
    port        BIGINT,
    auth_mode   TEXT   NOT NULL DEFAULT 'system',
    key_id      BIGINT,
    is_default  BIGINT NOT NULL DEFAULT 0,
    created_at  TEXT   NOT NULL DEFAULT (to_char(now() AT TIME ZONE 'utc', 'YYYY-MM-DD HH24:MI:SS')),
    updated_at  TEXT   NOT NULL DEFAULT (to_char(now() AT TIME ZONE 'utc', 'YYYY-MM-DD HH24:MI:SS')),
    FOREIGN KEY(key_id) REFERENCES ssh_keys(id)
);
