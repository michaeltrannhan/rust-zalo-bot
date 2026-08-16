-- Milestone 6: account deletion progress and export artifacts.

UPDATE schema_metadata SET value = '8' WHERE key = 'schema_version';

ALTER TABLE conversation_states
    DROP CONSTRAINT conversation_states_pending_action_type_check;

ALTER TABLE conversation_states
    ADD CONSTRAINT conversation_states_pending_action_type_check
    CHECK (
        pending_action_type IS NULL
        OR pending_action_type IN (
            'manual_expense_confirmation',
            'receipt_review',
            'account_deletion'
        )
    );

CREATE TABLE deletion_requests (
    id UUID PRIMARY KEY,
    account_id UUID NOT NULL REFERENCES accounts (id),
    state TEXT NOT NULL CHECK (
        state IN ('requested', 'running', 'completed', 'failed')
    ),
    expense_count INTEGER NOT NULL CHECK (expense_count >= 0),
    requested_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    completed_at TIMESTAMPTZ,
    UNIQUE (account_id)
);

CREATE TABLE export_artifacts (
    id UUID PRIMARY KEY,
    account_id UUID NOT NULL REFERENCES accounts (id),
    format TEXT NOT NULL CHECK (format IN ('json', 'csv')),
    object_key TEXT NOT NULL,
    byte_size BIGINT NOT NULL CHECK (byte_size >= 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_export_artifacts_account_id ON export_artifacts (account_id, created_at DESC);
