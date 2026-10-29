//! SQLite schema, the per-field register table and the tables derived from it.
//!
//! `fields` is the source of truth: one last-writer-wins register per
//! (kind, id, field). `lists`, `tasks`, `task_tags` and `attachments` are
//! rebuilt from it and exist only to make queries simple.

use std::collections::{BTreeSet, HashMap, HashSet};

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::Result;
use crate::hlc;
use crate::model::INBOX_ID;

pub const KIND_LIST: &str = "list";
pub const KIND_TASK: &str = "task";
pub const KIND_ATTACHMENT: &str = "attachment";
pub const KIND_FILTER: &str = "filter";
/// S48: attachment content that a device removed from the storage; the id is its hash.
pub const KIND_BLOB: &str = "blob";
/// Settings shared by every device: one entity, `SETTINGS_ID`, read straight from `fields`.
pub const KIND_SETTINGS: &str = "settings";
pub const SETTINGS_ID: &str = "app";

/// Bumped whenever a derived table changes shape or what it holds is built by
/// another rule: the tables are then dropped and rebuilt from `fields`, which
/// never changes shape.
const DERIVED_VERSION: &str = "5";
/// The version of the tables that are not rebuilt from the registers: `fields` and the bookkeeping.
/// A change that rewrites one of them raises it, and a database with a smaller number is backed up
/// before it is migrated (R92). Adding a column or a table needs no new number.
pub const SCHEMA_VERSION: u32 = 2;

pub(crate) const MAX_KEY_LEN: usize = 200;

/// What makes a key of a register acceptable to every device and to a restored backup (S51).
pub(crate) fn key_ok(key: &str) -> bool {
    !key.is_empty() && key.len() <= MAX_KEY_LEN
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Change {
    #[serde(rename = "k")]
    pub kind: String,
    #[serde(rename = "i")]
    pub id: String,
    #[serde(rename = "f")]
    pub field: String,
    #[serde(rename = "v")]
    pub value: Value,
    #[serde(rename = "t")]
    pub stamp: String,
}

impl Change {
    pub fn is_well_formed(&self) -> bool {
        key_ok(&self.kind) && key_ok(&self.id) && key_ok(&self.field) && hlc::is_valid(&self.stamp)
    }
}

/// Entities whose derived rows must be rebuilt before the transaction commits.
pub type Touched = BTreeSet<(String, String)>;

const DERIVED_SCHEMA: &str = "
        CREATE TABLE IF NOT EXISTS lists (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL DEFAULT '',
            color TEXT NOT NULL DEFAULT '',
            icon TEXT NOT NULL DEFAULT '',
            sort TEXT NOT NULL DEFAULT 'manual',
            show_done INTEGER NOT NULL DEFAULT 0,
            default_priority INTEGER NOT NULL DEFAULT 0,
            default_due INTEGER NOT NULL DEFAULT 0,
            pos TEXT NOT NULL DEFAULT '',
            archived INTEGER NOT NULL DEFAULT 0,
            deleted INTEGER NOT NULL DEFAULT 0
        ) WITHOUT ROWID;

        CREATE TABLE IF NOT EXISTS tasks (
            id TEXT PRIMARY KEY,
            list_id TEXT NOT NULL DEFAULT 'inbox',
            parent_id TEXT,
            parent_stamp TEXT NOT NULL DEFAULT '',
            title TEXT NOT NULL DEFAULT '',
            notes TEXT NOT NULL DEFAULT '',
            start TEXT,
            due TEXT,
            priority INTEGER NOT NULL DEFAULT 0,
            repeat TEXT,
            remind TEXT,
            pos TEXT NOT NULL DEFAULT '',
            done TEXT,
            -- when `done` was written: the first twelve digits are milliseconds (R68)
            done_stamp TEXT NOT NULL DEFAULT '',
            -- closed without being done: the `wont` register equals `done` (S33)
            wont INTEGER NOT NULL DEFAULT 0,
            deleted INTEGER NOT NULL DEFAULT 0,
            purged INTEGER NOT NULL DEFAULT 0,
            log_of TEXT,
            project INTEGER NOT NULL DEFAULT 0,
            -- derived by rebuild_tree
            eff_parent TEXT,
            eff_list TEXT NOT NULL DEFAULT 'inbox',
            eff_deleted INTEGER NOT NULL DEFAULT 0
        ) WITHOUT ROWID;
        CREATE INDEX IF NOT EXISTS tasks_parent ON tasks (eff_parent);
        CREATE INDEX IF NOT EXISTS tasks_list ON tasks (eff_list);

        CREATE TABLE IF NOT EXISTS task_tags (
            task_id TEXT NOT NULL,
            tag TEXT NOT NULL,
            PRIMARY KEY (task_id, tag)
        ) WITHOUT ROWID;
        CREATE INDEX IF NOT EXISTS task_tags_tag ON task_tags (tag);

        CREATE TABLE IF NOT EXISTS attachments (
            id TEXT PRIMARY KEY,
            task_id TEXT NOT NULL DEFAULT '',
            name TEXT NOT NULL DEFAULT '',
            mime TEXT NOT NULL DEFAULT '',
            size INTEGER NOT NULL DEFAULT 0,
            sha256 TEXT NOT NULL DEFAULT '',
            deleted INTEGER NOT NULL DEFAULT 0
        ) WITHOUT ROWID;
        CREATE INDEX IF NOT EXISTS attachments_task ON attachments (task_id);

        CREATE TABLE IF NOT EXISTS filters (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL DEFAULT '',
            spec TEXT NOT NULL DEFAULT '{}',
            pos TEXT NOT NULL DEFAULT '',
            deleted INTEGER NOT NULL DEFAULT 0
        ) WITHOUT ROWID;

";

/// Adds a column to a table created by an earlier version.
fn add_column(conn: &Connection, table: &str, column: &str, decl: &str) -> Result<()> {
    let present: bool = conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM pragma_table_info(?1) WHERE name = ?2)",
        [table, column],
        |r| r.get(0),
    )?;
    if !present {
        conn.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {column} {decl}"))?;
    }
    Ok(())
}

pub fn migrate(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "
        PRAGMA journal_mode = WAL;
        PRAGMA synchronous = NORMAL;
        PRAGMA busy_timeout = 5000;

        CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL) WITHOUT ROWID;

        CREATE TABLE IF NOT EXISTS fields (
            kind  TEXT NOT NULL,
            id    TEXT NOT NULL,
            field TEXT NOT NULL,
            value TEXT NOT NULL,
            stamp TEXT NOT NULL,
            dirty INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (kind, id, field)
        ) WITHOUT ROWID;
        CREATE INDEX IF NOT EXISTS fields_dirty ON fields (dirty) WHERE dirty = 1;

        -- sync bookkeeping
        CREATE TABLE IF NOT EXISTS peers (device TEXT PRIMARY KEY, seq INTEGER NOT NULL) WITHOUT ROWID;
        CREATE TABLE IF NOT EXISTS outbox (seq INTEGER PRIMARY KEY, body BLOB NOT NULL, changes INTEGER NOT NULL);
        CREATE TABLE IF NOT EXISTS blobs_uploaded (sha256 TEXT PRIMARY KEY) WITHOUT ROWID;

        -- S38: what this device held when it was told to put its data over the storage's
        CREATE TABLE IF NOT EXISTS kept (
            kind  TEXT NOT NULL,
            id    TEXT NOT NULL,
            field TEXT NOT NULL,
            value TEXT NOT NULL,
            PRIMARY KEY (kind, id, field)
        ) WITHOUT ROWID;

        -- CalDAV bookkeeping: what the server held when it was last read
        CREATE TABLE IF NOT EXISTS caldav_calendars (href TEXT PRIMARY KEY, list_id TEXT NOT NULL, sent TEXT) WITHOUT ROWID;
        CREATE TABLE IF NOT EXISTS caldav_items (
            href TEXT PRIMARY KEY,
            calendar TEXT NOT NULL,
            uid TEXT NOT NULL,
            etag TEXT NOT NULL,
            raw TEXT NOT NULL
        ) WITHOUT ROWID;
        CREATE INDEX IF NOT EXISTS caldav_items_uid ON caldav_items (uid);

        ",
    )?;
    // The task's registers as they were when the object was last found to say the same (C17).
    add_column(conn, "caldav_items", "synced", "TEXT")?;
    // Why the object could not be read; such a row may have no task (C23).
    add_column(conn, "caldav_items", "problem", "TEXT")?;
    // The calendar's change tag when its objects were last listed in full (C19).
    add_column(conn, "caldav_calendars", "tag", "TEXT")?;
    // The sync token that describes the calendar as it was last read (C20).
    add_column(conn, "caldav_calendars", "token", "TEXT")?;
    // The change tag of a deleted list's calendar when it was last found to hold objects (C15).
    add_column(conn, "caldav_calendars", "kept", "TEXT")?;
    if meta_get(conn, "derived_version")?.as_deref() != Some(DERIVED_VERSION) {
        conn.execute_batch("DROP TABLE IF EXISTS lists; DROP TABLE IF EXISTS tasks; DROP TABLE IF EXISTS task_tags; DROP TABLE IF EXISTS attachments; DROP TABLE IF EXISTS filters;")?;
    }
    conn.execute_batch(DERIVED_SCHEMA)?;
    conn.execute("INSERT OR IGNORE INTO lists (id) VALUES ('inbox')", [])?;
    if meta_get(conn, "derived_version")?.as_deref() != Some(DERIVED_VERSION) {
        let mut all = Touched::new();
        {
            let mut stmt = conn.prepare("SELECT DISTINCT kind, id FROM fields")?;
            let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
            for row in rows {
                all.insert(row?);
            }
        }
        settle(conn, &all)?;
        meta_set(conn, "derived_version", DERIVED_VERSION)?;
    }
    // A database written by a later version keeps its number.
    let stored: Option<u32> = meta_get(conn, "schema_version")?.and_then(|v| v.parse().ok());
    if stored.is_none_or(|stored| stored < SCHEMA_VERSION) {
        meta_set(conn, "schema_version", &SCHEMA_VERSION.to_string())?;
    }
    Ok(())
}

/// The version of the schema the database was left at (R92), or nothing for a database that has no tables yet.
/// A database from before the number was kept is at 1.
pub fn schema_version(conn: &Connection) -> Result<Option<u32>> {
    let tables: u32 = conn.query_row(
        "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name IN ('meta', 'fields')",
        [],
        |r| r.get(0),
    )?;
    if tables < 2 {
        return Ok(None);
    }
    Ok(Some(
        meta_get(conn, "schema_version")?
            .and_then(|v| v.parse().ok())
            .unwrap_or(1),
    ))
}

pub fn meta_get(conn: &Connection, key: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0))
        .optional()?)
}

pub fn meta_set(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO meta (key, value) VALUES (?1, ?2) ON CONFLICT (key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

pub fn meta_del(conn: &Connection, key: &str) -> Result<()> {
    conn.execute("DELETE FROM meta WHERE key = ?1", [key])?;
    Ok(())
}

pub fn max_stamp(conn: &Connection) -> Result<Option<String>> {
    Ok(conn.query_row("SELECT max(stamp) FROM fields", [], |r| r.get(0))?)
}

/// Writes one register if the incoming stamp is newer. Returns whether state changed.
pub fn apply(conn: &Connection, change: &Change, local: bool, touched: &mut Touched) -> Result<bool> {
    let current: Option<String> = conn
        .query_row(
            "SELECT stamp FROM fields WHERE kind = ?1 AND id = ?2 AND field = ?3",
            params![change.kind, change.id, change.field],
            |r| r.get(0),
        )
        .optional()?;
    if current.as_deref().is_some_and(|c| c >= change.stamp.as_str()) {
        return Ok(false);
    }
    conn.execute(
        "INSERT INTO fields (kind, id, field, value, stamp, dirty) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT (kind, id, field) DO UPDATE SET value = excluded.value, stamp = excluded.stamp, dirty = excluded.dirty",
        params![change.kind, change.id, change.field, change.value.to_string(), change.stamp, local as i64],
    )?;
    touched.insert((change.kind.clone(), change.id.clone()));
    Ok(true)
}

/// Rebuilds derived rows for everything in `touched`, then the task tree.
pub fn settle(conn: &Connection, touched: &Touched) -> Result<()> {
    if touched.is_empty() {
        return Ok(());
    }
    for (kind, id) in touched {
        match kind.as_str() {
            KIND_LIST => materialize_list(conn, id)?,
            KIND_TASK => materialize_task(conn, id)?,
            KIND_ATTACHMENT => materialize_attachment(conn, id)?,
            KIND_FILTER => materialize_filter(conn, id)?,
            // S48: the storage may no longer hold it, so this device checks again before it counts on it.
            KIND_BLOB => {
                conn.execute("DELETE FROM blobs_uploaded WHERE sha256 = ?1", [id])?;
            }
            // Written by a newer version: kept in `fields`, ignored here.
            _ => {}
        }
    }
    rebuild_tree(conn)
}

struct Registers(HashMap<String, (Value, String)>);

impl Registers {
    fn load(conn: &Connection, kind: &str, id: &str) -> Result<Self> {
        let mut stmt = conn.prepare_cached("SELECT field, value, stamp FROM fields WHERE kind = ?1 AND id = ?2")?;
        let rows = stmt.query_map(params![kind, id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))
        })?;
        let mut map = HashMap::new();
        for row in rows {
            let (field, value, stamp) = row?;
            map.insert(field, (serde_json::from_str(&value).unwrap_or(Value::Null), stamp));
        }
        Ok(Registers(map))
    }
    fn text(&self, field: &str) -> Option<String> {
        self.0
            .get(field)
            .and_then(|(v, _)| v.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    }
    fn flag(&self, field: &str) -> bool {
        self.0.get(field).and_then(|(v, _)| v.as_bool()).unwrap_or(false)
    }
    fn int(&self, field: &str) -> i64 {
        self.0.get(field).and_then(|(v, _)| v.as_i64()).unwrap_or(0)
    }
    fn stamp(&self, field: &str) -> String {
        self.0.get(field).map(|(_, s)| s.clone()).unwrap_or_default()
    }
    /// A structured value kept as JSON text; `null` and absent are the same.
    fn json(&self, field: &str) -> Option<String> {
        self.0
            .get(field)
            .map(|(v, _)| v)
            .filter(|v| v.is_object())
            .map(Value::to_string)
    }
}

fn materialize_list(conn: &Connection, id: &str) -> Result<()> {
    let r = Registers::load(conn, KIND_LIST, id)?;
    let inbox = id == INBOX_ID;
    conn.execute(
        "INSERT OR REPLACE INTO lists
         (id, name, color, icon, sort, show_done, default_priority, default_due, pos, archived, deleted)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            id,
            r.text("name").unwrap_or_default(),
            r.text("color").unwrap_or_default(),
            r.text("icon").unwrap_or_default(),
            r.text("sort").unwrap_or_else(|| "manual".into()),
            r.flag("show_done"),
            r.int("default_priority"),
            r.flag("default_due"),
            r.text("pos").unwrap_or_default(),
            !inbox && r.flag("archived"),
            !inbox && r.flag("deleted"),
        ],
    )?;
    Ok(())
}

fn materialize_task(conn: &Connection, id: &str) -> Result<()> {
    let r = Registers::load(conn, KIND_TASK, id)?;
    // Derived columns survive the rewrite; rebuild_tree corrects them afterwards.
    conn.execute(
        "INSERT INTO tasks
         (id, list_id, parent_id, parent_stamp, title, notes, start, due, priority, repeat, remind, pos, done, deleted, purged, log_of, project, done_stamp, wont)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)
         ON CONFLICT (id) DO UPDATE SET
            list_id = excluded.list_id, parent_id = excluded.parent_id, parent_stamp = excluded.parent_stamp,
            title = excluded.title, notes = excluded.notes, start = excluded.start, due = excluded.due,
            priority = excluded.priority, repeat = excluded.repeat, remind = excluded.remind, pos = excluded.pos,
            done = excluded.done, deleted = excluded.deleted, purged = excluded.purged, log_of = excluded.log_of,
            project = excluded.project, done_stamp = excluded.done_stamp, wont = excluded.wont",
        params![
            id,
            r.text("list").unwrap_or_else(|| INBOX_ID.into()),
            r.text("parent"),
            r.stamp("parent"),
            r.text("title").unwrap_or_default(),
            r.text("notes").unwrap_or_default(),
            r.text("start"),
            r.text("due"),
            r.int("priority").clamp(0, 3),
            r.json("repeat"),
            r.text("remind"),
            r.text("pos").unwrap_or_default(),
            r.text("done"),
            r.flag("deleted"),
            r.flag("purged"),
            r.text("log_of"),
            r.flag("project"),
            r.stamp("done"),
            r.text("done").is_some() && r.text("wont") == r.text("done"),
        ],
    )?;
    conn.execute("DELETE FROM task_tags WHERE task_id = ?1", [id])?;
    for (field, (value, _)) in &r.0 {
        if let Some(tag) = field.strip_prefix(crate::tag::PREFIX) {
            // A tag no register can carry stays out of the view (S51), even if an earlier version wrote it.
            if value.as_bool() == Some(true) && crate::tag::name_ok(tag) {
                conn.execute(
                    "INSERT OR IGNORE INTO task_tags (task_id, tag) VALUES (?1, ?2)",
                    params![id, tag],
                )?;
            }
        }
    }
    Ok(())
}

/// Attachment content is addressed by this value and it becomes a file name,
/// so nothing but a lowercase hex SHA-256 is ever accepted from a register.
pub fn is_sha256(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn materialize_attachment(conn: &Connection, id: &str) -> Result<()> {
    let r = Registers::load(conn, KIND_ATTACHMENT, id)?;
    conn.execute(
        "INSERT OR REPLACE INTO attachments (id, task_id, name, mime, size, sha256, deleted)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            id,
            r.text("task").unwrap_or_default(),
            r.text("name").unwrap_or_default(),
            r.text("mime").unwrap_or_default(),
            r.int("size").max(0),
            r.text("sha256").filter(|s| is_sha256(s)).unwrap_or_default(),
            r.flag("deleted"),
        ],
    )?;
    Ok(())
}

fn materialize_filter(conn: &Connection, id: &str) -> Result<()> {
    let r = Registers::load(conn, KIND_FILTER, id)?;
    conn.execute(
        "INSERT OR REPLACE INTO filters (id, name, spec, pos, deleted) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            id,
            r.text("name").unwrap_or_default(),
            r.json("spec").unwrap_or_else(|| "{}".into()),
            r.text("pos").unwrap_or_default(),
            r.flag("deleted"),
        ],
    )?;
    Ok(())
}

struct Node {
    parent: Option<String>,
    parent_stamp: String,
    list: String,
    gone: bool,
    eff_parent: Option<String>,
    eff_list: String,
    eff_deleted: bool,
}

/// Derives the effective tree from the stored `parent` registers.
///
/// Concurrent moves can leave a cycle in the stored parents. Every device
/// breaks it the same way: inside a cycle, the task whose `parent` register
/// has the greatest stamp is treated as having no parent.
pub fn rebuild_tree(conn: &Connection) -> Result<()> {
    let live_lists: HashSet<String> = conn
        .prepare("SELECT id FROM lists WHERE deleted = 0")?
        .query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;

    let mut nodes: HashMap<String, Node> = HashMap::new();
    {
        let mut stmt = conn.prepare(
            "SELECT id, parent_id, parent_stamp, list_id, deleted OR purged, eff_parent, eff_list, eff_deleted FROM tasks",
        )?;
        let mut rows = stmt.query([])?;
        while let Some(r) = rows.next()? {
            nodes.insert(
                r.get(0)?,
                Node {
                    parent: r.get(1)?,
                    parent_stamp: r.get(2)?,
                    list: r.get(3)?,
                    gone: r.get(4)?,
                    eff_parent: r.get(5)?,
                    eff_list: r.get(6)?,
                    eff_deleted: r.get(7)?,
                },
            );
        }
    }

    // A parent that is unknown here (not synced yet) or is the task itself counts as none.
    let mut parent: HashMap<String, String> = nodes
        .iter()
        .filter_map(|(id, n)| {
            let p = n.parent.as_ref()?;
            (p != id && nodes.contains_key(p)).then(|| (id.clone(), p.clone()))
        })
        .collect();

    // Break cycles. Ids are visited in sorted order so the result is deterministic.
    let mut ids: Vec<&String> = nodes.keys().collect();
    ids.sort();
    let mut settled: HashSet<String> = HashSet::new();
    for start in &ids {
        let mut path: Vec<String> = Vec::new();
        let mut on_path: HashSet<String> = HashSet::new();
        let mut cur = (*start).clone();
        loop {
            if settled.contains(&cur) {
                break;
            }
            if on_path.contains(&cur) {
                let from = path.iter().position(|p| *p == cur).unwrap_or(0);
                let cut = path[from..]
                    .iter()
                    .max_by(|a, b| (&nodes[*a].parent_stamp, *a).cmp(&(&nodes[*b].parent_stamp, *b)))
                    .cloned()
                    .unwrap_or_else(|| cur.clone());
                parent.remove(&cut);
                break;
            }
            on_path.insert(cur.clone());
            path.push(cur.clone());
            match parent.get(&cur) {
                Some(p) => cur = p.clone(),
                None => break,
            }
        }
        settled.extend(path);
    }

    // Root, list and visibility flow down from the topmost ancestor.
    let mut resolved: HashMap<String, (String, bool)> = HashMap::new();
    for id in &ids {
        let mut chain: Vec<&String> = Vec::new();
        let mut cur: &String = id;
        let (mut list, mut deleted) = loop {
            if let Some(known) = resolved.get(cur) {
                break known.clone();
            }
            chain.push(cur);
            match parent.get(cur) {
                Some(p) => cur = p,
                None => {
                    let root = &nodes[cur];
                    let list = if live_lists.contains(&root.list) {
                        root.list.clone()
                    } else {
                        INBOX_ID.to_string()
                    };
                    chain.pop();
                    resolved.insert(cur.clone(), (list.clone(), root.gone));
                    break (list, root.gone);
                }
            }
        };
        for node in chain.into_iter().rev() {
            deleted = deleted || nodes[node].gone;
            resolved.insert(node.clone(), (list.clone(), deleted));
        }
        list.clear();
    }

    let mut update =
        conn.prepare_cached("UPDATE tasks SET eff_parent = ?2, eff_list = ?3, eff_deleted = ?4 WHERE id = ?1")?;
    for id in ids {
        let node = &nodes[id];
        let eff_parent = parent.get(id);
        let (eff_list, eff_deleted) = &resolved[id];
        if node.eff_parent.as_ref() != eff_parent || &node.eff_list != eff_list || node.eff_deleted != *eff_deleted {
            update.execute(params![id, eff_parent, eff_list, eff_deleted])?;
        }
    }
    Ok(())
}
