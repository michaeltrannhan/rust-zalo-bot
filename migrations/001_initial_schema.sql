-- Milestone 1 P0 schema: accounts, provider identities, inbound events, ingress control.

CREATE TABLE schema_metadata (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

INSERT INTO schema_metadata (key, value) VALUES ('schema_version', '1');

CREATE TABLE accounts (
    id UUID PRIMARY KEY,
    lifecycle_state TEXT NOT NULL CHECK (
        lifecycle_state IN ('pending_consent', 'active', 'suspended', 'deleting', 'deleted')
    ),
    locale TEXT NOT NULL DEFAULT 'vi',
    timezone TEXT NOT NULL DEFAULT 'Asia/Ho_Chi_Minh',
    retention_preference_days INTEGER,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE provider_identities (
    id UUID PRIMARY KEY,
    account_id UUID NOT NULL REFERENCES accounts (id),
    provider_scope TEXT NOT NULL,
    provider_sender_id TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (provider_scope, provider_sender_id)
);

CREATE INDEX idx_provider_identities_account_id ON provider_identities (account_id);

CREATE TABLE inbound_events (
    id UUID PRIMARY KEY,
    provider_event_id TEXT NOT NULL,
    provider_scope TEXT NOT NULL,
    kind TEXT NOT NULL,
    payload_version INTEGER NOT NULL DEFAULT 1,
    processing_state TEXT NOT NULL CHECK (
        processing_state IN ('accepted', 'duplicate', 'rejected')
    ),
    received_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (provider_scope, provider_event_id)
);

CREATE INDEX idx_inbound_events_received_at ON inbound_events (received_at);

CREATE TABLE ingress_control (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    mode TEXT NOT NULL CHECK (mode IN ('webhook', 'polling')) DEFAULT 'webhook',
    mode_generation INTEGER NOT NULL DEFAULT 1,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

INSERT INTO ingress_control (id, mode, mode_generation) VALUES (1, 'webhook', 1);
