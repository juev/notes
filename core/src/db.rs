//! SQLite schema, the per-field register table and the tables derived from it.
//!
//! `fields` is the source of truth: one last-writer-wins register per
//! (kind, id, field). `notes`, `folders`, `note_tags`, `attachments` and
//! `filters` are rebuilt from it and exist only to make queries simple.
//! The text of a note is not a register: it is an Automerge document in `bodies` (S3).

use std::collections::{BTreeSet, HashMap, HashSet};

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::Result;
use crate::hlc;

pub const KIND_NOTE: &str = "note";
pub const KIND_FOLDER: &str = "folder";
pub const KIND_ATTACHMENT: &str = "attachment";
pub const KIND_FILTER: &str = "filter";
/// S27: attachment content that a device removed from the storage; the id is its hash.
pub const KIND_BLOB: &str = "blob";
/// Settings shared by every device: one entity, `SETTINGS_ID`, read straight from `fields`.
pub const KIND_SETTINGS: &str = "settings";
pub const SETTINGS_ID: &str = "app";

/// Bumped whenever a derived table changes shape or what it holds is built by
/// another rule: the tables are then dropped and rebuilt from `fields`, which
/// never changes shape.
const DERIVED_VERSION: &str = "2";
/// The version of the tables that are not rebuilt from the registers: `fields` and the bookkeeping.
/// A change that rewrites one of them raises it, and a database with a smaller number is backed up
/// before it is migrated (R92 of Lists). Adding a column or a table needs no new number.
pub const SCHEMA_VERSION: u32 = 2;

pub(crate) const MAX_KEY_LEN: usize = 200;

/// What makes a key of a register acceptable to every device and to a restored backup (S5).
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
    /// S2: the text of a note is no register; a line that claims it is one is not applied.
    pub fn is_well_formed(&self) -> bool {
        key_ok(&self.kind)
            && key_ok(&self.id)
            && key_ok(&self.field)
            && hlc::is_valid(&self.stamp)
            && !(self.kind == KIND_NOTE && self.field == "body")
    }
}

/// Entities whose derived rows must be rebuilt before the transaction commits.
pub type Touched = BTreeSet<(String, String)>;

const DERIVED_SCHEMA: &str = "
        CREATE TABLE IF NOT EXISTS notes (
            id TEXT PRIMARY KEY,
            title TEXT NOT NULL DEFAULT '',
            -- as stored; `eff_folder` is where the note is shown (S10)
            folder_id TEXT,
            -- milliseconds since the epoch
            created INTEGER NOT NULL DEFAULT 0,
            -- the greatest stamp among the registers of the note, in milliseconds
            edited INTEGER NOT NULL DEFAULT 0,
            pinned INTEGER NOT NULL DEFAULT 0,
            starred INTEGER NOT NULL DEFAULT 0,
            archived INTEGER NOT NULL DEFAULT 0,
            locked INTEGER NOT NULL DEFAULT 0,
            deleted INTEGER NOT NULL DEFAULT 0,
            -- when the note went to the Trash, in milliseconds (S6)
            deleted_at INTEGER NOT NULL DEFAULT 0,
            purged INTEGER NOT NULL DEFAULT 0,
            -- derived by rebuild_folders
            eff_folder TEXT,
            -- the text without markup, for the preview (R5) and the search (R47)
            plain TEXT NOT NULL DEFAULT ''
        ) WITHOUT ROWID;
        CREATE INDEX IF NOT EXISTS notes_folder ON notes (eff_folder);

        CREATE TABLE IF NOT EXISTS folders (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL DEFAULT '',
            icon TEXT NOT NULL DEFAULT '',
            parent_id TEXT,
            parent_stamp TEXT NOT NULL DEFAULT '',
            pos TEXT NOT NULL DEFAULT '',
            daily INTEGER NOT NULL DEFAULT 0,
            sort TEXT NOT NULL DEFAULT '',
            show_preview INTEGER NOT NULL DEFAULT 1,
            show_date INTEGER NOT NULL DEFAULT 1,
            deleted INTEGER NOT NULL DEFAULT 0,
            -- derived by rebuild_folders: the parent once cycles are broken (S9), and the one it is shown under (S10)
            tree_parent TEXT,
            eff_parent TEXT
        ) WITHOUT ROWID;
        CREATE INDEX IF NOT EXISTS folders_parent ON folders (eff_parent);

        CREATE TABLE IF NOT EXISTS note_tags (
            note_id TEXT NOT NULL,
            -- the name in lower case: tags are one tag whatever the case (R21)
            key TEXT NOT NULL,
            tag TEXT NOT NULL,
            -- when the register was written: the earliest spelling is the one shown
            stamp TEXT NOT NULL,
            PRIMARY KEY (note_id, key)
        ) WITHOUT ROWID;
        CREATE INDEX IF NOT EXISTS note_tags_key ON note_tags (key);

        CREATE TABLE IF NOT EXISTS attachments (
            id TEXT PRIMARY KEY,
            note_id TEXT NOT NULL DEFAULT '',
            name TEXT NOT NULL DEFAULT '',
            mime TEXT NOT NULL DEFAULT '',
            size INTEGER NOT NULL DEFAULT 0,
            sha256 TEXT NOT NULL DEFAULT '',
            deleted INTEGER NOT NULL DEFAULT 0
        ) WITHOUT ROWID;
        CREATE INDEX IF NOT EXISTS attachments_note ON attachments (note_id);

        -- R51: the notes the text of a note links to
        CREATE TABLE IF NOT EXISTS note_links (
            note_id TEXT NOT NULL,
            target TEXT NOT NULL,
            PRIMARY KEY (note_id, target)
        ) WITHOUT ROWID;
        CREATE INDEX IF NOT EXISTS note_links_target ON note_links (target);

        CREATE TABLE IF NOT EXISTS filters (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL DEFAULT '',
            icon TEXT NOT NULL DEFAULT '',
            query TEXT NOT NULL DEFAULT '{}',
            pos TEXT NOT NULL DEFAULT '',
            deleted INTEGER NOT NULL DEFAULT 0
        ) WITHOUT ROWID;

";

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

        -- S37: what this device held when it was told to put its data over the storage's
        CREATE TABLE IF NOT EXISTS kept (
            kind  TEXT NOT NULL,
            id    TEXT NOT NULL,
            field TEXT NOT NULL,
            value TEXT NOT NULL,
            PRIMARY KEY (kind, id, field)
        ) WITHOUT ROWID;
        CREATE TABLE IF NOT EXISTS kept_bodies (note_id TEXT PRIMARY KEY, text TEXT NOT NULL) WITHOUT ROWID;

        -- S3: the text of a note, one saved Automerge document per note, outside `fields`
        CREATE TABLE IF NOT EXISTS bodies (
            note_id  TEXT PRIMARY KEY,
            -- `save()` of the document, with its full history (S21)
            doc      BLOB NOT NULL,
            -- the heads of the version last put into a log file, 32 bytes each (S20)
            uploaded BLOB NOT NULL DEFAULT x'',
            -- the document has changes that no log file carries yet
            dirty    INTEGER NOT NULL DEFAULT 0,
            -- the stamp of the last edit seen, issued here or read from a log line (S13)
            stamp    TEXT NOT NULL DEFAULT '',
            -- the text of the document as the filter and the search read it, kept in step with `doc`
            text     TEXT NOT NULL DEFAULT ''
        ) WITHOUT ROWID;
        CREATE INDEX IF NOT EXISTS bodies_dirty ON bodies (dirty) WHERE dirty = 1;

        -- S22: body changes that could not be applied, kept as they were received
        CREATE TABLE IF NOT EXISTS body_pending (
            id     INTEGER PRIMARY KEY,
            note_id TEXT NOT NULL,
            device TEXT NOT NULL,
            crdt   TEXT NOT NULL,
            stamp  TEXT NOT NULL,
            digest TEXT NOT NULL,
            UNIQUE (note_id, digest)
        );
        ",
    )?;
    if meta_get(conn, "derived_version")?.as_deref() != Some(DERIVED_VERSION) {
        conn.execute_batch(
            "DROP TABLE IF EXISTS notes; DROP TABLE IF EXISTS folders; DROP TABLE IF EXISTS note_tags;
             DROP TABLE IF EXISTS attachments; DROP TABLE IF EXISTS filters; DROP TABLE IF EXISTS note_links;",
        )?;
    }
    conn.execute_batch(DERIVED_SCHEMA)?;
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

/// The version of the schema the database was left at (R92 of Lists), or nothing for a database that has no tables yet.
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

/// The greatest stamp this device holds, in a register or on a text.
pub fn max_stamp(conn: &Connection) -> Result<Option<String>> {
    Ok(conn.query_row(
        "SELECT max(s) FROM (SELECT max(stamp) AS s FROM fields UNION ALL SELECT max(stamp) FROM bodies)",
        [],
        |r| r.get(0),
    )?)
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

/// Rebuilds derived rows for everything in `touched`, then the folder tree.
pub fn settle(conn: &Connection, touched: &Touched) -> Result<()> {
    if touched.is_empty() {
        return Ok(());
    }
    for (kind, id) in touched {
        match kind.as_str() {
            KIND_NOTE => materialize_note(conn, id)?,
            KIND_FOLDER => materialize_folder(conn, id)?,
            KIND_ATTACHMENT => materialize_attachment(conn, id)?,
            KIND_FILTER => materialize_filter(conn, id)?,
            // S27: the storage may no longer hold it, so this device checks again before it counts on it.
            KIND_BLOB => {
                conn.execute("DELETE FROM blobs_uploaded WHERE sha256 = ?1", [id])?;
            }
            // Written by a newer version: kept in `fields`, ignored here.
            _ => {}
        }
    }
    if touched.iter().any(|(kind, _)| kind == KIND_NOTE || kind == KIND_FOLDER) {
        rebuild_folders(conn)?;
    }
    Ok(())
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
        self.flag_or(field, false)
    }
    /// A switch that is on until a register says otherwise.
    fn flag_or(&self, field: &str, default: bool) -> bool {
        self.0.get(field).and_then(|(v, _)| v.as_bool()).unwrap_or(default)
    }
    fn int(&self, field: &str) -> i64 {
        self.0.get(field).and_then(|(v, _)| v.as_i64()).unwrap_or(0)
    }
    fn stamp(&self, field: &str) -> String {
        self.0.get(field).map(|(_, s)| s.clone()).unwrap_or_default()
    }
    /// The moment a field with a time as its value was written (`deleted`, S6): the value when it is a number
    /// of milliseconds, the time of the stamp for a mark of another kind; none when the field is empty.
    fn moment(&self, field: &str) -> Option<i64> {
        let (value, stamp) = self.0.get(field)?;
        match value {
            Value::Null | Value::Bool(false) => None,
            Value::String(s) if s.is_empty() => None,
            Value::Number(n) => n
                .as_i64()
                .filter(|ms| *ms > 0)
                .or_else(|| Some(hlc::stamp_ms(stamp) as i64)),
            _ => Some(hlc::stamp_ms(stamp) as i64),
        }
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

fn materialize_note(conn: &Connection, id: &str) -> Result<()> {
    let r = Registers::load(conn, KIND_NOTE, id)?;
    // A text that arrived ahead of the registers of its note does not make a note of its own.
    if r.0.is_empty() {
        return Ok(());
    }
    // S7: the text of a note deleted for good is dropped with its pending changes.
    if r.flag("purged") {
        conn.execute("DELETE FROM bodies WHERE note_id = ?1", [id])?;
        conn.execute("DELETE FROM body_pending WHERE note_id = ?1", [id])?;
    }
    let stamps = || r.0.values().map(|(_, stamp)| hlc::stamp_ms(stamp));
    // The last edit of the text counts as an edit of the note (S13).
    let body: Option<(String, String)> = conn
        .query_row("SELECT stamp, text FROM bodies WHERE note_id = ?1", [id], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .optional()?;
    let (plain, links) = crate::markdown::plain_and_links(body.as_ref().map_or("", |b| b.1.as_str()));
    let body = body.map(|b| b.0);
    let edited = stamps()
        .chain(body.map(|stamp| hlc::stamp_ms(&stamp)))
        .max()
        .unwrap_or(0) as i64;
    // A note whose creation was never recorded counts from its first register.
    let created = Some(r.int("created"))
        .filter(|ms| *ms > 0)
        .unwrap_or_else(|| stamps().min().unwrap_or(0) as i64);
    let deleted_at = r.moment("deleted");
    // The folder a note is shown in is corrected by rebuild_folders afterwards.
    conn.execute(
        "INSERT INTO notes
         (id, title, folder_id, created, edited, pinned, starred, archived, locked, deleted, deleted_at, purged,
          plain)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
         ON CONFLICT (id) DO UPDATE SET
            title = excluded.title, folder_id = excluded.folder_id, created = excluded.created,
            edited = excluded.edited, pinned = excluded.pinned, starred = excluded.starred,
            archived = excluded.archived, locked = excluded.locked, deleted = excluded.deleted,
            deleted_at = excluded.deleted_at, purged = excluded.purged, plain = excluded.plain",
        params![
            id,
            r.text("title").unwrap_or_default(),
            r.text("folder"),
            created,
            edited,
            r.flag("pinned"),
            r.flag("starred"),
            r.flag("archived"),
            r.flag("locked"),
            deleted_at.is_some(),
            deleted_at.unwrap_or(0),
            r.flag("purged"),
            plain,
        ],
    )?;
    conn.execute("DELETE FROM note_links WHERE note_id = ?1", [id])?;
    for target in &links {
        conn.execute(
            "INSERT OR IGNORE INTO note_links (note_id, target) VALUES (?1, ?2)",
            params![id, target],
        )?;
    }
    conn.execute("DELETE FROM note_tags WHERE note_id = ?1", [id])?;
    // Two registers that differ in case are one tag; the one written first gives the spelling (R21).
    let mut tags: Vec<(&String, &str)> =
        r.0.iter()
            .filter_map(|(field, (value, stamp))| {
                let tag = field.strip_prefix(crate::tag::PREFIX)?;
                // A tag no register can carry stays out of the view (S5), even if an earlier version wrote it.
                (value.as_bool() == Some(true) && crate::tag::name_ok(tag)).then_some((field, stamp.as_str()))
            })
            .collect();
    tags.sort_by(|a, b| (a.1, a.0).cmp(&(b.1, b.0)));
    for (field, stamp) in tags {
        let tag = &field[crate::tag::PREFIX.len()..];
        conn.execute(
            "INSERT OR IGNORE INTO note_tags (note_id, key, tag, stamp) VALUES (?1, ?2, ?3, ?4)",
            params![id, crate::tag::key(tag), tag, stamp],
        )?;
    }
    Ok(())
}

fn materialize_folder(conn: &Connection, id: &str) -> Result<()> {
    let r = Registers::load(conn, KIND_FOLDER, id)?;
    // Derived columns survive the rewrite; rebuild_folders corrects them afterwards.
    conn.execute(
        "INSERT INTO folders
         (id, name, icon, parent_id, parent_stamp, pos, daily, sort, show_preview, show_date, deleted)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
         ON CONFLICT (id) DO UPDATE SET
            name = excluded.name, icon = excluded.icon, parent_id = excluded.parent_id,
            parent_stamp = excluded.parent_stamp, pos = excluded.pos, daily = excluded.daily,
            sort = excluded.sort, show_preview = excluded.show_preview, show_date = excluded.show_date,
            deleted = excluded.deleted",
        params![
            id,
            r.text("name").unwrap_or_default(),
            r.text("icon").unwrap_or_default(),
            r.text("parent"),
            r.stamp("parent"),
            r.text("pos").unwrap_or_default(),
            r.flag("daily"),
            r.text("sort").unwrap_or_default(),
            r.flag_or("show_preview", true),
            r.flag_or("show_date", true),
            r.moment("deleted").is_some(),
        ],
    )?;
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
        "INSERT OR REPLACE INTO attachments (id, note_id, name, mime, size, sha256, deleted)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            id,
            r.text("note").unwrap_or_default(),
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
        "INSERT OR REPLACE INTO filters (id, name, icon, query, pos, deleted) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            id,
            r.text("name").unwrap_or_default(),
            r.text("icon").unwrap_or_default(),
            r.json("query").unwrap_or_else(|| "{}".into()),
            r.text("pos").unwrap_or_default(),
            r.flag("deleted"),
        ],
    )?;
    Ok(())
}

struct Node {
    parent: Option<String>,
    parent_stamp: String,
    gone: bool,
    tree_parent: Option<String>,
    eff_parent: Option<String>,
}

/// Derives the tree of folders from the stored `parent` registers (S9), and the folder each note is
/// shown in (S10).
///
/// Concurrent moves can leave a cycle in the stored parents. Every device
/// breaks it the same way: inside a cycle, the folder whose `parent` register
/// has the greatest stamp is treated as having no parent. A folder whose
/// parent is deleted is shown at the root, and so is a note whose folder is
/// deleted or not known here.
pub fn rebuild_folders(conn: &Connection) -> Result<()> {
    let mut nodes: HashMap<String, Node> = HashMap::new();
    {
        let mut stmt =
            conn.prepare("SELECT id, parent_id, parent_stamp, deleted, tree_parent, eff_parent FROM folders")?;
        let mut rows = stmt.query([])?;
        while let Some(r) = rows.next()? {
            nodes.insert(
                r.get(0)?,
                Node {
                    parent: r.get(1)?,
                    parent_stamp: r.get(2)?,
                    gone: r.get(3)?,
                    tree_parent: r.get(4)?,
                    eff_parent: r.get(5)?,
                },
            );
        }
    }

    // A parent that is unknown here (not synced yet) or is the folder itself counts as none.
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

    let mut update = conn.prepare_cached("UPDATE folders SET tree_parent = ?2, eff_parent = ?3 WHERE id = ?1")?;
    for id in ids {
        let node = &nodes[id];
        let tree_parent = parent.get(id);
        let eff_parent = tree_parent.filter(|p| !nodes[*p].gone);
        if node.tree_parent.as_ref() != tree_parent || node.eff_parent.as_ref() != eff_parent {
            update.execute(params![id, tree_parent, eff_parent])?;
        }
    }
    conn.execute(
        "UPDATE notes SET eff_folder = CASE WHEN folder_id IN (SELECT id FROM folders WHERE deleted = 0)
                                           THEN folder_id END
         WHERE eff_folder IS NOT CASE WHEN folder_id IN (SELECT id FROM folders WHERE deleted = 0)
                                      THEN folder_id END",
        [],
    )?;
    Ok(())
}
