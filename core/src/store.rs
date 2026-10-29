//! The object the apps talk to. Every public method is synchronous and safe
//! to call from any thread; apps call them off the main thread.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use chrono::{NaiveDate, NaiveDateTime};
use rusqlite::{params, params_from_iter, Connection, OptionalExtension, Row};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::db::{
    self, Change, Touched, KIND_ATTACHMENT, KIND_FILTER, KIND_LIST, KIND_SETTINGS, KIND_TASK, SETTINGS_ID,
};
use crate::error::{AppError, Result};
use crate::hlc::{Clock, DEVICE_ID_LEN};
use crate::log::LogLevel;
use crate::model::*;
use crate::recur::DATE_FMT;
use crate::undo::Step;
use crate::{order, quickadd, recur, subtasks};

const MOMENT_FMT: &str = "%Y-%m-%dT%H:%M";

/// R140: where the limits of this device are kept in `meta`, next to the level of the log.
const MAX_FILE_KEY: &str = "max_file_bytes";
const MAX_TASK_KEY: &str = "max_task_bytes";

/// R140: a file that grew past the limit while it was copied.
fn over(limit: u64, amount: u64) -> bool {
    limit != 0 && amount > limit
}

/// R142: a folder only its owner can look into, on Unix. Other systems have their own rules.
pub(crate) fn private_dir(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    let _ = path;
    Ok(())
}

/// R142: a file only its owner can read, on Unix.
pub(crate) fn private_file(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    let _ = path;
    Ok(())
}

/// R142: creates the folder and everything above it, closed from other users from the start.
pub(crate) fn make_private_dir(path: &Path) -> std::io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)?;
    private_dir(path)
}

/// R140: the limits kept in `meta`, or the defaults of a device that has none.
fn stored_limits(conn: &Connection) -> Result<AttachmentLimits> {
    let read = |key: &str, fallback: u64| -> Result<u64> {
        Ok(db::meta_get(conn, key)?
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(fallback))
    };
    Ok(AttachmentLimits {
        max_file: read(MAX_FILE_KEY, DEFAULT_MAX_FILE)?,
        max_task: read(MAX_TASK_KEY, DEFAULT_MAX_TASK)?,
    })
}

pub(crate) struct Inner {
    pub conn: Connection,
    pub clock: Clock,
    /// Fixed "now" for tests; the wall clock otherwise.
    pub now: Option<NaiveDateTime>,
}

#[derive(uniffi::Object)]
pub struct Store {
    pub(crate) inner: Mutex<Inner>,
    /// Serialises sync runs without blocking local edits during network calls.
    pub(crate) sync_lock: Mutex<()>,
    /// One pass over attachment content at a time; runs of the fields do not wait for it (S34).
    pub(crate) blob_lock: Mutex<()>,
    /// One look at the backup schedule at a time, so that two callers do not both find it due (R88).
    pub(crate) backup_lock: Mutex<()>,
    pub(crate) dir: PathBuf,
    /// The store itself, for work that outlives the call that started it (S19).
    pub(crate) me: std::sync::Weak<Store>,
    /// The level of the log (R102) as its code, read on every line that may be written.
    pub(crate) log_level: std::sync::atomic::AtomicU8,
    /// One writer of the log file at a time.
    pub(crate) log_lock: Mutex<()>,
    /// S45: own log files that make a snapshot whatever they weigh.
    pub(crate) compact_after: Mutex<u32>,
    pub(crate) sync_password: Mutex<Option<String>>,
    /// S58: the passphrase of an encrypted storage. Like the password, never on disk.
    pub(crate) sync_passphrase: Mutex<Option<String>>,
    /// S54: the storage whose `vault.json` was read last, and what it said.
    pub(crate) sealed: Mutex<Option<(String, crate::sync::Sealing)>>,
    /// S54: the data key opened last, so that the key derivation does not run on every look.
    pub(crate) vault_key: Mutex<Option<crate::sync::VaultKey>>,
    /// The storage this process has already checked the format of and listed the snapshots of.
    pub(crate) storage_seen: Mutex<Option<String>>,
    /// Names in the log of the sync folder when it was last looked at.
    pub(crate) folder_seen: Mutex<Option<Vec<String>>>,
    /// How long a failed wait for a nudge holds back the next one.
    pub(crate) push_retry: Mutex<std::time::Duration>,
    /// Access token for this device's own ntfy server (S27). Like the password, never on disk.
    pub(crate) push_token: Mutex<Option<String>>,
    /// S30: the own server answered 401 or 403 to the last subscription, to the last nudge.
    pub(crate) push_refused: Mutex<(bool, bool)>,
    /// How many objects CalDAV sync has built for upload or comparison.
    pub(crate) caldav_renders: AtomicU64,
    /// What undo takes back (R131).
    pub(crate) history: Mutex<crate::undo::History>,
    /// R140: how much content this device takes, kept by the device and not synced.
    pub(crate) limits: Mutex<AttachmentLimits>,
}

/// One local write transaction: stamps and records field changes, then
/// rebuilds the derived rows before commit.
pub(crate) struct Writer<'a> {
    pub tx: &'a Connection,
    clock: &'a mut Clock,
    now_ms: u64,
    pub now: NaiveDateTime,
    touched: Touched,
    /// What an action that undo can take back has written so far (R131).
    noted: Option<Step>,
}

impl Writer<'_> {
    pub fn set(&mut self, kind: &str, id: &str, field: &str, value: Value) -> Result<()> {
        self.set_stamped(kind, id, field, value).map(drop)
    }
    /// The same, returning the stamp the register got.
    pub(crate) fn set_stamped(&mut self, kind: &str, id: &str, field: &str, value: Value) -> Result<String> {
        let stamp = self.clock.tick(self.now_ms);
        if let Some(step) = &mut self.noted {
            step.note(self.tx, kind, id, field, &stamp)?;
        }
        let change = Change {
            kind: kind.into(),
            id: id.into(),
            field: field.into(),
            value,
            stamp,
        };
        db::apply(self.tx, &change, true, &mut self.touched)?;
        Ok(change.stamp)
    }
    /// Writes that keep the data whole rather than carry out the action stay out of its step.
    fn unnoted<T>(&mut self, f: impl FnOnce(&mut Self) -> Result<T>) -> Result<T> {
        let noted = self.noted.take();
        let out = f(self);
        self.noted = noted;
        out
    }
    pub(crate) fn task(&mut self, id: &str, field: &str, value: Value) -> Result<()> {
        self.set(KIND_TASK, id, field, value)
    }
    pub(crate) fn list(&mut self, id: &str, field: &str, value: Value) -> Result<()> {
        self.set(KIND_LIST, id, field, value)
    }
    /// Makes pending changes visible to queries inside the same transaction.
    pub(crate) fn flush(&mut self) -> Result<()> {
        let touched = std::mem::take(&mut self.touched);
        db::settle(self.tx, &touched)
    }
    /// R112: brings what does not fit R10 to it. Called where the state is whole: see `subtasks`.
    pub(crate) fn tidy(&mut self) -> Result<()> {
        let (clock, now_ms) = (&mut *self.clock, self.now_ms);
        subtasks::tidy(self.tx, &mut || clock.tick(now_ms))
    }
    fn today(&self) -> NaiveDate {
        self.now.date()
    }
    pub(crate) fn moment(&self) -> String {
        self.now.format(MOMENT_FMT).to_string()
    }
}

thread_local! {
    /// The zone the calls of this thread read the time in (see `Store::in_zone`).
    static ZONE: std::cell::Cell<Option<chrono::FixedOffset>> = const { std::cell::Cell::new(None) };
}

impl Store {
    /// R146: what came due after `since` and by `until` (`YYYY-MM-DDTHH:MM[:SS]`, local time),
    /// earliest first. The web server sends these; it has no system to schedule notifications
    /// with. The caller gives both ends, so that windows which follow each other neither overlap
    /// nor leave a gap. A moment that cannot be read gives nothing.
    pub fn due_notifications(
        &self,
        settings: NotifySettings,
        since: &str,
        until: &str,
    ) -> Result<Vec<PlannedNotification>> {
        let read = |text: &str| {
            NaiveDateTime::parse_from_str(text, "%Y-%m-%dT%H:%M:%S")
                .or_else(|_| NaiveDateTime::parse_from_str(text, MOMENT_FMT))
                .ok()
        };
        let (Some(since), Some(until)) = (read(since), read(until)) else {
            return Ok(vec![]);
        };
        self.notifications(settings, Some((since, until)))
    }

    /// The plan of `planned_notifications` (no window) or the window of `due_notifications`.
    fn notifications(
        &self,
        settings: NotifySettings,
        window: Option<(NaiveDateTime, NaiveDateTime)>,
    ) -> Result<Vec<PlannedNotification>> {
        let planned = (|| {
            if !settings.enabled {
                return Ok(vec![]);
            }
            let inner = self.lock();
            let now = inner.now();
            let clock = |value: &Option<String>| {
                value
                    .as_deref()
                    .and_then(|v| chrono::NaiveTime::parse_from_str(v, "%H:%M").ok())
            };
            let all_day = clock(&settings.all_day_at);
            let moment = |value: &str| NaiveDateTime::parse_from_str(value, MOMENT_FMT).ok();
            let tasks = query_tasks(
                &inner.conn,
                &format!(
                "WHERE {LIVE} AND {NOT_ARCHIVED} AND t.done IS NULL AND (t.remind IS NOT NULL OR {EFF_DUE} IS NOT NULL)"
            ),
                &[],
            )?;
            // From now on for a plan; for a window, what fell into it.
            let wanted = |at: &NaiveDateTime| match window {
                None => *at > now,
                Some((since, until)) => *at > since && *at <= until,
            };
            let mut out = Vec::new();
            for task in &tasks {
                // (kind, moment, what tells notifications of one task apart)
                let planned: Vec<(NotificationKind, NaiveDateTime, String)> = match (&task.remind, &task.effective_due)
                {
                    // A reminder without a time of day follows the all-day setting, nine o'clock failing that.
                    (Some(remind), _) => moment(remind)
                        .or_else(|| {
                            recur::split(remind).map(|(d, _)| {
                                d.and_time(
                                    all_day.unwrap_or_else(|| chrono::NaiveTime::from_hms_opt(9, 0, 0).expect("valid")),
                                )
                            })
                        })
                        .map(|at| (NotificationKind::Reminder, at, String::new()))
                        .into_iter()
                        .collect(),
                    (None, Some(due)) if due.len() > 10 => moment(due)
                        .map(|at| {
                            let mut leads = settings.lead_minutes.clone();
                            leads.sort_unstable();
                            leads.dedup();
                            leads
                                .into_iter()
                                .map(|lead| {
                                    (
                                        NotificationKind::Due,
                                        at - chrono::Duration::minutes(i64::from(lead)),
                                        format!(":{lead}"),
                                    )
                                })
                                .collect()
                        })
                        .unwrap_or_default(),
                    (None, Some(due)) => all_day
                        .and_then(|time| {
                            recur::split(due).map(|(d, _)| (NotificationKind::Due, d.and_time(time), String::new()))
                        })
                        .into_iter()
                        .collect(),
                    (None, None) => Vec::new(),
                };
                for (kind, at, suffix) in planned.into_iter().filter(|(_, at, _)| wanted(at)) {
                    out.push(PlannedNotification {
                        key: format!("task:{}{suffix}", task.id),
                        kind,
                        at: at.format(MOMENT_FMT).to_string(),
                        task_id: Some(task.id.clone()),
                        title: task.title.clone(),
                        due: task.effective_due.clone(),
                        count: 0,
                    });
                }
            }
            if let Some(time) = clock(&settings.summary_at) {
                let first = window.map_or(now.date(), |(since, _)| since.date().min(now.date()));
                for offset in 0..7 {
                    let day = first + chrono::Duration::days(offset);
                    let at = day.and_time(time);
                    let key = day.format(DATE_FMT).to_string();
                    // Everything due by that day, as things stand now; a project is counted in its tasks (R125).
                    let count = tasks
                        .iter()
                        .filter(|t| !(t.is_project && t.parent_id.is_none()))
                        .filter(|t| t.effective_due.as_deref().is_some_and(|d| d[..10] <= *key))
                        .count() as u32;
                    if wanted(&at) && count > 0 {
                        out.push(PlannedNotification {
                            key: format!("summary:{key}"),
                            kind: NotificationKind::Summary,
                            at: at.format(MOMENT_FMT).to_string(),
                            task_id: None,
                            title: String::new(),
                            due: None,
                            count,
                        });
                    }
                }
            }
            out.sort_by(|a, b| (&a.at, &a.key).cmp(&(&b.at, &b.key)));
            if window.is_none() {
                out.truncate(60);
            }
            Ok(out)
        })();
        // R102: without the plan the app sets no reminder at all.
        if let Err(e) = &planned {
            self.note(LogLevel::Error, "notify", || format!("reminders not planned: {e}"));
        }
        planned
    }

    /// Runs `f` with today, the time of day and the moments it writes taken on a clock that many
    /// minutes east of UTC rather than on this device's. The web server answers each browser in the
    /// zone of the browser (R135). The stamps keep their clock: it has no time zone.
    pub fn in_zone<T>(&self, minutes_east: i32, f: impl FnOnce() -> T) -> T {
        let offset = minutes_east.checked_mul(60).and_then(chrono::FixedOffset::east_opt);
        let before = ZONE.with(|zone| zone.replace(offset));
        let out = f();
        ZONE.with(|zone| zone.set(before));
        out
    }
}

fn zone() -> Option<chrono::FixedOffset> {
    ZONE.with(|zone| zone.get())
}

/// A moment as the clock of the zone in force reads it: the one `in_zone` gives, or this device's.
pub(crate) fn local_time(utc: chrono::DateTime<chrono::Utc>) -> NaiveDateTime {
    match zone() {
        Some(offset) => utc.with_timezone(&offset).naive_local(),
        None => utc.with_timezone(&chrono::Local).naive_local(),
    }
}

impl Inner {
    pub fn now(&self) -> NaiveDateTime {
        match (self.now, zone()) {
            // The clock of the tests is read as UTC once a zone is given.
            (Some(fixed), Some(offset)) => fixed + offset,
            (Some(fixed), None) => fixed,
            (None, _) => local_time(chrono::Utc::now()),
        }
    }

    /// The clock of the stamps: milliseconds, the same on every device whatever its time zone.
    pub fn now_ms(&self) -> u64 {
        match self.now {
            Some(fixed) => fixed.and_utc().timestamp_millis().max(0) as u64,
            None => chrono::Utc::now().timestamp_millis().max(0) as u64,
        }
    }

    /// A moment of this device's local time on the clock of the stamps.
    fn ms_of(&self, local: NaiveDateTime) -> u64 {
        use chrono::TimeZone;
        let utc = match (self.now, zone()) {
            // A zone given by its offset has no hour that the clocks skip.
            (_, Some(offset)) => (local - offset).and_utc().timestamp_millis(),
            // The clock of the tests has no time zone.
            (Some(_), None) => local.and_utc().timestamp_millis(),
            // An hour the clocks skip has no moment of its own: the day then starts where that hour ends.
            (None, None) => [0, 1]
                .into_iter()
                .find_map(|hours| {
                    let shifted = local + chrono::Duration::hours(hours);
                    chrono::Local.from_local_datetime(&shifted).earliest()
                })
                .map_or_else(|| chrono::Utc::now().timestamp_millis(), |t| t.timestamp_millis()),
        };
        utc.max(0) as u64
    }

    pub fn write<T>(&mut self, f: impl FnOnce(&mut Writer) -> Result<T>) -> Result<T> {
        self.write_with(None, f).map(|(out, _)| out)
    }

    /// A write that notes what it changes, for undo (R131).
    pub(crate) fn write_noted<T>(&mut self, f: impl FnOnce(&mut Writer) -> Result<T>) -> Result<(T, Step)> {
        self.write_with(Some(Step::default()), f)
            .map(|(out, step)| (out, step.unwrap_or_default()))
    }

    fn write_with<T>(
        &mut self,
        noted: Option<Step>,
        f: impl FnOnce(&mut Writer) -> Result<T>,
    ) -> Result<(T, Option<Step>)> {
        let now = self.now();
        let now_ms = self.now_ms();
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let out = {
            let mut w = Writer {
                tx: &tx,
                clock: &mut self.clock,
                now_ms,
                now,
                touched: Touched::new(),
                noted,
            };
            let out = f(&mut w)?;
            w.flush()?;
            (out, w.noted)
        };
        tx.commit()?;
        Ok(out)
    }
}

/// A date of the project the task belongs to, when it belongs to one (R30).
macro_rules! project_date {
    ($field:literal) => {
        concat!(
            "(SELECT p.",
            $field,
            " FROM tasks p WHERE p.id = t.eff_parent AND p.project = 1 AND p.eff_parent IS NULL)"
        )
    };
}

/// The due date the views go by: the task's own, or its project's when it has none (R125).
macro_rules! eff_due {
    () => {
        concat!("coalesce(t.due, ", project_date!("due"), ")")
    };
}

/// The start the views go by: a task with a date does not start before its project, and the start
/// of the project gives no date to a task without one (R125).
macro_rules! eff_start {
    () => {
        concat!(
            "(CASE WHEN t.start IS NULL AND ",
            eff_due!(),
            " IS NULL THEN NULL ELSE max(coalesce(t.start, ",
            project_date!("start"),
            "), coalesce(",
            project_date!("start"),
            ", t.start)) END)"
        )
    };
}

const EFF_DUE: &str = eff_due!();
const EFF_START: &str = eff_start!();
/// The date a task is placed by: its effective due date, or its effective start without one.
const EFF_DATE: &str = concat!("coalesce(", eff_due!(), ", ", eff_start!(), ")");

const TASK_COLUMNS: &str = concat!(
    "
    t.id, t.eff_list, t.eff_parent,
    (SELECT p.title FROM tasks p WHERE p.id = t.eff_parent),
    t.title, t.notes, t.start, t.due, t.priority, t.repeat, t.remind, t.done, t.deleted,
    t.log_of IS NOT NULL,
    t.project,
    (SELECT count(*) FROM tasks c WHERE c.eff_parent = t.id AND c.deleted = 0 AND c.purged = 0 AND c.wont = 0),
    (SELECT count(*) FROM tasks c WHERE c.eff_parent = t.id AND c.deleted = 0 AND c.purged = 0 AND c.wont = 0 AND c.done IS NOT NULL),
    (SELECT count(*) FROM attachments a WHERE a.task_id = t.id AND a.deleted = 0),
    (SELECT group_concat(tag, char(31)) FROM (SELECT tag FROM task_tags WHERE task_id = t.id ORDER BY tag)),
    t.wont, ",
    eff_due!(),
    ", ",
    eff_start!(),
    ",
    CASE WHEN t.log_of IS NULL OR t.done IS NULL THEN 0 ELSE EXISTS (
      SELECT 1 FROM fields f, tasks o
      WHERE f.kind = 'task' AND f.id = t.id AND f.field = 'undo' AND o.id = t.log_of AND o.purged = 0 AND o.done IS NULL
        AND CASE WHEN json_valid(f.value) AND json_type(f.value) = 'object'
            THEN o.due IS json_extract(f.value, '$.next_due') AND o.start IS json_extract(f.value, '$.next_start')
            ELSE 0 END) END"
);

/// Visible, not a completion record.
const LIVE: &str = "t.eff_deleted = 0 AND t.log_of IS NULL";
/// A task, or the record of a repeat of a task that is not in a project: views of top-level tasks show the
/// record of a project's task no more than they show the task.
const OF_TOP_TASK: &str =
    "(t.log_of IS NULL OR NOT EXISTS (SELECT 1 FROM tasks o WHERE o.id = t.log_of AND o.eff_parent IS NOT NULL))";
/// What the Completed view lists: finished top-level tasks and the records of finished repeats.
const COMPLETED: &str = "t.eff_deleted = 0 AND t.done IS NOT NULL AND (t.eff_parent IS NULL OR t.log_of IS NOT NULL)";
const NOT_ARCHIVED: &str = "t.eff_list NOT IN (SELECT id FROM lists WHERE archived = 1)";
/// A project is shown in the sidebar, not as a row of the task views and their counters (R120).
const NOT_PROJECT: &str = "NOT (t.project = 1 AND t.eff_parent IS NULL)";

/// How long a completed task stays in its view when nothing else is set (R68).
const KEEP_DONE_DEFAULT: KeepDone = KeepDone::Seconds { seconds: 5 };
const KEEP_DONE_MAX_MINUTES: u32 = 24 * 60;

/// The setting as S32 stores it: a number is minutes, the form a version before the seconds reads;
/// a string is seconds with an `s`, or `day`.
fn keep_done_from_value(value: &Value) -> Option<KeepDone> {
    if let Some(minutes) = value.as_u64() {
        let minutes = minutes.min(u64::from(KEEP_DONE_MAX_MINUTES)) as u32;
        return Some(KeepDone::Seconds { seconds: minutes * 60 });
    }
    match value.as_str()? {
        "day" => Some(KeepDone::EndOfDay),
        text => {
            let seconds = text.strip_suffix('s')?.parse::<u32>().ok()?;
            Some(KeepDone::Seconds {
                seconds: seconds.min(KEEP_DONE_MAX_MINUTES * 60),
            })
        }
    }
}

fn keep_done_to_value(keep: KeepDone) -> Value {
    match keep {
        KeepDone::EndOfDay => json!("day"),
        KeepDone::Seconds { seconds } => {
            let seconds = seconds.min(KEEP_DONE_MAX_MINUTES * 60);
            if seconds % 60 == 0 {
                json!(seconds / 60)
            } else {
                json!(format!("{seconds}s"))
            }
        }
    }
}

/// One field of the shared settings (S32); none when it is not there or does not parse.
fn setting(conn: &Connection, field: &str) -> Result<Option<Value>> {
    let stored: Option<String> = conn
        .query_row(
            "SELECT value FROM fields WHERE kind = ?1 AND id = ?2 AND field = ?3",
            [KIND_SETTINGS, SETTINGS_ID, field],
            |r| r.get(0),
        )
        .optional()?;
    Ok(stored.and_then(|v| serde_json::from_str::<Value>(&v).ok()))
}

fn keep_done(conn: &Connection) -> Result<KeepDone> {
    Ok(setting(conn, "keep_done")?
        .and_then(|v| keep_done_from_value(&v))
        .unwrap_or(KEEP_DONE_DEFAULT))
}

/// R121: Inbox has no row while this is set.
pub(crate) fn inbox_hidden(conn: &Connection) -> Result<bool> {
    Ok(setting(conn, "inbox_hidden")?
        .and_then(|v| v.as_bool())
        .unwrap_or(false))
}

/// R40: `inbox`, `last` or the id of a list; what is not a string reads as `inbox`.
fn new_task_list(conn: &Connection) -> Result<String> {
    Ok(setting(conn, "new_task_list")?
        .and_then(|v| v.as_str().filter(|s| !s.is_empty()).map(str::to_string))
        .unwrap_or_else(|| INBOX_ID.to_string()))
}

/// The list a task without a list goes to (R40, R121). `last_used` is what this device used last.
pub(crate) fn default_list_id(conn: &Connection, last_used: Option<&str>) -> Result<String> {
    let hidden = inbox_hidden(conn)?;
    let choice = new_task_list(conn)?;
    let wanted = if choice == "last" {
        last_used
    } else {
        Some(choice.as_str())
    };
    let fits = |id: &str| -> Result<bool> {
        if id == INBOX_ID {
            return Ok(!hidden);
        }
        Ok(conn
            .query_row(
                "SELECT 1 FROM lists WHERE id = ?1 AND deleted = 0 AND archived = 0",
                [id],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    };
    if let Some(id) = wanted {
        if fits(id)? {
            return Ok(id.to_string());
        }
    }
    if !hidden {
        return Ok(INBOX_ID.to_string());
    }
    Ok(first_list(conn)?.unwrap_or_else(|| INBOX_ID.to_string()))
}

/// The first of the user's lists that is not archived, in their order.
fn first_list(conn: &Connection) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT id FROM lists WHERE deleted = 0 AND archived = 0 AND id != 'inbox' ORDER BY pos, id LIMIT 1",
            [],
            |r| r.get(0),
        )
        .optional()?)
}

/// R68 as SQL: a task completed a moment ago still counts as open.
struct Recent {
    /// Open, or completed within the time the setting gives.
    open: String,
    /// Completed long enough ago to go where the completed ones go.
    settled: String,
    /// What a task view may show: the live tasks, and the records of repeats completed within
    /// the time, which stand where their task stood (R68, #287).
    live: String,
    /// Records of the repeats of a project's tasks still kept; `?1` is the project.
    project_records: String,
}

impl Recent {
    fn at(inner: &Inner) -> Result<Self> {
        let Some(Kept { window, .. }) = kept(inner)? else {
            return Ok(Recent {
                open: "t.done IS NULL".into(),
                settled: "t.done IS NOT NULL".into(),
                live: LIVE.into(),
                project_records: "0".into(),
            });
        };
        Ok(Recent {
            open: format!("(t.done IS NULL OR {window})"),
            settled: format!("(t.done IS NOT NULL AND NOT {window})"),
            live: format!("t.eff_deleted = 0 AND (t.log_of IS NULL OR {window})"),
            project_records: format!(
                "(t.eff_deleted = 0 AND {window} AND t.log_of IN (SELECT id FROM tasks WHERE eff_parent = ?1))"
            ),
        })
    }
}

/// The completed tasks the setting still keeps in their views.
struct Kept {
    /// SQL that is true for such a task.
    window: String,
    /// When a task whose `done` was stamped at the given millisecond leaves, on the same clock.
    leaves_ms: Box<dyn Fn(u64) -> u64>,
}

/// None when the setting gives no time.
///
/// The time counts from the stamp of `done`: it has the precision the setting needs and no time zone.
/// The value of `done` has to agree with it to the minute, so that tasks whose completion was only
/// recorded now (an import, a calendar read for the first time) are not taken for just completed.
fn kept(inner: &Inner) -> Result<Option<Kept>> {
    let (now, now_ms) = (inner.now(), inner.now_ms());
    let window = |stamps: (u64, u64), done: (NaiveDateTime, NaiveDateTime)| {
        format!(
            "(substr(t.done_stamp, 1, 12) BETWEEN '{:012x}' AND '{:012x}' AND t.done BETWEEN '{}' AND '{}')",
            stamps.0,
            stamps.1,
            done.0.format(MOMENT_FMT),
            done.1.format(MOMENT_FMT)
        )
    };
    Ok(match keep_done(&inner.conn)? {
        KeepDone::Seconds { seconds: 0 } => None,
        KeepDone::Seconds { seconds } => {
            let span_ms = u64::from(seconds) * 1000;
            let slack = chrono::Duration::milliseconds(span_ms as i64) + chrono::Duration::minutes(1);
            Some(Kept {
                window: window(
                    (now_ms.saturating_sub(span_ms), now_ms + span_ms),
                    (now - slack, now + slack),
                ),
                leaves_ms: Box::new(move |done_ms| done_ms + span_ms),
            })
        }
        // The day is the one on the clock of this device: `done` is written in local time.
        KeepDone::EndOfDay => {
            let midnight = now.date().and_time(chrono::NaiveTime::MIN);
            let next = midnight + chrono::Duration::days(1);
            let (from_ms, until_ms) = (inner.ms_of(midnight), inner.ms_of(next));
            Some(Kept {
                window: window(
                    (from_ms, until_ms.saturating_sub(1)),
                    (midnight, next - chrono::Duration::minutes(1)),
                ),
                leaves_ms: Box::new(move |_| until_ms.saturating_sub(1)),
            })
        }
    })
}

fn task_from_row(r: &Row) -> rusqlite::Result<TaskItem> {
    let repeat: Option<String> = r.get(9)?;
    let tags: Option<String> = r.get(18)?;
    Ok(TaskItem {
        id: r.get(0)?,
        list_id: r.get(1)?,
        parent_id: r.get(2)?,
        parent_title: r.get(3)?,
        title: r.get(4)?,
        notes: r.get(5)?,
        start: r.get(6)?,
        due: r.get(7)?,
        priority: Priority::from_i64(r.get(8)?),
        repeat: repeat.and_then(|s| serde_json::from_str(&s).ok()),
        remind: r.get(10)?,
        done: r.get(11)?,
        wont: r.get(19)?,
        deleted: r.get(12)?,
        is_log: r.get(13)?,
        is_project: r.get(14)?,
        subtasks_total: r.get(15)?,
        subtasks_done: r.get(16)?,
        attachments: r.get(17)?,
        tags: tags
            .map(|t| t.split('\u{1f}').map(str::to_string).collect())
            .unwrap_or_default(),
        effective_due: r.get(20)?,
        effective_start: r.get(21)?,
        undoable: r.get(22)?,
    })
}

pub(crate) fn query_tasks(conn: &Connection, tail: &str, args: &[&str]) -> Result<Vec<TaskItem>> {
    let sql = format!("SELECT {TASK_COLUMNS} FROM tasks t {tail}");
    let mut stmt = conn.prepare_cached(&sql)?;
    let rows = stmt.query_map(params_from_iter(args.iter()), task_from_row)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

fn get_task(conn: &Connection, id: &str) -> Result<TaskItem> {
    query_tasks(conn, "WHERE t.id = ?1 AND t.purged = 0", &[id])?
        .pop()
        .ok_or_else(|| AppError::not_found(format!("task {id}")))
}

fn list_from_row(r: &Row) -> rusqlite::Result<TaskList> {
    Ok(TaskList {
        id: r.get(0)?,
        name: r.get(1)?,
        color: r.get(2)?,
        icon: r.get(3)?,
        sort: SortMode::parse(&r.get::<_, String>(4)?),
        show_done: r.get(5)?,
        default_priority: Priority::from_i64(r.get(6)?),
        default_due_today: r.get(7)?,
        archived: r.get(8)?,
        open_count: r.get(9)?,
    })
}

const LIST_COLUMNS: &str = "
    l.id, l.name, l.color, l.icon, l.sort, l.show_done, l.default_priority, l.default_due, l.archived,
    (SELECT count(*) FROM tasks t WHERE t.eff_list = l.id AND t.eff_parent IS NULL AND t.done IS NULL
        AND t.eff_deleted = 0 AND t.log_of IS NULL AND t.project = 0)";

fn get_list(conn: &Connection, id: &str) -> Result<TaskList> {
    conn.query_row(
        &format!("SELECT {LIST_COLUMNS} FROM lists l WHERE l.id = ?1 AND l.deleted = 0"),
        [id],
        list_from_row,
    )
    .optional()?
    .ok_or_else(|| AppError::not_found(format!("list {id}")))
}

fn new_id() -> String {
    Uuid::now_v7().to_string()
}

fn last_pos(conn: &Connection, sql: &str, args: &[&str]) -> Result<Option<String>> {
    Ok(conn.query_row(sql, params_from_iter(args.iter()), |r| r.get(0))?)
}

pub(crate) fn append_pos_in_list(conn: &Connection, list: &str) -> Result<String> {
    let last = last_pos(
        conn,
        "SELECT max(pos) FROM tasks WHERE eff_list = ?1 AND eff_parent IS NULL",
        &[list],
    )?;
    Ok(order::between(last.as_deref(), None))
}

/// The tasks of the top level that are projects: their children are full tasks (R30).
fn project_ids(conn: &Connection) -> Result<std::collections::HashSet<String>> {
    let mut stmt = conn.prepare_cached("SELECT id FROM tasks WHERE project = 1 AND eff_parent IS NULL")?;
    let rows = stmt.query_map([], |r| r.get(0))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

fn append_pos_in_parent(conn: &Connection, parent: &str) -> Result<String> {
    let last = last_pos(conn, "SELECT max(pos) FROM tasks WHERE eff_parent = ?1", &[parent])?;
    Ok(order::between(last.as_deref(), None))
}

fn descendants(conn: &Connection, id: &str) -> Result<Vec<String>> {
    let mut stmt = conn.prepare_cached(
        "WITH RECURSIVE sub (id) AS (
            SELECT id FROM tasks WHERE eff_parent = ?1
            UNION ALL
            SELECT t.id FROM tasks t JOIN sub ON t.eff_parent = sub.id
         ) SELECT id FROM sub",
    )?;
    let rows = stmt.query_map([id], |r| r.get(0))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

fn opt(value: Option<String>) -> Value {
    value.filter(|v| !v.is_empty()).map_or(Value::Null, Value::String)
}

fn valid_moment(value: &Option<String>) -> Result<()> {
    match value.as_deref() {
        None | Some("") => Ok(()),
        Some(v) if v.len() == 10 && recur::split(v).is_some() => Ok(()),
        Some(v) if NaiveDateTime::parse_from_str(v, MOMENT_FMT).is_ok() => Ok(()),
        Some(v) => Err(AppError::invalid(format!("bad date: {v}"))),
    }
}

fn clean_tag(tag: &str) -> String {
    tag.trim().trim_start_matches('#').to_lowercase()
}

/// The type of a file by its first bytes, as (MIME type, extension), for a name that does not tell it (R85).
/// Only types that are told apart for certain: an archive may be a document, so it stays unknown.
pub(crate) fn sniff_type(head: &[u8]) -> Option<(&'static str, &'static str)> {
    let brand = |b: &[u8]| head.len() >= 12 && &head[4..8] == b"ftyp" && &head[8..12] == b;
    if head.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some(("image/jpeg", "jpg"))
    } else if head.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(("image/png", "png"))
    } else if head.starts_with(b"GIF87a") || head.starts_with(b"GIF89a") {
        Some(("image/gif", "gif"))
    } else if head.starts_with(b"%PDF-") {
        Some(("application/pdf", "pdf"))
    } else if head.len() >= 12 && head.starts_with(b"RIFF") && &head[8..12] == b"WEBP" {
        Some(("image/webp", "webp"))
    } else if brand(b"heic") || brand(b"heix") || brand(b"mif1") || brand(b"msf1") {
        Some(("image/heic", "heic"))
    } else if brand(b"qt  ") {
        Some(("video/quicktime", "mov"))
    } else if brand(b"M4A ") {
        Some(("audio/mp4", "m4a"))
    } else if brand(b"isom") || brand(b"mp41") || brand(b"mp42") {
        Some(("video/mp4", "mp4"))
    } else {
        None
    }
}

pub(crate) fn guess_mime(name: &str) -> &'static str {
    let ext = name
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "heic" => "image/heic",
        "svg" => "image/svg+xml",
        "pdf" => "application/pdf",
        "txt" | "md" | "log" => "text/plain",
        "html" | "htm" => "text/html",
        "json" => "application/json",
        "zip" => "application/zip",
        "mp3" => "audio/mpeg",
        "m4a" => "audio/mp4",
        "mp4" => "video/mp4",
        "mov" => "video/quicktime",
        _ => UNKNOWN_MIME,
    }
}

const UNKNOWN_MIME: &str = "application/octet-stream";

impl Store {
    pub(crate) fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// R112, S42: brings the data to the rule of subtasks unless a sync run is halfway through it.
    pub(crate) fn tidy_if_whole(&self) -> Result<()> {
        if db::meta_get(&self.lock().conn, subtasks::HOLD)?.is_some() {
            return Ok(());
        }
        self.write(|w| w.tidy())
    }

    /// R112, S42: the same at the end of a sync run that read all there was to read.
    pub(crate) fn tidy_whole(&self) -> Result<()> {
        self.write(|w| w.tidy())
    }

    pub(crate) fn write<T>(&self, f: impl FnOnce(&mut Writer) -> Result<T>) -> Result<T> {
        self.lock().write(f)
    }

    pub(crate) fn blob_path(&self, sha256: &str) -> PathBuf {
        self.dir.join("blobs").join(sha256)
    }

    /// Pins "now" so tests can reason about dates.
    #[doc(hidden)]
    pub fn set_now_for_tests(&self, now: &str) {
        // Seconds are optional: most tests reason in minutes.
        self.lock().now = NaiveDateTime::parse_from_str(now, "%Y-%m-%dT%H:%M:%S")
            .or_else(|_| NaiveDateTime::parse_from_str(now, MOMENT_FMT))
            .ok();
    }

    /// Writes one register of a task the way a version with other rules would.
    #[doc(hidden)]
    pub fn set_task_field_for_tests(&self, id: &str, field: &str, value: Value) {
        self.write(|w| w.task(id, field, value)).unwrap();
    }

    /// Writes one register of the shared settings the way a version with other rules would.
    #[doc(hidden)]
    pub fn set_setting_for_tests(&self, field: &str, value: Value) {
        self.write(|w| w.set(KIND_SETTINGS, SETTINGS_ID, field, value)).unwrap();
    }

    /// One register of the shared settings as it is stored.
    #[doc(hidden)]
    pub fn setting_for_tests(&self, field: &str) -> Option<Value> {
        let stored: Option<String> = self
            .lock()
            .conn
            .query_row(
                "SELECT value FROM fields WHERE kind = ?1 AND id = ?2 AND field = ?3",
                [KIND_SETTINGS, SETTINGS_ID, field],
                |r| r.get(0),
            )
            .optional()
            .unwrap();
        stored.and_then(|v| serde_json::from_str(&v).ok())
    }

    #[doc(hidden)]
    pub fn set_compact_after_for_tests(&self, files: u32) {
        *self.compact_after.lock().unwrap() = files;
    }

    #[doc(hidden)]
    pub fn set_push_retry_for_tests(&self, millis: u64) {
        *self.push_retry.lock().unwrap() = std::time::Duration::from_millis(millis);
    }

    #[doc(hidden)]
    pub fn caldav_renders_for_tests(&self) -> u64 {
        self.caldav_renders.load(Ordering::Relaxed)
    }

    #[doc(hidden)]
    pub fn data_dir(&self) -> &Path {
        &self.dir
    }
}

pub(crate) fn new_device_id() -> String {
    let simple = Uuid::now_v7().simple().to_string();
    simple[simple.len() - DEVICE_ID_LEN..].to_string()
}

/// R140: the content of the live attachments of one task, as the task quota counts it.
fn task_attachment_bytes(conn: &Connection, task_id: &str) -> Result<u64> {
    let sum: Option<i64> = conn.query_row(
        "SELECT sum(size) FROM attachments WHERE task_id = ?1 AND deleted = 0",
        [task_id],
        |r| r.get(0),
    )?;
    Ok(sum.unwrap_or(0).max(0) as u64)
}

fn too_large_file(size: u64, limit: u64) -> AppError {
    AppError::too_large(format!(
        "the file is {size} bytes; this device takes files up to {limit} bytes"
    ))
}

fn too_large_task(used: u64, size: u64, limit: u64) -> AppError {
    AppError::too_large(format!(
        "the task holds {used} bytes and the file is {size} bytes; this device takes {limit} bytes per task"
    ))
}

/// R140: copies `source` into `tmp` in pieces, hashing each one; `tmp` is removed when anything fails.
fn copy_hashing(source: &Path, tmp: &Path, limit: u64) -> Result<(String, u64)> {
    let mut input = std::fs::File::open(source)?;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut output = options.open(tmp)?;
    let mut hasher = Sha256::new();
    let mut size = 0u64;
    let mut buffer = [0u8; 64 * 1024];
    let mut exceeded = false;
    loop {
        let read = match input.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) => {
                drop(output);
                let _ = std::fs::remove_file(tmp);
                return Err(e.into());
            }
        };
        size += read as u64;
        if over(limit, size) {
            exceeded = true;
            break;
        }
        if let Err(e) = output.write_all(&buffer[..read]) {
            drop(output);
            let _ = std::fs::remove_file(tmp);
            return Err(e.into());
        }
        hasher.update(&buffer[..read]);
    }
    if exceeded {
        drop(output);
        let _ = std::fs::remove_file(tmp);
        return Err(AppError::too_large(format!(
            "the file became larger than {limit} bytes while it was copied"
        )));
    }
    if let Err(e) = output.flush() {
        drop(output);
        let _ = std::fs::remove_file(tmp);
        return Err(e.into());
    }
    drop(output);
    Ok((hex(&hasher.finalize()), size))
}

#[uniffi::export]
impl Store {
    /// Opens (creating if needed) the database in `dir`.
    #[uniffi::constructor]
    pub fn open(dir: String) -> Result<Arc<Store>> {
        let dir = PathBuf::from(dir);
        // R142: the data folder and the content in it are for this user alone.
        make_private_dir(&dir)?;
        make_private_dir(&dir.join("blobs"))?;
        let database = dir.join("lists.sqlite");
        let conn = Connection::open(&database)?;
        private_file(&database)?;
        crate::backup::close_backups(&dir.join("backups"))?;
        // R92: a database of an earlier schema is backed up as it is, before anything in it is changed.
        let outdated = db::schema_version(&conn)?.is_some_and(|stored| stored < db::SCHEMA_VERSION);
        if !outdated {
            db::migrate(&conn)?;
        }
        let device = match db::meta_get(&conn, "device")? {
            Some(d) => d,
            None => {
                let d = new_device_id();
                db::meta_set(&conn, "device", &d)?;
                d
            }
        };
        let clock = Clock::new(&device, db::max_stamp(&conn)?.as_deref());
        let store = Arc::new_cyclic(|me| Store {
            me: me.clone(),
            inner: Mutex::new(Inner { conn, clock, now: None }),
            sync_lock: Mutex::new(()),
            blob_lock: Mutex::new(()),
            backup_lock: Mutex::new(()),
            dir,
            log_level: std::sync::atomic::AtomicU8::new(crate::log::LogLevel::Error.code()),
            log_lock: Mutex::new(()),
            compact_after: Mutex::new(256),
            sync_password: Mutex::new(None),
            sync_passphrase: Mutex::new(None),
            sealed: Mutex::new(None),
            vault_key: Mutex::new(None),
            storage_seen: Mutex::new(None),
            folder_seen: Mutex::new(None),
            push_retry: Mutex::new(std::time::Duration::from_secs(10)),
            push_token: Mutex::new(None),
            push_refused: Mutex::new((false, false)),
            caldav_renders: AtomicU64::new(0),
            history: Mutex::new(Default::default()),
            limits: Mutex::new(AttachmentLimits::default()),
        });
        // An earlier schema has the table the level is kept in as well.
        store.load_log_level()?;
        // R140: read after the migration, which may have created `meta`.
        let limits = stored_limits(&store.lock().conn)?;
        *store.limits.lock().unwrap_or_else(|p| p.into_inner()) = limits;
        if outdated {
            // Without the backup the migration does not start: the data stays as it was.
            // Asked of the registers alone: the derived tables are as the earlier version left them.
            let written: bool = store
                .lock()
                .conn
                .query_row("SELECT EXISTS (SELECT 1 FROM fields)", [], |r| r.get(0))?;
            if written {
                store.create_backup()?;
            }
            db::migrate(&store.lock().conn)?;
        }
        // R112: data written before the rule of R10, by this device or by an earlier version.
        // Not while a sync run was cut short: the run that comes to its end does it.
        store.tidy_if_whole()?;
        // S47: with no storage there is nothing to arrive, so the state on the device is whole.
        if matches!(store.sync_config()?, crate::model::SyncConfig::Off) {
            store.drop_unneeded_blobs()?;
        }
        Ok(store)
    }

    pub fn device_id(&self) -> String {
        self.lock().clock.device().to_string()
    }

    // ---- lists ----

    /// Inbox first, then the user's lists in their order. Archived lists are included.
    pub fn lists(&self) -> Result<Vec<TaskList>> {
        let inner = self.lock();
        let mut stmt = inner.conn.prepare_cached(&format!(
            "SELECT {LIST_COLUMNS} FROM lists l WHERE l.deleted = 0 ORDER BY l.id != 'inbox', l.pos, l.id"
        ))?;
        let rows = stmt.query_map([], list_from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn list(&self, id: String) -> Result<TaskList> {
        get_list(&self.lock().conn, &id)
    }

    pub fn create_list(&self, name: String) -> Result<TaskList> {
        let name = name.trim().to_string();
        if name.is_empty() {
            return Err(AppError::invalid("list name is empty"));
        }
        self.write(|w| {
            let id = new_id();
            let last = last_pos(w.tx, "SELECT max(pos) FROM lists WHERE id != 'inbox'", &[])?;
            w.list(&id, "name", json!(name))?;
            w.list(&id, "pos", json!(order::between(last.as_deref(), None)))?;
            w.flush()?;
            get_list(w.tx, &id)
        })
    }

    pub fn rename_list(&self, id: String, name: String) -> Result<()> {
        let name = name.trim().to_string();
        if name.is_empty() {
            return Err(AppError::invalid("list name is empty"));
        }
        self.list_field(id, "name", json!(name))
    }

    pub fn set_list_color(&self, id: String, color: String) -> Result<()> {
        self.list_field(id, "color", json!(color))
    }

    pub fn set_list_icon(&self, id: String, icon: String) -> Result<()> {
        self.list_field(id, "icon", json!(icon))
    }

    pub fn set_list_sort(&self, id: String, sort: SortMode) -> Result<()> {
        self.list_field(id, "sort", json!(sort.as_str()))
    }

    pub fn set_list_show_done(&self, id: String, show: bool) -> Result<()> {
        self.list_field(id, "show_done", json!(show))
    }

    /// How long a completed task stays where it was before it leaves the view (R68).
    pub fn keep_done(&self) -> Result<KeepDone> {
        keep_done(&self.lock().conn)
    }

    /// Seconds until the first of the kept tasks leaves its view; none when no task is kept.
    /// A screen that shows tasks looks again after that long (R68).
    pub fn seconds_until_kept_leaves(&self) -> Result<Option<u32>> {
        let inner = self.lock();
        let Some(Kept { window, leaves_ms }) = kept(&inner)? else {
            return Ok(None);
        };
        let first: Option<String> = inner.conn.query_row(
            &format!("SELECT min(substr(t.done_stamp, 1, 12)) FROM tasks t WHERE t.eff_deleted = 0 AND {window}"),
            [],
            |r| r.get(0),
        )?;
        let Some(done_ms) = first.and_then(|hex| u64::from_str_radix(&hex, 16).ok()) else {
            return Ok(None);
        };
        // One more second: the task is still kept at the last millisecond of its time.
        let left_ms = leaves_ms(done_ms).saturating_sub(inner.now_ms());
        Ok(Some((left_ms / 1000 + 1) as u32))
    }

    /// The setting is one for all devices and travels with sync.
    pub fn set_keep_done(&self, keep: KeepDone) -> Result<()> {
        self.write(|w| w.set(KIND_SETTINGS, SETTINGS_ID, "keep_done", keep_done_to_value(keep)))
    }

    /// Whether Inbox is hidden; one setting for all devices (R121).
    pub fn inbox_hidden(&self) -> Result<bool> {
        inbox_hidden(&self.lock().conn)
    }

    /// Hides Inbox (R121). With `move_tasks` its open top-level tasks and projects, with their
    /// subtasks, go to the end of `list_id`. The default list for new tasks becomes the one used last.
    pub fn hide_inbox(&self, list_id: String, move_tasks: bool) -> Result<InboxHiding> {
        self.write(|w| {
            let list = get_list(w.tx, &list_id)?;
            if list.id == INBOX_ID || list.archived {
                return Err(AppError::invalid(
                    "Inbox is hidden in favour of a list that is not archived",
                ));
            }
            let previous = new_task_list(w.tx)?;
            let mut moved: Vec<String> = Vec::new();
            if move_tasks {
                heal_tree(w)?;
                let mut stmt = w.tx.prepare(&format!(
                    "SELECT t.id FROM tasks t WHERE {LIVE} AND t.eff_list = 'inbox' AND t.eff_parent IS NULL
                       AND t.done IS NULL ORDER BY t.pos, t.id"
                ))?;
                moved = stmt.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
                drop(stmt);
                let mut last = last_pos(
                    w.tx,
                    "SELECT max(pos) FROM tasks WHERE eff_list = ?1 AND eff_parent IS NULL",
                    &[&list_id],
                )?;
                for id in &moved {
                    let pos = order::between(last.as_deref(), None);
                    w.task(id, "list", json!(list_id))?;
                    w.task(id, "pos", json!(pos))?;
                    last = Some(pos);
                }
            }
            w.set(KIND_SETTINGS, SETTINGS_ID, "inbox_hidden", json!(true))?;
            w.set(KIND_SETTINGS, SETTINGS_ID, "new_task_list", json!("last"))?;
            Ok(InboxHiding {
                moved,
                new_task_list: previous,
            })
        })
    }

    /// Takes hiding back: Inbox shows again, the moved tasks still at the top level go back to it,
    /// and the default list for new tasks is the one before (R121).
    pub fn undo_hide_inbox(&self, hiding: InboxHiding) -> Result<()> {
        self.write(|w| {
            let mut last = last_pos(
                w.tx,
                "SELECT max(pos) FROM tasks WHERE eff_list = 'inbox' AND eff_parent IS NULL",
                &[],
            )?;
            for id in &hiding.moved {
                let top: Option<i64> =
                    w.tx.query_row(
                        "SELECT 1 FROM tasks WHERE id = ?1 AND eff_parent IS NULL AND deleted = 0",
                        [id],
                        |r| r.get(0),
                    )
                    .optional()?;
                if top.is_none() {
                    continue;
                }
                let pos = order::between(last.as_deref(), None);
                w.task(id, "list", json!(INBOX_ID))?;
                w.task(id, "pos", json!(pos))?;
                last = Some(pos);
            }
            w.set(KIND_SETTINGS, SETTINGS_ID, "inbox_hidden", json!(false))?;
            w.set(KIND_SETTINGS, SETTINGS_ID, "new_task_list", json!(hiding.new_task_list))
        })
    }

    /// Shows Inbox again; the default list for new tasks stays as it is (R121).
    pub fn show_inbox(&self) -> Result<()> {
        self.write(|w| w.set(KIND_SETTINGS, SETTINGS_ID, "inbox_hidden", json!(false)))
    }

    /// The default list for new tasks as chosen: `inbox`, `last` or the id of a list (R40).
    pub fn new_task_list(&self) -> Result<String> {
        new_task_list(&self.lock().conn)
    }

    /// The choice is one for all devices and travels with sync (R40, S32).
    pub fn set_new_task_list(&self, choice: String) -> Result<()> {
        self.write(|w| w.set(KIND_SETTINGS, SETTINGS_ID, "new_task_list", json!(choice)))
    }

    /// The choice a version before #255 kept on the device: written only while the shared
    /// setting is empty and the choice is not Inbox, which is what an empty setting means (R40).
    pub fn adopt_new_task_list(&self, choice: String) -> Result<()> {
        if choice.is_empty() || choice == INBOX_ID {
            return Ok(());
        }
        self.write(|w| {
            if setting(w.tx, "new_task_list")?.is_none() {
                w.set(KIND_SETTINGS, SETTINGS_ID, "new_task_list", json!(choice))?;
            }
            Ok(())
        })
    }

    /// The list a new task goes to when neither the text nor the view names one (R40, R121).
    /// `last_used` is the list this device used last.
    pub fn default_list_id(&self, last_used: Option<String>) -> Result<String> {
        default_list_id(&self.lock().conn, last_used.as_deref())
    }

    pub fn set_list_defaults(&self, id: String, priority: Priority, due_today: bool) -> Result<()> {
        self.write(|w| {
            get_list(w.tx, &id)?;
            w.list(&id, "default_priority", json!(priority.as_i64()))?;
            w.list(&id, "default_due", json!(due_today))
        })
    }

    pub fn set_list_archived(&self, id: String, archived: bool) -> Result<()> {
        if id == INBOX_ID {
            return Err(AppError::invalid("the inbox cannot be archived"));
        }
        self.list_field(id, "archived", json!(archived))
    }

    /// Places the list right after `after`, or first among the user's lists.
    pub fn move_list(&self, id: String, after: Option<String>) -> Result<()> {
        if id == INBOX_ID {
            return Err(AppError::invalid("the inbox cannot be moved"));
        }
        self.write(|w| {
            get_list(w.tx, &id)?;
            let mut stmt = w.tx.prepare(
                "SELECT id, pos FROM lists WHERE deleted = 0 AND id != 'inbox' AND id != ?1 ORDER BY pos, id",
            )?;
            let siblings: Vec<(String, String)> = stmt
                .query_map([&id], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<_>>()?;
            drop(stmt);
            let pos = pos_after(&siblings, after.as_deref());
            w.list(&id, "pos", json!(pos))
        })
    }

    /// Deletes the list and moves its tasks to the trash.
    pub fn delete_list(&self, id: String) -> Result<()> {
        if id == INBOX_ID {
            return Err(AppError::invalid("the inbox cannot be deleted"));
        }
        self.write(|w| {
            get_list(w.tx, &id)?;
            let mut stmt =
                w.tx.prepare("SELECT id FROM tasks WHERE eff_list = ?1 AND eff_parent IS NULL AND deleted = 0")?;
            let tasks: Vec<String> = stmt.query_map([&id], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
            drop(stmt);
            for task in tasks {
                w.task(&task, "deleted", json!(true))?;
            }
            w.list(&id, "deleted", json!(true))
        })
    }

    // ---- queries ----

    pub fn tasks(&self, view: Scope) -> Result<Vec<TaskItem>> {
        let inner = self.lock();
        let today = inner.now().date().format(DATE_FMT).to_string();
        let conn = &inner.conn;
        let recent = Recent::at(&inner)?;
        let (open, settled, live) = (&recent.open, &recent.settled, &recent.live);
        match view {
            Scope::Inbox => list_tasks(conn, INBOX_ID, &recent),
            Scope::List { id } => list_tasks(conn, &id, &recent),
            Scope::Today => query_tasks(
                conn,
                &format!(
                    "WHERE {live} AND {NOT_ARCHIVED} AND {NOT_PROJECT} AND {open}
                       AND substr({EFF_DATE}, 1, 10) <= ?1
                       AND ({EFF_START} IS NULL OR substr({EFF_START}, 1, 10) <= ?1)
                     ORDER BY {EFF_DATE}, t.priority DESC, t.pos, t.id"
                ),
                &[&today],
            ),
            Scope::Upcoming => query_tasks(
                conn,
                &format!(
                    "WHERE {live} AND {NOT_ARCHIVED} AND {NOT_PROJECT} AND {open}
                       AND (substr({EFF_DATE}, 1, 10) > ?1
                            OR ({EFF_START} IS NOT NULL AND substr({EFF_START}, 1, 10) > ?1))
                     ORDER BY {EFF_DATE}, t.priority DESC, t.pos, t.id"
                ),
                &[&today],
            ),
            Scope::All => query_tasks(
                conn,
                &format!(
                    "WHERE {live} AND {OF_TOP_TASK} AND {NOT_ARCHIVED} AND {open} AND t.eff_parent IS NULL AND t.project = 0
                     ORDER BY t.eff_list != 'inbox', (SELECT pos FROM lists WHERE id = t.eff_list), t.eff_list, t.pos, t.id"
                ),
                &[],
            ),
            Scope::Completed => {
                query_tasks(conn, &format!("WHERE {COMPLETED} ORDER BY t.done DESC, t.id"), &[])
            }
            Scope::WontDo => {
                query_tasks(conn, &format!("WHERE {COMPLETED} AND t.wont = 1 ORDER BY t.done DESC, t.id"), &[])
            }
            Scope::Trash => query_tasks(conn, "WHERE t.deleted = 1 AND t.purged = 0 ORDER BY t.title, t.id", &[]),
            Scope::Tag { name } => query_tasks(
                conn,
                &format!(
                    "WHERE {live} AND {NOT_ARCHIVED} AND {NOT_PROJECT} AND {open}
                       AND EXISTS (SELECT 1 FROM task_tags g WHERE g.task_id = t.id AND g.tag = ?1)
                     ORDER BY t.priority DESC, {EFF_DATE} IS NULL, {EFF_DATE}, t.pos, t.id"
                ),
                &[&clean_tag(&name)],
            ),
            Scope::Project { id } => query_tasks(
                conn,
                &format!(
                    "WHERE ((t.eff_parent = ?1 AND t.deleted = 0 AND t.purged = 0) OR {}) ORDER BY {settled}, t.pos, t.id",
                    recent.project_records
                ),
                &[&id],
            ),
            Scope::Filter { id } => {
                let spec: String = conn
                    .query_row("SELECT spec FROM filters WHERE id = ?1 AND deleted = 0", [&id], |r| r.get(0))
                    .optional()?
                    .ok_or_else(|| AppError::not_found(format!("filter {id}")))?;
                filter_tasks(conn, &serde_json::from_str(&spec).unwrap_or_default(), &today, &recent)
            }
            Scope::Search { text } => {
                // SQLite's LIKE folds case for ASCII only, so matching is done here.
                let needle = text.trim().to_lowercase();
                if needle.is_empty() {
                    return Ok(vec![]);
                }
                let all = query_tasks(
                    conn,
                    &format!("WHERE {LIVE} ORDER BY t.done IS NOT NULL, {EFF_DATE} IS NULL, {EFF_DATE}, t.title"),
                    &[],
                )?;
                // R22: a subtask found by its title is shown inside its task, so the task stands for it.
                let projects = project_ids(conn)?;
                let subtask = |t: &TaskItem| t.parent_id.as_ref().is_some_and(|p| !projects.contains(p));
                let holders: std::collections::HashSet<String> = all
                    .iter()
                    .filter(|t| subtask(t) && t.title.to_lowercase().contains(&needle))
                    .filter_map(|t| t.parent_id.clone())
                    .collect();
                Ok(all
                    .into_iter()
                    .filter(|t| !subtask(t))
                    .filter(|t| {
                        t.title.to_lowercase().contains(&needle)
                            || t.notes.to_lowercase().contains(&needle)
                            || holders.contains(&t.id)
                    })
                    .take(200)
                    .collect())
            }
        }
    }

    pub fn subtasks(&self, parent_id: String) -> Result<Vec<TaskItem>> {
        query_tasks(
            &self.lock().conn,
            "WHERE t.eff_parent = ?1 AND t.deleted = 0 AND t.purged = 0 ORDER BY t.pos, t.id",
            &[&parent_id],
        )
    }

    /// R113: the subtasks typed in the card of a new task, added in one step and in the order they
    /// were typed. A title is taken as it stands; an empty one is passed over.
    pub fn add_subtasks(&self, parent_id: String, titles: Vec<String>) -> Result<Vec<TaskItem>> {
        self.act(|w| {
            let mut added = Vec::new();
            for title in titles.iter().map(|t| t.trim()).filter(|t| !t.is_empty()) {
                added.push(create_task(
                    w,
                    NewTask {
                        title: title.to_string(),
                        parent_id: Some(parent_id.clone()),
                        ..NewTask::default()
                    },
                )?);
            }
            Ok(added)
        })
    }

    /// R22: the subtasks of the task whose title holds the text of a search, for the lines shown under
    /// the task in the results. Nothing for a project: its tasks are found as rows of their own.
    pub fn found_subtasks(&self, parent_id: String, text: String) -> Result<Vec<TaskItem>> {
        let needle = text.trim().to_lowercase();
        let inner = self.lock();
        if needle.is_empty() || subtasks::is_project(&inner.conn, &parent_id)? {
            return Ok(vec![]);
        }
        let all = query_tasks(
            &inner.conn,
            "WHERE t.eff_parent = ?1 AND t.deleted = 0 AND t.purged = 0 AND t.log_of IS NULL ORDER BY t.pos, t.id",
            &[&parent_id],
        )?;
        Ok(all
            .into_iter()
            .filter(|t| t.title.to_lowercase().contains(&needle))
            .collect())
    }

    pub fn task(&self, id: String) -> Result<TaskItem> {
        get_task(&self.lock().conn, &id)
    }

    /// Open tasks that have a reminder, earliest first. The apps turn these into local notifications.
    pub fn reminders(&self) -> Result<Vec<TaskItem>> {
        query_tasks(
            &self.lock().conn,
            &format!("WHERE {LIVE} AND {NOT_ARCHIVED} AND t.done IS NULL AND t.remind IS NOT NULL ORDER BY t.remind, t.id LIMIT 64"),
            &[],
        )
    }

    /// What to notify about from now on, earliest first: reminders set on
    /// tasks, reminders derived from due dates, and daily summaries for the
    /// coming week. At most 60, which fits what the platforms let an app schedule.
    pub fn planned_notifications(&self, settings: NotifySettings) -> Result<Vec<PlannedNotification>> {
        self.notifications(settings, None)
    }

    pub fn tags(&self) -> Result<Vec<TagCount>> {
        let inner = self.lock();
        let mut stmt = inner.conn.prepare_cached(&format!(
            "SELECT g.tag, count(*) FROM task_tags g JOIN tasks t ON t.id = g.task_id
             WHERE {LIVE} AND {NOT_ARCHIVED} AND {NOT_PROJECT} AND t.done IS NULL GROUP BY g.tag ORDER BY g.tag"
        ))?;
        let rows = stmt.query_map([], |r| {
            Ok(TagCount {
                name: r.get(0)?,
                open_count: r.get(1)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn counts(&self) -> Result<Counts> {
        let inner = self.lock();
        let today = inner.now().date().format(DATE_FMT).to_string();
        let count = |sql: &str, args: &[&str]| -> Result<u32> {
            Ok(inner.conn.query_row(
                &format!("SELECT count(*) FROM tasks t WHERE {sql}"),
                params_from_iter(args.iter()),
                |r| r.get(0),
            )?)
        };
        let open = format!("{LIVE} AND {NOT_ARCHIVED} AND {NOT_PROJECT} AND t.done IS NULL");
        Ok(Counts {
            inbox: count(&format!("{LIVE} AND t.done IS NULL AND t.eff_list = 'inbox' AND t.eff_parent IS NULL AND t.project = 0"), &[])?,
            today: count(
                &format!("{open} AND substr({EFF_DATE}, 1, 10) <= ?1 AND ({EFF_START} IS NULL OR substr({EFF_START}, 1, 10) <= ?1)"),
                &[&today],
            )?,
            overdue: count(&format!("{open} AND substr({EFF_DUE}, 1, 10) < ?1"), &[&today])?,
            upcoming: count(
                &format!("{open} AND (substr({EFF_DATE}, 1, 10) > ?1 OR ({EFF_START} IS NOT NULL AND substr({EFF_START}, 1, 10) > ?1))"),
                &[&today],
            )?,
            trash: count("t.deleted = 1 AND t.purged = 0", &[])?,
            all: count(&format!("{open} AND t.eff_parent IS NULL"), &[])?,
            completed: count(COMPLETED, &[])?,
            wont_do: count(&format!("{COMPLETED} AND t.wont = 1"), &[])?,
        })
    }

    // ---- projects ----

    /// Open projects in the order of their lists.
    pub fn projects(&self) -> Result<Vec<TaskItem>> {
        query_tasks(
            &self.lock().conn,
            &format!(
                "WHERE {LIVE} AND {NOT_ARCHIVED} AND t.project = 1 AND t.done IS NULL AND t.eff_parent IS NULL
                 ORDER BY t.eff_list != 'inbox', (SELECT pos FROM lists WHERE id = t.eff_list), t.eff_list, t.pos, t.id"
            ),
            &[],
        )
    }

    /// Turns a top-level task into a project or back. Nothing else about it changes:
    /// a project is a task whose subtasks are shown as a list of their own.
    pub fn set_project(&self, id: String, project: bool) -> Result<()> {
        self.act(|w| {
            let task = get_task(w.tx, &id)?;
            if project && task.parent_id.is_some() {
                return Err(AppError::invalid("only a top-level task can be a project"));
            }
            // R32: the tasks of the project would become subtasks and lose their fields.
            // Those closed as "won't do" and those in the trash are its tasks as well.
            let tasks: bool = w.tx.query_row(
                "SELECT EXISTS (SELECT 1 FROM tasks WHERE eff_parent = ?1 AND purged = 0 AND log_of IS NULL)",
                [&id],
                |r| r.get(0),
            )?;
            if !project && task.is_project && tasks {
                return Err(AppError::invalid("a project with tasks is not turned back into a task"));
            }
            w.task(&id, "project", json!(project))
        })
    }

    /// Creates a task inside a project (or under any task) from one line of text. `first` and `after` place a
    /// task of a project as in `NewTask` (R130); a subtask goes last whatever they say.
    #[uniffi::method(default(first = false, after = None))]
    pub fn quick_add_under(
        &self,
        text: String,
        parent_id: String,
        first: bool,
        after: Option<String>,
    ) -> Result<TaskItem> {
        self.act(|w| {
            // R10: under anything but a project the line is the title of a subtask, as it is typed.
            if !subtasks::is_project(w.tx, &parent_id)? {
                return create_task(
                    w,
                    NewTask {
                        title: text,
                        parent_id: Some(parent_id),
                        ..NewTask::default()
                    },
                );
            }
            let parsed = quickadd::parse(&text, w.today());
            // A list named in the line makes no sense here: the parent decides the list.
            let title = match parsed.list_name {
                Some(name) => format!("{} @{name}", parsed.title).trim().to_string(),
                None => parsed.title,
            };
            create_task(
                w,
                NewTask {
                    title,
                    parent_id: Some(parent_id),
                    due: parsed.due,
                    priority: (parsed.priority != Priority::None).then_some(parsed.priority),
                    tags: parsed.tags,
                    first,
                    after,
                    ..NewTask::default()
                },
            )
        })
    }

    // ---- saved filters ----

    pub fn filters(&self) -> Result<Vec<SavedFilter>> {
        let inner = self.lock();
        let today = inner.now().date().format(DATE_FMT).to_string();
        let mut stmt = inner
            .conn
            .prepare_cached("SELECT id, name, spec FROM filters WHERE deleted = 0 ORDER BY pos, id")?;
        let rows: Vec<(String, String, String)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<rusqlite::Result<_>>()?;
        let recent = Recent::at(&inner)?;
        rows.into_iter()
            .map(|(id, name, spec)| {
                let spec: FilterSpec = serde_json::from_str(&spec).unwrap_or_default();
                let open_count = filter_tasks(&inner.conn, &spec, &today, &recent)?
                    .iter()
                    .filter(|t| t.done.is_none())
                    .count() as u32;
                Ok(SavedFilter {
                    id,
                    name,
                    spec,
                    open_count,
                })
            })
            .collect()
    }

    /// What a filter would show, without saving it.
    pub fn preview_filter(&self, spec: FilterSpec) -> Result<Vec<TaskItem>> {
        let inner = self.lock();
        let today = inner.now().date().format(DATE_FMT).to_string();
        filter_tasks(&inner.conn, &spec, &today, &Recent::at(&inner)?)
    }

    pub fn create_filter(&self, name: String, spec: FilterSpec) -> Result<SavedFilter> {
        let name = name.trim().to_string();
        if name.is_empty() {
            return Err(AppError::invalid("filter name is empty"));
        }
        let id = self.write(|w| {
            let id = new_id();
            let last = last_pos(w.tx, "SELECT max(pos) FROM filters", &[])?;
            w.set(KIND_FILTER, &id, "name", json!(name))?;
            w.set(KIND_FILTER, &id, "spec", serde_json::to_value(&spec)?)?;
            w.set(KIND_FILTER, &id, "pos", json!(order::between(last.as_deref(), None)))?;
            Ok(id)
        })?;
        self.filters()?
            .into_iter()
            .find(|f| f.id == id)
            .ok_or_else(|| AppError::not_found("filter"))
    }

    pub fn update_filter(&self, id: String, name: String, spec: FilterSpec) -> Result<()> {
        let name = name.trim().to_string();
        if name.is_empty() {
            return Err(AppError::invalid("filter name is empty"));
        }
        self.write(|w| {
            let (old_name, old_spec): (String, String) =
                w.tx.query_row(
                    "SELECT name, spec FROM filters WHERE id = ?1 AND deleted = 0",
                    [&id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?
                .ok_or_else(|| AppError::not_found(format!("filter {id}")))?;
            if old_name != name {
                w.set(KIND_FILTER, &id, "name", json!(name))?;
            }
            if serde_json::from_str::<FilterSpec>(&old_spec).ok().as_ref() != Some(&spec) {
                w.set(KIND_FILTER, &id, "spec", serde_json::to_value(&spec)?)?;
            }
            Ok(())
        })
    }

    pub fn delete_filter(&self, id: String) -> Result<()> {
        self.write(|w| w.set(KIND_FILTER, &id, "deleted", json!(true)))
    }

    // ---- creating ----

    pub fn create_task(&self, new: NewTask) -> Result<TaskItem> {
        self.act(|w| create_task(w, new))
    }

    /// Creates an empty project with only a title (R127). Without a list it goes to the default
    /// list for new tasks (R40). The defaults of the list are not applied: the due date of a
    /// project becomes that of its tasks (R125).
    pub fn create_project(&self, title: String, list_id: Option<String>) -> Result<TaskItem> {
        self.act(|w| {
            let title = title.trim().to_string();
            if title.is_empty() {
                return Err(AppError::invalid("title is empty"));
            }
            let list_id = match list_id {
                Some(id) => id,
                None => default_list_id(w.tx, None)?,
            };
            get_list(w.tx, &list_id)?;
            let id = new_id();
            w.task(&id, "list", json!(list_id))?;
            w.task(&id, "pos", json!(append_pos_in_list(w.tx, &list_id)?))?;
            w.task(&id, "title", json!(title))?;
            w.task(&id, "project", json!(true))?;
            w.flush()?;
            get_task(w.tx, &id)
        })
    }

    pub fn parse_quick(&self, text: String) -> QuickParse {
        let today = self.lock().now().date();
        quickadd::parse(&text, today)
    }

    /// Creates a task from one line of text. `list_id` is used unless the line names a list; `first` and
    /// `after` place it as in `NewTask` (R130).
    #[uniffi::method(default(first = false, after = None))]
    pub fn quick_add(
        &self,
        text: String,
        list_id: Option<String>,
        first: bool,
        after: Option<String>,
    ) -> Result<TaskItem> {
        self.act(|w| {
            let parsed = quickadd::parse(&text, w.today());
            let mut title = parsed.title;
            let mut list = list_id;
            if let Some(name) = parsed.list_name {
                match find_list_by_name(w.tx, &name)? {
                    Some(id) => list = Some(id),
                    // Not a list after all: keep the word in the title.
                    None => title = format!("{title} @{name}").trim().to_string(),
                }
            }
            create_task(
                w,
                NewTask {
                    title,
                    list_id: list,
                    due: parsed.due,
                    priority: (parsed.priority != Priority::None).then_some(parsed.priority),
                    tags: parsed.tags,
                    first,
                    after,
                    ..NewTask::default()
                },
            )
        })
    }

    /// Copies a task with its subtasks and attachments, placing the copy right after it.
    pub fn duplicate_task(&self, id: String) -> Result<TaskItem> {
        self.act(|w| {
            let original = get_task(w.tx, &id)?;
            let copy = duplicate(w, &original, original.parent_id.clone())?;
            w.flush()?;
            place_after(w, &copy, &original)?;
            w.flush()?;
            get_task(w.tx, &copy)
        })
    }

    // ---- editing ----

    pub fn set_title(&self, id: String, title: String) -> Result<()> {
        let title = title.trim().to_string();
        if title.is_empty() {
            return Err(AppError::invalid("title is empty"));
        }
        self.task_field(id, "title", json!(title))
    }

    pub fn set_notes(&self, id: String, notes: String) -> Result<()> {
        self.task_field(id, "notes", json!(notes))
    }

    pub fn set_start(&self, id: String, start: Option<String>) -> Result<()> {
        valid_moment(&start)?;
        self.task_field(id, "start", opt(start))
    }

    pub fn set_due(&self, id: String, due: Option<String>) -> Result<()> {
        valid_moment(&due)?;
        self.task_field(id, "due", opt(due))
    }

    pub fn set_priority(&self, id: String, priority: Priority) -> Result<()> {
        self.task_field(id, "priority", json!(priority.as_i64()))
    }

    pub fn set_remind(&self, id: String, remind: Option<String>) -> Result<()> {
        valid_moment(&remind)?;
        self.task_field(id, "remind", opt(remind))
    }

    pub fn set_repeat(&self, id: String, repeat: Option<Repeat>) -> Result<()> {
        self.act(|w| {
            let task = get_task(w.tx, &id)?;
            if repeat.is_some() && subtasks::is_subtask(w.tx, &id)? {
                return Err(AppError::invalid("a subtask has a title only"));
            }
            let value = match repeat {
                Some(rule) => {
                    serde_json::to_value(recur::normalized(rule, task.due.as_deref().or(task.start.as_deref())))?
                }
                None => Value::Null,
            };
            w.task(&id, "repeat", value)
        })
    }

    pub fn add_tag(&self, id: String, tag: String) -> Result<()> {
        let name = clean_tag(&tag);
        self.task_field(id, &crate::tag::require(&name)?, json!(true))
    }

    pub fn remove_tag(&self, id: String, tag: String) -> Result<()> {
        let name = clean_tag(&tag);
        self.task_field(id, &crate::tag::require_field(&name)?, json!(false))
    }

    // ---- state ----

    /// Completes the task and its open subtasks. A repeating task moves to
    /// its next occurrence instead and leaves a record in Completed.
    pub fn complete_task(&self, id: String) -> Result<TaskItem> {
        self.act(|w| complete_in(w, &id, false))
    }

    /// Closes the task and its open subtasks as "won't do" (R69). A repeating
    /// task skips the occurrence: it moves on and leaves a "won't do" record.
    pub fn wont_do_task(&self, id: String) -> Result<TaskItem> {
        self.act(|w| complete_in(w, &id, true))
    }

    /// The mark of a record of a completed repeat takes the completion back (R128).
    pub fn reopen_task(&self, id: String) -> Result<()> {
        self.act(|w| {
            if get_task(w.tx, &id)?.is_log {
                return undo_repeat_in(w, &id);
            }
            reopen_in(w, &id)
        })
    }

    /// Takes a completion or a "won't do" back: reopens the task and the subtasks that were
    /// closed with it. Those carry its closing moment, written in the same millisecond: the
    /// moment alone is exact to the minute and would take a subtask closed by hand just before.
    /// Subtasks closed earlier stay closed. A repeating task that moved on gets back what its last
    /// completion changed (R128); so does the task of a record given here.
    pub fn undo_close_task(&self, id: String) -> Result<()> {
        self.act(|w| {
            let task = get_task(w.tx, &id)?;
            if task.is_log {
                return undo_repeat_in(w, &id);
            }
            if task.done.is_none() {
                return match last_record(w.tx, &id)? {
                    Some(log_id) => undo_repeat_in(w, &log_id),
                    None => Ok(()),
                };
            }
            let together: Vec<String> = {
                let mut stmt = w.tx.prepare_cached(
                    "SELECT s.id FROM tasks s, tasks t
                     WHERE t.id = ?1 AND s.done IS t.done AND substr(s.done_stamp, 1, 12) = substr(t.done_stamp, 1, 12)",
                )?;
                let rows = stmt.query_map([&id], |r| r.get(0))?;
                rows.collect::<rusqlite::Result<_>>()?
            };
            for sub in descendants(w.tx, &id)? {
                if together.contains(&sub) {
                    reopen_in(w, &sub)?;
                }
            }
            reopen_in(w, &id)
        })
    }

    pub fn delete_task(&self, id: String) -> Result<()> {
        self.task_field(id, "deleted", json!(true))
    }

    pub fn restore_task(&self, id: String) -> Result<()> {
        self.task_field(id, "deleted", json!(false))
    }

    /// Removes everything in the trash for good. Returns how many tasks were removed.
    pub fn empty_trash(&self) -> Result<u32> {
        self.write(|w| {
            let mut stmt = w.tx.prepare("SELECT id FROM tasks WHERE deleted = 1 AND purged = 0")?;
            let ids: Vec<String> = stmt.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
            drop(stmt);
            for id in &ids {
                w.task(id, "purged", json!(true))?;
            }
            Ok(ids.len() as u32)
        })
    }

    /// Removes what the Completed view lists for good, bypassing the trash:
    /// everything, or only what was completed before the day `before`
    /// (`YYYY-MM-DD`). Subtasks go with their task. Returns how many entries
    /// of the view were removed.
    pub fn clear_completed(&self, before: Option<String>) -> Result<u32> {
        if let Some(day) = &before {
            if day.len() != 10 || recur::split(day).is_none() {
                return Err(AppError::invalid(format!("bad date: {day}")));
            }
        }
        self.write(|w| {
            let mut stmt = w.tx.prepare(&format!(
                "SELECT t.id FROM tasks t WHERE {COMPLETED} AND (?1 IS NULL OR substr(t.done, 1, 10) < ?1)"
            ))?;
            let ids: Vec<String> = stmt
                .query_map([&before], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            drop(stmt);
            for id in &ids {
                w.task(id, "purged", json!(true))?;
            }
            Ok(ids.len() as u32)
        })
    }

    // ---- moving ----

    /// Moves a top-level task (with its subtasks) to the end of another list.
    pub fn move_to_list(&self, id: String, list_id: String) -> Result<()> {
        self.act(|w| {
            get_task(w.tx, &id)?;
            get_list(w.tx, &list_id)?;
            if subtasks::is_subtask(w.tx, &id)? {
                return Err(AppError::invalid("a subtask stays with its task"));
            }
            heal_tree(w)?;
            let pos = append_pos_in_list(w.tx, &list_id)?;
            w.task(&id, "parent", Value::Null)?;
            w.task(&id, "list", json!(list_id))?;
            w.task(&id, "pos", json!(pos))
        })
    }

    /// Makes a task a task of the project, at the project's end (R129). A task that is already
    /// there stays where it is. R32: a project, a task with subtasks and a subtask are refused.
    pub fn move_to_project(&self, id: String, project_id: String) -> Result<()> {
        self.act(|w| {
            heal_tree(w)?;
            let task = get_task(w.tx, &id)?;
            let project = get_task(w.tx, &project_id)?;
            if !subtasks::is_project(w.tx, &project_id)? {
                return Err(AppError::invalid("a task goes only into a project"));
            }
            if task.parent_id.as_deref() == Some(project_id.as_str()) {
                return Ok(());
            }
            if subtasks::is_subtask(w.tx, &id)? {
                return Err(AppError::invalid("a subtask stays with its task"));
            }
            if task.is_project || !descendants(w.tx, &id)?.is_empty() {
                return Err(AppError::invalid("a task with tasks under it stays at the top level"));
            }
            let pos = append_pos_in_parent(w.tx, &project_id)?;
            w.task(&id, "parent", json!(project_id))?;
            w.task(&id, "list", json!(project.list_id))?;
            w.task(&id, "pos", json!(pos))
        })
    }

    /// Reorders, and moves a task into a project or out of one. With a parent the
    /// task is placed among its children; without one it becomes a top-level
    /// task of `list_id` (its current list when omitted). It is placed right
    /// after `after`, or first. R10: a subtask keeps its parent, and a task
    /// goes under another one only when that one is a project.
    pub fn move_task(
        &self,
        id: String,
        list_id: Option<String>,
        parent_id: Option<String>,
        after: Option<String>,
    ) -> Result<()> {
        self.act(|w| {
            heal_tree(w)?;
            let task = get_task(w.tx, &id)?;
            if task.parent_id != parent_id {
                if subtasks::is_subtask(w.tx, &id)? {
                    return Err(AppError::invalid("a subtask stays with its task"));
                }
                if let Some(parent) = &parent_id {
                    get_task(w.tx, parent)?;
                    if !subtasks::is_project(w.tx, parent)? {
                        return Err(AppError::invalid("a task does not become a subtask"));
                    }
                    // A project is a task of the top level, and the lines of a task do not go a level down.
                    if task.is_project || !descendants(w.tx, &id)?.is_empty() {
                        return Err(AppError::invalid("a task with tasks under it stays at the top level"));
                    }
                }
            }
            let siblings: Vec<(String, String)> = match &parent_id {
                Some(parent) => {
                    if *parent == id || descendants(w.tx, &id)?.contains(parent) {
                        return Err(AppError::invalid("a task cannot be moved into itself"));
                    }
                    get_task(w.tx, parent)?;
                    w.task(&id, "parent", json!(parent))?;
                    sibling_positions(w.tx, "eff_parent = ?1", parent, &id)?
                }
                None => {
                    let list = list_id.unwrap_or(task.list_id);
                    get_list(w.tx, &list)?;
                    w.task(&id, "parent", Value::Null)?;
                    w.task(&id, "list", json!(list))?;
                    sibling_positions(w.tx, "eff_list = ?1 AND eff_parent IS NULL", &list, &id)?
                }
            };
            w.task(&id, "pos", json!(pos_after(&siblings, after.as_deref())))
        })
    }

    // ---- attachments ----

    pub fn attachments(&self, task_id: String) -> Result<Vec<Attachment>> {
        let inner = self.lock();
        let mut stmt = inner.conn.prepare_cached(
            "SELECT id, task_id, name, mime, size, sha256 FROM attachments WHERE task_id = ?1 AND deleted = 0 ORDER BY id",
        )?;
        let rows = stmt.query_map([&task_id], |r| {
            let sha256: String = r.get(5)?;
            let path = self.blob_path(&sha256);
            let present = db::is_sha256(&sha256) && path.exists();
            let (mut name, mut mime): (String, String) = (r.get(2)?, r.get(3)?);
            // R85: a name that tells no type is helped by the content, when the content is here.
            // Nothing is written back: the registers keep what the file came with.
            if present && guess_mime(&name) == UNKNOWN_MIME {
                let mut head = [0u8; 16];
                let read = std::fs::File::open(&path)
                    .and_then(|mut f| std::io::Read::read(&mut f, &mut head))
                    .unwrap_or(0);
                if let Some((sniffed, ext)) = sniff_type(&head[..read]) {
                    mime = sniffed.to_string();
                    if !name.contains('.') {
                        name = format!("{name}.{ext}");
                    }
                }
            }
            Ok(Attachment {
                id: r.get(0)?,
                task_id: r.get(1)?,
                name,
                mime,
                size: r.get(4)?,
                local_path: present.then(|| path.to_string_lossy().into_owned()),
                sha256,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// R140: copies the file at `path` into the store and attaches it to the task.
    /// The content is read and written in pieces, so memory does not grow with the file.
    pub fn add_attachment(&self, task_id: String, path: String, name: Option<String>) -> Result<Attachment> {
        let added = (|| {
            let source = Path::new(&path);
            let limits = self.attachment_limits();
            let meta = std::fs::metadata(source)?;
            if !meta.is_file() {
                return Err(AppError::invalid("not a file"));
            }
            let expected = meta.len();
            if over(limits.max_file, expected) {
                return Err(too_large_file(expected, limits.max_file));
            }
            // Asked before the file is copied: an unknown task, a subtask and a full task cost nothing.
            {
                let conn = self.lock();
                get_task(&conn.conn, &task_id)?;
                if subtasks::is_subtask(&conn.conn, &task_id)? {
                    return Err(AppError::invalid("a subtask has a title only"));
                }
                let used = task_attachment_bytes(&conn.conn, &task_id)?;
                if over(limits.max_task, used.saturating_add(expected)) {
                    return Err(too_large_task(used, expected, limits.max_task));
                }
            }
            let tmp = self
                .dir
                .join("blobs")
                .join(format!(".upload-{}.part", Uuid::now_v7().simple()));
            let (sha256, size) = copy_hashing(source, &tmp, limits.max_file)?;
            let target = self.blob_path(&sha256);
            let created = !target.exists();
            if created {
                std::fs::rename(&tmp, &target)?;
                private_file(&target)?;
            } else {
                let _ = std::fs::remove_file(&tmp);
            }
            let name = name
                .filter(|n| !n.trim().is_empty())
                .or_else(|| source.file_name().map(|n| n.to_string_lossy().into_owned()))
                .unwrap_or_else(|| "file".into());
            let written = self.act(|w| {
                get_task(w.tx, &task_id)?;
                if subtasks::is_subtask(w.tx, &task_id)? {
                    return Err(AppError::invalid("a subtask has a title only"));
                }
                // Asked again with the size the file really had: it may have grown while it was copied.
                let used = task_attachment_bytes(w.tx, &task_id)?;
                if over(limits.max_task, used.saturating_add(size)) {
                    return Err(too_large_task(used, size, limits.max_task));
                }
                let id = new_id();
                w.set(KIND_ATTACHMENT, &id, "task", json!(task_id))?;
                w.set(KIND_ATTACHMENT, &id, "name", json!(name))?;
                w.set(KIND_ATTACHMENT, &id, "mime", json!(guess_mime(&name)))?;
                w.set(KIND_ATTACHMENT, &id, "size", json!(size))?;
                w.set(KIND_ATTACHMENT, &id, "sha256", json!(sha256))?;
                Ok(id)
            });
            let id = match written {
                Ok(id) => id,
                Err(e) => {
                    // The content came with this call and the task did not take it: it is left to no one.
                    if created {
                        self.drop_blob_if_unused(&sha256);
                    }
                    return Err(e);
                }
            };
            self.attachments(task_id)?
                .into_iter()
                .find(|a| a.id == id)
                .ok_or_else(|| AppError::not_found("attachment"))
        })();
        // R102: the path and the name stay out of the log; the reason goes in.
        if let Err(e) = &added {
            self.note(LogLevel::Error, "attachments", || format!("not added: {e}"));
        }
        added
    }

    /// R140: the limits this device takes.
    pub fn attachment_limits(&self) -> AttachmentLimits {
        *self.limits.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// R140: sets the limits of this device. Zero means no limit. Kept in the database of this
    /// device, not in the shared registers, so it does not travel to other devices.
    pub fn set_attachment_limits(&self, limits: AttachmentLimits) -> Result<()> {
        {
            let conn = self.lock();
            db::meta_set(&conn.conn, MAX_FILE_KEY, &limits.max_file.to_string())?;
            db::meta_set(&conn.conn, MAX_TASK_KEY, &limits.max_task.to_string())?;
        }
        *self.limits.lock().unwrap_or_else(|p| p.into_inner()) = limits;
        Ok(())
    }

    /// R140: a blob this call wrote is removed when no attachment kept it, as after a failed add.
    fn drop_blob_if_unused(&self, sha256: &str) {
        let used: bool = self
            .lock()
            .conn
            .query_row(
                "SELECT EXISTS (SELECT 1 FROM attachments WHERE sha256 = ?1)",
                [sha256],
                |r| r.get(0),
            )
            .unwrap_or(true);
        if !used {
            let _ = std::fs::remove_file(self.blob_path(sha256));
        }
    }

    pub fn remove_attachment(&self, id: String) -> Result<()> {
        self.act(|w| w.set(KIND_ATTACHMENT, &id, "deleted", json!(true)))
    }

    /// R91: writes every attachment of the task into the folder under its own name.
    /// Nothing in the folder is replaced: a name that is taken gets a number. Content that has
    /// not arrived is downloaded first (R76); what cannot be had or written is named in the
    /// result and does not stop the rest.
    pub fn save_attachments(&self, task_id: String, dir: String) -> Result<AttachmentsSaved> {
        let dir = Path::new(&dir);
        let mut report = AttachmentsSaved::default();
        for mut file in self.attachments(task_id)? {
            if file.local_path.is_none() {
                if let Ok(got) = self.fetch_attachment(file.id.clone()) {
                    report.fetched += u32::from(got.local_path.is_some());
                    file = got;
                }
            }
            let written = file
                .local_path
                .as_ref()
                .and_then(|path| copy_under_free_name(Path::new(path), dir, &file.name).ok());
            match written {
                Some(name) => report.saved.push(name),
                None => report.failed.push(file.name),
            }
        }
        Ok(report)
    }
}

/// The name of an attachment as a single path component: it comes from another device as it was typed there.
fn file_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if matches!(c, '/' | '\\' | ':') || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();
    match cleaned.trim() {
        "" | "." | ".." => "file".into(),
        _ => cleaned,
    }
}

/// Copies the file into the folder under the name, or under the first free one with a number
/// before the extension ("a 2.txt"). Returns the name it was written under.
fn copy_under_free_name(source: &Path, dir: &Path, name: &str) -> std::io::Result<String> {
    let name = file_name(name);
    let (stem, extension) = match name.rfind('.') {
        Some(dot) if dot > 0 => name.split_at(dot),
        _ => (name.as_str(), ""),
    };
    let mut input = std::fs::File::open(source)?;
    for number in 1..=9999 {
        let candidate = if number == 1 {
            name.clone()
        } else {
            format!("{stem} {number}{extension}")
        };
        let target = dir.join(&candidate);
        // Created only if nothing has the name: a file that appears meanwhile is not replaced either.
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&target) {
            Ok(mut output) => {
                if let Err(e) = std::io::copy(&mut input, &mut output) {
                    drop(output);
                    let _ = std::fs::remove_file(&target);
                    return Err(e);
                }
                return Ok(candidate);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists, "no free name"))
}

impl Store {
    fn task_field(&self, id: String, field: &str, value: Value) -> Result<()> {
        let text = matches!(field, "title" | "notes").then(|| (id.clone(), field.to_string()));
        self.act_on(text, |w| {
            get_task(w.tx, &id)?;
            // R10: a subtask is renamed, closed, reordered and deleted, and nothing else.
            if !matches!(field, "title" | "deleted") && subtasks::is_subtask(w.tx, &id)? {
                return Err(AppError::invalid("a subtask has a title only"));
            }
            w.task(&id, field, value)
        })
    }

    fn list_field(&self, id: String, field: &str, value: Value) -> Result<()> {
        self.write(|w| {
            get_list(w.tx, &id)?;
            w.list(&id, field, value)
        })
    }
}

/// Whether the `wont` register of a task holds a moment, whatever `done` says.
fn has_wont(conn: &Connection, id: &str) -> Result<bool> {
    let value: Option<String> = conn
        .query_row(
            "SELECT value FROM fields WHERE kind = ?1 AND id = ?2 AND field = 'wont'",
            params![KIND_TASK, id],
            |r| r.get(0),
        )
        .optional()?;
    Ok(value.is_some_and(|v| v != "null"))
}

/// Writes the closing moment of one task. A leftover `wont` is cleared on
/// completion, so that it cannot match the new moment (S33).
fn close(w: &mut Writer, id: &str, moment: &str, wont: bool) -> Result<()> {
    w.task(id, "done", json!(moment))?;
    if wont {
        w.task(id, "wont", json!(moment))?;
    } else if has_wont(w.tx, id)? {
        w.task(id, "wont", Value::Null)?;
    }
    Ok(())
}

/// Returns a closed task to work, whichever way it was closed.
pub(crate) fn reopen_in(w: &mut Writer, id: &str) -> Result<()> {
    w.task(id, "done", Value::Null)?;
    if has_wont(w.tx, id)? {
        w.task(id, "wont", Value::Null)?;
    }
    Ok(())
}

/// The newest record of a completed repeat of the task, if it is still completed and keeps what undo needs.
fn last_record(conn: &Connection, task_id: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT id FROM tasks t WHERE log_of = ?1 AND done IS NOT NULL AND purged = 0
               AND EXISTS (SELECT 1 FROM fields f WHERE f.kind = 'task' AND f.id = t.id AND f.field = 'undo' AND f.value LIKE '{%')
             ORDER BY done_stamp DESC, id LIMIT 1",
            [task_id],
            |r| r.get(0),
        )
        .optional()?)
}

/// Takes back the completion of a repeat (R128, S44): the record is no longer completed, and the task
/// and its subtasks get back what the completion changed. Nothing changes once the task has left
/// the date that completion moved it to. Err for a record that keeps nothing to give back.
fn undo_repeat_in(w: &mut Writer, log_id: &str) -> Result<()> {
    let record = get_task(w.tx, log_id)?;
    if record.done.is_none() {
        return Ok(());
    }
    let undo: Option<Value> =
        w.tx.query_row(
            "SELECT value FROM fields WHERE kind = ?1 AND id = ?2 AND field = 'undo'",
            params![KIND_TASK, log_id],
            |r| r.get::<_, String>(0),
        )
        .optional()?
        .and_then(|v| serde_json::from_str::<Value>(&v).ok())
        .filter(Value::is_object);
    let (Some(undo), Some(task_id)) = (undo, log_of(w.tx, log_id)?) else {
        return Err(AppError::invalid("a completed occurrence cannot be reopened"));
    };
    let text = |field: &str| undo.get(field).and_then(Value::as_str).map(str::to_string);
    let Ok(task) = get_task(w.tx, &task_id) else {
        return Ok(());
    };
    if task.done.is_some() || task.due != text("next_due") || task.start != text("next_start") {
        return Ok(());
    }
    reopen_in(w, log_id)?;
    w.task(&task_id, "due", opt(text("due")))?;
    w.task(&task_id, "start", opt(text("start")))?;
    w.task(&task_id, "remind", opt(text("remind")))?;
    if let Some(rule) = undo.get("repeat").filter(|v| v.is_object()) {
        if task.repeat.as_ref().map(serde_json::to_value).transpose()?.as_ref() != Some(rule) {
            w.task(&task_id, "repeat", rule.clone())?;
        }
    }
    if let Some(closed) = undo.get("subtasks").and_then(Value::as_object) {
        for (sub, state) in closed {
            let Some(done) = state.get("done").and_then(Value::as_str) else {
                continue;
            };
            let wont = state.get("wont").and_then(Value::as_bool).unwrap_or(false);
            close(w, sub, done, wont)?;
        }
    }
    w.flush()
}

fn log_of(conn: &Connection, id: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row("SELECT log_of FROM tasks WHERE id = ?1", [id], |r| {
            r.get::<_, Option<String>>(0)
        })
        .optional()?
        .flatten())
}

/// Closes a task inside a write transaction, as completed or as "won't do":
/// see `Store::complete_task` and `Store::wont_do_task`.
pub(crate) fn complete_in(w: &mut Writer, id: &str, wont: bool) -> Result<TaskItem> {
    let task = get_task(w.tx, id)?;
    if task.done.is_some() {
        return Ok(task);
    }
    let subtree = descendants(w.tx, id)?;
    let current = task
        .due
        .as_deref()
        .or(task.start.as_deref())
        .and_then(recur::split)
        .map(|(d, _)| d);
    let next = task
        .repeat
        .as_ref()
        .and_then(|rule| recur::next_occurrence(rule, current, w.today()));
    match (next, &task.repeat) {
        (Some(next), Some(rule)) => {
            let from = current.unwrap_or(w.today());
            // Same occurrence, same record: two devices completing it offline agree on the id.
            let log_id = Uuid::new_v5(&Uuid::NAMESPACE_OID, format!("{id}|{from}").as_bytes()).to_string();
            w.task(&log_id, "title", json!(task.title))?;
            w.task(&log_id, "list", json!(task.list_id))?;
            w.task(&log_id, "priority", json!(task.priority.as_i64()))?;
            w.task(&log_id, "due", opt(task.due.clone()))?;
            // The record stands where the task stood while the setting keeps it (R17, R68).
            w.task(&log_id, "start", opt(task.start.clone()))?;
            let pos: String =
                w.tx.query_row("SELECT pos FROM tasks WHERE id = ?1", [id], |r| r.get(0))?;
            w.task(&log_id, "pos", json!(pos))?;
            let had: Vec<String> = {
                let mut stmt = w.tx.prepare_cached("SELECT tag FROM task_tags WHERE task_id = ?1")?;
                let rows = stmt.query_map([&log_id], |r| r.get(0))?;
                rows.collect::<rusqlite::Result<_>>()?
            };
            // Tags the record kept from an earlier completion of the same occurrence go (S7).
            for tag in had.iter().filter(|t| !task.tags.contains(t)) {
                w.task(&log_id, &format!("tag:{tag}"), json!(false))?;
            }
            for tag in &task.tags {
                w.task(&log_id, &format!("tag:{tag}"), json!(true))?;
            }
            let moment = w.moment();
            close(w, &log_id, &moment, wont)?;
            w.task(&log_id, "log_of", json!(id))?;
            // What undo gives back (R128, S44): the subtasks closed before, with their moment.
            let mut closed = serde_json::Map::new();
            for sub in &subtree {
                let s = get_task(w.tx, sub)?;
                if let Some(done) = s.done {
                    closed.insert(sub.clone(), json!({ "done": done, "wont": s.wont }));
                }
            }

            let delta = (next - from).num_days();
            match (&task.due, &task.start) {
                (None, None) => w.task(id, "due", json!(next.format(DATE_FMT).to_string()))?,
                (due, start) => {
                    if let Some(due) = due {
                        w.task(id, "due", opt(recur::shift(due, delta)))?;
                    }
                    if let Some(start) = start {
                        w.task(id, "start", opt(recur::shift(start, delta)))?;
                    }
                }
            }
            if let Some(remind) = &task.remind {
                w.task(id, "remind", opt(recur::shift(remind, delta)))?;
            }
            if let Some(left) = rule.count {
                let mut rule = rule.clone();
                rule.count = Some(left.saturating_sub(1));
                w.task(id, "repeat", serde_json::to_value(rule)?)?;
            }
            for sub in subtree {
                reopen_in(w, &sub)?;
            }
            w.flush()?;
            let moved = get_task(w.tx, id)?;
            let undo = json!({
                "due": task.due,
                "start": task.start,
                "remind": task.remind,
                "repeat": task.repeat.as_ref().map(serde_json::to_value).transpose()?,
                "next_due": moved.due,
                "next_start": moved.start,
                "subtasks": closed,
            });
            w.task(&log_id, "undo", undo)?;
        }
        _ => {
            let moment = w.moment();
            close(w, id, &moment, wont)?;
            for sub in subtree {
                let open: bool =
                    w.tx.query_row("SELECT done IS NULL FROM tasks WHERE id = ?1", [&sub], |r| r.get(0))?;
                if open {
                    close(w, &sub, &moment, wont)?;
                }
            }
        }
    }
    w.flush()?;
    get_task(w.tx, id)
}

fn list_tasks(conn: &Connection, list_id: &str, recent: &Recent) -> Result<Vec<TaskItem>> {
    let list = get_list(conn, list_id)?;
    let order = match list.sort {
        SortMode::Manual => "t.pos, t.id",
        SortMode::Due => "coalesce(t.due, t.start) IS NULL, coalesce(t.due, t.start), t.pos, t.id",
        SortMode::Priority => "t.priority DESC, t.pos, t.id",
        SortMode::Title => "t.title COLLATE NOCASE, t.pos, t.id",
    };
    let (open, settled) = (&recent.open, &recent.settled);
    let done = if list.show_done {
        String::new()
    } else {
        format!("AND {open}")
    };
    query_tasks(
        conn,
        &format!(
            "WHERE {} AND {OF_TOP_TASK} AND t.eff_list = ?1 AND t.eff_parent IS NULL AND t.project = 0 {done} ORDER BY {settled}, {order}",
            recent.live
        ),
        &[list_id],
    )
}

/// Tasks a filter selects, subtasks included, dated ones first.
fn filter_tasks(conn: &Connection, spec: &FilterSpec, today: &str, recent: &Recent) -> Result<Vec<TaskItem>> {
    let mut sql = format!("WHERE {} AND {NOT_ARCHIVED} AND {NOT_PROJECT}", recent.live);
    let mut args: Vec<String> = Vec::new();
    let arg = |value: String, args: &mut Vec<String>| {
        args.push(value);
        format!("?{}", args.len())
    };
    match spec.status {
        FilterStatus::Open => sql.push_str(&format!(" AND {}", recent.open)),
        FilterStatus::Done => sql.push_str(" AND t.done IS NOT NULL AND t.wont = 0"),
        FilterStatus::Wont => sql.push_str(" AND t.wont = 1"),
        FilterStatus::All => {}
    }
    let date = format!("substr({EFF_DATE}, 1, 10)");
    match spec.due {
        DueWindow::Any => {}
        DueWindow::NoDate => sql.push_str(&format!(" AND {EFF_DATE} IS NULL")),
        DueWindow::Overdue => sql.push_str(&format!(
            " AND substr({EFF_DUE}, 1, 10) < {}",
            arg(today.to_string(), &mut args)
        )),
        DueWindow::Today => sql.push_str(&format!(" AND {date} = {}", arg(today.to_string(), &mut args))),
        DueWindow::Next { days } => {
            let last = recur::shift(today, i64::from(days.clamp(1, 3660)) - 1).unwrap_or_else(|| today.to_string());
            sql.push_str(&format!(" AND {date} <= {}", arg(last, &mut args)));
        }
    }
    if !spec.list_ids.is_empty() {
        let marks: Vec<String> = spec.list_ids.iter().map(|id| arg(id.clone(), &mut args)).collect();
        sql.push_str(&format!(" AND t.eff_list IN ({})", marks.join(", ")));
    }
    for tag in spec.tags.iter().map(|t| clean_tag(t)).filter(|t| !t.is_empty()) {
        sql.push_str(&format!(
            " AND EXISTS (SELECT 1 FROM task_tags g WHERE g.task_id = t.id AND g.tag = {})",
            arg(tag, &mut args)
        ));
    }
    if spec.min_priority != Priority::None {
        sql.push_str(&format!(" AND t.priority >= {}", spec.min_priority.as_i64()));
    }
    sql.push_str(&format!(
        " ORDER BY {}, {date} IS NULL, {EFF_DATE}, t.priority DESC, t.pos, t.id",
        recent.settled
    ));
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let mut tasks = query_tasks(conn, &sql, &refs)?;
    // Matching text is done here: SQLite folds case for ASCII only.
    let words: Vec<String> = spec
        .text
        .to_lowercase()
        .split_whitespace()
        .map(str::to_string)
        .collect();
    if !words.is_empty() {
        tasks.retain(|t| {
            let hay = format!("{}\n{}", t.title, t.notes).to_lowercase();
            words.iter().all(|w| hay.contains(w))
        });
    }
    Ok(tasks)
}

fn find_list_by_name(conn: &Connection, name: &str) -> Result<Option<String>> {
    let wanted = name.to_lowercase();
    let mut stmt = conn.prepare_cached("SELECT id, name FROM lists WHERE deleted = 0 ORDER BY archived, pos, id")?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
    for row in rows {
        let (id, list_name) = row?;
        if list_name.to_lowercase() == wanted || (id == INBOX_ID && matches!(wanted.as_str(), "inbox" | "входящие"))
        {
            return Ok(Some(id));
        }
    }
    Ok(None)
}

fn create_task(w: &mut Writer, new: NewTask) -> Result<TaskItem> {
    let title = new.title.trim().to_string();
    if title.is_empty() {
        return Err(AppError::invalid("title is empty"));
    }
    valid_moment(&new.start)?;
    valid_moment(&new.due)?;
    let id = new_id();
    let place = (new.first, new.after.clone());
    let mut priority = new.priority;
    let mut due = new.due.filter(|d| !d.is_empty());
    match &new.parent_id {
        Some(parent) => {
            let parent_task = get_task(w.tx, parent)?;
            // R10: under anything but a project the new task is a subtask.
            if !subtasks::is_project(w.tx, parent)? {
                if subtasks::is_subtask(w.tx, parent)? {
                    return Err(AppError::invalid("a subtask has no subtasks"));
                }
                let fields = !new.notes.is_empty()
                    || new.start.as_deref().is_some_and(|s| !s.is_empty())
                    || due.is_some()
                    || priority.is_some_and(|p| p != Priority::None)
                    || !new.tags.is_empty();
                if fields {
                    return Err(AppError::invalid("a subtask has a title only"));
                }
            }
            w.task(&id, "parent", json!(parent))?;
            w.task(&id, "list", json!(parent_task.list_id))?;
            // R130: a task of a project takes the place it asks for, a subtask goes last.
            let asked = if subtasks::is_project(w.tx, parent)? {
                placed(w, &place, Some(parent), &parent_task.list_id, &id)?
            } else {
                None
            };
            let pos = match asked {
                Some(pos) => pos,
                None => append_pos_in_parent(w.tx, parent)?,
            };
            w.task(&id, "pos", json!(pos))?;
        }
        None => {
            let list_id = new.list_id.clone().unwrap_or_else(|| INBOX_ID.to_string());
            let list = get_list(w.tx, &list_id)?;
            if priority.is_none() && list.default_priority != Priority::None {
                priority = Some(list.default_priority);
            }
            if due.is_none() && new.start.is_none() && list.default_due_today {
                due = Some(w.today().format(DATE_FMT).to_string());
            }
            w.task(&id, "list", json!(list_id))?;
            let pos = match placed(w, &place, None, &list_id, &id)? {
                Some(pos) => pos,
                None => append_pos_in_list(w.tx, &list_id)?,
            };
            w.task(&id, "pos", json!(pos))?;
        }
    }
    w.task(&id, "title", json!(title))?;
    if !new.notes.is_empty() {
        w.task(&id, "notes", json!(new.notes))?;
    }
    if let Some(start) = new.start.filter(|s| !s.is_empty()) {
        w.task(&id, "start", json!(start))?;
    }
    if let Some(due) = due {
        w.task(&id, "due", json!(due))?;
    }
    if let Some(priority) = priority {
        w.task(&id, "priority", json!(priority.as_i64()))?;
    }
    for raw in &new.tags {
        let name = clean_tag(raw);
        if name.is_empty() {
            continue;
        }
        w.task(&id, &crate::tag::require_field(&name)?, json!(true))?;
    }
    w.flush()?;
    get_task(w.tx, &id)
}

/// R130: the position a new task asks for with `first` or `after`, among the siblings it will have;
/// `None` when it asks for neither and goes last.
fn placed(
    w: &Writer,
    place: &(bool, Option<String>),
    parent: Option<&str>,
    list_id: &str,
    id: &str,
) -> Result<Option<String>> {
    let (first, after) = place;
    if !first && after.is_none() {
        return Ok(None);
    }
    let siblings = match parent {
        Some(parent) => sibling_positions(w.tx, "eff_parent = ?1", parent, id)?,
        None => sibling_positions(w.tx, "eff_list = ?1 AND eff_parent IS NULL", list_id, id)?,
    };
    let mut anchor = match after {
        Some(after) => Some(get_task(w.tx, after)?),
        None => None,
    };
    // Under a subtask the new task goes under its parent: a subtask is no sibling of a task.
    if let Some(up) = anchor.as_ref().and_then(|a| a.parent_id.clone()) {
        if parent != Some(up.as_str()) && !subtasks::is_project(w.tx, &up)? {
            anchor = Some(get_task(w.tx, &up)?);
        }
    }
    let below = anchor
        .filter(|a| a.parent_id.as_deref() == parent && a.list_id == list_id)
        .map(|a| a.id);
    Ok(Some(pos_after(&siblings, below.as_deref())))
}

fn duplicate(w: &mut Writer, original: &TaskItem, parent: Option<String>) -> Result<String> {
    let copy = create_task(
        w,
        NewTask {
            title: original.title.clone(),
            list_id: Some(original.list_id.clone()),
            parent_id: parent,
            notes: original.notes.clone(),
            start: original.start.clone(),
            due: original.due.clone(),
            priority: Some(original.priority),
            tags: original.tags.clone(),
            ..NewTask::default()
        },
    )?;
    if original.is_project {
        // Before its tasks are copied: under a project they keep their fields (R10).
        w.task(&copy.id, "project", json!(true))?;
        w.flush()?;
    }
    if let Some(rule) = &original.repeat {
        w.task(&copy.id, "repeat", serde_json::to_value(rule)?)?;
    }
    if let Some(remind) = &original.remind {
        w.task(&copy.id, "remind", json!(remind))?;
    }
    let mut stmt = w
        .tx
        .prepare("SELECT name, mime, size, sha256 FROM attachments WHERE task_id = ?1 AND deleted = 0 ORDER BY id")?;
    let files: Vec<(String, String, i64, String)> = stmt
        .query_map([&original.id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
        .collect::<rusqlite::Result<_>>()?;
    drop(stmt);
    for (name, mime, size, sha256) in files {
        let id = new_id();
        w.set(KIND_ATTACHMENT, &id, "task", json!(copy.id))?;
        w.set(KIND_ATTACHMENT, &id, "name", json!(name))?;
        w.set(KIND_ATTACHMENT, &id, "mime", json!(mime))?;
        w.set(KIND_ATTACHMENT, &id, "size", json!(size))?;
        w.set(KIND_ATTACHMENT, &id, "sha256", json!(sha256))?;
    }
    let children = query_tasks(
        w.tx,
        "WHERE t.eff_parent = ?1 AND t.deleted = 0 AND t.purged = 0 ORDER BY t.pos, t.id",
        &[&original.id],
    )?;
    for child in children {
        if child.id != copy.id {
            duplicate(w, &child, Some(copy.id.clone()))?;
        }
    }
    Ok(copy.id)
}

/// Writes down the cuts that `rebuild_tree` made to break cycles.
///
/// A cut is only a way of reading the stored parents. Without this, moving one
/// task of a former cycle would bring the suppressed parent of another back to
/// life and rearrange the tree behind the user's back.
fn heal_tree(w: &mut Writer) -> Result<()> {
    let mut stmt = w.tx.prepare(
        "SELECT id FROM tasks WHERE parent_id IS NOT NULL AND eff_parent IS NULL AND parent_id != id
           AND parent_id IN (SELECT id FROM tasks)",
    )?;
    let cut: Vec<String> = stmt.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
    drop(stmt);
    w.unnoted(|w| {
        for id in &cut {
            w.task(id, "parent", Value::Null)?;
        }
        if !cut.is_empty() {
            w.flush()?;
        }
        Ok(())
    })
}

fn sibling_positions(conn: &Connection, filter: &str, arg: &str, except: &str) -> Result<Vec<(String, String)>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT id, pos FROM tasks WHERE {filter} AND id != ?2 AND eff_deleted = 0 AND log_of IS NULL ORDER BY pos, id"
    ))?;
    let rows = stmt.query_map(params![arg, except], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Position right after `after` in an ordered sibling list, or before the first one.
fn pos_after(siblings: &[(String, String)], after: Option<&str>) -> String {
    let index = after.and_then(|a| siblings.iter().position(|(id, _)| id == a));
    let (lo, hi) = match index {
        Some(i) => (Some(siblings[i].1.as_str()), siblings.get(i + 1).map(|s| s.1.as_str())),
        None => (None, siblings.first().map(|s| s.1.as_str())),
    };
    order::between(lo, hi)
}

fn place_after(w: &mut Writer, id: &str, anchor: &TaskItem) -> Result<()> {
    let siblings = match &anchor.parent_id {
        Some(parent) => sibling_positions(w.tx, "eff_parent = ?1", parent, id)?,
        None => sibling_positions(w.tx, "eff_list = ?1 AND eff_parent IS NULL", &anchor.list_id, id)?,
    };
    let pos = pos_after(&siblings, Some(&anchor.id));
    w.task(id, "pos", json!(pos))
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Days between two calendar dates, used by the apps for "in 3 days" labels.
#[uniffi::export]
pub fn days_between(from: String, to: String) -> Option<i64> {
    let (a, _) = recur::split(&from)?;
    let (b, _) = recur::split(&to)?;
    Some((b - a).num_days())
}

/// The date `days` after `date`, keeping its time part.
#[uniffi::export]
pub fn shift_date(date: String, days: i64) -> Option<String> {
    recur::shift(&date, days)
}
