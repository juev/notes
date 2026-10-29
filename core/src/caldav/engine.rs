//! One sync run against a CalDAV server. Rules: docs/specs/caldav.md.
//!
//! The order matters: lists first (a task needs its calendar), then everything
//! the server has that this device has not seen, then what disappeared, and
//! only then local changes go up.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::atomic::Ordering;

use base64::Engine;
use rusqlite::{params, OptionalExtension};
use serde_json::{json, Value};
use uuid::Uuid;

use super::client::{Calendar, Client, Condition, Object, Written};
use super::ical;
use super::map::{self, Registers, Remote, Standard, TaskState};
use crate::db::{self, Change, Touched, KIND_ATTACHMENT, KIND_FILTER, KIND_LIST, KIND_TASK};
use crate::error::{AppError, Result};
use crate::hlc;
use crate::model::{Repeat, SyncReport, INBOX_ID};
use crate::store::{append_pos_in_list, complete_in, reopen_in, Store, Writer};
use crate::tag;

const INBOX_SLUG: &str = "lists-inbox";
const MAX_ID: usize = 200;
const HOME_KEY: &str = "caldav_home";
/// Objects asked for in one `calendar-multiget`.
const MULTIGET: usize = 20;
const TOO_LARGE: &str = "is larger than 32 MB";

struct Item {
    href: String,
    calendar: String,
    etag: String,
    raw: String,
    /// `task_version` at the last reconciliation with this object.
    synced: Option<String>,
    /// The version on the server could not be read (C23); `raw` is an older one.
    unreadable: bool,
}

fn registers(conn: &rusqlite::Connection, kind: &str, id: &str) -> Result<Registers> {
    let mut stmt = conn.prepare_cached("SELECT field, value, stamp FROM fields WHERE kind = ?1 AND id = ?2")?;
    let rows = stmt.query_map(params![kind, id], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))
    })?;
    let mut out = Registers::new();
    for row in rows {
        let (field, value, stamp) = row?;
        out.insert(field, (serde_json::from_str(&value).unwrap_or(Value::Null), stamp));
    }
    // A register no peer could take is not put into an object either (S51).
    out.retain(|field, (_, stamp)| db::key_ok(field) && hlc::is_valid(stamp));
    Ok(out)
}

fn task_state(conn: &rusqlite::Connection, id: &str) -> Result<TaskState> {
    let mut state = TaskState {
        fields: registers(conn, KIND_TASK, id)?,
        attachments: BTreeMap::new(),
    };
    let mut stmt = conn.prepare_cached("SELECT id FROM attachments WHERE task_id = ?1 ORDER BY id")?;
    let ids: Vec<String> = stmt.query_map([id], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
    for attachment in ids {
        state
            .attachments
            .insert(attachment.clone(), registers(conn, KIND_ATTACHMENT, &attachment)?);
    }
    Ok(state)
}

/// Changes whenever a register of the task or of one of its attachments does:
/// a register is only ever replaced by one with a greater stamp, and a new one
/// raises the count.
fn task_version(conn: &rusqlite::Connection, id: &str) -> Result<String> {
    let (stamp, count): (Option<String>, i64) = conn
        .prepare_cached(
            "SELECT max(stamp), count(*) FROM fields
             WHERE (kind = ?1 AND id = ?3)
                OR (kind = ?2 AND id IN (SELECT id FROM attachments WHERE task_id = ?3))",
        )?
        .query_row(params![KIND_TASK, KIND_ATTACHMENT, id], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(format!("{}/{count}", stamp.unwrap_or_default()))
}

fn usable_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= MAX_ID && !id.chars().any(char::is_control)
}

/// The list a calendar stands for. Calendars this app created are named after
/// the list; any other gets an id derived from its address, the same on every device.
fn list_id_of(href: &str) -> String {
    let slug = href.trim_end_matches('/').rsplit('/').next().unwrap_or_default();
    if slug == INBOX_SLUG {
        INBOX_ID.to_string()
    } else if Uuid::parse_str(slug).is_ok() {
        slug.to_string()
    } else {
        Uuid::new_v5(&Uuid::NAMESPACE_URL, href.as_bytes()).to_string()
    }
}

/// File name of a task's object: its id when that is safe in a URL.
fn object_name(uid: &str) -> String {
    let safe = uid
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'));
    if safe && uid.len() <= 100 {
        format!("{uid}.ics")
    } else {
        format!("{}.ics", Uuid::new_v5(&Uuid::NAMESPACE_OID, uid.as_bytes()))
    }
}

fn encode_registers(registers: &Registers) -> String {
    base64::engine::general_purpose::STANDARD.encode(serde_json::to_vec(registers).unwrap_or_default())
}

fn decode_registers(value: &str) -> Option<Registers> {
    let json = base64::engine::general_purpose::STANDARD.decode(value.trim()).ok()?;
    let mut registers: Registers = serde_json::from_slice(&json).ok()?;
    registers.retain(|field, (_, stamp)| db::key_ok(field) && hlc::is_valid(stamp));
    Some(registers)
}

fn text(registers: &Registers, field: &str) -> String {
    registers
        .get(field)
        .and_then(|(v, _)| v.as_str())
        .unwrap_or_default()
        .to_string()
}

/// `#RRGGBBAA` and lower case are the same colour as `#RRGGBB`.
fn color_key(color: &str) -> String {
    color.chars().take(7).collect::<String>().to_uppercase()
}

impl Store {
    /// Applies registers that came from the server; returns how many changed local state.
    fn apply_remote(&self, changes: Vec<Change>) -> Result<u32> {
        let mut inner = self.lock();
        let inner = &mut *inner;
        let tx = inner
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let mut touched = Touched::new();
        let mut changed = 0;
        for change in changes.iter().filter(|c| c.is_well_formed()) {
            inner.clock.observe(&change.stamp);
            if db::apply(&tx, change, false, &mut touched)? {
                changed += 1;
            }
        }
        db::settle(&tx, &touched)?;
        // C29: brought to the rule of subtasks at the end of the run: the object of a project may come after its tasks.
        if changed > 0 {
            db::meta_set(&tx, crate::subtasks::HOLD, "1")?;
        }
        tx.commit()?;
        Ok(changed)
    }

    fn item_by_uid(&self, uid: &str) -> Result<Option<Item>> {
        Ok(self
            .lock()
            .conn
            .query_row(
                "SELECT href, calendar, etag, raw, synced, problem IS NOT NULL FROM caldav_items WHERE uid = ?1",
                [uid],
                |r| {
                    Ok(Item {
                        href: r.get(0)?,
                        calendar: r.get(1)?,
                        etag: r.get(2)?,
                        raw: r.get(3)?,
                        synced: r.get(4)?,
                        unreadable: r.get(5)?,
                    })
                },
            )
            .optional()?)
    }

    /// Records what the server holds. `synced` is the task's version when the
    /// object is known to say the same as the task, `None` when that has to be found out.
    fn save_item(
        &self,
        href: &str,
        calendar: &str,
        uid: &str,
        etag: &str,
        raw: &str,
        synced: Option<&str>,
    ) -> Result<()> {
        let inner = self.lock();
        // One object per task: a copy left behind in another calendar is forgotten.
        inner.conn.execute(
            "DELETE FROM caldav_items WHERE uid = ?1 AND href != ?2",
            params![uid, href],
        )?;
        inner.conn.execute(
            "INSERT INTO caldav_items (href, calendar, uid, etag, raw, synced) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT (href) DO UPDATE SET calendar = excluded.calendar, uid = excluded.uid, etag = excluded.etag,
                raw = excluded.raw, synced = excluded.synced, problem = NULL",
            params![href, calendar, uid, etag, raw, synced],
        )?;
        Ok(())
    }

    /// C23: remembers the version of an object that could not be read, so that
    /// it is not fetched again. An object read before keeps its task.
    fn save_unreadable(&self, href: &str, calendar: &str, etag: &str, problem: &str) -> Result<()> {
        self.lock().conn.execute(
            "INSERT INTO caldav_items (href, calendar, uid, etag, raw, problem) VALUES (?1, ?2, '', ?3, '', ?4)
             ON CONFLICT (href) DO UPDATE SET etag = excluded.etag, problem = excluded.problem",
            params![href, calendar, etag, problem],
        )?;
        Ok(())
    }

    /// C25: remembers the version of an object that is not a task of this app,
    /// so that it is not fetched again. The row of a task is left as it is: an
    /// object that stopped being one is compared again when the task is written.
    fn save_foreign(&self, href: &str, calendar: &str, etag: &str) -> Result<()> {
        if etag.is_empty() {
            return Ok(()); // nothing to tell a later version from this one by
        }
        self.lock().conn.execute(
            "INSERT INTO caldav_items (href, calendar, uid, etag, raw) VALUES (?1, ?2, '', ?3, '')
             ON CONFLICT (href) DO UPDATE SET etag = excluded.etag, problem = NULL WHERE caldav_items.uid = ''",
            params![href, calendar, etag],
        )?;
        Ok(())
    }

    fn mark_synced(&self, href: &str, version: &str) -> Result<()> {
        self.lock().conn.execute(
            "UPDATE caldav_items SET synced = ?2 WHERE href = ?1",
            params![href, version],
        )?;
        Ok(())
    }

    /// Objects known in a calendar: address → (ETag, task).
    fn items_in(&self, calendar: &str) -> Result<BTreeMap<String, (String, String)>> {
        let inner = self.lock();
        let mut stmt = inner
            .conn
            .prepare("SELECT href, etag, uid FROM caldav_items WHERE calendar = ?1")?;
        let rows = stmt.query_map([calendar], |r| Ok((r.get(0)?, (r.get(1)?, r.get(2)?))))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    fn forget_item(&self, href: &str) -> Result<()> {
        self.lock()
            .conn
            .execute("DELETE FROM caldav_items WHERE href = ?1", [href])?;
        Ok(())
    }

    fn save_blobs(&self, calendar: &ical::Component) -> Result<u32> {
        let mut saved = 0;
        for (sha256, bytes) in map::blobs(calendar) {
            let path = self.blob_path(&sha256);
            if !path.exists() {
                let tmp = path.with_extension("part");
                std::fs::write(&tmp, &bytes)?;
                std::fs::rename(&tmp, &path)?;
                saved += 1;
            }
        }
        Ok(saved)
    }

    /// Brings one calendar's settings in: the app's own registers, then a name
    /// or colour changed by another client.
    fn merge_calendar(&self, list_id: &str, calendar: &Calendar) -> Result<u32> {
        let remote = calendar.state.as_deref().and_then(decode_registers);
        let known = !registers(&self.lock().conn, KIND_LIST, list_id)?.is_empty();
        let mut changed = match &remote {
            Some(registers) => self.apply_remote(
                registers
                    .iter()
                    .map(|(field, (value, stamp))| Change {
                        kind: KIND_LIST.into(),
                        id: list_id.into(),
                        field: field.clone(),
                        value: value.clone(),
                        stamp: stamp.clone(),
                    })
                    .collect(),
            )?,
            None => 0,
        };
        // What the standard properties are compared with: the state next to
        // them, or, without one, what this device already believes.
        let reference = match remote {
            Some(registers) => registers,
            None => registers(&self.lock().conn, KIND_LIST, list_id)?,
        };
        let is_inbox = list_id == INBOX_ID;
        let rename = !is_inbox && !calendar.name.is_empty() && calendar.name != text(&reference, "name");
        let recolor = !calendar.color.is_empty() && color_key(&calendar.color) != color_key(&text(&reference, "color"));
        if rename || recolor || !known {
            changed += self.write(|w| {
                let mut n = 0;
                if rename {
                    w.list(list_id, "name", json!(calendar.name))?;
                    n += 1;
                }
                if recolor {
                    w.list(list_id, "color", json!(color_key(&calendar.color)))?;
                    n += 1;
                }
                if !known && !is_inbox && !rename {
                    // A calendar seen for the first time with no usable name still becomes a list.
                    w.list(
                        list_id,
                        "name",
                        json!(if calendar.name.is_empty() {
                            "Calendar"
                        } else {
                            calendar.name.as_str()
                        }),
                    )?;
                    n += 1;
                }
                if !known && !is_inbox && text(&reference, "pos").is_empty() {
                    let last: Option<String> =
                        w.tx.query_row("SELECT max(pos) FROM lists WHERE id != 'inbox'", [], |r| r.get(0))?;
                    w.list(list_id, "pos", json!(crate::order::between(last.as_deref(), None)))?;
                }
                Ok(n)
            })?;
        }
        Ok(changed)
    }

    /// Brings one object in. Returns how many registers changed.
    fn merge_object(&self, list_id: &str, remote: &Remote) -> Result<u32> {
        let id = remote.uid.as_str();
        let known = !registers(&self.lock().conn, KIND_TASK, id)?.is_empty();
        let mut changed = 0;
        if let Some(state) = &remote.state {
            let mut changes: Vec<Change> = Vec::new();
            let mut push = |kind: &str, id: &str, registers: &Registers| {
                for (field, (value, stamp)) in registers {
                    changes.push(Change {
                        kind: kind.into(),
                        id: id.into(),
                        field: field.clone(),
                        value: value.clone(),
                        stamp: stamp.clone(),
                    });
                }
            };
            push(KIND_TASK, id, &state.fields);
            for (attachment, registers) in state.attachments.iter().filter(|(a, _)| usable_id(a)) {
                push(KIND_ATTACHMENT, attachment, registers);
            }
            changed += self.apply_remote(changes)?;
        }

        // C8, C9: standard properties that disagree with the reference are an
        // edit by another client and become new values.
        let reference = match &remote.state {
            Some(state) => Standard::of(state),
            None => Standard::of(&task_state(&self.lock().conn, id)?),
        };
        let stated_list = remote.state.as_ref().map(|s| text(&s.fields, "list"));
        let seen = &remote.standard;
        changed += self.write(|w| {
            let mut n = 0u32;
            let mut set = |w: &mut Writer, field: &str, value: Value| -> Result<()> {
                n += 1;
                w.task(id, field, value)
            };
            if seen.title != reference.title && !seen.title.is_empty() {
                set(w, "title", json!(seen.title))?;
            }
            if seen.notes != reference.notes {
                set(w, "notes", json!(seen.notes))?;
            }
            if seen.start != reference.start {
                set(w, "start", json!(seen.start))?;
            }
            if seen.due != reference.due {
                set(w, "due", json!(seen.due))?;
            }
            if seen.priority != reference.priority {
                set(w, "priority", json!(seen.priority))?;
            }
            for tag_name in seen.tags.difference(&reference.tags) {
                if let Some(field) = tag::field(tag_name) {
                    set(w, &field, json!(true))?;
                }
            }
            for tag_name in reference.tags.difference(&seen.tags) {
                if let Some(field) = tag::field(tag_name) {
                    set(w, &field, json!(false))?;
                }
            }
            if seen.parent != reference.parent && seen.parent.as_deref() != Some(id) {
                set(w, "parent", json!(seen.parent))?;
            }
            if !remote.foreign_rrule && seen.rrule != reference.rrule {
                let current: Option<Repeat> = registers(w.tx, KIND_TASK, id)?
                    .get("repeat")
                    .and_then(|(v, _)| serde_json::from_value(v.clone()).ok());
                let value = match &remote.repeat {
                    // Counting from completion has no standard form: keep the local choice.
                    Some(rule) => serde_json::to_value(Repeat {
                        from_done: current.is_some_and(|c| c.from_done),
                        ..rule.clone()
                    })?,
                    None => Value::Null,
                };
                set(w, "repeat", value)?;
            }
            // A task another client created, or moved to another calendar.
            let placed = match &stated_list {
                Some(list) => list == list_id,
                None => {
                    known
                        && w.tx
                            .query_row(
                                "SELECT eff_list = ?2 FROM tasks WHERE id = ?1",
                                params![id, list_id],
                                |r| r.get(0),
                            )
                            .optional()?
                            .unwrap_or(false)
                }
            };
            if !placed && seen.parent.is_none() {
                set(w, "list", json!(list_id))?;
            }
            if !known && remote.state.is_none() {
                let pos = append_pos_in_list(w.tx, list_id)?;
                set(w, "pos", json!(pos))?;
            }
            w.flush()?;
            if seen.done && !reference.done {
                let open_and_repeating: bool =
                    w.tx.query_row(
                        "SELECT done IS NULL AND repeat IS NOT NULL FROM tasks WHERE id = ?1",
                        [id],
                        |r| r.get(0),
                    )
                    .optional()?
                    .unwrap_or(false);
                if open_and_repeating {
                    // C11: completing one occurrence moves the task on.
                    complete_in(w, id, seen.wont)?;
                    n += 1;
                } else {
                    let moment = remote.completed.clone().unwrap_or_else(|| w.moment());
                    set(w, "done", json!(moment))?;
                    if seen.wont {
                        set(w, "wont", json!(moment))?;
                    }
                }
            } else if !seen.done && reference.done {
                reopen_in(w, id)?;
                n += 1;
            } else if seen.done && seen.wont != reference.wont {
                // C27: the outcome changed, the closing moment stays.
                let closed: Option<String> =
                    w.tx.query_row("SELECT done FROM tasks WHERE id = ?1", [id], |r| r.get(0))
                        .optional()?
                        .flatten();
                // A version that does not know the outcome writes COMPLETED with the
                // moment the task was closed at: that is not an edit.
                let unaware = !seen.wont && remote.completed.is_some() && remote.completed == closed;
                if let (Some(closed), false) = (closed, unaware) {
                    set(w, "wont", if seen.wont { json!(closed) } else { Value::Null })?;
                }
            }
            Ok(n)
        })?;
        Ok(changed)
    }
}

/// What the server holds for a list: name, colour and the app's own registers.
fn desired_props(list_id: &str, registers: &Registers) -> (String, String, String) {
    let name = text(registers, "name");
    let name = if list_id == INBOX_ID && name.is_empty() {
        "Inbox".to_string()
    } else {
        name
    };
    (name, text(registers, "color"), encode_registers(registers))
}

/// One run against the server at `url`. The collection of calendars is looked
/// up once and remembered (C18).
pub fn run(store: &Store, url: &str, user: &str, password: &str) -> Result<SyncReport> {
    let known = db::meta_get(&store.lock().conn, HOME_KEY)?;
    if let Some(home) = known {
        let client = Client::at(url, user, password, &home)?;
        if let Some(calendars) = client.calendars()? {
            return sync(store, &client, calendars);
        }
    }
    let client = Client::connect(url, user, password)?;
    let calendars = client
        .calendars()?
        .ok_or_else(|| AppError::sync(format!("{} does not exist on the server", client.home())))?;
    db::meta_set(&store.lock().conn, HOME_KEY, client.home())?;
    sync(store, &client, calendars)
}

/// C28: whether a calendar of the collection holds an object. Only reads.
pub fn holds_objects(url: &str, user: &str, password: &str) -> Result<bool> {
    let client = Client::connect(url, user, password)?;
    for calendar in client.calendars()?.unwrap_or_default() {
        if !client.list(&calendar.href)?.is_empty() {
            return Ok(true);
        }
    }
    Ok(false)
}

fn sync(store: &Store, client: &Client, calendars: Vec<Calendar>) -> Result<SyncReport> {
    let mut report = SyncReport::default();
    // C29: from here to the end of the run the state may be halfway: an object of a task of a project
    // may be read before the object of the project.
    db::meta_set(&store.lock().conn, crate::subtasks::HOLD, "1")?;

    // ---- lists ↔ calendars ----
    let mut calendar_of: HashMap<String, String> = HashMap::new();
    let mut seen_calendars: BTreeSet<String> = BTreeSet::new();
    for calendar in &calendars {
        let list_id = list_id_of(&calendar.href);
        if calendar_of.contains_key(&list_id) {
            continue;
        }
        report.pulled += store.merge_calendar(&list_id, calendar)?;
        calendar_of.insert(list_id.clone(), calendar.href.clone());
        seen_calendars.insert(calendar.href.clone());
    }
    let known_calendars: Vec<(String, String, Option<String>)> = {
        let inner = store.lock();
        let mut stmt = inner.conn.prepare("SELECT href, list_id, sent FROM caldav_calendars")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    // Change tag and sync token of each calendar as it was last read.
    let read_at: HashMap<String, (String, String)> = {
        let inner = store.lock();
        let mut stmt = inner
            .conn
            .prepare("SELECT href, coalesce(tag, ''), coalesce(token, '') FROM caldav_calendars")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, (r.get(1)?, r.get(2)?))))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    // Change tag at which the calendar of a deleted list was last found to hold objects.
    let kept_at: HashMap<String, String> = {
        let inner = store.lock();
        let mut stmt = inner
            .conn
            .prepare("SELECT href, kept FROM caldav_calendars WHERE kept IS NOT NULL")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    for (href, list_id, _) in &known_calendars {
        if !seen_calendars.contains(href) {
            // The calendar was removed on the server: the list goes with it.
            // C24: unless the list has a calendar under another address, which
            // is the same calendar moved.
            if list_id != INBOX_ID && !calendar_of.contains_key(list_id) {
                store.write(|w| w.list(list_id, "deleted", json!(true)))?;
                report.pulled += 1;
            }
            store
                .lock()
                .conn
                .execute("DELETE FROM caldav_calendars WHERE href = ?1", [href])?;
        }
    }
    let sent: HashMap<String, Option<String>> = known_calendars
        .into_iter()
        .map(|(href, _, sent)| (href, sent))
        .collect();
    let lists: Vec<(String, bool)> = {
        let inner = store.lock();
        let mut stmt = inner.conn.prepare("SELECT id, deleted FROM lists")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    for (list_id, deleted) in &lists {
        let regs = registers(&store.lock().conn, KIND_LIST, list_id)?;
        let (name, color, state) = desired_props(list_id, &regs);
        let href = match (calendar_of.get(list_id), deleted) {
            (Some(href), _) => href.clone(),
            (None, true) => continue,
            (None, false) => {
                let slug = if list_id == INBOX_ID {
                    INBOX_SLUG
                } else {
                    list_id.as_str()
                };
                let href = client.make_calendar(slug, &name, &color)?;
                calendar_of.insert(list_id.clone(), href.clone());
                report.pushed += 1;
                href
            }
        };
        // A deleted list goes on like any other: its calendar stays while it
        // holds objects, the state tells the other devices that the list is
        // gone (C15), and the row keeps the calendar from being read on every
        // run (C19).
        let remote = calendars.iter().find(|c| c.href == href);
        let already_sent = sent.get(&href).cloned().flatten();
        let standard_differs =
            remote.is_none_or(|c| c.name != name || (!color.is_empty() && color_key(&c.color) != color_key(&color)));
        // A server that does not keep the property is not asked again with the same value.
        let state_differs = match remote.and_then(|c| c.state.as_deref()) {
            Some(theirs) => decode_registers(theirs).as_ref() != Some(&regs),
            None => already_sent.as_deref() != Some(state.as_str()),
        };
        if standard_differs || state_differs {
            client.set_props(&href, &name, &color, Some(&state))?;
            if remote.is_some() {
                report.pushed += 1;
            }
        }
        store.lock().conn.execute(
            "INSERT INTO caldav_calendars (href, list_id, sent) VALUES (?1, ?2, ?3)
             ON CONFLICT (href) DO UPDATE SET list_id = excluded.list_id, sent = excluded.sent",
            params![href, list_id, state],
        )?;
    }

    // ---- saved filters: a property of the inbox calendar ----
    if let Some(inbox) = calendar_of.get(INBOX_ID) {
        let theirs: BTreeMap<String, Registers> = calendars
            .iter()
            .find(|c| c.href == *inbox)
            .and_then(|c| c.filters.as_deref())
            .and_then(|raw| base64::engine::general_purpose::STANDARD.decode(raw.trim()).ok())
            .and_then(|json| serde_json::from_slice(&json).ok())
            .unwrap_or_default();
        let mut changes = Vec::new();
        for (id, registers) in theirs.iter().filter(|(id, _)| usable_id(id)) {
            for (field, (value, stamp)) in registers {
                changes.push(Change {
                    kind: KIND_FILTER.into(),
                    id: id.clone(),
                    field: field.clone(),
                    value: value.clone(),
                    stamp: stamp.clone(),
                });
            }
        }
        report.pulled += store.apply_remote(changes)?;
        let ours: BTreeMap<String, Registers> = {
            let inner = store.lock();
            let ids: Vec<String> = {
                let mut stmt = inner
                    .conn
                    .prepare("SELECT DISTINCT id FROM fields WHERE kind = ?1 ORDER BY id")?;
                let rows = stmt.query_map([KIND_FILTER], |r| r.get(0))?;
                rows.collect::<rusqlite::Result<_>>()?
            };
            ids.into_iter()
                .map(|id| registers(&inner.conn, KIND_FILTER, &id).map(|r| (id, r)))
                .collect::<Result<_>>()?
        };
        if ours != theirs {
            let encoded =
                base64::engine::general_purpose::STANDARD.encode(serde_json::to_vec(&ours).unwrap_or_default());
            // A server that drops the property is not asked again with the same value.
            let already = db::meta_get(&store.lock().conn, "caldav_filters_sent")?;
            if already.as_deref() != Some(encoded.as_str()) {
                client.set_filters(inbox, &encoded);
                db::meta_set(&store.lock().conn, "caldav_filters_sent", &encoded)?;
                report.pushed += 1;
            }
        }
    }

    // ---- nudge addresses: one more property of the inbox calendar (C22) ----
    let me = store.device_id();
    let mut nudged: BTreeMap<String, String> = calendar_of
        .get(INBOX_ID)
        .and_then(|inbox| calendars.iter().find(|c| c.href == *inbox))
        .and_then(|c| c.push.as_deref())
        .and_then(|raw| base64::engine::general_purpose::STANDARD.decode(raw.trim()).ok())
        .and_then(|json| serde_json::from_slice(&json).ok())
        .unwrap_or_default();
    // S41: the table is read on every run, so the topic is compared on every run.
    store.share_topic(
        nudged
            .iter()
            .filter(|(device, _)| **device != me)
            .map(|(_, url)| url.as_str()),
    )?;
    let endpoint = store.push_endpoint()?;
    if nudged.get(&me) != endpoint.as_ref() {
        match &endpoint {
            Some(url) => nudged.insert(me.clone(), url.clone()),
            None => nudged.remove(&me),
        };
        if let Some(inbox) = calendar_of.get(INBOX_ID) {
            let encoded =
                base64::engine::general_purpose::STANDARD.encode(serde_json::to_vec(&nudged).unwrap_or_default());
            // A server that drops the property is not asked again with the same value.
            let already = db::meta_get(&store.lock().conn, "push_published")?;
            if already.as_deref() != Some(encoded.as_str()) {
                client.set_push(inbox, &encoded);
                db::meta_set(&store.lock().conn, "push_published", &encoded)?;
            }
        }
    }
    nudged.remove(&me);

    // ---- objects the server has ----
    let mut listed: BTreeSet<String> = BTreeSet::new();
    let mut seen_uids: BTreeSet<String> = BTreeSet::new();
    let mut multiget = true;
    // Calendars read in this run, with the tag and token to remember for them.
    let mut read_now: Vec<(&String, &str, String)> = Vec::new();
    // Calendars a full listing of this run found to hold objects.
    let mut occupied: BTreeSet<&String> = BTreeSet::new();
    for (list_id, calendar) in &calendar_of {
        let remote = calendars.iter().find(|c| c.href == *calendar);
        let tag = remote.map(|c| c.tag.as_str()).unwrap_or_default();
        let (read_tag, read_token) = read_at.get(calendar).cloned().unwrap_or_default();
        let held = store.items_in(calendar)?;
        // An object whose ETag is not known is read whatever the calendar says.
        let unread = held.values().any(|(etag, _)| etag.is_empty());
        if !tag.is_empty() && read_tag == tag && !unread {
            // C19: nothing in the calendar changed since it was last read.
            for (href, (_, uid)) in held {
                listed.insert(href);
                seen_uids.insert(uid);
            }
            continue;
        }

        // C20: what the calendar holds now, from the changes since the last
        // read or from a full listing, and the token that describes it.
        let delta = if read_token.is_empty() {
            None
        } else {
            client.changes_since(calendar, &read_token)?
        };
        let (current, token) = match delta {
            Some(delta) => {
                let mut current: BTreeMap<String, String> = held
                    .iter()
                    .filter(|(href, _)| !delta.removed.contains(href))
                    .map(|(href, (etag, _))| (href.clone(), etag.clone()))
                    .collect();
                current.extend(delta.changed);
                (current.into_iter().collect::<Vec<_>>(), delta.token)
            }
            // The token was read before the listing: a change in between is
            // reported again next time and found to be known.
            None => {
                let all = client.list(calendar)?;
                if !all.is_empty() {
                    occupied.insert(calendar);
                }
                (all, remote.map(|c| c.sync_token.clone()).unwrap_or_default())
            }
        };

        let mut changed: Vec<(String, String)> = Vec::new();
        for (href, etag) in current {
            listed.insert(href.clone());
            match held.get(&href) {
                Some((known_etag, uid)) if !etag.is_empty() && *known_etag == etag => {
                    seen_uids.insert(uid.clone());
                }
                _ => changed.push((href, etag)),
            }
        }
        for chunk in changed.chunks(MULTIGET) {
            // C21: one request for the lot where the server can; the rest one by one.
            let mut fetched: HashMap<String, (String, String)> = HashMap::new();
            if chunk.len() > 1 && multiget {
                let hrefs: Vec<&str> = chunk.iter().map(|(href, _)| href.as_str()).collect();
                match client.multiget(calendar, &hrefs)? {
                    Some(objects) => fetched.extend(objects.into_iter().map(|o| (o.href, (o.body, o.etag)))),
                    None => multiget = false,
                }
            }
            for (href, etag) in chunk {
                let (body, fresh_etag) = match fetched.remove(href) {
                    Some(object) => object,
                    None => match client.get(href)? {
                        Object::Found(body, etag) => (body, etag),
                        Object::Missing => {
                            listed.remove(href);
                            continue;
                        }
                        Object::TooLarge(fresh_etag) => {
                            let etag = if fresh_etag.is_empty() { etag } else { &fresh_etag };
                            store.save_unreadable(href, calendar, etag, TOO_LARGE)?;
                            continue;
                        }
                    },
                };
                let Some(parsed) = ical::parse(&body) else {
                    let etag = if fresh_etag.is_empty() { etag } else { &fresh_etag };
                    store.save_unreadable(href, calendar, etag, "is not iCalendar")?;
                    continue;
                };
                let Some(remote) = map::read(&parsed).filter(|r| usable_id(&r.uid)) else {
                    // An event, or something this app cannot identify.
                    let etag = if fresh_etag.is_empty() { etag } else { &fresh_etag };
                    store.save_foreign(href, calendar, etag)?;
                    continue;
                };
                report.pulled += store.merge_object(list_id, &remote)?;
                report.blobs_downloaded += store.save_blobs(&parsed)?;
                let etag = if fresh_etag.is_empty() { etag } else { &fresh_etag };
                store.save_item(href, calendar, &remote.uid, etag, &body, None)?;
                seen_uids.insert(remote.uid);
            }
        }
        read_now.push((calendar, tag, token));
    }

    // ---- objects that disappeared (C14) ----
    let stored: Vec<(String, String)> = {
        let inner = store.lock();
        let mut stmt = inner.conn.prepare("SELECT href, uid FROM caldav_items")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    for (href, uid) in stored {
        if listed.contains(&href) {
            continue;
        }
        store.forget_item(&href)?;
        if seen_uids.contains(&uid) {
            continue; // the same task now lives under another address
        }
        let live: bool = store
            .lock()
            .conn
            .query_row(
                "SELECT deleted = 0 AND purged = 0 FROM tasks WHERE id = ?1",
                [&uid],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(false);
        if live {
            store.write(|w| w.task(&uid, "deleted", json!(true)))?;
            report.pulled += 1;
        }
    }

    // Only now is a calendar remembered as read: had the run stopped before
    // the objects that disappeared were dealt with, a tag saved earlier would
    // hide them from every later run. The tag is the one read before the
    // objects, so a change that landed in between differs from it and is
    // picked up by the next run.
    for (calendar, tag, token) in read_now {
        store.lock().conn.execute(
            "UPDATE caldav_calendars SET tag = ?2, token = ?3 WHERE href = ?1",
            params![calendar, tag, token],
        )?;
    }

    // ---- local changes go up ----
    let tasks: Vec<(String, String, bool)> = {
        let inner = store.lock();
        let mut stmt = inner
            .conn
            .prepare("SELECT id, eff_list, eff_deleted OR purged FROM tasks ORDER BY id")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    let now = chrono::Utc::now().naive_utc();
    let blob = |sha: &str| std::fs::read(store.blob_path(sha)).ok();
    // Calendars this run removed objects from: what was known of them no longer holds.
    let mut emptied: BTreeSet<String> = BTreeSet::new();
    for (id, list_id, gone) in tasks {
        let item = store.item_by_uid(&id)?;
        if gone {
            // C6: a task in the trash is not kept on the server.
            if let Some(item) = item {
                if client.delete(&item.href, Some(&item.etag))? {
                    store.forget_item(&item.href)?;
                    emptied.insert(item.calendar.clone());
                    report.pushed += 1;
                } else {
                    // Changed on the server meanwhile: read it again on the next run.
                    store.save_item(&item.href, &item.calendar, &id, "", &item.raw, None)?;
                }
            }
            continue;
        }
        let Some(calendar) = calendar_of.get(&list_id) else {
            continue;
        };
        if item.as_ref().is_some_and(|i| i.unreadable) {
            continue; // C23: not written over what could not be read
        }
        let mut item = item;
        if let Some(old) = item.as_ref().filter(|i| i.calendar != *calendar) {
            // The task moved to another list: its object moves to that calendar.
            client.delete(&old.href, Some(&old.etag))?;
            store.forget_item(&old.href)?;
            emptied.insert(old.calendar.clone());
            item = None;
        }
        // C17: nothing changed on either side since the two were last compared.
        if let Some(synced) = item.as_ref().and_then(|i| i.synced.as_deref()) {
            if synced == task_version(&store.lock().conn, &id)? {
                continue;
            }
        }
        let href = item
            .as_ref()
            .map_or_else(|| format!("{calendar}{}", object_name(&id)), |i| i.href.clone());
        let mut base = item.as_ref().and_then(|i| ical::parse(&i.raw));
        let mut etag = item.as_ref().map(|i| i.etag.clone());
        // C13: on a refusal, read what is there, merge and try once more.
        for attempt in 0..2 {
            // Taken before the state: an edit that lands in between is seen on the next run.
            let version = task_version(&store.lock().conn, &id)?;
            let state = task_state(&store.lock().conn, &id)?;
            let rendered = map::render(&id, &state, base.as_ref(), &blob, now);
            store.caldav_renders.fetch_add(1, Ordering::Relaxed);
            if base
                .as_ref()
                .is_some_and(|b| map::fingerprint(b) == map::fingerprint(&rendered))
            {
                store.mark_synced(&href, &version)?;
                break; // C16: nothing to say
            }
            let body = ical::serialize(&rendered);
            let condition = match etag.as_deref() {
                Some(e) if !e.is_empty() => Condition::Match(e),
                Some(_) => Condition::None,
                None => Condition::New,
            };
            match client.put(&href, &body, condition)? {
                Written::Done(new_etag) => {
                    store.save_item(
                        &href,
                        calendar,
                        &id,
                        new_etag.as_deref().unwrap_or(""),
                        &body,
                        Some(&version),
                    )?;
                    report.pushed += 1;
                    let managed = |c: Option<&ical::Component>| -> BTreeSet<String> {
                        c.and_then(|c| c.sub("VTODO"))
                            .map(|t| {
                                t.all("ATTACH")
                                    .filter_map(|p| p.param(map::SHA_PARAM).map(str::to_string))
                                    .collect()
                            })
                            .unwrap_or_default()
                    };
                    report.blobs_uploaded +=
                        managed(Some(&rendered)).difference(&managed(base.as_ref())).count() as u32;
                    break;
                }
                Written::Conflict if attempt == 0 => {
                    let (fresh, fresh_etag) = match client.get(&href)? {
                        Object::Found(fresh, fresh_etag) => (fresh, fresh_etag),
                        Object::Missing => {
                            base = None;
                            etag = None;
                            continue;
                        }
                        Object::TooLarge(_) => return Err(AppError::sync(format!("{href} {TOO_LARGE}"))),
                    };
                    let parsed = ical::parse(&fresh);
                    if let Some(remote) = parsed.as_ref().and_then(map::read).filter(|r| r.uid == id) {
                        report.pulled += store.merge_object(&list_id, &remote)?;
                    }
                    store.save_item(&href, calendar, &id, &fresh_etag, &fresh, None)?;
                    base = parsed;
                    etag = Some(fresh_etag);
                }
                Written::Conflict => break, // settled on the next run
            }
        }
    }

    // ---- lists removed here: the calendar goes once it is empty (C15) ----
    for (list_id, deleted) in &lists {
        if !*deleted || list_id == INBOX_ID {
            continue;
        }
        let Some(href) = calendar_of.get(list_id) else {
            continue;
        };
        let tag = calendars
            .iter()
            .find(|c| c.href == *href)
            .map(|c| c.tag.as_str())
            .unwrap_or_default();
        // What is known of the calendar holds unless this run took objects out of it.
        let settled = !emptied.contains(href);
        if settled && !tag.is_empty() && kept_at.get(href).is_some_and(|kept| kept == tag) {
            continue; // nothing changed since it was found to hold objects
        }
        // A listing this run has already made is not repeated; an empty
        // calendar is listed right before it is removed.
        if (settled && occupied.contains(href)) || !client.list(href)?.is_empty() {
            // The tag was read before the listing: a change in between differs
            // from it, and the calendar is listed again on the next run.
            store.lock().conn.execute(
                "UPDATE caldav_calendars SET kept = ?2 WHERE href = ?1",
                params![href, tag],
            )?;
        } else if client.delete(href, None)? {
            store
                .lock()
                .conn
                .execute("DELETE FROM caldav_calendars WHERE href = ?1", [href])?;
            report.pushed += 1;
        }
    }
    if report.pushed > 0 || report.blobs_uploaded > 0 {
        store.poke(nudged.values().map(String::as_str));
    }
    // C29: every object was read; what the run brings to the rule of subtasks goes up with the next one.
    store.tidy_whole()?;
    // C30: content no attachment needs leaves this device; the server keeps it inside the objects.
    store.drop_unneeded_blobs()?;
    Ok(report)
}

/// C23: what the last run left unread on the server, as the sync status words it.
pub fn unreadable(conn: &rusqlite::Connection) -> Result<Option<String>> {
    let mut stmt =
        conn.prepare("SELECT href || ' ' || problem FROM caldav_items WHERE problem IS NOT NULL ORDER BY href")?;
    let found: Vec<String> = stmt.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
    Ok((!found.is_empty()).then(|| format!("not read, the rest is in sync: {}", found.join("; "))))
}
