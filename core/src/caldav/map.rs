//! Task ↔ VTODO: what goes into standard properties, what goes into
//! `X-LISTS-STATE`, and how an edit made by another client is recognised.

use std::collections::{BTreeMap, BTreeSet};

use base64::Engine;
use chrono::{Local, NaiveDate, NaiveDateTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::ical::{self, Component, Prop};
use crate::db::{self, is_sha256};
use crate::hlc;
use crate::model::{Freq, Repeat};
use crate::tag;

pub const STATE: &str = "X-LISTS-STATE";
pub const SHA_PARAM: &str = "X-LISTS-SHA256";
pub const PRODID: &str = "-//org.evsyukov//Lists//EN";
/// Larger attachments stay on the device: the whole object is re-sent on every edit.
pub const MAX_INLINE: usize = 5 * 1024 * 1024;
/// All inline attachments of one object together. In base64 and folded lines
/// this is 28 MB, which leaves the object readable (`MAX_BODY` in the client).
pub const MAX_INLINE_TOTAL: usize = 20 * 1024 * 1024;

/// field → (value, stamp)
pub type Registers = BTreeMap<String, (Value, String)>;

/// Everything the app knows about one task, as it travels in `X-LISTS-STATE`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TaskState {
    #[serde(rename = "f")]
    pub fields: Registers,
    /// attachment id → its registers
    #[serde(rename = "a", default, skip_serializing_if = "BTreeMap::is_empty")]
    pub attachments: BTreeMap<String, Registers>,
}

impl TaskState {
    fn text(&self, field: &str) -> Option<&str> {
        self.fields
            .get(field)
            .and_then(|(v, _)| v.as_str())
            .filter(|s| !s.is_empty())
    }

    pub fn encode(&self) -> String {
        let json = serde_json::to_vec(self).unwrap_or_default();
        base64::engine::general_purpose::STANDARD.encode(json)
    }

    /// `None` for anything that is not a state written by this app; registers
    /// with a malformed stamp are dropped.
    pub fn decode(value: &str) -> Option<TaskState> {
        let json = base64::engine::general_purpose::STANDARD.decode(value.trim()).ok()?;
        let mut state: TaskState = serde_json::from_slice(&json).ok()?;
        let valid = |r: &mut Registers| r.retain(|field, (_, stamp)| db::key_ok(field) && hlc::is_valid(stamp));
        valid(&mut state.fields);
        state.attachments.values_mut().for_each(valid);
        Some(state)
    }
}

/// The part of a task that standard properties can express.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Standard {
    pub title: String,
    pub notes: String,
    pub start: Option<String>,
    pub due: Option<String>,
    /// 0 none, 1 low, 2 medium, 3 high
    pub priority: i64,
    pub tags: BTreeSet<String>,
    /// Canonical RRULE text, `None` when the task does not repeat.
    pub rrule: Option<String>,
    pub done: bool,
    /// Closed as "won't do" (C27); `done` is then true as well.
    pub wont: bool,
    pub parent: Option<String>,
}

impl Standard {
    pub fn of(state: &TaskState) -> Standard {
        let repeat = state
            .fields
            .get("repeat")
            .and_then(|(v, _)| serde_json::from_value::<Repeat>(v.clone()).ok());
        Standard {
            title: state.text("title").unwrap_or_default().to_string(),
            notes: state.text("notes").unwrap_or_default().to_string(),
            start: state.text("start").map(str::to_string),
            due: state.text("due").map(str::to_string),
            priority: state
                .fields
                .get("priority")
                .and_then(|(v, _)| v.as_i64())
                .unwrap_or(0)
                .clamp(0, 3),
            tags: state
                .fields
                .iter()
                .filter_map(|(f, (v, _))| {
                    f.strip_prefix(tag::PREFIX)
                        .filter(|t| v.as_bool() == Some(true) && tag::name_ok(t))
                })
                .map(str::to_string)
                .collect(),
            rrule: repeat.as_ref().map(to_rrule),
            done: state.text("done").is_some(),
            wont: state.text("done").is_some() && state.text("wont") == state.text("done"),
            parent: state.text("parent").map(str::to_string),
        }
    }
}

/// What a VTODO says in standard properties, in the app's own value formats.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Remote {
    pub uid: String,
    pub standard: Standard,
    /// The rule behind `standard.rrule`, when the object has one this app can express.
    pub repeat: Option<Repeat>,
    /// The object has an RRULE that does not fit the app's model: leave repeats alone.
    pub foreign_rrule: bool,
    /// Completion moment in local time, when the object carries one.
    pub completed: Option<String>,
    pub state: Option<TaskState>,
}

pub fn read(calendar: &Component) -> Option<Remote> {
    let todo = calendar.sub("VTODO")?;
    let uid = todo.get("UID")?.value.trim().to_string();
    if uid.is_empty() {
        return None;
    }
    let text = |name: &str| todo.get(name).map(|p| ical::unescape(&p.value)).unwrap_or_default();
    let raw_rule = todo.get("RRULE").map(|p| p.value.clone());
    let repeat = raw_rule.as_deref().and_then(from_rrule);
    let status_done = todo
        .get("STATUS")
        .is_some_and(|p| p.value.eq_ignore_ascii_case("COMPLETED"));
    let cancelled = todo
        .get("STATUS")
        .is_some_and(|p| p.value.trim().eq_ignore_ascii_case("CANCELLED"));
    let completed = todo.get("COMPLETED").and_then(moment_from_ical);
    let standard = Standard {
        title: text("SUMMARY").trim().to_string(),
        notes: text("DESCRIPTION"),
        start: todo.get("DTSTART").and_then(moment_from_ical),
        due: todo.get("DUE").and_then(moment_from_ical),
        priority: match todo
            .get("PRIORITY")
            .and_then(|p| p.value.trim().parse::<i64>().ok())
            .unwrap_or(0)
        {
            1..=4 => 3,
            5 => 2,
            6..=9 => 1,
            _ => 0,
        },
        tags: todo
            .all("CATEGORIES")
            .flat_map(|p| ical::split_list(&p.value))
            .map(|t| t.to_lowercase().split_whitespace().collect::<Vec<_>>().join("-"))
            .filter(|t| tag::name_ok(t))
            .collect(),
        rrule: repeat.as_ref().map(to_rrule),
        done: status_done || cancelled || completed.is_some(),
        wont: cancelled,
        parent: todo
            .all("RELATED-TO")
            .find(|p| p.param("RELTYPE").is_none_or(|t| t.eq_ignore_ascii_case("PARENT")))
            .map(|p| p.value.trim().to_string())
            .filter(|p| !p.is_empty()),
    };
    Some(Remote {
        uid,
        foreign_rrule: raw_rule.is_some() && repeat.is_none(),
        repeat,
        completed,
        state: todo.get(STATE).and_then(|p| TaskState::decode(&p.value)),
        standard,
    })
}

/// Builds the object to upload. `base` is the object currently on the server:
/// everything in it that this app does not manage is kept as it is.
pub fn render(
    uid: &str,
    state: &TaskState,
    base: Option<&Component>,
    blob: &dyn Fn(&str) -> Option<Vec<u8>>,
    now: NaiveDateTime,
) -> Component {
    let mut calendar = base.cloned().unwrap_or_else(|| {
        let mut c = Component::new("VCALENDAR");
        c.props.push(Prop::new("VERSION", "2.0"));
        c.props.push(Prop::new("PRODID", PRODID));
        c
    });
    if calendar.sub("VTODO").is_none() {
        calendar.subs.push(Component::new("VTODO"));
    }
    let base_rule_is_foreign = base.and_then(read).is_some_and(|r| r.foreign_rrule);
    let standard = Standard::of(state);
    let stamp = to_utc_stamp(now);
    let todo = calendar.sub_mut("VTODO").expect("just ensured");

    let had_created = todo.get("CREATED").is_some();
    todo.set(Some(Prop::new("UID", uid)), "UID");
    todo.set(Some(Prop::new("DTSTAMP", stamp.clone())), "DTSTAMP");
    todo.set(Some(Prop::new("LAST-MODIFIED", stamp.clone())), "LAST-MODIFIED");
    if !had_created {
        todo.props.push(Prop::new("CREATED", stamp.clone()));
    }
    todo.set(Some(Prop::new("SUMMARY", ical::escape(&standard.title))), "SUMMARY");
    todo.set(
        (!standard.notes.is_empty()).then(|| Prop::new("DESCRIPTION", ical::escape(&standard.notes))),
        "DESCRIPTION",
    );
    todo.set(
        standard.start.as_deref().and_then(|v| moment_to_ical("DTSTART", v)),
        "DTSTART",
    );
    todo.set(standard.due.as_deref().and_then(|v| moment_to_ical("DUE", v)), "DUE");
    let priority = match standard.priority {
        3 => Some("1"),
        2 => Some("5"),
        1 => Some("9"),
        _ => None,
    };
    todo.set(priority.map(|p| Prop::new("PRIORITY", p)), "PRIORITY");
    let categories = standard
        .tags
        .iter()
        .map(|t| ical::escape(t))
        .collect::<Vec<_>>()
        .join(",");
    todo.set(
        (!categories.is_empty()).then(|| Prop::new("CATEGORIES", categories)),
        "CATEGORIES",
    );
    // A rule this app cannot express is not ours to remove.
    if standard.rrule.is_some() || !base_rule_is_foreign {
        todo.set(standard.rrule.as_deref().map(|r| Prop::new("RRULE", r)), "RRULE");
    }
    let done = state
        .text("done")
        .and_then(|d| NaiveDateTime::parse_from_str(d, "%Y-%m-%dT%H:%M").ok());
    todo.set(
        Some(Prop::new(
            "STATUS",
            match (standard.done, standard.wont) {
                (true, true) => "CANCELLED",
                (true, false) => "COMPLETED",
                _ => "NEEDS-ACTION",
            },
        )),
        "STATUS",
    );
    todo.set(done.map(|d| Prop::new("COMPLETED", local_to_utc_stamp(d))), "COMPLETED");
    todo.set(
        (standard.done && !standard.wont).then(|| Prop::new("PERCENT-COMPLETE", "100")),
        "PERCENT-COMPLETE",
    );
    todo.props
        .retain(|p| p.name != "RELATED-TO" || p.param("RELTYPE").is_some_and(|t| !t.eq_ignore_ascii_case("PARENT")));
    if let Some(parent) = &standard.parent {
        todo.props
            .push(Prop::new("RELATED-TO", parent.clone()).with("RELTYPE", "PARENT"));
    }

    // Attachments written by this app carry their hash; others are left alone.
    let mut kept: BTreeMap<String, Prop> = BTreeMap::new();
    todo.props.retain(|p| match (p.name.as_str(), p.param(SHA_PARAM)) {
        ("ATTACH", Some(sha)) => {
            kept.insert(sha.to_string(), p.clone());
            false
        }
        _ => true,
    });
    let wanted: BTreeSet<&str> = state
        .attachments
        .values()
        .filter(|a| a.get("deleted").and_then(|(v, _)| v.as_bool()) != Some(true))
        .filter_map(|a| a.get("sha256").and_then(|(v, _)| v.as_str()))
        .filter(|s| is_sha256(s))
        .collect();
    // What does not fit the object stays out of it. The order is that of the
    // hashes, so every device leaves out the same attachments.
    let mut room = MAX_INLINE_TOTAL;
    for sha in wanted {
        let fresh = blob(sha).filter(|bytes| bytes.len() <= MAX_INLINE).map(|bytes| {
            Prop::new("ATTACH", base64::engine::general_purpose::STANDARD.encode(bytes))
                .with("ENCODING", "BASE64")
                .with("VALUE", "BINARY")
                .with(SHA_PARAM, sha)
        });
        // A device that has not downloaded the content yet must not drop it from the object.
        let Some(prop) = fresh.or_else(|| kept.remove(sha)) else {
            continue;
        };
        let size = prop.value.trim_end_matches('=').len() * 3 / 4;
        if size <= room {
            room -= size;
            todo.props.push(prop);
        }
    }

    todo.set(Some(Prop::new(STATE, state.encode())), STATE);
    calendar
}

/// Attachment contents carried by the object, verified against their names.
pub fn blobs(calendar: &Component) -> Vec<(String, Vec<u8>)> {
    let Some(todo) = calendar.sub("VTODO") else {
        return Vec::new();
    };
    todo.all("ATTACH")
        .filter_map(|p| {
            let sha = p.param(SHA_PARAM).filter(|s| is_sha256(s))?;
            let bytes = base64::engine::general_purpose::STANDARD.decode(p.value.trim()).ok()?;
            (crate::store::hex(&Sha256::digest(&bytes)) == sha).then(|| (sha.to_string(), bytes))
        })
        .collect()
}

/// Identity of an object's content, independent of property order, line
/// folding and the timestamps every write refreshes.
pub fn fingerprint(calendar: &Component) -> String {
    fn canonical(c: &Component, out: &mut Vec<String>, depth: usize) {
        let mut props: Vec<String> = c
            .props
            .iter()
            .filter(|p| {
                !matches!(
                    p.name.as_str(),
                    "DTSTAMP" | "LAST-MODIFIED" | "CREATED" | "PRODID" | "SEQUENCE"
                )
            })
            .map(|p| {
                let mut params: Vec<String> = p.params.iter().map(|(n, v)| format!("{n}={v}")).collect();
                params.sort();
                format!("{depth}|{}|{}|{}", p.name, params.join(";"), p.value)
            })
            .collect();
        props.sort();
        out.push(format!("{depth}>{}", c.name));
        out.append(&mut props);
        let mut subs: Vec<Vec<String>> = c
            .subs
            .iter()
            .map(|s| {
                let mut lines = Vec::new();
                canonical(s, &mut lines, depth + 1);
                lines
            })
            .collect();
        subs.sort();
        out.extend(subs.into_iter().flatten());
    }
    let mut lines = Vec::new();
    canonical(calendar, &mut lines, 0);
    crate::store::hex(&Sha256::digest(lines.join("\n").as_bytes()))
}

// ---- dates ----

fn to_utc_stamp(utc: NaiveDateTime) -> String {
    utc.format("%Y%m%dT%H%M%SZ").to_string()
}

fn local_to_utc_stamp(local: NaiveDateTime) -> String {
    let utc = Local
        .from_local_datetime(&local)
        .earliest()
        .map(|d| d.with_timezone(&Utc).naive_utc())
        .unwrap_or(local);
    to_utc_stamp(utc)
}

/// `2026-10-05` → `DUE;VALUE=DATE:20261005`, `2026-10-05T18:30` → `DUE:20261005T183000` (floating).
fn moment_to_ical(name: &str, value: &str) -> Option<Prop> {
    if value.len() == 10 {
        let date = NaiveDate::parse_from_str(value, "%Y-%m-%d").ok()?;
        return Some(Prop::new(name, date.format("%Y%m%d").to_string()).with("VALUE", "DATE"));
    }
    let moment = NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M").ok()?;
    Some(Prop::new(name, moment.format("%Y%m%dT%H%M%S").to_string()))
}

/// Reads DATE, floating, zoned and UTC values. A UTC value becomes local time;
/// a value with a TZID keeps its wall-clock time.
fn moment_from_ical(prop: &Prop) -> Option<String> {
    let value = prop.value.trim();
    if value.len() == 8 {
        let date = NaiveDate::parse_from_str(value, "%Y%m%d").ok()?;
        return Some(date.format("%Y-%m-%d").to_string());
    }
    let (body, utc) = match value.strip_suffix('Z') {
        Some(body) => (body, true),
        None => (value, false),
    };
    let mut moment = NaiveDateTime::parse_from_str(body, "%Y%m%dT%H%M%S").ok()?;
    if utc {
        moment = Utc.from_utc_datetime(&moment).with_timezone(&Local).naive_local();
    }
    Some(moment.format("%Y-%m-%dT%H:%M").to_string())
}

// ---- recurrence ----

const DAYS: [&str; 7] = ["MO", "TU", "WE", "TH", "FR", "SA", "SU"];

pub fn to_rrule(rule: &Repeat) -> String {
    let mut parts = vec![format!(
        "FREQ={}",
        match rule.freq {
            Freq::Daily => "DAILY",
            Freq::Weekly => "WEEKLY",
            Freq::Monthly => "MONTHLY",
            Freq::Yearly => "YEARLY",
        }
    )];
    if rule.interval > 1 {
        parts.push(format!("INTERVAL={}", rule.interval));
    }
    let day = |d: u32| DAYS.get(d.wrapping_sub(1) as usize).copied();
    match rule.freq {
        Freq::Weekly => {
            let mut days: Vec<u32> = rule.weekdays.iter().copied().filter(|d| (1..=7).contains(d)).collect();
            days.sort_unstable();
            days.dedup();
            if !days.is_empty() {
                parts.push(format!(
                    "BYDAY={}",
                    days.into_iter().filter_map(day).collect::<Vec<_>>().join(",")
                ));
            }
        }
        Freq::Monthly => match (rule.nth, rule.nth_weekday.and_then(day)) {
            (Some(nth), Some(name)) => {
                parts.push(format!("BYDAY={}{name}", if nth < 0 { -1 } else { nth.clamp(1, 5) }))
            }
            // "The 31st" means the last day of shorter months, which is what -1 says.
            _ => match rule.monthday {
                Some(31) => parts.push("BYMONTHDAY=-1".into()),
                Some(d) if (1..=30).contains(&d) => parts.push(format!("BYMONTHDAY={d}")),
                _ => {}
            },
        },
        _ => {}
    }
    if let Some(count) = rule.count {
        parts.push(format!("COUNT={count}"));
    }
    if let Some(until) = rule
        .until
        .as_deref()
        .and_then(|u| NaiveDate::parse_from_str(u, "%Y-%m-%d").ok())
    {
        parts.push(format!("UNTIL={}", until.format("%Y%m%d")));
    }
    parts.join(";")
}

/// `None` when the rule uses anything the app's model cannot express.
pub fn from_rrule(value: &str) -> Option<Repeat> {
    let mut rule = Repeat {
        freq: Freq::Daily,
        interval: 1,
        weekdays: vec![],
        monthday: None,
        nth: None,
        nth_weekday: None,
        from_done: false,
        count: None,
        until: None,
    };
    let mut freq = None;
    let mut by_day: Vec<(Option<i32>, u32)> = Vec::new();
    for part in value.trim().split(';').filter(|p| !p.is_empty()) {
        let (key, val) = part.split_once('=')?;
        match key.to_ascii_uppercase().as_str() {
            "FREQ" => {
                freq = Some(match val.to_ascii_uppercase().as_str() {
                    "DAILY" => Freq::Daily,
                    "WEEKLY" => Freq::Weekly,
                    "MONTHLY" => Freq::Monthly,
                    "YEARLY" => Freq::Yearly,
                    _ => return None,
                })
            }
            "INTERVAL" => rule.interval = val.parse().ok().filter(|n| *n >= 1)?,
            "COUNT" => rule.count = Some(val.parse().ok()?),
            "UNTIL" => {
                let date = NaiveDate::parse_from_str(val.get(..8)?, "%Y%m%d").ok()?;
                rule.until = Some(date.format("%Y-%m-%d").to_string());
            }
            "BYDAY" => {
                for item in val.split(',') {
                    let split = item.len().checked_sub(2)?;
                    let (ordinal, name) = item.split_at(split);
                    let weekday = DAYS.iter().position(|d| d.eq_ignore_ascii_case(name))? as u32 + 1;
                    let ordinal = if ordinal.is_empty() {
                        None
                    } else {
                        Some(ordinal.parse::<i32>().ok()?)
                    };
                    by_day.push((ordinal, weekday));
                }
            }
            "BYMONTHDAY" => {
                rule.monthday = Some(match val.parse::<i32>().ok()? {
                    -1 => 31,
                    d @ 1..=31 => d as u32,
                    _ => return None,
                })
            }
            // The week start does not change any rule this model can hold.
            "WKST" => {}
            _ => return None,
        }
    }
    rule.freq = freq?;
    match rule.freq {
        Freq::Weekly if rule.monthday.is_none() && by_day.iter().all(|(o, _)| o.is_none()) => {
            rule.weekdays = by_day.into_iter().map(|(_, d)| d).collect();
            rule.weekdays.sort_unstable();
        }
        Freq::Monthly if by_day.len() == 1 && rule.monthday.is_none() => {
            let (ordinal, weekday) = by_day[0];
            rule.nth = Some(match ordinal? {
                -1 => -1,
                n @ 1..=5 => n,
                _ => return None,
            });
            rule.nth_weekday = Some(weekday);
        }
        Freq::Monthly | Freq::Daily | Freq::Yearly if by_day.is_empty() => {
            if rule.freq != Freq::Monthly && rule.monthday.is_some() {
                return None;
            }
        }
        _ => return None,
    }
    Some(rule)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn stamp(n: u64) -> String {
        format!("{n:012x}-0000-aaaaaaaaaaaa")
    }

    fn state(pairs: &[(&str, Value)]) -> TaskState {
        TaskState {
            fields: pairs
                .iter()
                .enumerate()
                .map(|(i, (f, v))| (f.to_string(), (v.clone(), stamp(i as u64 + 1))))
                .collect(),
            attachments: BTreeMap::new(),
        }
    }

    fn now() -> NaiveDateTime {
        NaiveDateTime::parse_from_str("2026-10-05T10:00", "%Y-%m-%dT%H:%M").unwrap()
    }

    fn weekly_rule() -> Repeat {
        from_rrule("FREQ=WEEKLY").unwrap()
    }

    #[test]
    fn c3_c4_standard_properties_and_state_round_trip() {
        let s = state(&[
            ("title", json!("Купить молоко, хлеб")),
            ("notes", json!("строка 1\nстрока 2")),
            ("start", json!("2026-10-04")),
            ("due", json!("2026-10-05T18:30")),
            ("priority", json!(3)),
            ("tag:дом", json!(true)),
            ("tag:старый", json!(false)),
            ("repeat", serde_json::to_value(weekly_rule()).unwrap()),
            ("parent", json!("parent-uid")),
            ("pos", json!("V")),
            ("remind", json!("2026-10-05T18:00")),
        ]);
        let cal = render("task-1", &s, None, &|_| None, now());
        let text = ical::serialize(&cal);
        assert!(text.contains("SUMMARY:Купить молоко\\, хлеб"));
        assert!(text.contains("DUE:20261005T183000"));
        assert!(text.contains("DTSTART;VALUE=DATE:20261004"));
        assert!(text.contains("PRIORITY:1"));
        assert!(text.contains("CATEGORIES:дом"));
        assert!(text.contains("RRULE:FREQ=WEEKLY"));
        assert!(text.contains("RELATED-TO;RELTYPE=PARENT:parent-uid"));
        assert!(text.contains("STATUS:NEEDS-ACTION"));

        let back = read(&ical::parse(&text).unwrap()).unwrap();
        assert_eq!(back.uid, "task-1");
        assert_eq!(
            back.standard,
            Standard::of(&s),
            "standard properties say what the state says"
        );
        assert_eq!(
            back.state.unwrap(),
            s,
            "fields without a standard property survive in the state"
        );
    }

    #[test]
    fn completion_is_written_and_read_back() {
        let s = state(&[("title", json!("x")), ("done", json!("2026-10-05T09:15"))]);
        let cal = render("t", &s, None, &|_| None, now());
        let text = ical::serialize(&cal);
        assert!(text.contains("STATUS:COMPLETED") && text.contains("PERCENT-COMPLETE:100"));
        let back = read(&cal).unwrap();
        assert!(back.standard.done);
        assert_eq!(
            back.completed.as_deref(),
            Some("2026-10-05T09:15"),
            "UTC on the wire, local time in the app"
        );
    }

    #[test]
    fn c7_foreign_properties_and_components_survive_a_rewrite() {
        let foreign = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Other//EN\r\nBEGIN:VTODO\r\nUID:t\r\nSUMMARY:old\r\nX-OTHER:keep\r\nATTACH:https://example.org/file.pdf\r\nRELATED-TO;RELTYPE=SIBLING:other\r\nBEGIN:VALARM\r\nACTION:DISPLAY\r\nTRIGGER:-PT15M\r\nEND:VALARM\r\nEND:VTODO\r\nEND:VCALENDAR\r\n";
        let base = ical::parse(foreign).unwrap();
        let cal = render("t", &state(&[("title", json!("new"))]), Some(&base), &|_| None, now());
        let todo = cal.sub("VTODO").unwrap();
        assert_eq!(todo.get("SUMMARY").unwrap().value, "new");
        assert_eq!(todo.get("X-OTHER").unwrap().value, "keep");
        assert_eq!(todo.get("ATTACH").unwrap().value, "https://example.org/file.pdf");
        assert_eq!(todo.get("RELATED-TO").unwrap().param("RELTYPE"), Some("SIBLING"));
        assert!(todo.sub("VALARM").is_some());
        assert_eq!(cal.get("PRODID").unwrap().value, "-//Other//EN");
    }

    #[test]
    fn c12_rule_the_app_cannot_express_is_left_alone() {
        let foreign = "BEGIN:VCALENDAR\r\nBEGIN:VTODO\r\nUID:t\r\nSUMMARY:x\r\nRRULE:FREQ=MONTHLY;BYDAY=MO,TU;BYSETPOS=-1\r\nEND:VTODO\r\nEND:VCALENDAR\r\n";
        let base = ical::parse(foreign).unwrap();
        let remote = read(&base).unwrap();
        assert!(remote.foreign_rrule && remote.standard.rrule.is_none());
        let cal = render("t", &state(&[("title", json!("x"))]), Some(&base), &|_| None, now());
        assert_eq!(
            cal.sub("VTODO").unwrap().get("RRULE").unwrap().value,
            "FREQ=MONTHLY;BYDAY=MO,TU;BYSETPOS=-1"
        );
    }

    #[test]
    fn rules_round_trip() {
        for text in [
            "FREQ=DAILY",
            "FREQ=DAILY;INTERVAL=3;COUNT=5",
            "FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR",
            "FREQ=WEEKLY;INTERVAL=2;UNTIL=20270101",
            "FREQ=MONTHLY;BYMONTHDAY=15",
            "FREQ=MONTHLY;BYMONTHDAY=-1",
            "FREQ=MONTHLY;INTERVAL=3;BYDAY=1MO",
            "FREQ=MONTHLY;BYDAY=-1FR",
            "FREQ=YEARLY",
        ] {
            let rule = from_rrule(text).unwrap_or_else(|| panic!("{text}"));
            assert_eq!(to_rrule(&rule), text);
        }
        assert_eq!(from_rrule("FREQ=MONTHLY;BYMONTHDAY=-1").unwrap().monthday, Some(31));
        assert_eq!(from_rrule("FREQ=WEEKLY;WKST=MO;BYDAY=TU").unwrap().weekdays, vec![2]);
        assert_eq!(
            from_rrule("FREQ=WEEKLY;UNTIL=20270101T000000Z")
                .unwrap()
                .until
                .as_deref(),
            Some("2027-01-01")
        );
        for unsupported in [
            "FREQ=HOURLY",
            "FREQ=YEARLY;BYMONTH=3",
            "FREQ=MONTHLY;BYDAY=MO,TU",
            "FREQ=WEEKLY;BYDAY=1MO",
            "INTERVAL=2",
        ] {
            assert!(from_rrule(unsupported).is_none(), "{unsupported}");
        }
    }

    #[test]
    fn foreign_values_are_read_in_the_apps_formats() {
        let foreign = "BEGIN:VCALENDAR\r\nBEGIN:VTODO\r\nUID:f\r\nSUMMARY: Call \r\nDUE;TZID=Europe/Moscow:20261005T183000\r\nPRIORITY:3\r\nCATEGORIES:Home,Work Stuff\r\nCATEGORIES:home\r\nEND:VTODO\r\nEND:VCALENDAR\r\n";
        let remote = read(&ical::parse(foreign).unwrap()).unwrap();
        assert_eq!(remote.standard.title, "Call");
        assert_eq!(remote.standard.due.as_deref(), Some("2026-10-05T18:30"));
        assert_eq!(remote.standard.priority, 3);
        assert_eq!(
            remote.standard.tags.iter().map(String::as_str).collect::<Vec<_>>(),
            ["home", "work-stuff"]
        );
        assert!(remote.state.is_none());
    }

    #[test]
    fn s51_a_tag_that_does_not_fit_its_register_is_not_read_or_written() {
        let fits = "a".repeat(196);
        let long = "b".repeat(197);
        let foreign = format!(
            "BEGIN:VCALENDAR\r\nBEGIN:VTODO\r\nUID:f\r\nSUMMARY:Task\r\nCATEGORIES:{fits},{long}\r\nEND:VTODO\r\nEND:VCALENDAR\r\n"
        );
        let remote = read(&ical::parse(&foreign).unwrap()).unwrap();
        assert_eq!(
            remote.standard.tags.into_iter().collect::<Vec<_>>(),
            [fits],
            "the category that fits is read, the longer one is not"
        );

        // A register from an earlier version does not become a standard property either.
        let field = format!("tag:{long}");
        let s = state(&[("title", json!("Task")), (field.as_str(), json!(true))]);
        assert!(Standard::of(&s).tags.is_empty());
        let text = ical::serialize(&render("task-1", &s, None, &|_| None, now()));
        assert!(!text.contains("CATEGORIES:"), "{text}");
    }

    #[test]
    fn c5_attachment_content_travels_with_the_object_and_is_verified() {
        let bytes = b"hello".to_vec();
        let sha = crate::store::hex(&Sha256::digest(&bytes));
        let mut s = state(&[("title", json!("x"))]);
        s.attachments.insert(
            "att-1".into(),
            [
                ("sha256".to_string(), (json!(sha), stamp(9))),
                ("name".to_string(), (json!("a.txt"), stamp(9))),
            ]
            .into(),
        );
        let with_blob = render("t", &s, None, &|wanted| (wanted == sha).then(|| bytes.clone()), now());
        assert_eq!(blobs(&with_blob), vec![(sha.clone(), bytes.clone())]);

        // A device without the content keeps what is already in the object.
        let kept = render("t", &s, Some(&with_blob), &|_| None, now());
        assert_eq!(blobs(&kept).len(), 1);

        // Tampered content is not accepted under the old name.
        let mut bad = with_blob.clone();
        let attach = bad
            .sub_mut("VTODO")
            .unwrap()
            .props
            .iter_mut()
            .find(|p| p.name == "ATTACH")
            .unwrap();
        attach.value = base64::engine::general_purpose::STANDARD.encode(b"other");
        assert!(blobs(&bad).is_empty());

        // A deleted attachment leaves the object.
        s.attachments
            .get_mut("att-1")
            .unwrap()
            .insert("deleted".into(), (json!(true), stamp(10)));
        assert!(blobs(&render("t", &s, Some(&with_blob), &|_| None, now())).is_empty());
    }

    #[test]
    fn c5_inline_attachments_are_capped_in_hash_order() {
        let shas: Vec<String> = (1..=5).map(|i| format!("{i:064x}")).collect();
        let mut s = state(&[("title", json!("x"))]);
        for sha in &shas {
            s.attachments.insert(
                format!("att-{sha}"),
                [("sha256".to_string(), (json!(sha), stamp(9)))].into(),
            );
        }
        let inline = |c: &Component| -> Vec<String> {
            c.sub("VTODO")
                .unwrap()
                .all("ATTACH")
                .filter_map(|p| p.param(SHA_PARAM).map(str::to_string))
                .collect()
        };
        let largest = vec![0u8; MAX_INLINE];
        let capped = render("t", &s, None, &|_| Some(largest.clone()), now());
        assert_eq!(inline(&capped), shas[..4]);
        assert!(
            ical::serialize(&capped).len() < 30 * 1024 * 1024,
            "a full object stays readable"
        );

        // An object written before the cap shrinks the same way, also on a device without the contents.
        let mut old = capped.clone();
        let todo = old.sub_mut("VTODO").unwrap();
        let content = todo.get("ATTACH").unwrap().value.clone();
        todo.props
            .insert(0, Prop::new("ATTACH", content).with(SHA_PARAM, &shas[4]));
        assert_eq!(inline(&old).len(), 5);
        assert_eq!(inline(&render("t", &s, Some(&old), &|_| None, now())), shas[..4]);
    }

    #[test]
    fn fingerprint_ignores_order_folding_and_timestamps_but_not_content() {
        let s = state(&[("title", json!("x")), ("due", json!("2026-10-05"))]);
        let a = render("t", &s, None, &|_| None, now());
        let later = render("t", &s, None, &|_| None, now() + chrono::Duration::hours(5));
        assert_eq!(fingerprint(&a), fingerprint(&later));

        let mut shuffled = a.clone();
        shuffled.sub_mut("VTODO").unwrap().props.reverse();
        assert_eq!(
            fingerprint(&a),
            fingerprint(&ical::parse(&ical::serialize(&shuffled)).unwrap())
        );

        let changed = render(
            "t",
            &state(&[("title", json!("y")), ("due", json!("2026-10-05"))]),
            None,
            &|_| None,
            now(),
        );
        assert_ne!(fingerprint(&a), fingerprint(&changed));
    }

    #[test]
    fn state_with_bad_stamps_is_cleaned() {
        let json = br#"{"f":{"title":["x","000000000001-0000-aaaaaaaaaaaa"],"notes":["y","zzz"]}}"#;
        let decoded = TaskState::decode(&base64::engine::general_purpose::STANDARD.encode(json)).unwrap();
        assert_eq!(decoded.fields.keys().collect::<Vec<_>>(), ["title"]);
        assert!(TaskState::decode("not base64 !!!").is_none());
    }
}
