-- 0011_secrets.sql
--
-- Generic key-value secret store used by the Telegram cockpit's
-- `surge telegram setup` command to persist the Bot API token. Other
-- subsystems may share this table; rows are namespaced by their `key`
-- prefix (e.g. `telegram.cockpit.bot_token`).
--
-- This is a stop-gap store — values are not encrypted at rest. The
-- filesystem permissions on `~/.surge/db/registry.sqlite` are the only
-- protection; deployments that need stronger guarantees should layer a
-- secrets manager (Vault, AWS Secrets Manager, etc.) above this table and
-- treat the local row as a cache.

CREATE TABLE IF NOT EXISTS secrets (
    key         TEXT    PRIMARY KEY,
    value       TEXT    NOT NULL,
    created_at  INTEGER NOT NULL,    -- Unix epoch ms
    updated_at  INTEGER NOT NULL     -- Unix epoch ms
);
