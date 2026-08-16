-- Milestone 6: optional LLM narrative columns on insight snapshots.

UPDATE schema_metadata SET value = '10' WHERE key = 'schema_version';

ALTER TABLE insight_snapshots
    ADD COLUMN IF NOT EXISTS narrative_text TEXT,
    ADD COLUMN IF NOT EXISTS narrative_profile TEXT,
    ADD COLUMN IF NOT EXISTS narrative_generated_at TIMESTAMPTZ;

ALTER TABLE insight_snapshots
    ADD CONSTRAINT insight_snapshots_narrative_text_length CHECK (
        narrative_text IS NULL OR char_length(narrative_text) BETWEEN 1 AND 2000
    );
