//! Turns one line of text into a task draft: `buy milk tomorrow 18:30 !! #home @Shopping`.

use chrono::{Datelike, Duration, NaiveDate};

use crate::model::{Priority, QuickParse};
use crate::recur::DATE_FMT;

const WEEKDAYS: &[(&[&str], u32)] = &[
    (&["пн", "понедельник", "mon", "monday"], 1),
    (&["вт", "вторник", "tue", "tuesday"], 2),
    (&["ср", "среда", "среду", "wed", "wednesday"], 3),
    (&["чт", "четверг", "thu", "thursday"], 4),
    (&["пт", "пятница", "пятницу", "fri", "friday"], 5),
    (&["сб", "суббота", "субботу", "sat", "saturday"], 6),
    (&["вс", "воскресенье", "sun", "sunday"], 7),
];

/// Words that only introduce a date or a time and are dropped together with it.
const PREPOSITIONS: &[&str] = &["в", "во", "на", "к", "at", "on", "by"];

fn date_word(word: &str, today: NaiveDate) -> Option<NaiveDate> {
    match word {
        "сегодня" | "today" => return Some(today),
        "завтра" | "tomorrow" => return Some(today + Duration::days(1)),
        "послезавтра" => return Some(today + Duration::days(2)),
        _ => {}
    }
    for (names, number) in WEEKDAYS {
        if names.contains(&word) {
            let now = today.weekday().number_from_monday();
            let ahead = (*number + 7 - now - 1) % 7 + 1;
            return Some(today + Duration::days(ahead as i64));
        }
    }
    numeric_date(word, today)
}

/// `DD.MM` or `DD.MM.YYYY`. A date without a year that has already passed means next year.
fn numeric_date(word: &str, today: NaiveDate) -> Option<NaiveDate> {
    let parts: Vec<&str> = word.split('.').collect();
    if !(2..=3).contains(&parts.len())
        || parts
            .iter()
            .any(|p| p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()))
    {
        return None;
    }
    if parts[0].len() > 2 || parts[1].len() > 2 {
        return None;
    }
    let day: u32 = parts[0].parse().ok()?;
    let month: u32 = parts[1].parse().ok()?;
    if parts.len() == 3 {
        let year: i32 = parts[2].parse().ok()?;
        let year = if parts[2].len() == 2 { 2000 + year } else { year };
        return NaiveDate::from_ymd_opt(year, month, day);
    }
    let this_year = NaiveDate::from_ymd_opt(today.year(), month, day)?;
    if this_year < today {
        NaiveDate::from_ymd_opt(today.year() + 1, month, day)
    } else {
        Some(this_year)
    }
}

fn time_word(word: &str) -> Option<String> {
    let (h, m) = word.split_once(':')?;
    if h.is_empty() || h.len() > 2 || m.len() != 2 || !h.bytes().chain(m.bytes()).all(|b| b.is_ascii_digit()) {
        return None;
    }
    let (h, m): (u32, u32) = (h.parse().ok()?, m.parse().ok()?);
    (h < 24 && m < 60).then(|| format!("{h:02}:{m:02}"))
}

pub fn parse(text: &str, today: NaiveDate) -> QuickParse {
    let mut out = QuickParse::default();
    let mut date: Option<NaiveDate> = None;
    let mut time: Option<String> = None;
    let mut title: Vec<&str> = Vec::new();

    for word in text.split_whitespace() {
        let lower = word.to_lowercase();
        let consumed = if let Some(tag) = word.strip_prefix('#').filter(|t| !t.is_empty()) {
            let tag = tag.to_lowercase();
            if !out.tags.contains(&tag) {
                out.tags.push(tag);
            }
            true
        } else if let Some(list) = word.strip_prefix('@').filter(|l| !l.is_empty()) {
            out.list_name = Some(list.replace('_', " "));
            true
        } else if !word.is_empty() && word.len() <= 3 && word.chars().all(|c| c == '!') {
            out.priority = Priority::from_i64(word.len() as i64);
            true
        } else if let (None, Some(t)) = (&time, time_word(&lower)) {
            time = Some(t);
            true
        } else if let (None, Some(d)) = (date, date_word(&lower, today)) {
            date = Some(d);
            true
        } else {
            false
        };
        if consumed {
            // "в пятницу", "at 18:30": the preposition belongs to the recognised token.
            let is_date_or_time = !word.starts_with(['#', '@', '!']);
            if is_date_or_time
                && title
                    .last()
                    .is_some_and(|p| PREPOSITIONS.contains(&p.to_lowercase().as_str()))
            {
                title.pop();
            }
        } else {
            title.push(word);
        }
    }

    out.title = title.join(" ");
    out.due = match (date, time) {
        (Some(d), Some(t)) => Some(format!("{}T{}", d.format(DATE_FMT), t)),
        (Some(d), None) => Some(d.format(DATE_FMT).to_string()),
        (None, Some(t)) => Some(format!("{}T{}", today.format(DATE_FMT), t)),
        (None, None) => None,
    };
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // 2026-10-05 is a Monday.
    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, 5).unwrap()
    }

    #[test]
    fn r23_full_line() {
        let p = parse("купить молоко завтра 18:30 !! #дом @Покупки", today());
        assert_eq!(p.title, "купить молоко");
        assert_eq!(p.due.as_deref(), Some("2026-10-06T18:30"));
        assert_eq!(p.priority, Priority::Medium);
        assert_eq!(p.tags, vec!["дом"]);
        assert_eq!(p.list_name.as_deref(), Some("Покупки"));
    }

    #[test]
    fn plain_text_is_untouched() {
        let p = parse("Позвонить в банк", today());
        assert_eq!(p.title, "Позвонить в банк");
        assert_eq!(p.due, None);
        assert_eq!(p.priority, Priority::None);
    }

    #[test]
    fn weekday_means_the_next_one_and_drops_the_preposition() {
        let p = parse("отчёт в пятницу", today());
        assert_eq!(p.title, "отчёт");
        assert_eq!(p.due.as_deref(), Some("2026-10-09"));
        // Today is Monday: "monday" is a week ahead, not today.
        assert_eq!(parse("standup on monday", today()).due.as_deref(), Some("2026-10-12"));
    }

    #[test]
    fn numeric_dates() {
        assert_eq!(parse("визит 15.11", today()).due.as_deref(), Some("2026-11-15"));
        assert_eq!(parse("визит 01.02", today()).due.as_deref(), Some("2027-02-01"));
        assert_eq!(parse("визит 01.02.2028", today()).due.as_deref(), Some("2028-02-01"));
        // Not a date: version numbers and impossible days stay in the title.
        assert_eq!(parse("обновить до 1.2.3.4", today()).title, "обновить до 1.2.3.4");
        assert_eq!(parse("релиз 45.13", today()).title, "релиз 45.13");
    }

    #[test]
    fn time_alone_means_today() {
        let p = parse("call at 9:05 !!!", today());
        assert_eq!(p.title, "call");
        assert_eq!(p.due.as_deref(), Some("2026-10-05T09:05"));
        assert_eq!(p.priority, Priority::High);
    }

    #[test]
    fn only_the_first_date_is_taken() {
        let p = parse("сравнить сегодня и завтра", today());
        assert_eq!(p.due.as_deref(), Some("2026-10-05"));
        assert_eq!(p.title, "сравнить и завтра");
    }
}
