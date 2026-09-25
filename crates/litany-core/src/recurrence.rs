//! Recurrence engine: compute next due dates from rules.

use chrono::{Datelike, NaiveDate};

use crate::model::Recurrence;

/// Compute the next due date strictly after `from`, given a rule.
///
/// `anchor` supplies defaults for `None` rule fields (weekly weekday,
/// monthly day-of-month).
///
/// Contract:
/// - When `Some(date)` is returned, `date > from` always holds.
/// - Returns `None` for invalid rules: a zero interval on any `EveryN*`
///   variant or `FromLast`, a weekday outside `0..=6`, or a day of month
///   outside `1..=31`.
/// - Returns `None` when the next occurrence is not representable as a
///   `NaiveDate` (upper date boundary). Never panics: all calendar
///   arithmetic is checked.
pub fn next_due(rule: &Recurrence, from: NaiveDate, anchor: NaiveDate) -> Option<NaiveDate> {
    use Recurrence::*;
    match rule {
        Daily => from.succ_opt(),
        EveryNDays { n } => {
            if *n == 0 {
                return None;
            }
            from.checked_add_signed(chrono::Duration::days(i64::from(*n)))
        }
        Weekly { weekday } => {
            let target = match weekday {
                Some(w) if *w <= 6 => *w,
                Some(_) => return None,
                None => anchor.weekday().num_days_from_monday(),
            };
            let current = from.weekday().num_days_from_monday();
            // target+7-current is in 1..=13, so delta is in 0..=6 and a
            // delta of 0 means "same weekday" -> advance a full week.
            let delta = (target + 7 - current) % 7;
            let next = if delta == 0 { 7 } else { delta };
            from.checked_add_signed(chrono::Duration::days(i64::from(next)))
        }
        EveryNWeeks { n } => {
            if *n == 0 {
                return None;
            }
            from.checked_add_signed(chrono::Duration::weeks(i64::from(*n)))
        }
        Monthly { day } => {
            let d = match day {
                Some(d) if (1..=31).contains(d) => *d,
                Some(_) => return None,
                None => anchor.day(),
            };
            next_monthday(from, d)
        }
        EveryNMonths { n } => {
            if *n == 0 {
                return None;
            }
            let d = anchor.day();
            // Each single-month step lands in the month after its input
            // (any date in a later month is strictly after `from`), so n
            // steps from `from` land exactly n months later, with the
            // anchor day clamped in the destination month. Bounded
            // arithmetic: no loop proportional to `n`.
            let month_index =
                i64::from(from.year()) * 12 + i64::from(from.month() - 1) + i64::from(*n);
            let y = i32::try_from(month_index.div_euclid(12)).ok()?;
            let m = u32::try_from(month_index.rem_euclid(12) + 1).ok()?;
            let last = days_in_month_opt(y, m)?;
            NaiveDate::from_ymd_opt(y, m, d.min(last))
        }
        FromLast { days } => {
            if *days == 0 {
                return None;
            }
            from.checked_add_signed(chrono::Duration::days(i64::from(*days)))
        }
    }
}

/// First date strictly after `from` whose day-of-month is `d` (clamped to
/// month length). The result is always in the month following `from`'s.
fn next_monthday(from: NaiveDate, d: u32) -> Option<NaiveDate> {
    let y = from.year();
    let m = from.month();
    let (ny, nm) = next_month(y, m)?;
    let last = days_in_month_opt(ny, nm)?;
    NaiveDate::from_ymd_opt(ny, nm, d.min(last))
}

/// The month after `y`-`m`, checked at the year boundary.
fn next_month(y: i32, m: u32) -> Option<(i32, u32)> {
    if m == 12 {
        Some((y.checked_add(1)?, 1))
    } else {
        Some((y, m + 1))
    }
}

/// Days in a month, leap-aware. Total: a table lookup with no next-month
/// construction, so it cannot panic at the date-range boundary.
fn days_in_month_opt(y: i32, m: u32) -> Option<u32> {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => Some(31),
        4 | 6 | 9 | 11 => Some(30),
        2 if is_leap_year(y) => Some(29),
        2 => Some(28),
        _ => None,
    }
}

fn is_leap_year(y: i32) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;

    use super::*;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    #[test]
    fn daily_advances_one_day() {
        let from = d(2026, 9, 23);
        assert_eq!(
            next_due(&Recurrence::Daily, from, from),
            Some(d(2026, 9, 24))
        );
    }

    #[test]
    fn weekly_lands_on_target_weekday() {
        // 2026-09-23 is a Wednesday.
        let from = d(2026, 9, 23);
        let next = next_due(&Recurrence::Weekly { weekday: Some(6) }, from, from).unwrap();
        assert_eq!(next, d(2026, 9, 27)); // Sunday
    }

    #[test]
    fn monthly_clamps_to_month_length() {
        let from = d(2026, 1, 31);
        let next = next_due(&Recurrence::Monthly { day: Some(31) }, from, from).unwrap();
        assert_eq!(next, d(2026, 2, 28));
    }

    #[test]
    fn every_six_weeks_from_last() {
        let from = d(2026, 9, 1);
        let next = next_due(&Recurrence::FromLast { days: 42 }, from, from).unwrap();
        assert_eq!(next, d(2026, 10, 13));
    }

    #[test]
    fn weekly_same_weekday_as_anchor() {
        let from = d(2026, 9, 23); // Wed
        let next = next_due(&Recurrence::Weekly { weekday: None }, from, from).unwrap();
        assert_eq!(next, d(2026, 9, 30)); // next Wed
    }

    // --- Zero intervals are invalid rules -> None ---

    #[test]
    fn zero_intervals_return_none() {
        let from = d(2026, 9, 23);
        assert_eq!(next_due(&Recurrence::EveryNDays { n: 0 }, from, from), None);
        assert_eq!(
            next_due(&Recurrence::EveryNWeeks { n: 0 }, from, from),
            None
        );
        assert_eq!(
            next_due(&Recurrence::EveryNMonths { n: 0 }, from, from),
            None
        );
        assert_eq!(
            next_due(&Recurrence::FromLast { days: 0 }, from, from),
            None
        );
    }

    // --- Out-of-range weekday / day-of-month -> None ---

    #[test]
    fn invalid_weekday_returns_none() {
        let from = d(2026, 9, 23);
        for w in [7u32, 8, 100, u32::MAX] {
            assert_eq!(
                next_due(&Recurrence::Weekly { weekday: Some(w) }, from, from),
                None,
                "weekday {w} must be rejected"
            );
        }
    }

    #[test]
    fn invalid_monthly_day_returns_none() {
        let from = d(2026, 9, 23);
        for day in [0u32, 32, 100, u32::MAX] {
            assert_eq!(
                next_due(&Recurrence::Monthly { day: Some(day) }, from, from),
                None,
                "day {day} must be rejected"
            );
        }
    }

    // --- Upper date boundary: None, never panic ---

    #[test]
    fn daily_at_max_date_returns_none() {
        let max = chrono::NaiveDate::MAX;
        assert_eq!(next_due(&Recurrence::Daily, max, max), None);
    }

    #[test]
    fn boundaries_at_max_date_return_none() {
        let max = chrono::NaiveDate::MAX;
        assert_eq!(next_due(&Recurrence::EveryNDays { n: 1 }, max, max), None);
        assert_eq!(next_due(&Recurrence::EveryNWeeks { n: 1 }, max, max), None);
        // `max` weekday is whatever it is; a weekly rule either targets it
        // (delta 0 -> +7 days -> overflow) or a later weekday (overflow).
        assert_eq!(
            next_due(&Recurrence::Weekly { weekday: None }, max, max),
            None
        );
        assert_eq!(next_due(&Recurrence::FromLast { days: 1 }, max, max), None);
    }

    #[test]
    fn monthly_in_december_at_max_year_exercises_month_length() {
        // The last representable month is December of NaiveDate::MAX's
        // year; the next occurrence would be January of MAX.year()+1,
        // which does not exist.
        let max = chrono::NaiveDate::MAX; // ...-12-31
        let dec15 = NaiveDate::from_ymd_opt(max.year(), 12, 15).unwrap();
        assert_eq!(
            next_due(&Recurrence::Monthly { day: Some(1) }, dec15, dec15),
            None
        );
        assert_eq!(
            next_due(&Recurrence::EveryNMonths { n: 1 }, dec15, dec15),
            None
        );
        // One year earlier the same rule is representable: day 31 clamps
        // into a 31-day January.
        let dec15_prev = NaiveDate::from_ymd_opt(max.year() - 1, 12, 15).unwrap();
        assert_eq!(
            next_due(
                &Recurrence::Monthly { day: Some(31) },
                dec15_prev,
                dec15_prev
            ),
            Some(NaiveDate::from_ymd_opt(max.year(), 1, 31).unwrap())
        );
    }

    #[test]
    fn huge_intervals_return_none_without_traversal() {
        let from = d(2026, 9, 23);
        assert_eq!(
            next_due(&Recurrence::EveryNMonths { n: u32::MAX }, from, from),
            None
        );
        assert_eq!(
            next_due(&Recurrence::EveryNDays { n: u32::MAX }, from, from),
            None
        );
        assert_eq!(
            next_due(&Recurrence::EveryNWeeks { n: u32::MAX }, from, from),
            None
        );
        assert_eq!(
            next_due(&Recurrence::FromLast { days: u32::MAX }, from, from),
            None
        );
    }

    // --- Strictly-after invariant across the matrix ---

    #[test]
    fn every_some_result_is_strictly_after_from() {
        let dates = [
            d(2026, 1, 31),
            d(2026, 2, 28),
            d(2024, 2, 29), // leap day
            d(2026, 9, 23),
            d(2026, 12, 31),
            d(2026, 7, 1),
        ];
        let anchors = [d(2026, 1, 31), d(2024, 2, 29), d(2026, 9, 23)];
        let rules = [
            Recurrence::Daily,
            Recurrence::EveryNDays { n: 1 },
            Recurrence::EveryNDays { n: 45 },
            Recurrence::Weekly { weekday: None },
            Recurrence::Weekly { weekday: Some(0) },
            Recurrence::Weekly { weekday: Some(6) },
            Recurrence::EveryNWeeks { n: 1 },
            Recurrence::EveryNWeeks { n: 13 },
            Recurrence::Monthly { day: None },
            Recurrence::Monthly { day: Some(1) },
            Recurrence::Monthly { day: Some(29) },
            Recurrence::Monthly { day: Some(31) },
            Recurrence::EveryNMonths { n: 1 },
            Recurrence::EveryNMonths { n: 6 },
            Recurrence::EveryNMonths { n: 48 },
            Recurrence::FromLast { days: 1 },
            Recurrence::FromLast { days: 42 },
        ];
        let mut checked = 0;
        for from in dates {
            for anchor in anchors {
                for rule in &rules {
                    if let Some(next) = next_due(rule, from, anchor) {
                        assert!(
                            next > from,
                            "rule {rule:?} from {from} anchor {anchor} gave {next}"
                        );
                        checked += 1;
                    }
                }
            }
        }
        assert!(
            checked > 200,
            "matrix should exercise many cases: {checked}"
        );
    }

    // --- Anchor fallback and clamping/restoration semantics ---

    #[test]
    fn monthly_none_day_uses_anchor_day() {
        let from = d(2026, 8, 15);
        let anchor = d(2026, 1, 31);
        // Anchor day 31 clamped into September = 30.
        let next = next_due(&Recurrence::Monthly { day: None }, from, anchor).unwrap();
        assert_eq!(next, d(2026, 9, 30));
    }

    #[test]
    fn jan_31_clamps_then_restores() {
        let rule = Recurrence::Monthly { day: Some(31) };
        let jan31 = d(2026, 1, 31);
        let feb = next_due(&rule, jan31, jan31).unwrap();
        assert_eq!(feb, d(2026, 2, 28)); // clamped, non-leap 2026
        let mar = next_due(&rule, feb, jan31).unwrap();
        assert_eq!(mar, d(2026, 3, 31)); // restored
                                         // Leap year: 2024-01-31 -> 2024-02-29.
        let jan31_24 = d(2024, 1, 31);
        assert_eq!(next_due(&rule, jan31_24, jan31_24).unwrap(), d(2024, 2, 29));
    }

    #[test]
    fn every_n_months_matches_iterated_single_steps() {
        // Golden dates pinning OLD per-month clamping semantics (each step
        // clamps the fixed anchor day): anchor 2025-01-31.
        let anchor = d(2025, 1, 31);
        let from = d(2025, 1, 31);
        assert_eq!(
            next_due(&Recurrence::EveryNMonths { n: 1 }, from, anchor),
            Some(d(2025, 2, 28))
        );
        assert_eq!(
            next_due(&Recurrence::EveryNMonths { n: 2 }, from, anchor),
            Some(d(2025, 3, 31))
        );
        assert_eq!(
            next_due(&Recurrence::EveryNMonths { n: 13 }, from, anchor),
            Some(d(2026, 2, 28))
        );
        assert_eq!(
            next_due(&Recurrence::EveryNMonths { n: 14 }, from, anchor),
            Some(d(2026, 3, 31))
        );
        // Leap-year February: anchor 2024-01-31.
        let leap_anchor = d(2024, 1, 31);
        assert_eq!(
            next_due(&Recurrence::EveryNMonths { n: 1 }, leap_anchor, leap_anchor),
            Some(d(2024, 2, 29))
        );
        assert_eq!(
            next_due(
                &Recurrence::EveryNMonths { n: 25 },
                leap_anchor,
                leap_anchor
            ),
            Some(d(2026, 2, 28))
        );
        // Composition: n single steps from `from` must equal the direct
        // n-month computation with the fixed anchor day.
        for from in [d(2025, 1, 31), d(2025, 3, 30), d(2024, 2, 29)] {
            let mut cur = from;
            for n in 1..=40u32 {
                cur = next_due(&Recurrence::EveryNMonths { n: 1 }, cur, anchor)
                    .expect("1-month step always representable here");
                let direct = next_due(&Recurrence::EveryNMonths { n }, from, anchor).unwrap();
                assert_eq!(
                    direct, cur,
                    "EveryNMonths({n}) from {from} diverged from iterated steps"
                );
            }
        }
    }

    #[test]
    fn from_last_advances_from_supplied_date_not_completion() {
        // The engine only sees dates the caller supplies; FromLast is
        // measured from the given `from` (the last due date).
        let from = d(2026, 9, 1);
        let anchor = d(2020, 1, 1); // anchor irrelevant for FromLast
        let next = next_due(&Recurrence::FromLast { days: 42 }, from, anchor).unwrap();
        assert_eq!(next, d(2026, 10, 13));
    }
}
