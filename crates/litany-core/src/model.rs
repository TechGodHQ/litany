//! Domain model: Task, Recurrence, Occurrence.

use chrono::{NaiveDate, NaiveDateTime};
use serde::{Deserialize, Serialize};

/// How a collapsed completion advances the recurrence schedule.
///
/// `Anchor` preserves the recurrence's configured calendar anchor. `Slide`
/// advances by one full recurrence interval from the completion date. This
/// setting is meaningful when `Task::collapse` is enabled; ordinary stacking
/// retains the legacy due-date advancement behavior.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AdvanceMode {
    /// Keep the existing recurrence grid anchored to its configured weekday/day.
    #[default]
    Anchor,
    /// Start a full recurrence interval from the completion date.
    Slide,
}

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
    /// When true, one completion may collapse multiple overdue scheduled dues
    /// into one occurrence. Missing fields from legacy inputs default to false.
    #[serde(default)]
    pub collapse: bool,
    /// Advance mode for collapsed completions. Legacy inputs default to anchor.
    #[serde(default)]
    pub advance: AdvanceMode,
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
