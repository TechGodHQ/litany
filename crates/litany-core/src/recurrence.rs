//! Recurrence engine: compute next due dates from rules.

use chrono::{Datelike, NaiveDate};

use crate::model::Recurrence;

/// Compute the next due date strictly after `from`, given a rule.
pub fn next_due(rule: &Recurrence, from: NaiveDate, anchor: NaiveDate) -> Option<NaiveDate> {
    use Recurrence::*;
    match rule {
        Daily => Some(from.succ_opt().expect("succ_opt only fails on overflow")),
        EveryNDays { n } => from.checked_add_signed(chrono::Duration::days(*n as i64)),
        Weekly { weekday } => {
            let target = weekday.unwrap_or_else(|| anchor.weekday().num_days_from_monday());
            let current = from.weekday().num_days_from_monday();
            let delta = (target + 7 - current) % 7;
            let next = if delta == 0 { 7 } else { delta };
            from.checked_add_signed(chrono::Duration::days(next as i64))
        }
        EveryNWeeks { n } => from.checked_add_signed(chrono::Duration::weeks(*n as i64)),
        Monthly { day } => {
            let d = day.unwrap_or_else(|| anchor.day());
            next_monthday(from, d)
        }
        EveryNMonths { n } => {
            let d = anchor.day();
            let mut cur = from;
            for _ in 0..*n {
                cur = next_monthday(cur, d)?;
            }
            Some(cur)
        }
        FromLast { days } => from.checked_add_signed(chrono::Duration::days(*days as i64)),
    }
}

/// First date > `from` whose day-of-month is `d` (clamped to month length).
fn next_monthday(from: NaiveDate, d: u32) -> Option<NaiveDate> {
    let mut y = from.year();
    let mut m = from.month();
    loop {
        m += 1;
        if m > 12 {
            m = 1;
            y += 1;
        }
        let last = days_in_month(y, m);
        let day = d.min(last);
        let cand = NaiveDate::from_ymd_opt(y, m, day)?;
        if cand > from {
            return Some(cand);
        }
    }
}

fn days_in_month(y: i32, m: u32) -> u32 {
    NaiveDate::from_ymd_opt(
        if m == 12 { y + 1 } else { y },
        if m == 12 { 1 } else { m + 1 },
        1,
    )
    .unwrap()
    .signed_duration_since(NaiveDate::from_ymd_opt(y, m, 1).unwrap())
    .num_days() as u32
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;

    use super::*;

    #[test]
    fn daily_advances_one_day() {
        let d = NaiveDate::from_ymd_opt(2026, 9, 23).unwrap();
        assert_eq!(
            next_due(&Recurrence::Daily, d, d),
            Some(NaiveDate::from_ymd_opt(2026, 9, 24).unwrap())
        );
    }

    #[test]
    fn weekly_lands_on_target_weekday() {
        // 2026-09-23 is a Wednesday.
        let d = NaiveDate::from_ymd_opt(2026, 9, 23).unwrap();
        let next = next_due(&Recurrence::Weekly { weekday: Some(6) }, d, d).unwrap();
        assert_eq!(next, NaiveDate::from_ymd_opt(2026, 9, 27).unwrap()); // Sunday
    }

    #[test]
    fn monthly_clamps_to_month_length() {
        let d = NaiveDate::from_ymd_opt(2026, 1, 31).unwrap();
        let next = next_due(&Recurrence::Monthly { day: Some(31) }, d, d).unwrap();
        assert_eq!(next, NaiveDate::from_ymd_opt(2026, 2, 28).unwrap());
    }

    #[test]
    fn every_six_weeks_from_last() {
        let d = NaiveDate::from_ymd_opt(2026, 9, 1).unwrap();
        let next = next_due(&Recurrence::FromLast { days: 42 }, d, d).unwrap();
        assert_eq!(next, NaiveDate::from_ymd_opt(2026, 10, 13).unwrap());
    }

    #[test]
    fn weekly_same_weekday_as_anchor() {
        let d = NaiveDate::from_ymd_opt(2026, 9, 23).unwrap(); // Wed
        let next = next_due(&Recurrence::Weekly { weekday: None }, d, d).unwrap();
        assert_eq!(next, NaiveDate::from_ymd_opt(2026, 9, 30).unwrap()); // next Wed
    }
}
