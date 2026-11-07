ALTER TABLE tasks
    ADD COLUMN collapse INTEGER NOT NULL DEFAULT 0 CHECK (collapse IN (0, 1));

ALTER TABLE tasks
    ADD COLUMN advance_mode TEXT NOT NULL DEFAULT 'anchor'
    CHECK (advance_mode IN ('anchor', 'slide'));
