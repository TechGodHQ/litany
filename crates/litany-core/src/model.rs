//! Domain model: Task, Recurrence, Occurrence.

use chrono::{NaiveDate, NaiveDateTime};
use serde::{Deserialize, Serialize};

/// How a task repeats. `None` on the task = one-shot / no-due task.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Recurrence {
    /// Every day.
    Daily,
    /// Every N days, counted from the last due date.
    EveryNDays { n: u32 },
    /// Every week on the given weekday (0=Mon .. 6=Sun). `None` = same
    /// weekday as the task's anchor date.
    Weekly { weekday: Option<u32> },
    /// Every N weeks from the last due date.
    EveryNWeeks { n: u32 },
    /// Every month on the given day of month (1..=31; clamped to month
    /// length, e.g. Jan 31 -> Feb 28). `None` = day of the anchor date.
    Monthly { day: Option<u32> },
    /// Every N months on the anchor day-of-month.
    EveryNMonths { n: u32 },
    /// Every N days counted from the last due date (for "every 6 weeks"
    /// style intervals that don't align to a weekday or month grid).
    FromLast { days: u32 },
}

/// A task. May have a due date and/or a recurrence; both optional.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: i64,
    pub name: String,
    /// Optional next due date (UTC date).
    pub due: Option<NaiveDate>,
    /// Optional recurrence rule.
    pub recurrence: Option<Recurrence>,
    /// Anchor date for rules that need a starting point.
    pub anchor: NaiveDate,
    pub created_at: NaiveDateTime,
    pub archived: bool,
}

/// One materialized completion of a task. History and streaks are
/// computed over this table; nothing else stores progress.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Occurrence {
    pub id: i64,
    pub task_id: i64,
    /// The due date this occurrence satisfies (UTC date).
    pub due_date: NaiveDate,
    /// When it was actually completed (UTC timestamp).
    pub completed_at: NaiveDateTime,
}
