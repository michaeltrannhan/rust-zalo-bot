-- Milestone 2: conversation state, expenses, outbound outbox, ingress event extensions.

UPDATE schema_metadata SET value = '2' WHERE key = 'schema_version';

ALTER TABLE inbound_events
    ADD COLUMN ingress_source TEXT CHECK (ingress_source IN ('webhook', 'polling')),
    ADD COLUMN account_id UUID REFERENCES accounts (id),
    ADD COLUMN processed_at TIMESTAMPTZ;

CREATE TABLE conversation_states (
    account_id UUID PRIMARY KEY REFERENCES accounts (id) ON DELETE CASCADE,
    pending_action_type TEXT,
    pending_payload_ref TEXT,
    expires_at TIMESTAMPTZ,
    version INTEGER NOT NULL DEFAULT 1,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CHECK (
        (pending_action_type IS NULL AND pending_payload_ref IS NULL AND expires_at IS NULL)
        OR (pending_action_type IS NOT NULL AND expires_at IS NOT NULL)
    )
);

CREATE TABLE expenses (
    id UUID PRIMARY KEY,
    account_id UUID NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    amount_minor BIGINT NOT NULL,
    currency CHAR(3) NOT NULL,
    occurred_at TIMESTAMPTZ NOT NULL,
    description TEXT NOT NULL,
    source TEXT NOT NULL CHECK (source IN ('manual', 'receipt')),
    state TEXT NOT NULL CHECK (
        state IN ('awaiting_confirmation', 'confirmed', 'rejected')
    ),
    version INTEGER NOT NULL DEFAULT 1,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_expenses_account_occurred ON expenses (account_id, occurred_at DESC);
CREATE INDEX idx_expenses_account_state ON expenses (account_id, state);

CREATE TABLE outbound_messages (
    id UUID PRIMARY KEY,
    account_id UUID REFERENCES accounts (id) ON DELETE SET NULL,
    inbound_event_id UUID REFERENCES inbound_events (id) ON DELETE SET NULL,
    idempotency_key TEXT NOT NULL UNIQUE,
    provider_scope TEXT NOT NULL,
    provider_target TEXT NOT NULL,
    body TEXT NOT NULL,
    state TEXT NOT NULL CHECK (
        state IN ('queued', 'sending', 'sent', 'failed', 'suppressed', 'ambiguous')
    ),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_outbound_messages_account_id ON outbound_messages (account_id);
CREATE INDEX idx_outbound_messages_state ON outbound_messages (state);
