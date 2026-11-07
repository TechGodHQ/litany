//! Pure streak derivation from occurrence history.
//!
//! An occurrence is attributed to the recurrence period identified by its
//! `due_date`, not its completion timestamp. This means a late completion
//! satisfies its scheduled period while `completed_at` remains the truthful
//! record of when the work happened.

use std::collections::BTreeMap;

use chrono::NaiveDate;

use crate::{next_due, next_due_for_mode, AdvanceMode, Occurrence, Recurrence};

/// Return the number of consecutively completed periods ending at `as_of`.
///
/// `initial_due` is the first materialized due date; `anchor` remains explicit
/// because several recurrence variants derive their schedule from it. Neither
/// is inferred from history: a task may have an anchor distinct from its first
/// due date, and inferring either would move the period grid after misses. The
/// current open period has a one-period grace rule: when it has no completion
/// but the immediately preceding period does, that preceding completed run
/// remains current. A missed preceding period therefore breaks the streak.
///
/// The supplied occurrences must be materialized schedule rows (as produced
/// by `litany-store`); this pure core does not fabricate or validate stored
/// history. Multiple completions for one due date count as one period. The
/// function is pure: callers provide `as_of`; it never reads a clock.
pub fn current(
    occurrences: &[Occurrence],
    rule: &Recurrence,
    initial_due: NaiveDate,
    anchor: NaiveDate,
    as_of: NaiveDate,
) -> usize {
    current_with_settings(
        occurrences,
        rule,
        initial_due,
        anchor,
        as_of,
        false,
        AdvanceMode::Anchor,
    )
}

/// Current streak using the task's current collapse and advance settings.
/// Historical setting changes are not reconstructed; the supplied settings
/// are applied to the available occurrence history.
pub fn current_with_settings(
    occurrences: &[Occurrence],
    rule: &Recurrence,
    initial_due: NaiveDate,
    anchor: NaiveDate,
    as_of: NaiveDate,
    collapse: bool,
    advance: AdvanceMode,
) -> usize {
    let completed = completion_dates(occurrences, initial_due, Some(as_of));
    let Some((&last_due, &last_completed_at)) = completed.last_key_value() else {
        return 0;
    };

    // A completion keeps the streak current through the following open
    // recurrence period. It expires only when the period after that begins
    // without a completion. At the representable-date boundary, a missing
    // following boundary leaves the known current period open rather than
    // inventing an expiry.
    let open_period =
        next_due_after_occurrence(rule, last_due, last_completed_at, anchor, collapse, advance);
    let expires_at = open_period.and_then(|open_period| {
        next_due_after_open_period(rule, open_period, anchor, collapse, advance)
    });
    if expires_at.is_some_and(|boundary| as_of >= boundary) {
        return 0;
    }

    consecutive_run_ending_at(&completed, rule, anchor, collapse, advance)
}

/// Return the largest run of consecutively completed recurrence periods.
///
/// `initial_due` is explicit because it is the first materialized period;
/// `anchor` controls recurrence defaults without being assumed to be due.
pub fn best(
    occurrences: &[Occurrence],
    rule: &Recurrence,
    initial_due: NaiveDate,
    anchor: NaiveDate,
) -> usize {
    best_with_settings(
        occurrences,
        rule,
        initial_due,
        anchor,
        false,
        AdvanceMode::Anchor,
    )
}

/// Best streak using the task's current collapse and advance settings.
/// Historical setting changes are not reconstructed; the supplied settings
/// are applied to the available occurrence history.
pub fn best_with_settings(
    occurrences: &[Occurrence],
    rule: &Recurrence,
    initial_due: NaiveDate,
    anchor: NaiveDate,
    collapse: bool,
    advance: AdvanceMode,
) -> usize {
    let completed = completion_dates(occurrences, initial_due, None);
    let mut best = 0;
    let mut run = 0;
    let mut previous = None;
    for (&due, &completed_at) in &completed {
        run = match previous {
            Some((prior_due, prior_completed_at))
                if next_due_after_occurrence(
                    rule,
                    prior_due,
                    prior_completed_at,
                    anchor,
                    collapse,
                    advance,
                ) == Some(due) =>
            {
                run + 1
            }
            _ => 1,
        };
        best = best.max(run);
        previous = Some((due, completed_at));
    }
    best
}

fn completion_dates(
    occurrences: &[Occurrence],
    initial_due: NaiveDate,
    as_of: Option<NaiveDate>,
) -> BTreeMap<NaiveDate, chrono::NaiveDateTime> {
    let mut completed = BTreeMap::new();
    for occurrence in occurrences {
        let due = occurrence.due_date;
        if due < initial_due || as_of.is_some_and(|date| due > date) {
            continue;
        }
        completed
            .entry(due)
            .and_modify(|completed_at: &mut chrono::NaiveDateTime| {
                *completed_at = (*completed_at).max(occurrence.completed_at);
            })
            .or_insert(occurrence.completed_at);
    }
    completed
}

fn consecutive_run_ending_at(
    completed: &BTreeMap<NaiveDate, chrono::NaiveDateTime>,
    rule: &Recurrence,
    anchor: NaiveDate,
    collapse: bool,
    advance: AdvanceMode,
) -> usize {
    let mut completed = completed.iter().rev();
    let Some((&last_due, _)) = completed.next() else {
        return 0;
    };

    let mut run = 1;
    let mut later = last_due;
    for (&earlier, &completed_at) in completed {
        if next_due_after_occurrence(rule, earlier, completed_at, anchor, collapse, advance)
            != Some(later)
        {
            break;
        }
        run += 1;
        later = earlier;
    }
    run
}

fn next_due_after_occurrence(
    rule: &Recurrence,
    due: NaiveDate,
    completed_at: chrono::NaiveDateTime,
    anchor: NaiveDate,
    collapse: bool,
    advance: AdvanceMode,
) -> Option<NaiveDate> {
    if collapse && advance == AdvanceMode::Slide && completed_at.date() >= due {
        next_due_for_mode(rule, completed_at.date(), anchor, AdvanceMode::Slide)
    } else {
        // Stacking and early completion retain the legacy one-step anchored
        // advancement, even when the task is configured for slide collapse.
        next_due(rule, due, anchor)
    }
}

fn next_due_after_open_period(
    rule: &Recurrence,
    open_period: NaiveDate,
    anchor: NaiveDate,
    collapse: bool,
    advance: AdvanceMode,
) -> Option<NaiveDate> {
    if collapse && advance == AdvanceMode::Slide {
        next_due_for_mode(rule, open_period, anchor, AdvanceMode::Slide)
    } else {
        next_due(rule, open_period, anchor)
    }
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;

    use super::*;

    fn date(y: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, month, day).unwrap()
    }

    fn occurrence(due_date: NaiveDate, completed_at: NaiveDate) -> Occurrence {
        Occurrence {
            id: 0,
            task_id: 1,
            due_date,
            completed_at: completed_at.and_hms_opt(12, 0, 0).unwrap(),
        }
    }

    #[test]
    fn perfect_daily_run_includes_open_period_grace() {
        let anchor = date(2026, 9, 1);
        let rule = Recurrence::Daily;
        let occurrences = [
            occurrence(date(2026, 9, 1), date(2026, 9, 1)),
            occurrence(date(2026, 9, 2), date(2026, 9, 2)),
            occurrence(date(2026, 9, 3), date(2026, 9, 3)),
        ];

        let initial_due = anchor;
        assert_eq!(
            current(&occurrences, &rule, initial_due, anchor, date(2026, 9, 3)),
            3
        );
        // September 4 is the following open period, so the prior run remains
        // current. It expires at the start of September 5 if September 4 is
        // not completed.
        assert_eq!(
            current(&occurrences, &rule, initial_due, anchor, date(2026, 9, 4)),
            3
        );
        assert_eq!(
            current(&occurrences, &rule, initial_due, anchor, date(2026, 9, 5)),
            0
        );
        let with_sep4 = [
            occurrence(date(2026, 9, 1), date(2026, 9, 1)),
            occurrence(date(2026, 9, 2), date(2026, 9, 2)),
            occurrence(date(2026, 9, 3), date(2026, 9, 3)),
            occurrence(date(2026, 9, 4), date(2026, 9, 4)),
        ];
        assert_eq!(
            current(&with_sep4, &rule, initial_due, anchor, date(2026, 9, 5)),
            4
        );
        assert_eq!(best(&occurrences, &rule, initial_due, anchor), 3);
    }

    #[test]
    fn missed_day_breaks_current_and_best_streaks() {
        let anchor = date(2026, 9, 1);
        let rule = Recurrence::Daily;
        // Deliberately unsorted, with two completions for September 4.
        let occurrences = [
            occurrence(date(2026, 9, 4), date(2026, 9, 5)),
            occurrence(date(2026, 9, 1), date(2026, 9, 1)),
            occurrence(date(2026, 9, 2), date(2026, 9, 2)),
            occurrence(date(2026, 9, 4), date(2026, 9, 4)),
        ];

        let initial_due = anchor;
        assert_eq!(
            current(&occurrences, &rule, initial_due, anchor, date(2026, 9, 4)),
            1
        );
        assert_eq!(best(&occurrences, &rule, initial_due, anchor), 2);
    }

    #[test]
    fn monthly_clamped_due_date_counts_as_its_period() {
        let anchor = date(2026, 1, 31);
        let rule = Recurrence::Monthly { day: None };
        let occurrences = [
            occurrence(date(2026, 1, 31), date(2026, 1, 31)),
            // Completed late, but it satisfies February's clamped period.
            occurrence(date(2026, 2, 28), date(2026, 3, 1)),
        ];

        let initial_due = anchor;
        assert_eq!(
            current(&occurrences, &rule, initial_due, anchor, date(2026, 2, 28)),
            2
        );
        assert_eq!(best(&occurrences, &rule, initial_due, anchor), 2);
    }

    #[test]
    fn every_six_weeks_uses_due_periods_not_completion_spacing() {
        let anchor = date(2026, 1, 1);
        let rule = Recurrence::FromLast { days: 42 };
        let occurrences = [
            occurrence(date(2026, 1, 1), date(2026, 1, 2)),
            occurrence(date(2026, 2, 12), date(2026, 2, 20)),
            occurrence(date(2026, 3, 26), date(2026, 3, 26)),
        ];

        let initial_due = anchor;
        assert_eq!(
            current(&occurrences, &rule, initial_due, anchor, date(2026, 4, 1)),
            3
        );
        assert_eq!(best(&occurrences, &rule, initial_due, anchor), 3);
    }

    #[test]
    fn six_week_grace_covers_the_full_open_period() {
        let anchor = date(2026, 1, 1);
        let rule = Recurrence::FromLast { days: 42 };
        let january_only = [occurrence(anchor, anchor)];

        for as_of in [date(2026, 2, 12), date(2026, 2, 13), date(2026, 3, 25)] {
            assert_eq!(current(&january_only, &rule, anchor, anchor, as_of), 1);
        }
        assert_eq!(
            current(&january_only, &rule, anchor, anchor, date(2026, 3, 26)),
            0
        );

        let with_february = [
            occurrence(anchor, anchor),
            occurrence(date(2026, 2, 12), date(2026, 2, 12)),
        ];
        assert_eq!(
            current(&with_february, &rule, anchor, anchor, date(2026, 3, 26)),
            2
        );
        assert_eq!(best(&with_february, &rule, anchor, anchor), 2);
    }

    #[test]
    fn recurrence_sized_open_period_grace_expires_at_the_following_boundary() {
        let cases = [
            (
                Recurrence::Weekly { weekday: Some(0) },
                date(2026, 1, 5),
                [date(2026, 1, 12), date(2026, 1, 15), date(2026, 1, 18)],
                date(2026, 1, 19),
            ),
            (
                Recurrence::EveryNWeeks { n: 2 },
                date(2026, 1, 5),
                [date(2026, 1, 19), date(2026, 1, 26), date(2026, 2, 1)],
                date(2026, 2, 2),
            ),
            (
                Recurrence::EveryNDays { n: 10 },
                date(2026, 1, 1),
                [date(2026, 1, 11), date(2026, 1, 15), date(2026, 1, 20)],
                date(2026, 1, 21),
            ),
        ];

        for (rule, anchor, during_open_period, expiry) in cases {
            let occurrences = [occurrence(anchor, anchor)];
            for as_of in during_open_period {
                assert_eq!(current(&occurrences, &rule, anchor, anchor, as_of), 1);
            }
            assert_eq!(current(&occurrences, &rule, anchor, anchor, expiry), 0);
        }
    }

    #[test]
    fn month_rules_keep_clamped_open_periods_current_and_restore_anchor_day() {
        let anchor = date(2024, 1, 31);
        let occurrences = [occurrence(anchor, anchor)];

        for rule in [
            Recurrence::Monthly { day: None },
            Recurrence::EveryNMonths { n: 1 },
        ] {
            // February is clamped in a leap year; March restores the 31st.
            assert_eq!(
                current(&occurrences, &rule, anchor, anchor, date(2024, 2, 29)),
                1
            );
            assert_eq!(
                current(&occurrences, &rule, anchor, anchor, date(2024, 3, 30)),
                1
            );
            assert_eq!(
                current(&occurrences, &rule, anchor, anchor, date(2024, 3, 31)),
                0
            );
        }
    }

    #[test]
    fn unrepresentable_following_boundary_keeps_known_open_period_current() {
        let max = NaiveDate::MAX;
        let previous = max.pred_opt().expect("NaiveDate has a predecessor");
        let occurrences = [occurrence(previous, previous)];

        assert_eq!(
            current(&occurrences, &Recurrence::Daily, previous, previous, max),
            1
        );
    }

    #[test]
    fn distinct_initial_due_and_anchor_preserve_the_materialized_schedule() {
        // Store permits a task anchor that differs from its first due date.
        // The first Wednesday due must not be replaced by an anchor-derived
        // January schedule.
        let anchor = date(2026, 1, 31);
        let initial_due = date(2026, 9, 23);
        let rule = Recurrence::Weekly { weekday: Some(2) };
        let occurrences = [occurrence(initial_due, date(2026, 9, 24))];

        assert_eq!(
            current(&occurrences, &rule, initial_due, anchor, date(2026, 9, 23)),
            1
        );
        assert_eq!(best(&occurrences, &rule, initial_due, anchor), 1);
    }

    #[test]
    fn empty_or_pre_anchor_history_has_no_streak() {
        let anchor = date(2026, 9, 1);
        let rule = Recurrence::Daily;
        let initial_due = anchor;
        assert_eq!(
            current(&[], &rule, initial_due, anchor, date(2026, 9, 1)),
            0
        );
        assert_eq!(best(&[], &rule, initial_due, anchor), 0);
        let before = [occurrence(date(2026, 8, 31), date(2026, 8, 31))];
        assert_eq!(best(&before, &rule, initial_due, anchor), 0);
    }

    #[test]
    fn slide_streaks_follow_completion_relative_months_and_preserve_real_gaps() {
        let anchor = date(2026, 1, 31);
        let rule = Recurrence::Monthly { day: Some(31) };
        let continuous = [
            occurrence(anchor, anchor),
            occurrence(date(2026, 2, 28), date(2026, 2, 28)),
            occurrence(date(2026, 3, 28), date(2026, 3, 28)),
        ];

        assert_eq!(
            current_with_settings(
                &continuous,
                &rule,
                anchor,
                anchor,
                date(2026, 4, 29),
                true,
                AdvanceMode::Slide,
            ),
            3
        );
        assert_eq!(
            best_with_settings(&continuous, &rule, anchor, anchor, true, AdvanceMode::Slide,),
            3
        );
        assert_eq!(
            current_with_settings(
                &continuous,
                &rule,
                anchor,
                anchor,
                date(2026, 5, 28),
                true,
                AdvanceMode::Slide,
            ),
            0,
            "an uncompleted open slide period expires at its next boundary"
        );

        // March 28 was missed; collapsing at the end of April records only
        // April 28 and must not reconnect the March-to-April streak.
        let with_gap = [
            continuous[0].clone(),
            continuous[1].clone(),
            occurrence(date(2026, 4, 28), date(2026, 4, 30)),
        ];
        assert_eq!(
            current_with_settings(
                &with_gap,
                &rule,
                anchor,
                anchor,
                date(2026, 4, 30),
                true,
                AdvanceMode::Slide,
            ),
            1
        );
        assert_eq!(
            best_with_settings(&with_gap, &rule, anchor, anchor, true, AdvanceMode::Slide,),
            2
        );
    }

    #[test]
    fn early_slide_completion_keeps_legacy_single_step_streak_boundary() {
        let due = date(2026, 11, 1);
        let occurrences = [occurrence(due, date(2026, 10, 31))];
        assert_eq!(
            current_with_settings(
                &occurrences,
                &Recurrence::Daily,
                due,
                due,
                due,
                true,
                AdvanceMode::Slide,
            ),
            1
        );
        assert_eq!(
            best_with_settings(
                &occurrences,
                &Recurrence::Daily,
                due,
                due,
                true,
                AdvanceMode::Slide,
            ),
            1
        );
    }
}
