-- Milestone 3: durable work queue with leases, attempts, and serialization.

UPDATE schema_metadata SET value = '3' WHERE key = 'schema_version';

CREATE TABLE jobs (
    id UUID PRIMARY KEY,
    job_type TEXT NOT NULL,
    payload JSONB NOT NULL,
    payload_version INTEGER NOT NULL CHECK (payload_version > 0),
    state TEXT NOT NULL CHECK (
        state IN ('queued', 'leased', 'completed', 'cancelled', 'dead')
    ),
    priority INTEGER NOT NULL DEFAULT 0,
    run_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    dedupe_key TEXT NOT NULL,
    serialization_key TEXT,
    attempt_count INTEGER NOT NULL DEFAULT 0 CHECK (attempt_count >= 0),
    max_attempts INTEGER NOT NULL DEFAULT 10 CHECK (max_attempts > 0),
    lease_token UUID,
    lease_owner TEXT,
    lease_deadline TIMESTAMPTZ,
    last_error_class TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    completed_at TIMESTAMPTZ,
    CHECK (
        (state = 'leased' AND lease_token IS NOT NULL AND lease_owner IS NOT NULL AND lease_deadline IS NOT NULL)
        OR (
            state <> 'leased'
            AND lease_token IS NULL
            AND lease_owner IS NULL
            AND lease_deadline IS NULL
        )
    ),
    CHECK (
        (state IN ('completed', 'cancelled', 'dead') AND completed_at IS NOT NULL)
        OR (state NOT IN ('completed', 'cancelled', 'dead') AND completed_at IS NULL)
    )
);

CREATE UNIQUE INDEX idx_jobs_dedupe_key ON jobs (dedupe_key);

CREATE UNIQUE INDEX idx_jobs_active_serialization_key ON jobs (serialization_key)
WHERE serialization_key IS NOT NULL AND state = 'leased';

CREATE INDEX idx_jobs_claim ON jobs (priority DESC, run_at ASC, created_at ASC)
WHERE state IN ('queued', 'leased');

CREATE TABLE job_attempts (
    id UUID PRIMARY KEY,
    job_id UUID NOT NULL REFERENCES jobs (id) ON DELETE CASCADE,
    attempt_number INTEGER NOT NULL CHECK (attempt_number > 0),
    lease_token UUID NOT NULL,
    lease_owner TEXT NOT NULL,
    started_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    ended_at TIMESTAMPTZ,
    outcome TEXT CHECK (
        outcome IS NULL
        OR outcome IN ('completed', 'failed', 'cancelled', 'lost_lease', 'superseded')
    ),
    error_class TEXT,
    UNIQUE (job_id, attempt_number)
);

CREATE INDEX idx_job_attempts_job_id ON job_attempts (job_id, attempt_number DESC);
