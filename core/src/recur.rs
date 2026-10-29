//! Recurrence arithmetic on calendar dates.

use chrono::{Datelike, Duration, NaiveDate};

use crate::model::{Freq, Repeat};

pub const DATE_FMT: &str = "%Y-%m-%d";

/// Splits `YYYY-MM-DD` or `YYYY-MM-DDTHH:MM` into the date and the time suffix
/// (including the `T`, empty when there is none).
pub fn split(value: &str) -> Option<(NaiveDate, &str)> {
    let date = NaiveDate::parse_from_str(value.get(..10)?, DATE_FMT).ok()?;
    Some((date, &value[10..]))
}

pub fn shift(value: &str, days: i64) -> Option<String> {
    let (date, time) = split(value)?;
    Some(format!("{}{}", (date + Duration::days(days)).format(DATE_FMT), time))
}

fn days_in_month(year: i32, month: u32) -> u32 {
    let (y, m) = if month == 12 { (year + 1, 1) } else { (year, month + 1) };
    NaiveDate::from_ymd_opt(y, m, 1)
        .and_then(|d| d.pred_opt())
        .map_or(28, |d| d.day())
}

fn add_months(year: i32, month: u32, add: u32) -> (i32, u32) {
    let total = year as i64 * 12 + (month as i64 - 1) + add as i64;
    ((total.div_euclid(12)) as i32, (total.rem_euclid(12)) as u32 + 1)
}

fn clamped(year: i32, month: u32, day: u32) -> Option<NaiveDate> {
    NaiveDate::from_ymd_opt(year, month, day.clamp(1, days_in_month(year, month)))
}

/// n-th (1..5) or last (-1) given weekday of a month; a missing fifth falls back to the last.
fn nth_weekday(year: i32, month: u32, nth: i32, weekday: u32) -> Option<NaiveDate> {
    let first = NaiveDate::from_ymd_opt(year, month, 1)?;
    let offset = (weekday as i64 - first.weekday().number_from_monday() as i64).rem_euclid(7);
    let first_hit = first + Duration::days(offset);
    let last_day = days_in_month(year, month);
    let mut hits = Vec::new();
    let mut d = first_hit;
    while d.month() == month && d.day() <= last_day {
        hits.push(d);
        d += Duration::days(7);
    }
    if nth < 0 || nth as usize > hits.len() {
        hits.last().copied()
    } else {
        hits.get((nth.max(1) - 1) as usize).copied()
    }
}

fn step(rule: &Repeat, from: NaiveDate) -> Option<NaiveDate> {
    let interval = rule.interval.max(1);
    match rule.freq {
        Freq::Daily => Some(from + Duration::days(interval as i64)),
        Freq::Weekly => {
            let mut days: Vec<i64> = rule
                .weekdays
                .iter()
                .filter(|d| (1..=7).contains(*d))
                .map(|d| *d as i64)
                .collect();
            days.sort_unstable();
            days.dedup();
            if days.is_empty() {
                return Some(from + Duration::weeks(interval as i64));
            }
            let today = from.weekday().number_from_monday() as i64;
            match days.iter().find(|d| **d > today) {
                Some(next) => Some(from + Duration::days(next - today)),
                None => {
                    let monday = from - Duration::days(today - 1);
                    Some(monday + Duration::weeks(interval as i64) + Duration::days(days[0] - 1))
                }
            }
        }
        Freq::Monthly => {
            let (y, m) = add_months(from.year(), from.month(), interval);
            match (rule.nth, rule.nth_weekday) {
                (Some(nth), Some(wd)) if (1..=7).contains(&wd) => nth_weekday(y, m, nth, wd),
                _ => clamped(y, m, rule.monthday.unwrap_or(from.day())),
            }
        }
        Freq::Yearly => clamped(from.year() + interval as i32, from.month(), from.day()),
    }
}

/// Date of the occurrence that follows `current`, or `None` when the rule is exhausted.
///
/// `current` is the date of the occurrence being completed (due date, else start
/// date), `done` is the completion date. Occurrences already in the past are
/// skipped, so an overdue task never lands on another overdue date.
pub fn next_occurrence(rule: &Repeat, current: Option<NaiveDate>, done: NaiveDate) -> Option<NaiveDate> {
    if rule.count == Some(0) || rule.count == Some(1) {
        return None;
    }
    let anchor = if rule.from_done { done } else { current.unwrap_or(done) };
    let mut next = step(rule, anchor)?;
    let mut guard = 0;
    while next <= done && guard < 10_000 {
        next = step(rule, next)?;
        guard += 1;
    }
    if let Some(until) = rule
        .until
        .as_deref()
        .and_then(|u| NaiveDate::parse_from_str(u, DATE_FMT).ok())
    {
        if next > until {
            return None;
        }
    }
    Some(next)
}

/// Fills in what the rule needs to stay stable across months: a monthly rule
/// without an explicit day remembers the day of the first due date, so that
/// the 31st survives February.
pub fn normalized(mut rule: Repeat, due: Option<&str>) -> Repeat {
    rule.interval = rule.interval.max(1);
    if rule.freq == Freq::Monthly && rule.monthday.is_none() && rule.nth.is_none() && !rule.from_done {
        if let Some((date, _)) = due.and_then(split) {
            rule.monthday = Some(date.day());
        }
    }
    rule
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, DATE_FMT).unwrap()
    }

    fn rule(freq: Freq, interval: u32) -> Repeat {
        Repeat {
            freq,
            interval,
            weekdays: vec![],
            monthday: None,
            nth: None,
            nth_weekday: None,
            from_done: false,
            count: None,
            until: None,
        }
    }

    #[test]
    fn daily_and_weekly_from_due() {
        assert_eq!(
            next_occurrence(&rule(Freq::Daily, 3), Some(d("2026-10-05")), d("2026-10-05")),
            Some(d("2026-10-08"))
        );
        assert_eq!(
            next_occurrence(&rule(Freq::Weekly, 1), Some(d("2026-10-05")), d("2026-10-05")),
            Some(d("2026-10-12"))
        );
        assert_eq!(
            next_occurrence(&rule(Freq::Weekly, 2), Some(d("2026-10-05")), d("2026-10-03")),
            Some(d("2026-10-19"))
        );
    }

    #[test]
    fn overdue_task_skips_past_occurrences() {
        // Due three weeks ago, completed today (a Monday): next is the coming Monday.
        assert_eq!(
            next_occurrence(&rule(Freq::Weekly, 1), Some(d("2026-09-14")), d("2026-10-05")),
            Some(d("2026-10-12"))
        );
    }

    #[test]
    fn from_completion_date() {
        let mut r = rule(Freq::Daily, 10);
        r.from_done = true;
        assert_eq!(
            next_occurrence(&r, Some(d("2026-09-01")), d("2026-10-05")),
            Some(d("2026-10-15"))
        );
    }

    #[test]
    fn weekdays_within_and_across_weeks() {
        let mut r = rule(Freq::Weekly, 1);
        r.weekdays = vec![1, 2, 3, 4, 5];
        // Monday -> Tuesday, Friday -> Monday.
        assert_eq!(
            next_occurrence(&r, Some(d("2026-10-05")), d("2026-10-05")),
            Some(d("2026-10-06"))
        );
        assert_eq!(
            next_occurrence(&r, Some(d("2026-10-09")), d("2026-10-09")),
            Some(d("2026-10-12"))
        );
        let mut every_other = rule(Freq::Weekly, 2);
        every_other.weekdays = vec![2, 4];
        // Thursday -> Tuesday two weeks later.
        assert_eq!(
            next_occurrence(&every_other, Some(d("2026-10-08")), d("2026-10-08")),
            Some(d("2026-10-20"))
        );
    }

    #[test]
    fn r18_month_end_is_clamped_and_restored() {
        let r = normalized(rule(Freq::Monthly, 1), Some("2026-01-31"));
        assert_eq!(r.monthday, Some(31));
        let feb = next_occurrence(&r, Some(d("2026-01-31")), d("2026-01-31")).unwrap();
        assert_eq!(feb, d("2026-02-28"));
        assert_eq!(next_occurrence(&r, Some(feb), feb), Some(d("2026-03-31")));
    }

    #[test]
    fn quarterly_and_yearly() {
        assert_eq!(
            next_occurrence(&rule(Freq::Monthly, 3), Some(d("2026-11-15")), d("2026-11-15")),
            Some(d("2027-02-15"))
        );
        assert_eq!(
            next_occurrence(&rule(Freq::Yearly, 1), Some(d("2028-02-29")), d("2028-02-29")),
            Some(d("2029-02-28"))
        );
    }

    #[test]
    fn nth_weekday_of_month() {
        let mut r = rule(Freq::Monthly, 1);
        r.nth = Some(1);
        r.nth_weekday = Some(1);
        assert_eq!(
            next_occurrence(&r, Some(d("2026-10-05")), d("2026-10-05")),
            Some(d("2026-11-02"))
        );
        r.nth = Some(-1);
        r.nth_weekday = Some(5);
        assert_eq!(
            next_occurrence(&r, Some(d("2026-10-30")), d("2026-10-30")),
            Some(d("2026-11-27"))
        );
        r.nth = Some(5);
        r.nth_weekday = Some(1);
        // November 2026 has five Mondays, December only four: fall back to the last.
        assert_eq!(
            next_occurrence(&r, Some(d("2026-11-30")), d("2026-11-30")),
            Some(d("2026-12-28"))
        );
    }

    #[test]
    fn count_and_until_end_the_series() {
        let mut r = rule(Freq::Daily, 1);
        r.count = Some(1);
        assert_eq!(next_occurrence(&r, Some(d("2026-10-05")), d("2026-10-05")), None);
        r.count = Some(2);
        assert!(next_occurrence(&r, Some(d("2026-10-05")), d("2026-10-05")).is_some());
        let mut u = rule(Freq::Weekly, 1);
        u.until = Some("2026-10-10".into());
        assert_eq!(next_occurrence(&u, Some(d("2026-10-05")), d("2026-10-05")), None);
    }

    #[test]
    fn shift_keeps_time() {
        assert_eq!(shift("2026-10-05T09:30", 7).as_deref(), Some("2026-10-12T09:30"));
        assert_eq!(shift("2026-12-31", 1).as_deref(), Some("2027-01-01"));
    }
}
