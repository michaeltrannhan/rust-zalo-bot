-- Multi-field edit + English category labels + expense category columns.

UPDATE schema_metadata SET value = '11' WHERE key = 'schema_version';

ALTER TABLE categories
    ADD COLUMN display_name_en TEXT;

UPDATE categories SET display_name_en = CASE key
    WHEN 'an-uong' THEN 'Food & drink'
    WHEN 'thuc-pham' THEN 'Groceries'
    WHEN 'di-lai' THEN 'Transport'
    WHEN 'hoa-don' THEN 'Bills'
    WHEN 'mua-sam' THEN 'Shopping'
    WHEN 'suc-khoe' THEN 'Health'
    WHEN 'giai-tri' THEN 'Entertainment'
    WHEN 'giao-duc' THEN 'Education'
    WHEN 'nha-o' THEN 'Housing'
    WHEN 'thu-nhap' THEN 'Income'
    WHEN 'hoan-tien' THEN 'Refund'
    WHEN 'chuyen-khoan' THEN 'Transfer'
    WHEN 'khac' THEN 'Other'
    ELSE initcap(replace(key, '-', ' '))
END
WHERE display_name_en IS NULL;

ALTER TABLE categories
    ALTER COLUMN display_name_en SET NOT NULL;

ALTER TABLE expenses
    ADD COLUMN category_key TEXT REFERENCES categories (key),
    ADD COLUMN transaction_type TEXT;

UPDATE expenses AS e
SET
    category_key = COALESCE(d.category_key, 'khac'),
    transaction_type = COALESCE(d.transaction_type, 'expense')
FROM expense_drafts AS d
WHERE e.receipt_submission_id = d.submission_id
  AND e.account_id = d.account_id;

UPDATE expenses
SET
    category_key = COALESCE(category_key, 'khac'),
    transaction_type = COALESCE(transaction_type, 'expense')
WHERE category_key IS NULL OR transaction_type IS NULL;

ALTER TABLE expenses
    ALTER COLUMN category_key SET DEFAULT 'khac',
    ALTER COLUMN category_key SET NOT NULL,
    ALTER COLUMN transaction_type SET DEFAULT 'expense',
    ALTER COLUMN transaction_type SET NOT NULL;

ALTER TABLE expenses
    ADD CONSTRAINT expenses_transaction_type_check CHECK (
        transaction_type IN ('expense', 'income', 'refund', 'transfer', 'adjustment')
    );
