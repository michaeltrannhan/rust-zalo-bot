-- Milestone 4 ingress: image event metadata and receipt review pending actions.

UPDATE schema_metadata SET value = '5' WHERE key = 'schema_version';

ALTER TABLE inbound_events
    ADD COLUMN media_url TEXT,
    ADD COLUMN provider_chat_id TEXT;

ALTER TABLE conversation_states
    DROP CONSTRAINT conversation_states_pending_action_type_check;

ALTER TABLE conversation_states
    ADD CONSTRAINT conversation_states_pending_action_type_check
    CHECK (
        pending_action_type IS NULL
        OR pending_action_type IN ('manual_expense_confirmation', 'receipt_review')
    );
