-- Milestone 6: summary schedules, default currency, and role leases.

UPDATE schema_metadata SET value = '7' WHERE key = 'schema_version';

ALTER TABLE accounts
ADD COLUMN IF NOT EXISTS default_currency CHAR(3) NOT NULL DEFAULT 'VND';

CREATE TABLE summary_schedules (
    id UUID PRIMARY KEY,
    account_id UUID NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    frequency TEXT NOT NULL CHECK (frequency IN ('daily', 'weekly', 'monthly')),
    delivery_minute INTEGER NOT NULL CHECK (delivery_minute BETWEEN 0 AND 1439),
    provider_scope TEXT NOT NULL,
    provider_chat_id TEXT NOT NULL,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    next_run_at TIMESTAMPTZ NOT NULL,
    last_emitted_at TIMESTAMPTZ,
    UNIQUE (account_id, frequency)
);

CREATE INDEX idx_summary_schedules_due ON summary_schedules (next_run_at)
WHERE enabled = TRUE;

CREATE TABLE role_leases (
    role TEXT PRIMARY KEY,
    owner TEXT NOT NULL,
    deadline TIMESTAMPTZ NOT NULL
);
