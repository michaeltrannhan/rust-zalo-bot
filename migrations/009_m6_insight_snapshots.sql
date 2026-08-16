-- Milestone 6: deterministic insight snapshots and account AI preferences.

UPDATE schema_metadata SET value = '9' WHERE key = 'schema_version';

CREATE TABLE insight_snapshots (
    id UUID PRIMARY KEY,
    account_id UUID NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    period_kind TEXT NOT NULL CHECK (period_kind IN ('day', 'week', 'month')),
    period_start TIMESTAMPTZ NOT NULL,
    period_end TIMESTAMPTZ NOT NULL,
    timezone TEXT NOT NULL,
    schema_name TEXT NOT NULL DEFAULT 'insight-v1',
    aggregate JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (account_id, period_kind, period_start, timezone, schema_name)
);

CREATE INDEX idx_insight_snapshots_account_created
    ON insight_snapshots (account_id, created_at DESC);

CREATE TABLE account_ai_preferences (
    account_id UUID PRIMARY KEY REFERENCES accounts (id) ON DELETE CASCADE,
    extraction_profile TEXT,
    insight_profile TEXT
);
