-- Milestone 4: receipt lifecycle, assets, extraction attempts, drafts, categories.

UPDATE schema_metadata SET value = '4' WHERE key = 'schema_version';

CREATE TABLE categories (
    key TEXT PRIMARY KEY,
    display_name_vi TEXT NOT NULL
);

INSERT INTO categories (key, display_name_vi) VALUES
    ('an-uong', 'Ăn uống'),
    ('thuc-pham', 'Thực phẩm'),
    ('di-lai', 'Đi lại'),
    ('hoa-don', 'Hóa đơn'),
    ('mua-sam', 'Mua sắm'),
    ('suc-khoe', 'Sức khỏe'),
    ('giai-tri', 'Giải trí'),
    ('giao-duc', 'Giáo dục'),
    ('nha-o', 'Nhà ở'),
    ('thu-nhap', 'Thu nhập'),
    ('hoan-tien', 'Hoàn tiền'),
    ('chuyen-khoan', 'Chuyển khoản'),
    ('khac', 'Khác');

CREATE TABLE receipt_submissions (
    id UUID PRIMARY KEY,
    account_id UUID NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    inbound_event_id UUID REFERENCES inbound_events (id) ON DELETE SET NULL,
    lifecycle_state TEXT NOT NULL CHECK (
        lifecycle_state IN (
            'pending',
            'queued',
            'stored',
            'extracting',
            'review_required',
            'confirmed',
            'rejected',
            'failed_transient',
            'failed_permanent',
            'expired',
            'deleted'
        )
    ),
    version INTEGER NOT NULL DEFAULT 1 CHECK (version > 0),
    duplicate_of_submission_id UUID,
    confirmed_expense_id UUID REFERENCES expenses (id) ON DELETE SET NULL,
    failure_error_class TEXT,
    review_expires_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (id, account_id),
    CHECK (updated_at >= created_at),
    CHECK (duplicate_of_submission_id IS NULL OR duplicate_of_submission_id <> id),
    CHECK (lifecycle_state <> 'confirmed' OR confirmed_expense_id IS NOT NULL),
    CHECK (
        confirmed_expense_id IS NULL
        OR lifecycle_state IN ('confirmed', 'deleted')
    ),
    CHECK (
        lifecycle_state <> 'review_required' OR review_expires_at IS NOT NULL
    ),
    CHECK (
        failure_error_class IS NULL
        OR lifecycle_state IN ('failed_transient', 'failed_permanent', 'deleted')
    ),
    CHECK (
        duplicate_of_submission_id IS NULL
        OR lifecycle_state IN ('failed_permanent', 'deleted')
    ),
    FOREIGN KEY (duplicate_of_submission_id, account_id)
        REFERENCES receipt_submissions (id, account_id)
);

CREATE UNIQUE INDEX idx_receipt_submissions_inbound_event
    ON receipt_submissions (inbound_event_id)
    WHERE inbound_event_id IS NOT NULL;

CREATE INDEX idx_receipt_submissions_account_state
    ON receipt_submissions (account_id, lifecycle_state);

CREATE INDEX idx_receipt_submissions_review_expires
    ON receipt_submissions (review_expires_at)
    WHERE lifecycle_state = 'review_required';

CREATE TABLE receipt_assets (
    id UUID PRIMARY KEY,
    submission_id UUID NOT NULL UNIQUE,
    account_id UUID NOT NULL,
    object_key TEXT NOT NULL,
    content_sha256 CHAR(64) NOT NULL,
    mime_type TEXT NOT NULL,
    size_bytes BIGINT NOT NULL CHECK (size_bytes > 0 AND size_bytes <= 10485760),
    width_px INTEGER NOT NULL CHECK (width_px > 0),
    height_px INTEGER NOT NULL CHECK (height_px > 0),
    pixel_count BIGINT NOT NULL CHECK (
        pixel_count > 0
        AND pixel_count <= 25000000
        AND pixel_count = width_px::BIGINT * height_px::BIGINT
    ),
    retention_deadline TIMESTAMPTZ NOT NULL,
    deletion_state TEXT NOT NULL DEFAULT 'active' CHECK (deletion_state IN ('active', 'deleted')),
    original_deleted_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    FOREIGN KEY (submission_id, account_id)
        REFERENCES receipt_submissions (id, account_id)
        ON DELETE CASCADE,
    CHECK (
        (deletion_state = 'deleted' AND original_deleted_at IS NOT NULL)
        OR (deletion_state = 'active' AND original_deleted_at IS NULL)
    )
);

CREATE UNIQUE INDEX idx_receipt_assets_account_hash_active
    ON receipt_assets (account_id, content_sha256)
    WHERE deletion_state = 'active';

CREATE INDEX idx_receipt_assets_retention_sweep
    ON receipt_assets (retention_deadline)
    WHERE deletion_state = 'active';

CREATE TABLE expense_drafts (
    id UUID PRIMARY KEY,
    submission_id UUID NOT NULL UNIQUE,
    account_id UUID NOT NULL,
    amount_minor BIGINT NOT NULL CHECK (amount_minor > 0),
    currency CHAR(3) NOT NULL CHECK (currency ~ '^[A-Z]{3}$'),
    merchant TEXT NOT NULL CHECK (char_length(btrim(merchant)) BETWEEN 1 AND 200),
    category_key TEXT NOT NULL REFERENCES categories (key),
    transaction_type TEXT NOT NULL DEFAULT 'expense' CHECK (
        transaction_type IN ('expense', 'income', 'refund', 'transfer', 'adjustment')
    ),
    occurred_at TIMESTAMPTZ NOT NULL,
    confidence REAL CHECK (confidence IS NULL OR (confidence >= 0 AND confidence <= 1)),
    version INTEGER NOT NULL DEFAULT 1 CHECK (version > 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    FOREIGN KEY (submission_id, account_id)
        REFERENCES receipt_submissions (id, account_id)
        ON DELETE CASCADE,
    CHECK (updated_at >= created_at)
);

CREATE INDEX idx_expense_drafts_account_id ON expense_drafts (account_id);

CREATE TABLE draft_corrections (
    id UUID PRIMARY KEY,
    draft_id UUID NOT NULL REFERENCES expense_drafts (id) ON DELETE CASCADE,
    submission_id UUID NOT NULL REFERENCES receipt_submissions (id) ON DELETE CASCADE,
    field_name TEXT NOT NULL,
    old_value TEXT,
    new_value TEXT,
    corrected_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_draft_corrections_submission
    ON draft_corrections (submission_id, corrected_at DESC);

CREATE TABLE extraction_attempts (
    id UUID PRIMARY KEY,
    submission_id UUID NOT NULL REFERENCES receipt_submissions (id) ON DELETE CASCADE,
    attempt_number INTEGER NOT NULL CHECK (attempt_number > 0),
    provider TEXT NOT NULL,
    model TEXT NOT NULL,
    profile_name TEXT NOT NULL,
    prompt_version TEXT NOT NULL,
    outcome TEXT NOT NULL CHECK (outcome IN ('success', 'failed')),
    error_class TEXT,
    latency_ms INTEGER NOT NULL CHECK (latency_ms >= 0),
    input_tokens INTEGER,
    output_tokens INTEGER,
    started_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    ended_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (submission_id, attempt_number)
);

CREATE INDEX idx_extraction_attempts_submission
    ON extraction_attempts (submission_id, attempt_number DESC);

ALTER TABLE expenses
    ADD COLUMN receipt_submission_id UUID;

ALTER TABLE expenses
    ADD CONSTRAINT expenses_receipt_account_fk
    FOREIGN KEY (receipt_submission_id, account_id)
    REFERENCES receipt_submissions (id, account_id)
    ON DELETE SET NULL;

CREATE UNIQUE INDEX idx_expenses_receipt_submission
    ON expenses (receipt_submission_id)
    WHERE receipt_submission_id IS NOT NULL;
