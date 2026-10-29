//! Import from other task managers. Formats and what is known about each:
//! docs/specs/import.md.
//!
//! Every importer turns its source into the same small model, which is then
//! written as ordinary local changes. Ids are derived from the ids in the
//! source, so importing the same file twice updates instead of duplicating.

use std::collections::{BTreeMap, HashMap};
use std::io::Read;
use std::path::{Path, PathBuf};

use chrono::{DateTime, NaiveDate, NaiveDateTime, TimeZone, Utc};
use rusqlite::{Connection, OpenFlags};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::db::{KIND_ATTACHMENT, KIND_FILTER, KIND_LIST, KIND_TASK};
use crate::error::{AppError, Result};
use crate::model::{FilterSpec, Freq, Priority, Repeat, SortMode, INBOX_ID};
use crate::store::{self, hex, Store};
use crate::LogLevel;
use crate::{order, quickadd};

#[derive(Debug, Clone, PartialEq, Default, uniffi::Record)]
pub struct ImportReport {
    /// Name of the recognised format.
    pub source: String,
    pub lists: u32,
    pub tasks: u32,
    pub attachments: u32,
    /// What could not be carried over, in words meant for the user.
    pub notes: Vec<String>,
}

#[derive(Default)]
struct List {
    key: String,
    name: String,
    color: String,
    /// Other settings of the list, as registers: only a file of ours has them (I11).
    settings: Vec<(&'static str, Value)>,
}

#[derive(Default)]
struct Task {
    key: String,
    /// `None` is the inbox.
    list: Option<String>,
    parent: Option<String>,
    title: String,
    notes: String,
    start: Option<String>,
    due: Option<String>,
    remind: Option<String>,
    priority: i64,
    tags: Vec<String>,
    done: Option<String>,
    /// Closed as "won't do" at the moment of `done` (S33).
    wont: bool,
    project: bool,
    repeat: Option<Repeat>,
    files: Vec<(String, PathBuf)>,
}

#[derive(Default)]
struct Parsed {
    source: &'static str,
    lists: Vec<List>,
    tasks: Vec<Task>,
    /// Saved filters: key, name and what they select, with the keys of the lists in `list_ids`.
    filters: Vec<(String, String, FilterSpec)>,
    notes: Vec<String>,
}

fn invalid(msg: impl Into<String>) -> AppError {
    AppError::invalid(msg)
}

fn local_moment(utc: DateTime<Utc>) -> String {
    store::local_time(utc).format("%Y-%m-%dT%H:%M").to_string()
}

fn rule(freq: Freq, interval: u32) -> Repeat {
    Repeat {
        freq,
        interval: interval.max(1),
        weekdays: vec![],
        monthday: None,
        nth: None,
        nth_weekday: None,
        from_done: false,
        count: None,
        until: None,
    }
}

// ---------------------------------------------------------------- 2Do

/// Dates at or beyond this mean "none" in a 2Do database.
const TWODO_NO_DATE: f64 = 6_000_000_000.0;
const TWODO_TAG_SEPARATOR: &str = "_~|$$@$$|~_";

/// A due date: the day is a UTC instant on that day (noon), the time of day
/// is a separate number written as HHMM (930 is 09:30, 999999 is "no time").
fn twodo_date(seconds: f64, time_of_day: f64) -> Option<String> {
    if seconds <= 0.0 || seconds >= TWODO_NO_DATE {
        return None;
    }
    let date = Utc.timestamp_opt(seconds as i64, 0).single()?.date_naive();
    let (h, m) = ((time_of_day as i64) / 100, (time_of_day as i64) % 100);
    if (0.0..2400.0).contains(&time_of_day) && h < 24 && m < 60 {
        return Some(format!("{}T{h:02}:{m:02}", date.format("%Y-%m-%d")));
    }
    Some(date.format("%Y-%m-%d").to_string())
}

/// A start date: one instant whose UTC fields are the local date and time;
/// midnight means "no time".
fn twodo_start(seconds: f64) -> Option<String> {
    if seconds <= 0.0 || seconds >= TWODO_NO_DATE {
        return None;
    }
    let moment = Utc.timestamp_opt(seconds as i64, 0).single()?.naive_utc();
    let format = if moment.time() == chrono::NaiveTime::MIN {
        "%Y-%m-%d"
    } else {
        "%Y-%m-%dT%H:%M"
    };
    Some(moment.format(format).to_string())
}

/// The repeat rule of a 2Do task. Every code here was read from tasks created
/// in 2Do 4.19 with a known rule; the table is in docs/specs/import.md.
///
/// - 1..=127: weekly on chosen weekdays, one bit per day (Sunday 1, Saturday 2,
///   Friday 4, Thursday 8, Wednesday 16, Tuesday 32, Monday 64);
/// - 256..=259: every `value` days, weeks, months, years (2Do's Daily, Weekly,
///   Monthly and Yearly presets are stored this way with the value 1);
/// - 260..=266: the n-th weekday of each month, Monday 260 to Sunday 266, with
///   `value` 1 to 5 for the ordinal and 6 for "last".
///
/// `mode` 2 counts from the completion date. `end` is the end type (1 on a
/// date, 2 after a number of repeats), the repeats left, and the end date.
fn twodo_repeat(kind: i64, value: i64, mode: i64, end: (i64, i64, f64), due: Option<NaiveDate>) -> Option<Repeat> {
    let mut out = match kind {
        1..=127 => {
            // Monday first, as the rest of the app counts them.
            let bits = [64, 32, 16, 8, 4, 2, 1];
            let days: Vec<u32> = (0..7).filter(|i| kind & bits[*i] != 0).map(|i| i as u32 + 1).collect();
            let same_as_due = due.is_some_and(|d| days == [chrono::Datelike::weekday(&d).number_from_monday()]);
            // "Every week on the weekday of the due date" is the plain weekly rule.
            Repeat {
                weekdays: if same_as_due { vec![] } else { days },
                ..rule(Freq::Weekly, value.clamp(1, 999) as u32)
            }
        }
        256..=259 if (1..=999).contains(&value) => {
            let freq = [Freq::Daily, Freq::Weekly, Freq::Monthly, Freq::Yearly][(kind - 256) as usize];
            rule(freq, value as u32)
        }
        260..=266 if (1..=6).contains(&value) => Repeat {
            nth: Some(if value == 6 { -1 } else { value as i32 }),
            nth_weekday: Some((kind - 259) as u32),
            ..rule(Freq::Monthly, 1)
        },
        _ => return None,
    };
    out.from_done = mode == 2;
    match end {
        (1, _, date) => out.until = twodo_date(date, -1.0),
        (2, left, _) if left > 0 => out.count = Some(left as u32),
        _ => {}
    }
    Some(out)
}

/// A `.2dodb` backup: a zip with `2do.db` (SQLite) and the attachment files.
fn parse_2do(archive: &Path, work: &Path) -> Result<Parsed> {
    let file = std::fs::File::open(archive)?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| invalid(format!("not a 2Do backup: {e}")))?;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).map_err(|e| invalid(e.to_string()))?;
        // `enclosed_name` refuses paths that would leave the folder.
        let Some(relative) = entry.enclosed_name() else {
            continue;
        };
        let target = work.join(relative);
        if entry.is_dir() {
            std::fs::create_dir_all(&target)?;
            continue;
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut out = std::fs::File::create(&target)?;
        std::io::copy(&mut entry, &mut out)?;
    }
    let db = work.join("2do.db");
    if !db.exists() {
        return Err(invalid("the backup has no 2do.db inside"));
    }
    let conn = Connection::open_with_flags(&db, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let mut out = Parsed {
        source: "2Do",
        ..Parsed::default()
    };

    // Ordinary lists only: listtype 0. Focus lists and smart lists are views, not containers.
    let mut inbox_uid = String::new();
    {
        let mut stmt = conn.prepare(
            "SELECT uid, coalesce(title, ''), coalesce(colorhex, ''), isinboxcal FROM calendars
             WHERE isdeleted = 0 AND (listtype = 0 OR isinboxcal = 1) AND coalesce(smartsearch, '') = '' ORDER BY displayorder",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, bool>(3)?,
            ))
        })?;
        for row in rows {
            let (uid, name, color, inbox) = row?;
            if inbox {
                inbox_uid = uid;
            } else {
                out.lists.push(List {
                    key: uid,
                    name,
                    color,
                    ..List::default()
                });
            }
        }
    }
    let known: HashMap<&str, ()> = out.lists.iter().map(|l| (l.key.as_str(), ())).collect();

    let mut files: HashMap<String, Vec<(String, PathBuf)>> = HashMap::new();
    if let Ok(mut stmt) = conn.prepare(
        "SELECT taskuid, coalesce(displayname, ''), coalesce(fileextension, ''), relativepath FROM taskattachments
         WHERE isdeleted = 0 AND relativepath IS NOT NULL ORDER BY orderindex",
    ) {
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        })?;
        for row in rows {
            let (task, name, ext, relative) = row?;
            if relative.split('/').any(|p| p == ".." || p.is_empty()) {
                continue;
            }
            let path = work.join("2DoBackupPayload/Attachments").join(&relative);
            // 2Do keeps the extension apart from the name: a photo is "Image" with "jpg" beside it (I8).
            let name = if name.is_empty() {
                format!("attachment.{ext}")
            } else if ext.is_empty() || name.to_lowercase().ends_with(&format!(".{}", ext.to_lowercase())) {
                name
            } else {
                format!("{name}.{ext}")
            };
            if path.is_file() {
                files.entry(task).or_default().push((name, path));
            }
        }
    }

    let (mut repeats, mut converted, mut odd_priority) = (0u32, 0u32, 0u32);
    let mut stmt = conn.prepare(
        "SELECT uid, calendaruid, title, coalesce(notes, ''), duedate, coalesce(duetime, 999999), startdate, priority,
                iscompleted, coalesce(completeddate, 0), coalesce(tags, ''), coalesce(parent, ''), tasktype, repeattype, coalesce(url, ''),
                repeatvalue, recurrence, recurrenceendtype, recurrenceendrepeats, recurrenceenddate
         FROM tasks WHERE isdeleted = 0 ORDER BY displayorder, primid",
    )?;
    let mut rows = stmt.query([])?;
    while let Some(r) = rows.next()? {
        let uid: String = r.get(0)?;
        let calendar: String = r.get(1)?;
        let mut task = Task {
            list: (calendar != inbox_uid && known.contains_key(calendar.as_str())).then_some(calendar),
            title: r.get(2)?,
            notes: r.get(3)?,
            due: twodo_date(r.get::<_, f64>(4)?, r.get::<_, f64>(5)?),
            start: twodo_start(r.get::<_, f64>(6)?),
            files: files.remove(&uid).unwrap_or_default(),
            ..Task::default()
        };
        // The scale of iCalendar: 1 is the highest. 2Do writes 1, 5 and 9, and 10 for "none".
        task.priority = match r.get::<_, i64>(7)? {
            1..=4 => 3,
            5 => 2,
            6..=9 => 1,
            0 | 10 => 0,
            _ => {
                odd_priority += 1;
                0
            }
        };
        if r.get::<_, bool>(8)? {
            let at: f64 = r.get(9)?;
            let when = Utc
                .timestamp_opt(at as i64, 0)
                .single()
                .filter(|_| at > 0.0)
                .unwrap_or_else(Utc::now);
            task.done = Some(local_moment(when));
        }
        // One tag is eight fields; the first is its name.
        let tags: String = r.get(10)?;
        task.tags = tags
            .split(TWODO_TAG_SEPARATOR)
            .step_by(8)
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(str::to_string)
            .collect();
        let parent: String = r.get(11)?;
        task.parent = (!parent.is_empty()).then_some(parent);
        // 2 is a project, 1 a checklist; a checklist becomes a plain task with subtasks.
        task.project = r.get::<_, i64>(12)? == 2;
        let repeat_type: i64 = r.get(13)?;
        if repeat_type != 0 {
            let end = (r.get::<_, i64>(17)?, r.get::<_, i64>(18)?, r.get::<_, f64>(19)?);
            let due_day = task
                .due
                .as_deref()
                .or(task.start.as_deref())
                .and_then(|d| NaiveDate::parse_from_str(&d[..10], "%Y-%m-%d").ok());
            match twodo_repeat(repeat_type, r.get(15)?, r.get(16)?, end, due_day) {
                Some(rule) => {
                    converted += 1;
                    task.repeat = Some(crate::recur::normalized(
                        rule,
                        task.due.as_deref().or(task.start.as_deref()),
                    ));
                }
                None => {
                    repeats += 1;
                    task.tags.push("2do-repeat".into());
                }
            }
        }
        let url: String = r.get(14)?;
        if !url.is_empty() && !task.notes.contains(&url) {
            task.notes = format!("{}\n{url}", task.notes).trim().to_string();
        }
        task.key = uid;
        out.tasks.push(task);
    }
    if converted > 0 {
        out.notes.push(format!("{converted} repeat rules were converted."));
    }
    if repeats > 0 {
        out.notes.push(format!(
            "{repeats} repeating tasks use a rule this importer does not know; they were tagged #2do-repeat: set the repeat again."
        ));
    }
    if odd_priority > 0 {
        out.notes.push(format!(
            "{odd_priority} tasks had a priority value this importer does not know; it was left unset."
        ));
    }
    out.notes
        .push("Alarms, locations, actions, smart lists and list groups are not imported.".into());
    Ok(out)
}

// ---------------------------------------------------------------- Todoist

/// Minimal CSV reader: quoted fields, doubled quotes, line breaks inside quotes.
fn csv(text: &str) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    let (mut row, mut field, mut quoted) = (Vec::new(), String::new(), false);
    let mut chars = text.trim_start_matches('\u{feff}').chars().peekable();
    while let Some(c) = chars.next() {
        match (c, quoted) {
            ('"', true) if chars.peek() == Some(&'"') => {
                field.push('"');
                chars.next();
            }
            ('"', _) => quoted = !quoted,
            (',', false) => row.push(std::mem::take(&mut field)),
            ('\r', false) => {}
            ('\n', false) => {
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
            }
            (c, _) => field.push(c),
        }
    }
    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        rows.push(row);
    }
    rows
}

/// A Todoist project exported as a CSV template.
fn parse_todoist(text: &str, name: &str) -> Result<Parsed> {
    let rows = csv(text);
    let header: Vec<String> = rows
        .first()
        .ok_or_else(|| invalid("empty file"))?
        .iter()
        .map(|h| h.trim().to_uppercase())
        .collect();
    let col = |name: &str| header.iter().position(|h| h == name);
    let (Some(kind), Some(content)) = (col("TYPE"), col("CONTENT")) else {
        return Err(invalid("not a Todoist CSV: no TYPE and CONTENT columns"));
    };
    let get = |row: &Vec<String>, index: Option<usize>| {
        index
            .and_then(|i| row.get(i))
            .map(|s| s.trim().to_string())
            .unwrap_or_default()
    };
    let mut out = Parsed {
        source: "Todoist",
        ..Parsed::default()
    };
    out.lists.push(List {
        key: name.to_string(),
        name: name.to_string(),
        color: String::new(),
        ..List::default()
    });
    let today = store::local_time(Utc::now()).date();
    let mut stack: Vec<String> = Vec::new();
    let mut undated = 0u32;
    for (n, row) in rows.iter().enumerate().skip(1) {
        match get(row, Some(kind)).to_lowercase().as_str() {
            "task" => {
                let indent = get(row, col("INDENT")).parse::<usize>().unwrap_or(1).max(1);
                stack.truncate(indent - 1);
                let key = format!("{name}#{n}");
                let mut task = Task {
                    key: key.clone(),
                    list: Some(name.to_string()),
                    parent: stack.last().cloned(),
                    notes: get(row, col("DESCRIPTION")),
                    ..Task::default()
                };
                // Labels live in the content as @label.
                let mut words = Vec::new();
                for word in get(row, Some(content)).split_whitespace() {
                    match word.strip_prefix('@').filter(|l| !l.is_empty()) {
                        Some(label) => task.tags.push(label.to_string()),
                        None => words.push(word.to_string()),
                    }
                }
                task.title = words.join(" ");
                // In the CSV 1 is the highest of four; 4 is "no priority".
                task.priority = match get(row, col("PRIORITY")).as_str() {
                    "1" => 3,
                    "2" => 2,
                    "3" => 1,
                    _ => 0,
                };
                let date = get(row, col("DATE"));
                if !date.is_empty() {
                    let iso = date
                        .get(..10)
                        .and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
                        .map(|d| d.format("%Y-%m-%d").to_string());
                    task.due = iso.or_else(|| quickadd::parse(&date, today).due);
                    if task.due.is_none() || date.to_lowercase().contains("every") {
                        undated += 1;
                        task.notes = format!("{}\nTodoist date: {date}", task.notes).trim().to_string();
                    }
                }
                stack.push(key);
                out.tasks.push(task);
            }
            "note" => {
                if let Some(task) = out.tasks.last_mut() {
                    task.notes = format!("{}\n{}", task.notes, get(row, Some(content)))
                        .trim()
                        .to_string();
                }
            }
            // Sections have no counterpart; their tasks stay in the list.
            _ => stack.clear(),
        }
    }
    if undated > 0 {
        out.notes.push(format!("{undated} tasks had a date or a repeat written in words that was not understood; the original text was added to their notes."));
    }
    out.notes
        .push("Sections, assignees and completed tasks are not part of a Todoist export.".into());
    Ok(out)
}

// ---------------------------------------------------------------- TickTick

/// Whether a CSV row is the header of a TickTick backup.
fn ticktick_header(row: &[String]) -> bool {
    ["Folder Name", "List Name", "Title"]
        .iter()
        .all(|name| row.iter().any(|h| h.trim() == *name))
}

fn ticktick_instant(text: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_str(text.trim(), "%Y-%m-%dT%H:%M:%S%z")
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

/// The day of a task without a time: TickTick writes midnight in the zone of
/// the task as UTC. With no zone database here, an instant from 11:00 UTC on
/// is the midnight of the next day (right for zones from -10:00 to +13:00).
fn ticktick_day(utc: DateTime<Utc>) -> NaiveDate {
    let date = utc.date_naive();
    if chrono::Timelike::hour(&utc) >= 11 {
        date.succ_opt().unwrap_or(date)
    } else {
        date
    }
}

/// An ISO 8601 duration as a reminder offset: `PT0S`, `-PT30M`, `P0DT9H30M0S`.
fn ticktick_offset(text: &str) -> Option<chrono::Duration> {
    let text = text.trim();
    let (sign, rest) = match text.strip_prefix('-') {
        Some(rest) => (-1, rest),
        None => (1, text.strip_prefix('+').unwrap_or(text)),
    };
    let rest = rest.strip_prefix('P')?;
    let (mut seconds, mut number, mut in_time, mut parts) = (0i64, String::new(), false, 0);
    for c in rest.chars() {
        match c {
            '0'..='9' => number.push(c),
            'T' if number.is_empty() => in_time = true,
            _ => {
                let n: i64 = std::mem::take(&mut number).parse().ok()?;
                let unit = match (c, in_time) {
                    ('W', false) => 7 * 86_400,
                    ('D', false) => 86_400,
                    ('H', true) => 3_600,
                    ('M', true) => 60,
                    ('S', true) => 1,
                    _ => return None,
                };
                seconds = seconds.checked_add(n.checked_mul(unit)?)?;
                parts += 1;
            }
        }
    }
    if !number.is_empty() || parts == 0 {
        return None;
    }
    chrono::Duration::try_seconds(sign * seconds)
}

/// Start, due and reminder of a TickTick task, as the app writes them.
fn ticktick_dates(
    start: &str,
    due: &str,
    all_day: bool,
    reminder: &str,
) -> (Option<String>, Option<String>, Option<String>) {
    let (start, due) = (ticktick_instant(start), ticktick_instant(due));
    let write = |utc: DateTime<Utc>| {
        if all_day {
            ticktick_day(utc).format("%Y-%m-%d").to_string()
        } else {
            local_moment(utc)
        }
    };
    // RFC 5545: an offset counts from the start; a day without a time starts at midnight.
    let remind = start.or(due).and_then(|anchor| {
        let offset = ticktick_offset(reminder.lines().next()?)?;
        let moment = if all_day {
            ticktick_day(anchor).and_hms_opt(0, 0, 0)?.checked_add_signed(offset)?
        } else {
            store::local_time(anchor.checked_add_signed(offset)?)
        };
        Some(moment.format("%Y-%m-%dT%H:%M").to_string())
    });
    let due_text = due.or(start).map(write);
    let start_text = start.filter(|s| due.is_some_and(|d| d != *s)).map(write);
    (start_text, due_text, remind)
}

/// A TickTick backup: a few lines about the file, then one CSV row per task.
fn parse_ticktick(rows: &[Vec<String>]) -> Result<Parsed> {
    let at = rows
        .iter()
        .position(|r| ticktick_header(r))
        .ok_or_else(|| invalid("not a TickTick backup: no Folder Name, List Name and Title columns"))?;
    let header: Vec<&str> = rows[at].iter().map(|h| h.trim()).collect();
    let col = |name: &str| header.iter().position(|h| *h == name);
    // Line breaks inside a field are a lone CR in the backups seen.
    let get = |row: &Vec<String>, name: &str| {
        col(name)
            .and_then(|i| row.get(i))
            .map(|v| v.replace("\r\n", "\n").replace('\r', "\n"))
            .unwrap_or_default()
    };
    let mut out = Parsed {
        source: "TickTick",
        ..Parsed::default()
    };
    let (mut more_reminders, mut text_repeats, mut notes, mut boards) = (0u32, 0u32, 0u32, Vec::<String>::new());
    let mut ordered: Vec<(i128, usize, Task)> = Vec::new();
    for (n, row) in rows.iter().enumerate().skip(at + 1) {
        let id = get(row, "taskId");
        if row.len() < header.len() / 2 || id.is_empty() {
            continue;
        }
        let (folder, name) = (get(row, "Folder Name"), get(row, "List Name"));
        let list = if folder.trim().is_empty() && name == "Inbox" {
            None
        } else {
            let key = format!("{folder}\u{0}{name}");
            if !out.lists.iter().any(|l| l.key == key) {
                out.lists.push(List {
                    key: key.clone(),
                    name: if folder.trim().is_empty() {
                        name.clone()
                    } else {
                        format!("{}: {}", folder.trim(), name.trim())
                    },
                    ..List::default()
                });
            }
            if get(row, "View Mode") == "kanban" && !boards.contains(&key) {
                boards.push(key.clone());
            }
            Some(key)
        };
        let reminder = get(row, "Reminder");
        if reminder.lines().filter(|l| !l.trim().is_empty()).count() > 1 {
            more_reminders += reminder.lines().filter(|l| !l.trim().is_empty()).count() as u32 - 1;
        }
        let (start, due, remind) = ticktick_dates(
            &get(row, "Start Date"),
            &get(row, "Due Date"),
            get(row, "Is All Day") == "true",
            &reminder,
        );
        let finished = ticktick_instant(&get(row, "Completed Time"))
            .map(local_moment)
            .unwrap_or_else(|| local_moment(Utc::now()));
        let status = get(row, "Status");
        let kind = get(row, "Kind");
        if kind == "NOTE" {
            notes += 1;
        }
        // Checklist items are lines of the content: ▫ open, ▪ checked; other lines are the note.
        let content = get(row, "Content");
        let mut items = Vec::new();
        let mut text = Vec::new();
        for line in content.lines() {
            match (kind.as_str(), line.chars().next()) {
                ("CHECKLIST", Some(mark @ ('▫' | '▪'))) => {
                    items.push((line[mark.len_utf8()..].trim().to_string(), mark == '▪'))
                }
                _ => text.push(line),
            }
        }
        let mut task = Task {
            key: id.clone(),
            list: list.clone(),
            parent: Some(get(row, "parentId")).filter(|p| !p.is_empty()),
            title: get(row, "Title"),
            notes: text.join("\n").trim().to_string(),
            start,
            due,
            remind,
            priority: match get(row, "Priority").trim() {
                "5" => 3,
                "3" => 2,
                "1" => 1,
                _ => 0,
            },
            tags: get(row, "Tags").split(',').map(str::to_string).collect(),
            done: matches!(status.trim(), "2" | "-1").then(|| finished.clone()),
            wont: status.trim() == "-1",
            ..Task::default()
        };
        let rule = get(row, "Repeat");
        let rule = rule.trim().trim_start_matches("RRULE:");
        if !rule.is_empty() {
            task.repeat = crate::caldav::map::from_rrule(rule);
            if task.repeat.is_none() {
                text_repeats += 1;
                task.notes = format!("{}\nTickTick repeat: {rule}", task.notes).trim().to_string();
            }
        }
        let order = get(row, "Order").trim().parse::<i128>().unwrap_or(0);
        ordered.push((order, n, task));
        for (i, (title, checked)) in items.into_iter().enumerate() {
            let task = Task {
                key: format!("{id}#{}", i + 1),
                list: list.clone(),
                parent: Some(id.clone()),
                title,
                done: checked.then(|| finished.clone()),
                ..Task::default()
            };
            ordered.push((i as i128, n, task));
        }
    }
    // TickTick shows a list by ascending `Order`; ties keep the order of the file.
    ordered.sort_by_key(|(order, n, _)| (*order, *n));
    out.tasks = ordered.into_iter().map(|(_, _, t)| t).collect();
    let plural = |n: u32, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
    if more_reminders > 0 {
        out.notes.push(format!(
            "{} beyond the first of a task not imported.",
            plural(more_reminders, "reminder was", "reminders were")
        ));
    }
    if text_repeats > 0 {
        out.notes.push(format!(
            "{} not expressible here; the rule text was added to the notes of the task.",
            plural(text_repeats, "repeat rule was", "repeat rules were")
        ));
    }
    if notes > 0 {
        out.notes.push(format!(
            "{} imported as tasks.",
            plural(notes, "note was", "notes were")
        ));
    }
    if !boards.is_empty() {
        out.notes.push(format!(
            "{} kanban: the columns were not imported.",
            plural(boards.len() as u32, "list was", "lists were")
        ));
    }
    out.notes
        .push("A TickTick backup has no list colours, attachments or comments.".into());
    Ok(out)
}

// ---------------------------------------------------------------- OmniFocus

/// Whether a CSV row is the header of an OmniFocus export.
fn omnifocus_header(row: &[String]) -> bool {
    ["Task ID", "Type", "Name"]
        .iter()
        .all(|name| row.iter().any(|h| h.trim() == *name))
}

/// An OmniFocus CSV export: one row per project and action, `Task ID` is the path in the outline.
fn parse_omnifocus(rows: &[Vec<String>], name: &str) -> Result<Parsed> {
    let header: Vec<&str> = rows
        .first()
        .filter(|r| omnifocus_header(r))
        .ok_or_else(|| invalid("not an OmniFocus export: no Task ID, Type and Name columns"))?
        .iter()
        .map(|h| h.trim())
        .collect();
    let col = |name: &str| header.iter().position(|h| *h == name);
    let get = |row: &Vec<String>, name: &str| {
        col(name)
            .and_then(|i| row.get(i))
            .map(|v| v.replace("\r\n", "\n").trim().to_string())
            .unwrap_or_default()
    };
    let moment = |text: &str| {
        DateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S %z")
            .ok()
            .map(|d| local_moment(d.with_timezone(&Utc)))
    };
    let mut out = Parsed {
        source: "OmniFocus",
        ..Parsed::default()
    };
    out.lists.push(List {
        key: name.to_string(),
        name: name.to_string(),
        ..List::default()
    });
    let ids: std::collections::HashSet<String> = rows.iter().skip(1).map(|r| get(r, "Task ID")).collect();
    let (mut durations, mut planned) = (0u32, 0u32);
    for row in rows.iter().skip(1) {
        let id = get(row, "Task ID");
        if id.is_empty() {
            continue;
        }
        let kind = get(row, "Type");
        // The parent is the nearest shorter path in the file: `1.3` for `1.3.2`, `1` for `1.3`.
        let parent = (kind != "Inbox")
            .then(|| {
                let mut path = id.as_str();
                while let Some((up, _)) = path.rsplit_once('.') {
                    if ids.contains(up) {
                        return Some(up.to_string());
                    }
                    path = up;
                }
                None
            })
            .flatten();
        let status = get(row, "Status").to_lowercase();
        let completed = get(row, "Completion Date");
        let finished = !completed.is_empty() || matches!(status.as_str(), "done" | "completed" | "dropped");
        let tags = match col("Tags") {
            Some(_) => get(row, "Tags"),
            None => get(row, "Context"),
        };
        if !get(row, "Duration").is_empty() {
            durations += 1;
        }
        if !get(row, "Planned Date").is_empty() {
            planned += 1;
        }
        out.tasks.push(Task {
            key: id.clone(),
            list: (kind != "Inbox").then(|| name.to_string()),
            parent,
            title: get(row, "Name"),
            notes: get(row, "Notes"),
            start: moment(&get(row, "Start Date")),
            due: moment(&get(row, "Due Date")),
            priority: if get(row, "Flagged") == "1" { 3 } else { 0 },
            tags: tags.split(',').map(str::to_string).collect(),
            done: finished.then(|| moment(&completed).unwrap_or_else(|| local_moment(Utc::now()))),
            wont: status == "dropped",
            project: kind == "Project",
            ..Task::default()
        });
    }
    let plural = |n: u32, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
    if durations > 0 {
        out.notes.push(format!(
            "{} an estimated duration, which is not imported.",
            plural(durations, "task had", "tasks had")
        ));
    }
    if planned > 0 {
        out.notes.push(format!(
            "{} a planned date, which is not imported.",
            plural(planned, "task had", "tasks had")
        ));
    }
    out.notes
        .push("An OmniFocus export has no repeat rules, folders or attachments.".into());
    Ok(out)
}

// ---------------------------------------------------------------- Things 3

/// The database version the importer was checked against (Things 3.24.1).
const THINGS_VERSION: i64 = 29;
/// `ed` of a repeat rule that never ends: 4001-01-01.
const THINGS_NEVER: f64 = 64_092_211_200.0;

/// A date of a Things database: `year << 16 | month << 12 | day << 7`; year 4001 is "none".
fn things_date(value: Option<i64>) -> Option<NaiveDate> {
    let v = value.filter(|v| *v > 0)?;
    let year = (v >> 16) as i32;
    if year >= 4000 {
        return None;
    }
    NaiveDate::from_ymd_opt(year, ((v >> 12) & 15) as u32, ((v >> 7) & 31) as u32)
}

/// A reminder time of a Things database: `hour << 26 | minute << 20`.
fn things_time(value: Option<i64>) -> Option<(u32, u32)> {
    let v = value.filter(|v| *v > 0)?;
    let (hour, minute) = ((v >> 26) as u32, ((v >> 20) & 63) as u32);
    (hour < 24 && minute < 60).then_some((hour, minute))
}

/// An XML property list as JSON: dictionaries, arrays, numbers, strings and booleans.
fn plist(xml: &[u8]) -> Option<Value> {
    fn value(node: roxmltree::Node) -> Option<Value> {
        let children = || node.children().filter(|c| c.is_element());
        Some(match node.tag_name().name() {
            "dict" => {
                let mut map = serde_json::Map::new();
                let mut key = None;
                for child in children() {
                    if child.tag_name().name() == "key" {
                        key = Some(child.text().unwrap_or_default().to_string());
                    } else {
                        map.insert(key.take()?, value(child)?);
                    }
                }
                Value::Object(map)
            }
            "array" => Value::Array(children().map(value).collect::<Option<_>>()?),
            "integer" => json!(node.text()?.trim().parse::<i64>().ok()?),
            "real" => json!(node.text()?.trim().parse::<f64>().ok()?),
            "string" => json!(node.text().unwrap_or_default()),
            "true" => json!(true),
            "false" => json!(false),
            _ => return None,
        })
    }
    let text = std::str::from_utf8(xml).ok()?;
    let doc = roxmltree::Document::parse_with_options(
        text,
        roxmltree::ParsingOptions {
            allow_dtd: true,
            ..Default::default()
        },
    )
    .ok()?;
    value(doc.root_element().children().find(|c| c.is_element())?)
}

/// The repeat rule of a Things template; `None` when the model here cannot hold
/// it. Every code read here was checked against the dates of the tasks the
/// rule made (docs/specs/import.md):
///
/// - `fu`: the unit, 16 day, 256 week, 8 month, 4 year; `fa`: every how many;
/// - `of`: `wd` a weekday with Sunday 0, `dy` the day of the month counted from
///   0, `mo` the month counted from 0 (the day of a yearly rule is the date of
///   the task, so `dy` and `mo` add nothing there);
/// - `tp` 1 counts from the completion date; `ed` the end, seconds since 1970.
fn things_rule(plist: &Value) -> Option<Repeat> {
    let int = |key: &str| plist.get(key).and_then(Value::as_i64);
    // Never seen with another value than 0 except on one paused rule: not known what they do.
    if int("rc").unwrap_or(0) != 0 || int("ts").unwrap_or(0) != 0 {
        return None;
    }
    let interval = int("fa").filter(|n| (1..=999).contains(n))? as u32;
    let offsets = plist.get("of").and_then(Value::as_array).cloned().unwrap_or_default();
    let only = |offset: &Value, keys: &[&str]| {
        offset
            .as_object()
            .is_some_and(|o| o.keys().all(|k| keys.contains(&k.as_str())))
    };
    let mut out = match int("fu")? {
        16 => rule(Freq::Daily, interval),
        256 => {
            let mut days = Vec::new();
            for offset in &offsets {
                let day = offset.get("wd").and_then(Value::as_i64).filter(|d| (0..=6).contains(d));
                if !only(offset, &["wd"]) || day.is_none() {
                    return None;
                }
                // Monday first here, Sunday 0 in Things.
                days.push(if day == Some(0) { 7 } else { day? as u32 });
            }
            days.sort_unstable();
            days.dedup();
            Repeat {
                weekdays: days,
                ..rule(Freq::Weekly, interval)
            }
        }
        8 => {
            let [offset] = offsets.as_slice() else { return None };
            let day = offset
                .get("dy")
                .and_then(Value::as_i64)
                .filter(|d| (0..=30).contains(d));
            if !only(offset, &["dy"]) {
                return None;
            }
            Repeat {
                monthday: Some(day? as u32 + 1),
                ..rule(Freq::Monthly, interval)
            }
        }
        4 if offsets.len() <= 1 && offsets.iter().all(|o| only(o, &["dy", "mo"])) => rule(Freq::Yearly, interval),
        _ => return None,
    };
    out.from_done = int("tp") == Some(1);
    let end = plist.get("ed").and_then(Value::as_f64).unwrap_or(THINGS_NEVER);
    if end < THINGS_NEVER {
        out.until = Utc
            .timestamp_opt(end as i64, 0)
            .single()
            .map(|d| d.date_naive().format("%Y-%m-%d").to_string());
    }
    Some(out)
}

struct ThingsRow {
    uuid: String,
    kind: i64,
    status: i64,
    trashed: bool,
    title: String,
    notes: String,
    start: i64,
    start_date: Option<i64>,
    deadline: Option<i64>,
    reminder: Option<i64>,
    stop: Option<f64>,
    area: Option<String>,
    project: Option<String>,
    heading: Option<String>,
    rule: Option<Vec<u8>>,
    template: Option<String>,
    paused: bool,
    next: Option<i64>,
}

/// A Things 3 database (`main.sqlite`), read from a copy in `work`.
fn parse_things(db: &Path, work: &Path) -> Result<Parsed> {
    // The live database keeps recent changes in its write-ahead log: copy both, open the copy.
    let copy = work.join("main.sqlite");
    std::fs::copy(db, &copy)?;
    let wal = PathBuf::from(format!("{}-wal", db.display()));
    if wal.exists() {
        std::fs::copy(&wal, work.join("main.sqlite-wal"))?;
    }
    let conn = Connection::open(&copy)?;
    let is_things: bool = conn.query_row(
        "SELECT count(*) > 0 FROM sqlite_master WHERE type = 'table' AND name = 'TMTask'",
        [],
        |r| r.get(0),
    )?;
    if !is_things {
        return Err(invalid("an SQLite database, but not one of Things 3"));
    }
    let version: Option<i64> = conn
        .query_row("SELECT value FROM Meta WHERE key = 'databaseVersion'", [], |r| {
            r.get::<_, String>(0)
        })
        .ok()
        .and_then(|v| plist(v.as_bytes()))
        .and_then(|v| v.as_i64());
    let unsupported = |e: rusqlite::Error| {
        invalid(format!(
            "this Things database (version {}) is laid out differently from version {THINGS_VERSION}, which the import knows: {e}",
            version.map_or("unknown".into(), |v| v.to_string())
        ))
    };
    let mut out = Parsed {
        source: "Things",
        ..Parsed::default()
    };
    let mut stmt = conn
        .prepare(r#"SELECT uuid, title FROM TMArea ORDER BY "index""#)
        .map_err(unsupported)?;
    for area in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)))? {
        let (key, name) = area?;
        out.lists.push(List {
            key,
            name: name.unwrap_or_default(),
            ..List::default()
        });
    }
    let mut tags: HashMap<String, Vec<String>> = HashMap::new();
    let mut stmt = conn
        .prepare("SELECT x.tasks, t.title FROM TMTaskTag x JOIN TMTag t ON t.uuid = x.tags")
        .map_err(unsupported)?;
    for tag in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)))? {
        let (task, name) = tag?;
        tags.entry(task).or_default().extend(name);
    }
    let mut stmt = conn
        .prepare(
            r#"SELECT uuid, type, status, trashed, title, notes, start, startDate, deadline, reminderTime, stopDate,
                      area, project, heading, rt1_recurrenceRule, rt1_repeatingTemplate, rt1_instanceCreationPaused,
                      rt1_nextInstanceStartDate
               FROM TMTask ORDER BY "index", creationDate"#,
        )
        .map_err(unsupported)?;
    let rows: Vec<ThingsRow> = stmt
        .query_map([], |r| {
            Ok(ThingsRow {
                uuid: r.get(0)?,
                kind: r.get::<_, Option<i64>>(1)?.unwrap_or(0),
                status: r.get::<_, Option<i64>>(2)?.unwrap_or(0),
                trashed: r.get::<_, Option<i64>>(3)?.unwrap_or(0) != 0,
                title: r.get::<_, Option<String>>(4)?.unwrap_or_default(),
                notes: r.get::<_, Option<String>>(5)?.unwrap_or_default(),
                start: r.get::<_, Option<i64>>(6)?.unwrap_or(1),
                start_date: r.get(7)?,
                deadline: r.get(8)?,
                reminder: r.get(9)?,
                stop: r.get(10)?,
                area: r.get(11)?,
                project: r.get(12)?,
                heading: r.get(13)?,
                rule: r.get(14)?,
                template: r.get(15)?,
                paused: r.get::<_, Option<i64>>(16)?.unwrap_or(0) != 0,
                next: r.get(17)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    let by_id: HashMap<&str, &ThingsRow> = rows.iter().map(|r| (r.uuid.as_str(), r)).collect();
    let project_of = |row: &ThingsRow| {
        row.project.clone().or_else(|| {
            row.heading
                .as_deref()
                .and_then(|h| by_id.get(h))
                .and_then(|h| h.project.clone())
        })
    };
    // In the Trash itself, or with its heading or project there (the project of a heading counts).
    let gone = |row: &ThingsRow| {
        row.trashed
            || [project_of(row), row.heading.clone()]
                .into_iter()
                .flatten()
                .any(|p| by_id.get(p.as_str()).is_some_and(|p| p.trashed))
    };

    // A repeating task is a template and the tasks it makes: the rule goes to the
    // open task it made, or to the template when there is none.
    let (mut paused, mut text_rules) = (0u32, 0u32);
    let mut rules: HashMap<String, std::result::Result<Repeat, String>> = HashMap::new();
    let mut templates_kept: Vec<&str> = Vec::new();
    for template in rows.iter().filter(|r| r.rule.is_some() && !gone(r)) {
        if template.paused {
            paused += 1;
            continue;
        }
        let raw = template.rule.as_deref().unwrap_or_default();
        let parsed = plist(raw);
        let rule = parsed
            .as_ref()
            .and_then(things_rule)
            .ok_or_else(|| parsed.map_or_else(|| "?".into(), |v| v.to_string()));
        let open = rows
            .iter()
            .filter(|r| r.template.as_deref() == Some(&template.uuid) && r.status == 0 && !gone(r))
            .max_by_key(|r| r.start_date);
        let target = match open {
            Some(task) => task.uuid.clone(),
            None => {
                templates_kept.push(&template.uuid);
                template.uuid.clone()
            }
        };
        rules.insert(target, rule);
    }

    let areas: Vec<String> = out.lists.iter().map(|l| l.key.clone()).collect();
    let (mut someday, mut headings, mut trashed) = (0u32, 0u32, 0u32);
    let mut done_items: HashMap<String, String> = HashMap::new();
    for row in &rows {
        if row.kind == 2 {
            headings += u32::from(!gone(row));
            continue;
        }
        if gone(row) {
            trashed += 1;
            continue;
        }
        if row.rule.is_some() && !templates_kept.contains(&row.uuid.as_str()) {
            continue;
        }
        let project = if row.kind == 0 { project_of(row) } else { None };
        let area = row
            .area
            .clone()
            .or_else(|| {
                project
                    .as_deref()
                    .and_then(|p| by_id.get(p))
                    .and_then(|p| p.area.clone())
            })
            .filter(|a| areas.contains(a));
        let start_date = things_date(row.start_date).or_else(|| {
            templates_kept
                .contains(&row.uuid.as_str())
                .then(|| things_date(row.next))
                .flatten()
        });
        let remind = start_date
            .zip(things_time(row.reminder))
            .and_then(|(d, (h, m))| d.and_hms_opt(h, m, 0))
            .map(|d| d.format("%Y-%m-%dT%H:%M").to_string());
        let finished = row
            .stop
            .and_then(|s| Utc.timestamp_opt(s as i64, 0).single())
            .map(local_moment)
            .unwrap_or_else(|| local_moment(Utc::now()));
        let mut task = Task {
            key: row.uuid.clone(),
            list: area,
            parent: project.filter(|p| by_id.contains_key(p.as_str())),
            title: row.title.clone(),
            notes: row.notes.trim().to_string(),
            start: start_date.map(|d| d.format("%Y-%m-%d").to_string()),
            due: things_date(row.deadline).map(|d| d.format("%Y-%m-%d").to_string()),
            remind,
            tags: tags.remove(&row.uuid).unwrap_or_default(),
            done: matches!(row.status, 2 | 3).then(|| finished.clone()),
            wont: row.status == 2,
            project: row.kind == 1,
            ..Task::default()
        };
        if row.start == 2 {
            someday += 1;
            task.tags.push("someday".into());
        }
        match rules.remove(&row.uuid) {
            Some(Ok(rule)) => task.repeat = Some(rule),
            Some(Err(text)) => {
                text_rules += 1;
                task.notes = format!("{}\nThings repeat: {text}", task.notes).trim().to_string();
            }
            None => {}
        }
        done_items.insert(row.uuid.clone(), finished);
        out.tasks.push(task);
    }
    let mut stmt = conn
        .prepare(r#"SELECT uuid, task, title, status, stopDate FROM TMChecklistItem ORDER BY task, "index""#)
        .map_err(unsupported)?;
    for item in stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, Option<String>>(2)?.unwrap_or_default(),
            r.get::<_, Option<i64>>(3)?.unwrap_or(0),
            r.get::<_, Option<f64>>(4)?,
        ))
    })? {
        let (key, parent, title, status, stop) = item?;
        let Some(fallback) = done_items.get(&parent) else {
            continue;
        };
        let finished = stop
            .and_then(|s| Utc.timestamp_opt(s as i64, 0).single())
            .map(local_moment)
            .unwrap_or_else(|| fallback.clone());
        out.tasks.push(Task {
            key,
            parent: Some(parent),
            title,
            done: matches!(status, 2 | 3).then_some(finished),
            wont: status == 2,
            ..Task::default()
        });
    }

    let plural = |n: u32, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
    if version != Some(THINGS_VERSION) {
        out.notes.push(format!(
            "The import knows Things databases of version {THINGS_VERSION} (Things 3.24); this one is version {}.",
            version.map_or("unknown".into(), |v| v.to_string())
        ));
    }
    if someday > 0 {
        out.notes.push(format!(
            "{} in Someday: they got the tag \"someday\".",
            plural(someday, "task was", "tasks were")
        ));
    }
    if headings > 0 {
        out.notes.push(format!(
            "{} not imported; their tasks stay in the project.",
            plural(headings, "heading was", "headings were")
        ));
    }
    if trashed > 0 {
        out.notes.push(format!(
            "{} in the Trash and not imported.",
            plural(trashed, "task was", "tasks were")
        ));
    }
    if paused > 0 {
        out.notes.push(format!(
            "{} paused and not imported.",
            plural(paused, "repeating task was", "repeating tasks were")
        ));
    }
    if text_rules > 0 {
        out.notes.push(format!(
            "{} not expressible here; the rule was added to the notes of the task.",
            plural(text_rules, "repeat rule was", "repeat rules were")
        ));
    }
    Ok(out)
}

// ---------------------------------------------------------------- Trello

fn iso_instant(value: &Value) -> Option<String> {
    DateTime::parse_from_rfc3339(value.as_str()?)
        .ok()
        .map(|d| local_moment(d.with_timezone(&Utc)))
}

/// A Trello board exported as JSON: lists become lists, cards tasks, checklist items subtasks.
fn parse_trello(board: &Value) -> Result<Parsed> {
    let array = |key: &str| board.get(key).and_then(Value::as_array).cloned().unwrap_or_default();
    let text = |v: &Value, key: &str| v.get(key).and_then(Value::as_str).unwrap_or_default().to_string();
    let closed = |v: &Value| v.get("closed").and_then(Value::as_bool).unwrap_or(false);
    let mut out = Parsed {
        source: "Trello",
        ..Parsed::default()
    };
    let board_name = text(board, "name");
    for list in array("lists").iter().filter(|l| !closed(l)) {
        let name = if board_name.is_empty() {
            text(list, "name")
        } else {
            format!("{board_name}: {}", text(list, "name"))
        };
        out.lists.push(List {
            key: text(list, "id"),
            name,
            color: String::new(),
            ..List::default()
        });
    }
    let labels: HashMap<String, String> = array("labels")
        .iter()
        .map(|l| {
            (
                text(l, "id"),
                if text(l, "name").is_empty() {
                    text(l, "color")
                } else {
                    text(l, "name")
                },
            )
        })
        .collect();
    let mut checklists: HashMap<String, Vec<Value>> = HashMap::new();
    for checklist in array("checklists") {
        checklists
            .entry(text(&checklist, "idCard"))
            .or_default()
            .push(checklist);
    }
    let mut archived = 0u32;
    for card in array("cards") {
        if closed(&card) || !out.lists.iter().any(|l| l.key == text(&card, "idList")) {
            archived += 1;
            continue;
        }
        let id = text(&card, "id");
        let complete = card.get("dueComplete").and_then(Value::as_bool).unwrap_or(false);
        out.tasks.push(Task {
            key: id.clone(),
            list: Some(text(&card, "idList")),
            title: text(&card, "name"),
            notes: text(&card, "desc"),
            due: card.get("due").and_then(iso_instant),
            start: card.get("start").and_then(iso_instant).map(|s| s[..10].to_string()),
            done: complete.then(|| local_moment(Utc::now())),
            tags: card
                .get("idLabels")
                .and_then(Value::as_array)
                .map(|ids| {
                    ids.iter()
                        .filter_map(|i| labels.get(i.as_str()?).cloned())
                        .filter(|l| !l.is_empty())
                        .collect()
                })
                .unwrap_or_default(),
            ..Task::default()
        });
        for checklist in checklists.remove(&id).unwrap_or_default() {
            let mut items = checklist
                .get("checkItems")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            items.sort_by(|a, b| {
                a.get("pos")
                    .and_then(Value::as_f64)
                    .partial_cmp(&b.get("pos").and_then(Value::as_f64))
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            for item in items {
                out.tasks.push(Task {
                    key: text(&item, "id"),
                    parent: Some(id.clone()),
                    title: text(&item, "name"),
                    due: item.get("due").and_then(iso_instant),
                    done: (text(&item, "state") == "complete").then(|| local_moment(Utc::now())),
                    ..Task::default()
                });
            }
        }
    }
    if archived > 0 {
        out.notes.push(format!(
            "{archived} archived cards (or cards of archived lists) were skipped."
        ));
    }
    out.notes
        .push("Comments, members, attachments and custom fields are not imported.".into());
    Ok(out)
}

// ---------------------------------------------------------------- Microsoft To Do

fn graph_moment(value: Option<&Value>, date_only: bool) -> Option<String> {
    let value = value?;
    let text = value.get("dateTime").and_then(Value::as_str)?;
    let moment = NaiveDateTime::parse_from_str(text.get(..19)?, "%Y-%m-%dT%H:%M:%S").ok()?;
    if date_only {
        return Some(moment.date().format("%Y-%m-%d").to_string());
    }
    let utc = value
        .get("timeZone")
        .and_then(Value::as_str)
        .is_none_or(|z| z.eq_ignore_ascii_case("UTC"));
    Some(if utc {
        local_moment(Utc.from_utc_datetime(&moment))
    } else {
        moment.format("%Y-%m-%dT%H:%M").to_string()
    })
}

/// Lists with their tasks as Microsoft Graph returns them: either an array
/// of `todoTaskList` objects each carrying `tasks`, or `{ "value": [...] }`.
fn parse_mstodo(root: &Value) -> Result<Parsed> {
    let lists = root
        .as_array()
        .or_else(|| root.get("value").and_then(Value::as_array))
        .ok_or_else(|| invalid("not a Microsoft To Do export"))?;
    let text = |v: &Value, key: &str| v.get(key).and_then(Value::as_str).unwrap_or_default().to_string();
    let mut out = Parsed {
        source: "Microsoft To Do",
        ..Parsed::default()
    };
    let mut odd_repeat = 0u32;
    for list in lists {
        let key = text(list, "id");
        // The export tool writes `wellKnownListName`, Graph itself `wellknownListName`.
        let well_known = list
            .get("wellknownListName")
            .or_else(|| list.get("wellKnownListName"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        let default = well_known == "defaultList";
        if !default {
            out.lists.push(List {
                key: key.clone(),
                name: text(list, "displayName"),
                color: String::new(),
                ..List::default()
            });
        }
        for task in list.get("tasks").and_then(Value::as_array).cloned().unwrap_or_default() {
            let id = text(&task, "id");
            let repeat = task.get("recurrence").and_then(|r| r.get("pattern")).and_then(|p| {
                let interval = p.get("interval").and_then(Value::as_u64).unwrap_or(1) as u32;
                Some(match p.get("type").and_then(Value::as_str)? {
                    "daily" => rule(Freq::Daily, interval),
                    "weekly" => {
                        let names = [
                            "monday",
                            "tuesday",
                            "wednesday",
                            "thursday",
                            "friday",
                            "saturday",
                            "sunday",
                        ];
                        let days = p
                            .get("daysOfWeek")
                            .and_then(Value::as_array)
                            .cloned()
                            .unwrap_or_default();
                        let mut weekdays: Vec<u32> = days
                            .iter()
                            .filter_map(|d| {
                                names
                                    .iter()
                                    .position(|n| d.as_str().is_some_and(|s| s.eq_ignore_ascii_case(n)))
                            })
                            .map(|i| i as u32 + 1)
                            .collect();
                        weekdays.sort_unstable();
                        Repeat {
                            weekdays: if weekdays.len() > 1 { weekdays } else { vec![] },
                            ..rule(Freq::Weekly, interval)
                        }
                    }
                    "absoluteMonthly" => Repeat {
                        monthday: p.get("dayOfMonth").and_then(Value::as_u64).map(|d| d as u32),
                        ..rule(Freq::Monthly, interval)
                    },
                    "absoluteYearly" => rule(Freq::Yearly, interval),
                    _ => return None,
                })
            });
            if task.get("recurrence").is_some_and(|r| !r.is_null()) && repeat.is_none() {
                odd_repeat += 1;
            }
            let done = (text(&task, "status") == "completed").then(|| {
                graph_moment(task.get("completedDateTime"), false).unwrap_or_else(|| local_moment(Utc::now()))
            });
            out.tasks.push(Task {
                key: id.clone(),
                list: (!default).then(|| key.clone()),
                title: text(&task, "title"),
                notes: task
                    .get("body")
                    .map(|b| text(b, "content"))
                    .unwrap_or_default()
                    .trim()
                    .to_string(),
                due: graph_moment(task.get("dueDateTime"), true),
                start: graph_moment(task.get("startDateTime"), true),
                remind: graph_moment(task.get("reminderDateTime"), false),
                priority: match text(&task, "importance").as_str() {
                    "high" => 3,
                    "low" => 1,
                    _ => 0,
                },
                tags: task
                    .get("categories")
                    .and_then(Value::as_array)
                    .map(|c| c.iter().filter_map(|t| t.as_str().map(str::to_string)).collect())
                    .unwrap_or_default(),
                done,
                repeat,
                ..Task::default()
            });
            for item in task
                .get("checklistItems")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
            {
                out.tasks.push(Task {
                    key: format!("{id}/{}", text(&item, "id")),
                    parent: Some(id.clone()),
                    title: text(&item, "displayName"),
                    done: item
                        .get("isChecked")
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
                        .then(|| local_moment(Utc::now())),
                    ..Task::default()
                });
            }
        }
    }
    if odd_repeat > 0 {
        out.notes.push(format!(
            "{odd_repeat} tasks repeat on a relative pattern (such as \"the second Tuesday\") that was not converted."
        ));
    }
    out.notes
        .push("Attachments and linked resources are not imported.".into());
    Ok(out)
}

// ---------------------------------------------------------------- writing

// ---------------------------------------------------------------- Lists

/// I11: a JSON export of Lists (docs/json-format.md). Attachment content is not in it.
fn parse_lists(v: &Value) -> Result<Parsed> {
    let version = v["version"].as_u64().unwrap_or(0);
    if version == 0 || version > crate::export::VERSION {
        return Err(invalid(format!(
            "the export uses version {version} of the format, this version understands version {}; update the app",
            crate::export::VERSION
        )));
    }
    let text = |v: &Value| v.as_str().map(str::to_string);
    let mut out = Parsed {
        source: "Lists",
        ..Parsed::default()
    };
    for l in v["lists"].as_array().into_iter().flatten() {
        let Some(key) = text(&l["id"]) else { continue };
        // The inbox is the inbox of the device that imports.
        if key == INBOX_ID {
            continue;
        }
        let mut settings: Vec<(&'static str, Value)> = Vec::new();
        if let Some(icon) = text(&l["icon"]).filter(|i| !i.is_empty()) {
            settings.push(("icon", json!(icon)));
        }
        if let Some(sort) = l["sort"].as_str() {
            settings.push(("sort", json!(SortMode::parse(sort).as_str())));
        }
        for (field, name) in [
            ("show_done", "showDone"),
            ("default_due", "defaultDueToday"),
            ("archived", "archived"),
        ] {
            if let Some(b) = l[name].as_bool() {
                settings.push((field, json!(b)));
            }
        }
        if let Some(p) = l["defaultPriority"].as_str() {
            settings.push(("default_priority", json!(priority_of(p).as_i64())));
        }
        out.lists.push(List {
            key,
            name: text(&l["name"]).unwrap_or_default(),
            color: text(&l["color"]).unwrap_or_default(),
            settings,
        });
    }
    let mut files = 0usize;
    for t in v["tasks"].as_array().into_iter().flatten() {
        let Some(key) = text(&t["id"]) else { continue };
        files += t["attachments"].as_array().map_or(0, Vec::len);
        let repeat = match &t["repeat"] {
            Value::Null => None,
            r => serde_json::from_value::<Repeat>(r.clone()).ok(),
        };
        let done = text(&t["done"]);
        out.tasks.push(Task {
            key,
            list: text(&t["list"]).filter(|l| l != INBOX_ID),
            parent: text(&t["parent"]),
            title: text(&t["title"]).unwrap_or_default(),
            notes: text(&t["notes"]).unwrap_or_default(),
            start: text(&t["start"]),
            due: text(&t["due"]),
            remind: text(&t["remind"]),
            priority: priority_of(t["priority"].as_str().unwrap_or_default()).as_i64(),
            tags: t["tags"].as_array().into_iter().flatten().filter_map(text).collect(),
            wont: done.is_some() && t["wontDo"].as_bool() == Some(true),
            done,
            project: t["project"].as_bool() == Some(true),
            repeat,
            files: vec![],
        });
    }
    for f in v["filters"].as_array().into_iter().flatten() {
        let (Some(key), Some(name)) = (text(&f["id"]), text(&f["name"])) else {
            continue;
        };
        if let Ok(spec) = serde_json::from_value::<FilterSpec>(f["spec"].clone()) {
            out.filters.push((key, name, spec));
        }
    }
    if files > 0 {
        out.notes.push(format!(
            "A JSON export holds no attachment content: {files} attachment{} not imported.",
            if files == 1 { " was" } else { "s were" }
        ));
    }
    Ok(out)
}

fn priority_of(name: &str) -> Priority {
    match name {
        "low" => Priority::Low,
        "medium" => Priority::Medium,
        "high" => Priority::High,
        _ => Priority::None,
    }
}

fn derived_id(source: &str, kind: &str, key: &str) -> String {
    Uuid::new_v5(
        &Uuid::NAMESPACE_URL,
        format!("lists-import:{source}:{kind}:{key}").as_bytes(),
    )
    .to_string()
}

fn clean_tag(tag: &str) -> String {
    tag.trim()
        .trim_start_matches('#')
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join("-")
}

impl Store {
    fn write_import(&self, parsed: Parsed) -> Result<ImportReport> {
        let source = parsed.source;
        let mut report = ImportReport {
            source: source.to_string(),
            notes: parsed.notes,
            ..ImportReport::default()
        };
        let list_id: HashMap<String, String> = parsed
            .lists
            .iter()
            .map(|l| (l.key.clone(), derived_id(source, "list", &l.key)))
            .collect();
        let task_id: HashMap<String, String> = parsed
            .tasks
            .iter()
            .map(|t| (t.key.clone(), derived_id(source, "task", &t.key)))
            .collect();
        let mut blobs: Vec<(PathBuf, String)> = Vec::new();
        let mut long_tags = 0u32;

        self.write(|w| {
            // R121: while Inbox is hidden, tasks without a list go to the default list instead.
            let no_list = if store::inbox_hidden(w.tx)? {
                store::default_list_id(w.tx, None)?
            } else {
                INBOX_ID.to_string()
            };
            let mut last: Option<String> =
                w.tx.query_row("SELECT max(pos) FROM lists WHERE id != 'inbox'", [], |r| r.get(0))?;
            for list in &parsed.lists {
                let id = &list_id[&list.key];
                let exists: bool = w.tx.query_row(
                    "SELECT count(*) > 0 FROM fields WHERE kind = 'list' AND id = ?1",
                    [id],
                    |r| r.get(0),
                )?;
                w.set(
                    KIND_LIST,
                    id,
                    "name",
                    json!(if list.name.trim().is_empty() {
                        "Imported"
                    } else {
                        list.name.trim()
                    }),
                )?;
                if !list.color.is_empty() {
                    w.set(KIND_LIST, id, "color", json!(list.color.to_uppercase()))?;
                }
                for (field, value) in &list.settings {
                    w.set(KIND_LIST, id, field, value.clone())?;
                }
                if !exists {
                    let pos = order::between(last.as_deref(), None);
                    w.set(KIND_LIST, id, "pos", json!(pos))?;
                    w.set(KIND_LIST, id, "deleted", json!(false))?;
                    last = Some(pos);
                }
                report.lists += 1;
            }
            // Positions follow the order in the source, per list and per parent.
            let mut cursor: BTreeMap<String, String> = BTreeMap::new();
            for task in &parsed.tasks {
                if task.title.trim().is_empty() {
                    continue;
                }
                let id = &task_id[&task.key];
                let parent = task.parent.as_ref().and_then(|p| task_id.get(p)).filter(|p| *p != id);
                let list = task
                    .list
                    .as_ref()
                    .and_then(|l| list_id.get(l))
                    .cloned()
                    .unwrap_or_else(|| no_list.clone());
                let container = parent.cloned().unwrap_or_else(|| format!("list:{list}"));
                let pos = order::between(cursor.get(&container).map(String::as_str), None);
                cursor.insert(container, pos.clone());
                let mut set = |field: &str, value: Value| w.set(KIND_TASK, id, field, value);
                set("title", json!(task.title.trim()))?;
                set("list", json!(list))?;
                set("parent", json!(parent))?;
                set("pos", json!(pos))?;
                set("notes", json!(task.notes.trim()))?;
                set("start", json!(task.start))?;
                set("due", json!(task.due))?;
                set("remind", json!(task.remind))?;
                set("priority", json!(task.priority.clamp(0, 3)))?;
                set("done", json!(task.done))?;
                set("wont", if task.wont { json!(task.done) } else { Value::Null })?;
                set("project", json!(task.project && parent.is_none()))?;
                set(
                    "repeat",
                    task.repeat
                        .as_ref()
                        .map(serde_json::to_value)
                        .transpose()?
                        .unwrap_or(Value::Null),
                )?;
                set("deleted", json!(false))?;
                for raw in &task.tags {
                    let name = clean_tag(raw);
                    if name.is_empty() {
                        continue;
                    }
                    match crate::tag::field(&name) {
                        Some(field) => set(&field, json!(true))?,
                        None => long_tags += 1,
                    }
                }
                for (index, (name, path)) in task.files.iter().enumerate() {
                    let Ok(bytes) = std::fs::read(path) else { continue };
                    let sha256 = hex(&Sha256::digest(&bytes));
                    let attachment = derived_id(source, "attachment", &format!("{}#{index}", task.key));
                    w.set(KIND_ATTACHMENT, &attachment, "task", json!(id))?;
                    w.set(KIND_ATTACHMENT, &attachment, "name", json!(name))?;
                    w.set(
                        KIND_ATTACHMENT,
                        &attachment,
                        "mime",
                        json!(crate::store::guess_mime(name)),
                    )?;
                    w.set(KIND_ATTACHMENT, &attachment, "size", json!(bytes.len()))?;
                    w.set(KIND_ATTACHMENT, &attachment, "sha256", json!(sha256))?;
                    w.set(KIND_ATTACHMENT, &attachment, "deleted", json!(false))?;
                    blobs.push((path.clone(), sha256));
                    report.attachments += 1;
                }
                report.tasks += 1;
            }
            let mut last: Option<String> = w.tx.query_row("SELECT max(pos) FROM filters", [], |r| r.get(0))?;
            for (key, name, spec) in &parsed.filters {
                let id = derived_id(source, "filter", key);
                let exists: bool = w.tx.query_row(
                    "SELECT count(*) > 0 FROM fields WHERE kind = 'filter' AND id = ?1",
                    [&id],
                    |r| r.get(0),
                )?;
                let mut spec = spec.clone();
                // The inbox of the file is the inbox here; a list the file does not have is dropped.
                spec.list_ids = spec
                    .list_ids
                    .iter()
                    .filter_map(|l| {
                        if l == INBOX_ID {
                            Some(l.clone())
                        } else {
                            list_id.get(l).cloned()
                        }
                    })
                    .collect();
                w.set(KIND_FILTER, &id, "name", json!(name.trim()))?;
                w.set(KIND_FILTER, &id, "spec", serde_json::to_value(&spec)?)?;
                if !exists {
                    let pos = order::between(last.as_deref(), None);
                    w.set(KIND_FILTER, &id, "pos", json!(pos))?;
                    last = Some(pos);
                }
            }
            Ok(())
        })?;
        if !parsed.filters.is_empty() {
            let n = parsed.filters.len();
            report
                .notes
                .push(format!("{n} saved filter{} imported.", if n == 1 { "" } else { "s" }));
        }
        if long_tags > 0 {
            let (noun, verb) = if long_tags == 1 {
                ("tag", "was")
            } else {
                ("tags", "were")
            };
            report.notes.push(format!(
                "{long_tags} {noun} {verb} longer than {} bytes and {verb} not imported.",
                crate::tag::MAX_NAME_LEN
            ));
        }
        // I9: what the file says about subtasks is brought to the rule of R10.
        self.tidy_if_whole()?;
        for (path, sha256) in blobs {
            let target = self.blob_path(&sha256);
            if !target.exists() {
                let tmp = target.with_extension("part");
                std::fs::copy(&path, &tmp)?;
                std::fs::rename(&tmp, &target)?;
            }
        }
        Ok(report)
    }
}

#[uniffi::export]
impl Store {
    /// Imports a file exported from another task manager. The format is
    /// recognised from the content: a 2Do backup (`.2dodb`), a Todoist CSV
    /// template, a TickTick CSV backup, an OmniFocus CSV export, a Things 3 database (`main.sqlite` or
    /// its `.thingsdatabase` folder), a Trello board JSON, Microsoft To Do lists
    /// as JSON, or a JSON export of this app.
    pub fn import_file(&self, path: String) -> Result<ImportReport> {
        let result = (|| {
            let mut path = PathBuf::from(&path);
            // A Things database is a folder (`Things Database.thingsdatabase`) with `main.sqlite` in it.
            if path.is_dir() {
                path = path.join("main.sqlite");
            }
            let path = path.as_path();
            let mut head = [0u8; 16];
            let read = std::fs::File::open(path)?.read(&mut head)?;
            let zip = read >= 2 && &head[..2] == b"PK";
            if zip || (read == 16 && &head == b"SQLite format 3\0") {
                let work = std::env::temp_dir().join(format!("lists-import-{}", Uuid::now_v7().simple()));
                std::fs::create_dir_all(&work)?;
                let parsed = if zip {
                    parse_2do(path, &work)
                } else {
                    parse_things(path, &work)
                };
                let written = parsed.and_then(|p| self.write_import(p));
                let _ = std::fs::remove_dir_all(&work);
                return written;
            }
            let parsed = {
                let text =
                    String::from_utf8(std::fs::read(path)?).map_err(|_| invalid("the file is not UTF-8 text"))?;
                let trimmed = text.trim_start_matches('\u{feff}').trim_start();
                if trimmed.starts_with('{') || trimmed.starts_with('[') {
                    let value: Value =
                        serde_json::from_str(trimmed).map_err(|e| invalid(format!("not valid JSON: {e}")))?;
                    if value.get("format").and_then(Value::as_str) == Some(crate::export::FORMAT) {
                        parse_lists(&value)?
                    } else if value.get("cards").is_some() && value.get("lists").is_some() {
                        parse_trello(&value)?
                    } else {
                        parse_mstodo(&value)?
                    }
                } else {
                    let name = path
                        .file_stem()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_else(|| "Todoist".into());
                    let rows = csv(&text);
                    if rows.iter().take(16).any(|r| ticktick_header(r)) {
                        parse_ticktick(&rows)?
                    } else if rows.first().is_some_and(|r| omnifocus_header(r)) {
                        parse_omnifocus(&rows, &name)?
                    } else {
                        parse_todoist(&text, &name)?
                    }
                }
            };
            self.write_import(parsed)
        })();
        self.note_result("import", &result, LogLevel::Info, |report| {
            format!(
                "{}: {} lists, {} tasks, {} attachments",
                report.source, report.lists, report.tasks, report.attachments
            )
        });
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv_reader_handles_quotes_and_line_breaks() {
        let rows = csv("\u{feff}a,\"b, c\",\"d \"\"e\"\"\"\r\n1,\"two\nlines\",3\n");
        assert_eq!(rows, vec![vec!["a", "b, c", "d \"e\""], vec!["1", "two\nlines", "3"]]);
    }

    #[test]
    fn twodo_dates() {
        // Noon UTC on 2026-09-27; the time of day is a number written as HHMM.
        assert_eq!(twodo_date(1_790_510_400.0, 999_999.0).as_deref(), Some("2026-09-27"));
        assert_eq!(twodo_date(1_790_510_400.0, 930.0).as_deref(), Some("2026-09-27T09:30"));
        assert_eq!(twodo_date(1_790_510_400.0, 1405.0).as_deref(), Some("2026-09-27T14:05"));
        assert_eq!(twodo_date(1_790_510_400.0, 30.0).as_deref(), Some("2026-09-27T00:30"));
        assert_eq!(twodo_date(1_790_510_400.0, 2359.0).as_deref(), Some("2026-09-27T23:59"));
        assert_eq!(
            twodo_date(1_790_510_400.0, 1299.0).as_deref(),
            Some("2026-09-27"),
            "99 minutes is not a time"
        );
        assert_eq!(twodo_date(6_406_192_800.0, 999_999.0), None);
        assert_eq!(twodo_date(0.0, 999_999.0), None);
        // Start dates as 2Do wrote them for 2026-12-05 and for 2026-12-05 08:15.
        assert_eq!(twodo_start(1_796_428_800.0).as_deref(), Some("2026-12-05"));
        assert_eq!(twodo_start(1_796_458_500.0).as_deref(), Some("2026-12-05T08:15"));
        assert_eq!(twodo_start(6_406_192_800.0), None);
    }

    /// Codes exactly as 2Do 4.19 stored them for tasks created with these rules.
    #[test]
    fn twodo_repeat_codes() {
        let none = (0, 0, 0.0);
        let monday = NaiveDate::from_ymd_opt(2026, 12, 7);
        let get = |kind, value| twodo_repeat(kind, value, 1, none, monday).unwrap();
        let unit = |kind, value| (get(kind, value).freq, get(kind, value).interval);
        assert_eq!(unit(256, 1), (Freq::Daily, 1));
        assert_eq!(unit(257, 1), (Freq::Weekly, 1));
        assert_eq!(unit(257, 2), (Freq::Weekly, 2));
        assert_eq!(unit(258, 1), (Freq::Monthly, 1));
        assert_eq!(unit(258, 3), (Freq::Monthly, 3));
        assert_eq!(unit(259, 1), (Freq::Yearly, 1));
        assert_eq!(unit(259, 3), (Freq::Yearly, 3));

        let days = |kind| get(kind, 1).weekdays;
        assert_eq!(
            days(64),
            Vec::<u32>::new(),
            "Monday on a task due on a Monday is the plain weekly rule"
        );
        assert_eq!(days(1), [7]);
        assert_eq!(days(32), [2]);
        assert_eq!(days(2), [6]);
        assert_eq!(days(84), [1, 3, 5]);
        assert_eq!(days(124), [1, 2, 3, 4, 5]);
        assert_eq!(get(84, 1).freq, Freq::Weekly);

        let nth = |kind, value| {
            (
                get(kind, value).nth.unwrap(),
                get(kind, value).nth_weekday.unwrap(),
                get(kind, value).freq,
            )
        };
        assert_eq!(nth(260, 1), (1, 1, Freq::Monthly), "first Monday");
        assert_eq!(nth(261, 2), (2, 2, Freq::Monthly), "second Tuesday");
        assert_eq!(nth(262, 3), (3, 3, Freq::Monthly), "third Wednesday");
        assert_eq!(nth(263, 4), (4, 4, Freq::Monthly), "fourth Thursday");
        assert_eq!(nth(264, 6), (-1, 5, Freq::Monthly), "last Friday");
        assert_eq!(nth(265, 5), (5, 6, Freq::Monthly), "fifth Saturday");
        assert_eq!(nth(266, 1), (1, 7, Freq::Monthly), "first Sunday");
        assert_eq!(nth(260, 6), (-1, 1, Freq::Monthly), "last Monday");

        assert!(twodo_repeat(256, 1, 2, none, monday).unwrap().from_done);
        assert!(twodo_repeat(2, 1, 2, none, monday).unwrap().from_done);
        assert_eq!(twodo_repeat(256, 1, 1, (2, 5, 0.0), monday).unwrap().count, Some(5));
        assert_eq!(
            twodo_repeat(256, 1, 1, (1, 0, 1_800_014_400.0), monday)
                .unwrap()
                .until
                .as_deref(),
            Some("2027-01-15")
        );

        for (kind, value) in [(128, 1), (255, 1), (256, 0), (260, 7), (267, 1), (512, 1)] {
            assert!(twodo_repeat(kind, value, 1, none, monday).is_none(), "{kind}/{value}");
        }
    }

    /// Values exactly as they stand in real TickTick backups (docs/specs/import.md).
    #[test]
    fn ticktick_dates_and_reminders() {
        let day = |text: &str| ticktick_day(ticktick_instant(text).unwrap()).to_string();
        // Midnight of a day without a time, written by tasks in these zones.
        assert_eq!(day("2024-08-05T16:00:00+0000"), "2024-08-06", "Australia/Perth");
        assert_eq!(day("2026-05-31T18:30:00+0000"), "2026-06-01", "Asia/Kolkata");
        assert_eq!(day("2024-06-01T22:00:00+0000"), "2024-06-02", "Europe/Warsaw");
        assert_eq!(day("2024-04-15T05:00:00+0000"), "2024-04-15", "America/Chicago");
        assert_eq!(day("2024-04-15T10:00:00+0000"), "2024-04-15", "Pacific/Honolulu");
        assert_eq!(day("2024-04-14T11:00:00+0000"), "2024-04-15", "Pacific/Tongatapu");

        let minutes = |text: &str| ticktick_offset(text).map(|d| d.num_minutes());
        assert_eq!(minutes("PT0S"), Some(0));
        assert_eq!(minutes("-PT0S"), Some(0));
        assert_eq!(minutes("-PT30M"), Some(-30));
        assert_eq!(minutes("-P0DT2H0M0S"), Some(-120));
        assert_eq!(minutes("P0DT9H30M0S"), Some(570));
        assert_eq!(minutes("P1W"), Some(7 * 24 * 60));
        for broken in ["", "P", "PT5", "30M", "PT1Y", "P1H"] {
            assert_eq!(minutes(broken), None, "{broken:?}");
        }

        // A day without a time: the reminder counts from its midnight.
        let (start, due, remind) = ticktick_dates(
            "2024-08-05T16:00:00+0000",
            "2024-08-05T16:00:00+0000",
            true,
            "-P0DT2H0M0S",
        );
        assert_eq!(
            (start, due.as_deref(), remind.as_deref()),
            (None, Some("2024-08-06"), Some("2024-08-05T22:00"))
        );
        let (_, _, remind) = ticktick_dates(
            "2024-06-18T22:00:00+0000",
            "2024-06-18T22:00:00+0000",
            true,
            "P0DT9H30M0S\nP0DT9H0M0S",
        );
        assert_eq!(
            remind.as_deref(),
            Some("2024-06-19T09:30"),
            "the first of two reminders"
        );
        // A task with a duration keeps both ends, in the zone of the device; the reminder counts from the start.
        let local = |text: &str| Some(local_moment(ticktick_instant(text).unwrap()));
        assert_eq!(
            ticktick_dates("2026-06-07T04:30:00+0000", "2026-06-07T06:30:00+0000", false, "-PT30M"),
            (
                local("2026-06-07T04:30:00+0000"),
                local("2026-06-07T06:30:00+0000"),
                local("2026-06-07T04:00:00+0000")
            )
        );
        assert_eq!(ticktick_dates("", "", false, "PT0S"), (None, None, None));
        // An offset beyond any calendar is no reminder, not a crash.
        for huge in ["P9999999999999D", "P99999999999999999999D", "P106751991167300D"] {
            let (_, due, remind) = ticktick_dates("2024-08-05T16:00:00+0000", "2024-08-05T16:00:00+0000", false, huge);
            assert!(due.is_some() && remind.is_none(), "{huge}");
        }
    }

    /// Values exactly as the Things 3.24.1 database holds them (docs/specs/import.md).
    #[test]
    fn things_dates_and_repeat_rules() {
        // Dates Things itself reported for these values over AppleScript.
        assert_eq!(things_date(Some(132818432)).unwrap().to_string(), "2026-10-12");
        assert_eq!(things_date(Some(132816000)).unwrap().to_string(), "2026-09-25");
        assert_eq!(things_date(Some(132821120)).unwrap().to_string(), "2026-11-01");
        assert_eq!(things_date(Some(262213760)), None, "year 4001 is no date");
        assert_eq!(things_date(None), None);
        assert_eq!(things_time(Some(568328192)), Some((8, 30)));
        assert_eq!(things_time(Some(1342177280)), Some((20, 0)));
        assert_eq!(things_time(Some(785383424)), Some((11, 45)));

        let xml = br#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>ed</key>
	<real>64092211200</real>
	<key>fa</key>
	<integer>1</integer>
	<key>fu</key>
	<integer>256</integer>
	<key>of</key>
	<array>
		<dict>
			<key>wd</key>
			<integer>6</integer>
		</dict>
	</array>
	<key>tp</key>
	<integer>0</integer>
</dict>
</plist>"#;
        let weekly = things_rule(&plist(xml).unwrap()).unwrap();
        assert_eq!(
            (weekly.freq, weekly.interval, weekly.weekdays),
            (Freq::Weekly, 1, vec![6])
        );

        // The thirteen rules of the database, with what the dates of their tasks showed.
        let never = 64092211200.0;
        let cases: [(Value, Option<Repeat>); 13] = [
            (
                json!({"fu":8,"fa":1,"of":[{"dy":4}],"tp":0,"ts":-2,"rc":0,"ed":never}),
                None,
            ),
            (
                json!({"fu":256,"fa":1,"of":[{"wd":6}],"tp":0,"ts":0,"rc":0,"ed":never}),
                Some(Repeat {
                    weekdays: vec![6],
                    ..rule(Freq::Weekly, 1)
                }),
            ),
            (
                json!({"fu":8,"fa":1,"of":[{"dy":24}],"tp":0,"ts":0,"rc":0,"ed":never}),
                Some(Repeat {
                    monthday: Some(25),
                    ..rule(Freq::Monthly, 1)
                }),
            ),
            (
                json!({"fu":256,"fa":1,"of":[{"wd":6}],"tp":0,"ts":0,"rc":0,"ed":never}),
                Some(Repeat {
                    weekdays: vec![6],
                    ..rule(Freq::Weekly, 1)
                }),
            ),
            (
                json!({"fu":8,"fa":2,"of":[{"dy":1}],"tp":0,"ts":0,"rc":0,"ed":never}),
                Some(Repeat {
                    monthday: Some(2),
                    ..rule(Freq::Monthly, 2)
                }),
            ),
            (
                json!({"fu":8,"fa":1,"of":[{"dy":12}],"tp":0,"ts":0,"rc":0,"ed":never}),
                Some(Repeat {
                    monthday: Some(13),
                    ..rule(Freq::Monthly, 1)
                }),
            ),
            (
                json!({"fu":16,"fa":1,"of":[{"dy":0}],"tp":0,"ts":0,"rc":0,"ed":never}),
                Some(rule(Freq::Daily, 1)),
            ),
            (
                json!({"fu":4,"fa":1,"of":[{"dy":20,"mo":1}],"tp":0,"ts":0,"rc":0,"ed":never}),
                Some(rule(Freq::Yearly, 1)),
            ),
            (
                json!({"fu":16,"fa":5,"of":[{"dy":0}],"tp":1,"ts":0,"rc":0,"ed":never}),
                Some(Repeat {
                    from_done: true,
                    ..rule(Freq::Daily, 5)
                }),
            ),
            (
                json!({"fu":8,"fa":1,"of":[{"dy":14}],"tp":0,"ts":0,"rc":0,"ed":never}),
                Some(Repeat {
                    monthday: Some(15),
                    ..rule(Freq::Monthly, 1)
                }),
            ),
            (
                json!({"fu":8,"fa":1,"of":[{"dy":17}],"tp":0,"ts":0,"rc":0,"ed":never}),
                Some(Repeat {
                    monthday: Some(18),
                    ..rule(Freq::Monthly, 1)
                }),
            ),
            (
                json!({"fu":256,"fa":1,"of":[{"wd":0}],"tp":0,"ts":0,"rc":0,"ed":never}),
                Some(Repeat {
                    weekdays: vec![7],
                    ..rule(Freq::Weekly, 1)
                }),
            ),
            (
                json!({"fu":8,"fa":1,"of":[{"dy":2}],"tp":0,"ts":0,"rc":0,"ed":never}),
                Some(Repeat {
                    monthday: Some(3),
                    ..rule(Freq::Monthly, 1)
                }),
            ),
        ];
        for (plist, expected) in cases {
            assert_eq!(things_rule(&plist), expected, "{plist}");
        }

        // Forms the database did not have: an end date is kept, the rest is refused.
        let until = things_rule(&json!({"fu":16,"fa":1,"of":[],"tp":0,"ts":0,"rc":0,"ed":1798761600.0})).unwrap();
        assert_eq!(until.until.as_deref(), Some("2027-01-01"));
        for odd in [
            json!({"fu":16,"fa":1,"rc":3}),
            json!({"fu":32,"fa":1}),
            json!({"fu":8,"fa":1,"of":[{"wd":2,"wo":2}]}),
            json!({"fu":8,"fa":1,"of":[{"dy":-1}]}),
            json!({"fu":16,"fa":0}),
        ] {
            assert_eq!(things_rule(&odd), None, "{odd}");
        }
    }
}
