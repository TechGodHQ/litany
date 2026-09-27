CREATE TABLE tasks (
    id              INTEGER PRIMARY KEY,
    name            TEXT NOT NULL,
    due             TEXT,
    recurrence_json TEXT,
    anchor          TEXT NOT NULL,
    created_at      TEXT NOT NULL,
    archived        INTEGER NOT NULL DEFAULT 0 CHECK (archived IN (0, 1))
);

CREATE TABLE occurrences (
    id           INTEGER PRIMARY KEY,
    task_id      INTEGER NOT NULL REFERENCES tasks(id),
    due_date     TEXT NOT NULL,
    completed_at TEXT NOT NULL
);

CREATE INDEX occurrences_task_due_date ON occurrences(task_id, due_date);
