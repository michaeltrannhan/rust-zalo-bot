-- Milestone 6: usage quotas and feature kill-switch counters.

UPDATE schema_metadata SET value = '6' WHERE key = 'schema_version';

CREATE TABLE usage_counters (
    scope TEXT NOT NULL,
    scope_id TEXT NOT NULL,
    period TEXT NOT NULL,
    metric TEXT NOT NULL,
    count BIGINT NOT NULL CHECK (count >= 0),
    limit_value BIGINT NOT NULL CHECK (limit_value >= 0),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (scope, scope_id, period, metric)
);
