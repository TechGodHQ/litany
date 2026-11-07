//! SQLite persistence for Litany's occurrence-sourced domain model.
//!
//! This crate owns I/O only. Callers provide every date and timestamp; it
//! never reads a clock.

use chrono::{NaiveDate, NaiveDateTime};
use litany_core::{
    most_recent_satisfied_due, next_due, next_due_for_mode, AdvanceMode, Occurrence, Recurrence,
    Task,
};
use rusqlite::{params, Connection, OptionalExtension, Transaction};

const SCHEMA_V0: &str = include_str!("schema.sql");
const MIGRATION_V2: &str = include_str!("migration_v2.sql");

#[derive(Debug)]
pub enum StoreError {
    Sql(rusqlite::Error),
    Json(serde_json::Error),
    Chrono(chrono::ParseError),
    InvalidCompletion(&'static str),
    UnrepresentableNextDue,
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Sql(error) => write!(f, "sqlite error: {error}"),
            Self::Json(error) => write!(f, "recurrence JSON error: {error}"),
            Self::Chrono(error) => write!(f, "stored date/time error: {error}"),
            Self::InvalidCompletion(reason) => write!(f, "invalid completion: {reason}"),
            Self::UnrepresentableNextDue => {
                write!(f, "recurrence has no representable next due date")
            }
        }
    }
}

impl std::error::Error for StoreError {}
impl From<rusqlite::Error> for StoreError {
    fn from(value: rusqlite::Error) -> Self {
        Self::Sql(value)
    }
}
impl From<serde_json::Error> for StoreError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}
impl From<chrono::ParseError> for StoreError {
    fn from(value: chrono::ParseError) -> Self {
        Self::Chrono(value)
    }
}

pub type Result<T> = std::result::Result<T, StoreError>;

pub struct Store {
    connection: Connection,
}

impl Store {
    pub fn open(path: impl AsRef<std::path::Path>) -> Result<Self> {
        let mut store = Self {
            connection: Connection::open(path)?,
        };
        store.configure()?;
        store.migrate()?;
        Ok(store)
    }

    pub fn open_in_memory() -> Result<Self> {
        let mut store = Self {
            connection: Connection::open_in_memory()?,
        };
        store.configure()?;
        store.migrate()?;
        Ok(store)
    }

    fn configure(&mut self) -> Result<()> {
        self.connection.pragma_update(None, "foreign_keys", "ON")?;
        Ok(())
    }

    /// Applies embedded migrations, tracked by SQLite's `user_version`.
    pub fn migrate(&mut self) -> Result<()> {
        let mut version: u32 = self
            .connection
            .pragma_query_value(None, "user_version", |row| row.get(0))?;
        if version > 2 {
            return Err(StoreError::InvalidCompletion(
                "database schema is newer than this store",
            ));
        }
        if version == 0 {
            let tx = self.connection.transaction()?;
            tx.execute_batch(SCHEMA_V0)?;
            tx.pragma_update(None, "user_version", 1)?;
            tx.commit()?;
            version = 1;
        }
        if version == 1 {
            let tx = self.connection.transaction()?;
            tx.execute_batch(MIGRATION_V2)?;
            tx.pragma_update(None, "user_version", 2)?;
            tx.commit()?;
        }
        Ok(())
    }

    pub fn create_task(&mut self, task: &Task) -> Result<Task> {
        validate_task(task)?;
        let tx = self.connection.transaction()?;
        tx.execute(
            "INSERT INTO tasks (name, due, recurrence_json, anchor, collapse, advance_mode, created_at, archived) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![task.name, date_text(task.due), recurrence_text(task.recurrence.as_ref())?, task.anchor.to_string(), task.collapse, advance_text(task.advance), timestamp_text(task.created_at), task.archived],
        )?;
        let id = tx.last_insert_rowid();
        tx.commit()?;
        self.task(id)?
            .ok_or(StoreError::InvalidCompletion("created task disappeared"))
    }

    pub fn update_task(&mut self, task: &Task) -> Result<()> {
        validate_task(task)?;
        let changed = self.connection.execute(
            "UPDATE tasks SET name=?1, due=?2, recurrence_json=?3, anchor=?4, collapse=?5, advance_mode=?6, created_at=?7, archived=?8 WHERE id=?9",
            params![task.name, date_text(task.due), recurrence_text(task.recurrence.as_ref())?, task.anchor.to_string(), task.collapse, advance_text(task.advance), timestamp_text(task.created_at), task.archived, task.id],
        )?;
        if changed == 0 {
            return Err(StoreError::InvalidCompletion("task does not exist"));
        }
        Ok(())
    }

    pub fn archive_task(&mut self, id: i64) -> Result<()> {
        let changed = self
            .connection
            .execute("UPDATE tasks SET archived=1 WHERE id=?1", [id])?;
        if changed == 0 {
            return Err(StoreError::InvalidCompletion("task does not exist"));
        }
        Ok(())
    }

    pub fn list_tasks(&self, due_before: Option<NaiveDate>) -> Result<Vec<Task>> {
        let mut statement = if due_before.is_some() {
            self.connection.prepare("SELECT id, name, due, recurrence_json, anchor, collapse, advance_mode, created_at, archived FROM tasks WHERE archived=0 AND due <= ?1 ORDER BY due, id")?
        } else {
            self.connection.prepare("SELECT id, name, due, recurrence_json, anchor, collapse, advance_mode, created_at, archived FROM tasks WHERE archived=0 ORDER BY due IS NULL, due, id")?
        };
        let mut rows = match due_before {
            Some(date) => statement.query([date.to_string()])?,
            None => statement.query([])?,
        };
        let mut tasks = Vec::new();
        while let Some(row) = rows.next()? {
            tasks.push(task_from_row(row)?);
        }
        Ok(tasks)
    }

    pub fn occurrences_for_task(&self, task_id: i64) -> Result<Vec<Occurrence>> {
        let mut statement = self.connection.prepare("SELECT id, task_id, due_date, completed_at FROM occurrences WHERE task_id=?1 ORDER BY due_date, id")?;
        let rows = statement.query_map([task_id], occurrence_from_row)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    /// Records a completion and advances a recurring task atomically. In
    /// collapse mode, a late completion is attributed to the most recent
    /// scheduled due on or before its UTC date; missed periods are not
    /// materialized. Early completions retain the legacy one-step behavior.
    /// For no-due tasks, callers attribute the occurrence to the UTC date of
    /// `completed_at`; such tasks cannot carry a recurrence.
    pub fn record_occurrence(
        &mut self,
        task_id: i64,
        due_date: NaiveDate,
        completed_at: NaiveDateTime,
    ) -> Result<Occurrence> {
        let tx = self.connection.transaction()?;
        let task = task_in_transaction(&tx, task_id)?
            .ok_or(StoreError::InvalidCompletion("task does not exist"))?;
        if task.archived {
            return Err(StoreError::InvalidCompletion("task is archived"));
        }
        match (task.due, task.recurrence.as_ref()) {
            (Some(current_due), _) if current_due != due_date => {
                return Err(StoreError::InvalidCompletion(
                    "due date does not match task",
                ))
            }
            (None, Some(_)) => {
                return Err(StoreError::InvalidCompletion(
                    "recurring task requires a due date",
                ))
            }
            (None, None) if due_date != completed_at.date() => {
                return Err(StoreError::InvalidCompletion(
                    "no-due task uses completion's UTC date",
                ))
            }
            _ => {}
        }

        let (recorded_due, next) = if let Some(rule) = task.recurrence.as_ref() {
            let current_due = task.due.ok_or(StoreError::InvalidCompletion(
                "recurring task requires a due date",
            ))?;
            if task.collapse && completed_at.date() >= current_due {
                let recorded_due = most_recent_satisfied_due(
                    rule,
                    current_due,
                    task.anchor,
                    completed_at.date(),
                    task.advance,
                );
                let advance_from = match task.advance {
                    AdvanceMode::Anchor => recorded_due,
                    AdvanceMode::Slide => completed_at.date(),
                };
                let next = next_due_for_mode(rule, advance_from, task.anchor, task.advance)
                    .ok_or(StoreError::UnrepresentableNextDue)?;
                (recorded_due, Some(next))
            } else {
                // Stacking (and early completion) always preserves the legacy
                // single-step advance, independent of the configured mode.
                let next = next_due(rule, current_due, task.anchor)
                    .ok_or(StoreError::UnrepresentableNextDue)?;
                (current_due, Some(next))
            }
        } else {
            (due_date, None)
        };

        tx.execute(
            "INSERT INTO occurrences (task_id, due_date, completed_at) VALUES (?1, ?2, ?3)",
            params![
                task_id,
                recorded_due.to_string(),
                timestamp_text(completed_at)
            ],
        )?;
        let id = tx.last_insert_rowid();
        if let Some(next) = next {
            tx.execute(
                "UPDATE tasks SET due=?1 WHERE id=?2",
                params![next.to_string(), task_id],
            )?;
        }
        tx.commit()?;
        Ok(Occurrence {
            id,
            task_id,
            due_date: recorded_due,
            completed_at,
        })
    }

    /// Fetch a task by its stable ID, including archived tasks.
    pub fn get_task(&self, id: i64) -> Result<Option<Task>> {
        self.task(id)
    }

    fn task(&self, id: i64) -> Result<Option<Task>> {
        task_in_connection(&self.connection, id)
    }
}

fn validate_task(task: &Task) -> Result<()> {
    match (task.due, task.recurrence.as_ref()) {
        (None, Some(_)) => Err(StoreError::InvalidCompletion(
            "recurring task requires an initial due date",
        )),
        (Some(due), Some(rule)) if next_due(rule, due, task.anchor).is_none() => Err(
            StoreError::InvalidCompletion("recurrence cannot advance from task due date"),
        ),
        _ => Ok(()),
    }
}

fn date_text(date: Option<NaiveDate>) -> Option<String> {
    date.map(|value| value.to_string())
}
fn timestamp_text(timestamp: NaiveDateTime) -> String {
    timestamp.format("%Y-%m-%dT%H:%M:%S%.f").to_string()
}
fn advance_text(advance: AdvanceMode) -> &'static str {
    match advance {
        AdvanceMode::Anchor => "anchor",
        AdvanceMode::Slide => "slide",
    }
}

fn advance_from_text(value: String) -> rusqlite::Result<AdvanceMode> {
    match value.as_str() {
        "anchor" => Ok(AdvanceMode::Anchor),
        "slide" => Ok(AdvanceMode::Slide),
        _ => Err(conversion_error(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "invalid recurrence advance mode",
        ))),
    }
}

fn recurrence_text(recurrence: Option<&Recurrence>) -> Result<Option<String>> {
    recurrence
        .map(serde_json::to_string)
        .transpose()
        .map_err(Into::into)
}

fn task_in_connection(connection: &Connection, id: i64) -> Result<Option<Task>> {
    connection
        .query_row(
            "SELECT id, name, due, recurrence_json, anchor, collapse, advance_mode, created_at, archived FROM tasks WHERE id=?1",
            [id],
            task_from_row,
        )
        .optional()
        .map_err(Into::into)
}

fn task_in_transaction(transaction: &Transaction<'_>, id: i64) -> Result<Option<Task>> {
    transaction
        .query_row(
            "SELECT id, name, due, recurrence_json, anchor, collapse, advance_mode, created_at, archived FROM tasks WHERE id=?1",
            [id],
            task_from_row,
        )
        .optional()
        .map_err(Into::into)
}

fn conversion_error(error: impl std::error::Error + Send + Sync + 'static) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(error))
}

fn task_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Task> {
    let due: Option<String> = row.get(2)?;
    let recurrence: Option<String> = row.get(3)?;
    Ok(Task {
        id: row.get(0)?,
        name: row.get(1)?,
        due: due
            .map(|value| value.parse::<NaiveDate>().map_err(conversion_error))
            .transpose()?,
        recurrence: recurrence
            .map(|value| serde_json::from_str(&value).map_err(conversion_error))
            .transpose()?,
        anchor: row
            .get::<_, String>(4)?
            .parse::<NaiveDate>()
            .map_err(conversion_error)?,
        collapse: row.get(5)?,
        advance: advance_from_text(row.get(6)?)?,
        created_at: row
            .get::<_, String>(7)?
            .parse::<NaiveDateTime>()
            .map_err(conversion_error)?,
        archived: row.get(8)?,
    })
}

fn occurrence_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Occurrence> {
    Ok(Occurrence {
        id: row.get(0)?,
        task_id: row.get(1)?,
        due_date: row
            .get::<_, String>(2)?
            .parse::<NaiveDate>()
            .map_err(conversion_error)?,
        completed_at: row
            .get::<_, String>(3)?
            .parse::<NaiveDateTime>()
            .map_err(conversion_error)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{NaiveDate, NaiveDateTime};
    use litany_core::{Recurrence, Task};

    fn date(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).unwrap()
    }
    fn time(year: i32, month: u32, day: u32) -> NaiveDateTime {
        date(year, month, day).and_hms_opt(12, 0, 0).unwrap()
    }
    fn task(due: Option<NaiveDate>, recurrence: Option<Recurrence>) -> Task {
        Task {
            id: 0,
            name: "wash teeth".into(),
            due,
            recurrence,
            anchor: date(2026, 1, 31),
            collapse: false,
            advance: AdvanceMode::Anchor,
            created_at: time(2026, 1, 1),
            archived: false,
        }
    }

    #[test]
    fn create_complete_and_relist_round_trip() {
        let mut store = Store::open_in_memory().unwrap();
        let task = store
            .create_task(&task(
                Some(date(2026, 9, 23)),
                Some(Recurrence::Weekly { weekday: Some(2) }),
            ))
            .unwrap();
        let occurrence = store
            .record_occurrence(task.id, date(2026, 9, 23), time(2026, 9, 24))
            .unwrap();
        assert_eq!(occurrence.due_date, date(2026, 9, 23));
        let listed = store.list_tasks(Some(date(2026, 9, 30))).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].due, Some(date(2026, 9, 30)));
        assert_eq!(store.occurrences_for_task(task.id).unwrap().len(), 1);
    }

    #[test]
    fn monthly_completion_advances_from_satisfied_due_and_clamps() {
        let mut store = Store::open_in_memory().unwrap();
        let task = store
            .create_task(&task(
                Some(date(2026, 1, 31)),
                Some(Recurrence::Monthly { day: Some(31) }),
            ))
            .unwrap();
        store
            .record_occurrence(task.id, date(2026, 1, 31), time(2026, 2, 4))
            .unwrap();
        assert_eq!(
            store.list_tasks(None).unwrap()[0].due,
            Some(date(2026, 2, 28))
        );
    }

    #[test]
    fn failed_next_due_rolls_back_occurrence() {
        let mut store = Store::open_in_memory().unwrap();
        let task = store
            .create_task(&task(
                Some(NaiveDate::MAX.pred_opt().unwrap()),
                Some(Recurrence::Daily),
            ))
            .unwrap();
        // Simulate a legacy database row at the representable date boundary.
        store
            .connection
            .execute(
                "UPDATE tasks SET due=?1 WHERE id=?2",
                [NaiveDate::MAX.to_string(), task.id.to_string()],
            )
            .unwrap();
        assert!(matches!(
            store.record_occurrence(
                task.id,
                NaiveDate::MAX,
                NaiveDate::MAX.and_hms_opt(0, 0, 0).unwrap()
            ),
            Err(StoreError::UnrepresentableNextDue)
        ));
        assert!(store.occurrences_for_task(task.id).unwrap().is_empty());
    }

    #[test]
    fn invalid_recurring_tasks_are_rejected() {
        let mut store = Store::open_in_memory().unwrap();
        assert!(matches!(
            store.create_task(&task(None, Some(Recurrence::Daily))),
            Err(StoreError::InvalidCompletion(_))
        ));
    }

    #[test]
    fn foreign_key_enforcement_rejects_orphan_occurrences() {
        let store = Store::open_in_memory().unwrap();
        let result = store.connection.execute(
            "INSERT INTO occurrences (task_id, due_date, completed_at) VALUES (999, '2026-01-01', '2026-01-01T00:00:00')",
            [],
        );
        assert!(matches!(result, Err(rusqlite::Error::SqliteFailure(_, _))));
    }

    #[test]
    fn collapse_records_only_the_latest_satisfied_daily_due() {
        let mut store = Store::open_in_memory().unwrap();
        let mut input = task(Some(date(2026, 11, 1)), Some(Recurrence::Daily));
        input.anchor = date(2026, 11, 1);
        input.collapse = true;
        let created = store.create_task(&input).unwrap();
        let completed_at = date(2026, 11, 5).and_hms_opt(23, 59, 12).unwrap();

        let occurrence = store
            .record_occurrence(created.id, date(2026, 11, 1), completed_at)
            .unwrap();
        assert_eq!(occurrence.due_date, date(2026, 11, 5));
        assert_eq!(occurrence.completed_at, completed_at);
        let history = store.occurrences_for_task(created.id).unwrap();
        assert_eq!(history.len(), 1, "missed dates must not be backfilled");
        assert_eq!(history[0].due_date, date(2026, 11, 5));
        let stored = store.get_task(created.id).unwrap().unwrap();
        assert_eq!(stored.due, Some(date(2026, 11, 6)));
        assert!(stored.collapse);
        assert_eq!(stored.advance, AdvanceMode::Anchor);
    }

    #[test]
    fn slide_month_collapse_uses_completion_day_and_persists_across_reopen() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("litany.db");
        let mut input = task(
            Some(date(2026, 1, 31)),
            Some(Recurrence::Monthly { day: Some(31) }),
        );
        input.collapse = true;
        input.advance = AdvanceMode::Slide;
        let created = {
            let mut store = Store::open(&database).unwrap();
            let created = store.create_task(&input).unwrap();
            let completed_at = date(2026, 4, 30).and_hms_opt(7, 8, 9).unwrap();
            let occurrence = store
                .record_occurrence(created.id, date(2026, 1, 31), completed_at)
                .unwrap();
            assert_eq!(occurrence.due_date, date(2026, 4, 28));
            created
        };

        let reopened = Store::open(&database).unwrap();
        let stored = reopened.get_task(created.id).unwrap().unwrap();
        assert_eq!(stored.due, Some(date(2026, 5, 30)));
        assert!(stored.collapse);
        assert_eq!(stored.advance, AdvanceMode::Slide);
        let history = reopened.occurrences_for_task(created.id).unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].due_date, date(2026, 4, 28));
        assert_eq!(
            history[0].completed_at,
            date(2026, 4, 30).and_hms_opt(7, 8, 9).unwrap()
        );
    }

    #[test]
    fn advance_mode_does_not_enable_collapse_and_early_completion_consumes_one_step() {
        let mut store = Store::open_in_memory().unwrap();
        let mut stacking = task(Some(date(2026, 11, 1)), Some(Recurrence::Daily));
        stacking.anchor = date(2026, 11, 1);
        stacking.advance = AdvanceMode::Slide;
        let created = store.create_task(&stacking).unwrap();
        let occurrence = store
            .record_occurrence(
                created.id,
                date(2026, 11, 1),
                date(2026, 11, 5).and_hms_opt(12, 0, 0).unwrap(),
            )
            .unwrap();
        assert_eq!(occurrence.due_date, date(2026, 11, 1));
        assert_eq!(
            store.get_task(created.id).unwrap().unwrap().due,
            Some(date(2026, 11, 2))
        );

        let mut early = task(Some(date(2026, 11, 1)), Some(Recurrence::Daily));
        early.anchor = date(2026, 11, 1);
        early.collapse = true;
        early.advance = AdvanceMode::Slide;
        let early = store.create_task(&early).unwrap();
        let occurrence = store
            .record_occurrence(
                early.id,
                date(2026, 11, 1),
                date(2026, 10, 31).and_hms_opt(12, 0, 0).unwrap(),
            )
            .unwrap();
        assert_eq!(occurrence.due_date, date(2026, 11, 1));
        assert_eq!(
            store.get_task(early.id).unwrap().unwrap().due,
            Some(date(2026, 11, 2))
        );

        let mut monthly = task(
            Some(date(2026, 1, 31)),
            Some(Recurrence::Monthly { day: Some(31) }),
        );
        monthly.anchor = date(2026, 1, 31);
        monthly.collapse = true;
        monthly.advance = AdvanceMode::Slide;
        let monthly = store.create_task(&monthly).unwrap();
        let occurrence = store
            .record_occurrence(
                monthly.id,
                date(2026, 1, 31),
                date(2026, 1, 15).and_hms_opt(12, 0, 0).unwrap(),
            )
            .unwrap();
        assert_eq!(occurrence.due_date, date(2026, 1, 31));
        assert_eq!(
            store.get_task(monthly.id).unwrap().unwrap().due,
            Some(date(2026, 2, 28)),
            "early completion preserves the legacy step from the outstanding due"
        );
    }

    #[test]
    fn completion_uses_utc_date_boundary_for_overdue_and_on_time_collapses() {
        let mut store = Store::open_in_memory().unwrap();
        let mut input = task(Some(date(2026, 11, 1)), Some(Recurrence::Daily));
        input.anchor = date(2026, 11, 1);
        input.collapse = true;
        let created = store.create_task(&input).unwrap();

        let before_midnight = date(2026, 11, 4).and_hms_opt(23, 59, 59).unwrap();
        let first = store
            .record_occurrence(created.id, date(2026, 11, 1), before_midnight)
            .unwrap();
        assert_eq!(first.due_date, date(2026, 11, 4));
        assert_eq!(first.completed_at, before_midnight);

        let after_midnight = date(2026, 11, 5).and_hms_opt(0, 0, 1).unwrap();
        let second = store
            .record_occurrence(created.id, date(2026, 11, 5), after_midnight)
            .unwrap();
        assert_eq!(second.due_date, date(2026, 11, 5));
        assert_eq!(second.completed_at, after_midnight);
        assert_eq!(
            store
                .occurrences_for_task(created.id)
                .unwrap()
                .iter()
                .map(|row| row.due_date)
                .collect::<Vec<_>>(),
            vec![date(2026, 11, 4), date(2026, 11, 5)]
        );
    }

    #[test]
    fn migration_v2_preserves_legacy_rows_with_stacking_anchor_defaults() {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch(SCHEMA_V0).unwrap();
        connection.execute(
            "INSERT INTO tasks (id, name, due, recurrence_json, anchor, created_at, archived) VALUES (7, 'legacy', '2026-11-01', '{\"kind\":\"daily\"}', '2026-11-01', '2026-10-01T00:00:00', 0)",
            [],
        ).unwrap();
        connection.execute(
            "INSERT INTO occurrences (task_id, due_date, completed_at) VALUES (7, '2026-10-31', '2026-10-31T12:00:00')",
            [],
        ).unwrap();
        connection.pragma_update(None, "user_version", 1).unwrap();
        let mut store = Store { connection };

        store.migrate().unwrap();
        let version: u32 = store
            .connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, 2);
        let legacy = store.get_task(7).unwrap().unwrap();
        assert!(!legacy.collapse);
        assert_eq!(legacy.advance, AdvanceMode::Anchor);
        let history = store.occurrences_for_task(7).unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].due_date, date(2026, 10, 31));
    }
}
