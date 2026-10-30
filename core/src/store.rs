//! The object the apps talk to. Every public method is synchronous and safe
//! to call from any thread; apps call them off the main thread.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use chrono::{NaiveDate, NaiveDateTime};
use rusqlite::{params, params_from_iter, Connection, OptionalExtension, Row};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::db::{
    self, Change, Touched, KIND_ATTACHMENT, KIND_FILTER, KIND_FOLDER, KIND_NOTE, KIND_SETTINGS, SETTINGS_ID,
};
use crate::error::{AppError, Result};
use crate::hlc::{Clock, DEVICE_ID_LEN};
use crate::log::LogLevel;
use crate::model::*;
use crate::undo::Step;
use crate::{order, tag};

const MOMENT_FMT: &str = "%Y-%m-%dT%H:%M";

/// R36: where the limits of this device are kept in `meta`, next to the level of the log.
const MAX_FILE_KEY: &str = "max_file_bytes";
const MAX_NOTE_KEY: &str = "max_note_bytes";

/// R36: a file that grew past the limit while it was copied.
fn over(limit: u64, amount: u64) -> bool {
    limit != 0 && amount > limit
}

/// R142 of Lists: a folder only its owner can look into, on Unix. Other systems have their own rules.
pub(crate) fn private_dir(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    let _ = path;
    Ok(())
}

/// R142 of Lists: a file only its owner can read, on Unix.
pub(crate) fn private_file(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    let _ = path;
    Ok(())
}

/// R142 of Lists: creates the folder and everything above it, closed from other users from the start.
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

/// R36: the limits kept in `meta`, or the defaults of a device that has none.
fn stored_limits(conn: &Connection) -> Result<AttachmentLimits> {
    let read = |key: &str, fallback: u64| -> Result<u64> {
        Ok(db::meta_get(conn, key)?
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(fallback))
    };
    Ok(AttachmentLimits {
        max_file: read(MAX_FILE_KEY, DEFAULT_MAX_FILE)?,
        max_note: read(MAX_NOTE_KEY, DEFAULT_MAX_NOTE)?,
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
    /// One pass over attachment content at a time; runs of the fields do not wait for it (S24).
    pub(crate) blob_lock: Mutex<()>,
    /// One look at the backup schedule at a time, so that two callers do not both find it due (R70).
    pub(crate) backup_lock: Mutex<()>,
    pub(crate) dir: PathBuf,
    /// The store itself, for work that outlives the call that started it (S41).
    pub(crate) me: std::sync::Weak<Store>,
    /// The level of the log (R95) as its code, read on every line that may be written.
    pub(crate) log_level: std::sync::atomic::AtomicU8,
    /// One writer of the log file at a time.
    pub(crate) log_lock: Mutex<()>,
    /// S15: own log files that make a snapshot whatever they weigh.
    pub(crate) compact_after: Mutex<u32>,
    pub(crate) sync_password: Mutex<Option<String>>,
    /// S31: the passphrase of an encrypted storage. Like the password, never on disk.
    pub(crate) sync_passphrase: Mutex<Option<String>>,
    /// S28: the storage whose `vault.json` was read last, and what it said.
    pub(crate) sealed: Mutex<Option<(String, crate::sync::Sealing)>>,
    /// S28: the data key opened last, so that the key derivation does not run on every look.
    pub(crate) vault_key: Mutex<Option<crate::sync::VaultKey>>,
    /// The storage this process has already checked the format of and listed the snapshots of.
    pub(crate) storage_seen: Mutex<Option<String>>,
    /// Names in the log of the sync folder when it was last looked at.
    pub(crate) folder_seen: Mutex<Option<Vec<String>>>,
    /// How long a failed wait for a nudge holds back the next one.
    pub(crate) push_retry: Mutex<std::time::Duration>,
    /// Access token for this device's own ntfy server (S45). Like the password, never on disk.
    pub(crate) push_token: Mutex<Option<String>>,
    /// S45: the own server answered 401 or 403 to the last subscription, to the last nudge.
    pub(crate) push_refused: Mutex<(bool, bool)>,
    /// What undo takes back (R75).
    pub(crate) history: Mutex<crate::undo::History>,
    /// R36: how much content this device takes, kept by the device and not synced.
    pub(crate) limits: Mutex<AttachmentLimits>,
}

/// One local write transaction: stamps and records field changes, then
/// rebuilds the derived rows before commit.
pub(crate) struct Writer<'a> {
    pub tx: &'a Connection,
    clock: &'a mut Clock,
    now_ms: u64,
    touched: Touched,
    /// What an action that undo can take back has written so far (R75).
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
    /// I5 in docs/specs/import.md: a register of a new entity stamped at `ms`, the moment it was last
    /// written in another app, never later than now.
    pub(crate) fn set_at(&mut self, kind: &str, id: &str, field: &str, value: Value, ms: u64) -> Result<()> {
        let stamp = self.clock.stamp_at(ms.min(self.now_ms));
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
        Ok(())
    }
    /// A stamp at `ms` for what is not a register, never later than now (I5).
    pub(crate) fn stamp_at(&self, ms: u64) -> String {
        self.clock.stamp_at(ms.min(self.now_ms))
    }
    /// Writes that keep the data whole rather than carry out the action stay out of its step.
    fn unnoted<T>(&mut self, f: impl FnOnce(&mut Self) -> Result<T>) -> Result<T> {
        let noted = self.noted.take();
        let out = f(self);
        self.noted = noted;
        out
    }
    pub(crate) fn note(&mut self, id: &str, field: &str, value: Value) -> Result<()> {
        self.set(KIND_NOTE, id, field, value)
    }
    pub(crate) fn folder(&mut self, id: &str, field: &str, value: Value) -> Result<()> {
        self.set(KIND_FOLDER, id, field, value)
    }
    /// Makes pending changes visible to queries inside the same transaction.
    pub(crate) fn flush(&mut self) -> Result<()> {
        let touched = std::mem::take(&mut self.touched);
        db::settle(self.tx, &touched)
    }
    /// The moment of this write on the clock of the stamps: what `created` and `deleted` hold (S6).
    pub(crate) fn now_ms(&self) -> u64 {
        self.now_ms
    }
    /// A stamp for something that is not a register, the edit of a text (S13).
    pub(crate) fn stamp(&mut self) -> String {
        self.clock.tick(self.now_ms)
    }
    /// Asks for the derived rows of an entity to be rebuilt before the transaction commits.
    pub(crate) fn touch(&mut self, kind: &str, id: &str) {
        self.touched.insert((kind.into(), id.into()));
    }
}

thread_local! {
    /// The zone the calls of this thread read the time in (see `Store::in_zone`).
    static ZONE: std::cell::Cell<Option<chrono::FixedOffset>> = const { std::cell::Cell::new(None) };
}

impl Store {
    /// Runs `f` with today, the time of day and the moments it writes taken on a clock that many
    /// minutes east of UTC rather than on this device's. The web server answers each browser in the
    /// zone of the browser (R135 of Lists). The stamps keep their clock: it has no time zone.
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

    /// A write that notes what it changes, for undo (R75).
    pub(crate) fn write_noted<T>(&mut self, f: impl FnOnce(&mut Writer) -> Result<T>) -> Result<(T, Step)> {
        self.write_with(Some(Step::default()), f)
            .map(|(out, step)| (out, step.unwrap_or_default()))
    }

    fn write_with<T>(
        &mut self,
        noted: Option<Step>,
        f: impl FnOnce(&mut Writer) -> Result<T>,
    ) -> Result<(T, Option<Step>)> {
        let now_ms = self.now_ms();
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let out = {
            let mut w = Writer {
                tx: &tx,
                clock: &mut self.clock,
                now_ms,
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

/// One field of the shared settings (S12); none when it is not there or does not parse.
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

fn setting_text(conn: &Connection, field: &str, default: &str) -> Result<String> {
    Ok(setting(conn, field)?
        .and_then(|v| v.as_str().filter(|s| !s.is_empty()).map(str::to_string))
        .unwrap_or_else(|| default.to_string()))
}

fn setting_flag(conn: &Connection, field: &str, default: bool) -> Result<bool> {
    Ok(setting(conn, field)?.and_then(|v| v.as_bool()).unwrap_or(default))
}

fn setting_number(conn: &Connection, field: &str, default: f64) -> Result<f64> {
    Ok(setting(conn, field)?
        .and_then(|v| v.as_f64())
        .filter(|n| n.is_finite())
        .unwrap_or(default))
}

/// S8, S12: the days a note stays in the Trash; a value outside 1 to 3650 applies the default.
fn trash_days(conn: &Connection) -> Result<u32> {
    Ok(setting(conn, "trash_days")?
        .and_then(|v| v.as_u64())
        .filter(|days| (1..=u64::from(MAX_TRASH_DAYS)).contains(days))
        .map_or(DEFAULT_TRASH_DAYS, |days| days as u32))
}

/// S12: every shared setting as the registers give it. A field that is not there, or holds a value this
/// version cannot read, applies its default.
fn read_settings(conn: &Connection) -> Result<AppSettings> {
    let d = AppSettings::default();
    Ok(AppSettings {
        trash_days: trash_days(conn)?,
        font_body: setting_text(conn, "font_body", &d.font_body)?,
        font_mono: setting_text(conn, "font_mono", &d.font_mono)?,
        line_height: setting_number(conn, "line_height", d.line_height)?,
        paragraph_spacing: setting_number(conn, "paragraph_spacing", d.paragraph_spacing)?,
        align: match setting_text(conn, "align", "left")?.as_str() {
            "justify" => TextAlign::Justify,
            _ => TextAlign::Left,
        },
        hyphenate: setting_flag(conn, "hyphenate", d.hyphenate)?,
        smart_typography: setting_flag(conn, "smart_typography", d.smart_typography)?,
        spellcheck: setting_flag(conn, "spellcheck", d.spellcheck)?,
        spell_language: setting_text(conn, "spell_language", &d.spell_language)?,
        new_note_folder: setting_text(conn, "new_note_folder", &d.new_note_folder)?,
    })
}

/// R2: the folder a note goes to when the view it is created from names none: `root`, `last` or a folder.
/// `last_used` is what this device used last. A folder that is gone gives the root.
pub(crate) fn default_folder_id(conn: &Connection, last_used: Option<&str>) -> Result<Option<String>> {
    let choice = read_settings(conn)?.new_note_folder;
    let wanted = match choice.as_str() {
        "root" => None,
        "last" => last_used,
        id => Some(id),
    };
    Ok(match wanted {
        Some(id) if folder_is_live(conn, id)? => Some(id.to_string()),
        _ => None,
    })
}

const NOTE_COLUMNS: &str = "
    n.id, n.title, n.eff_folder, n.pinned, n.starred, n.archived, n.locked, n.deleted, n.created, n.edited,
    n.deleted_at,
    (SELECT count(*) FROM attachments a WHERE a.note_id = n.id AND a.deleted = 0),
    (SELECT group_concat(tag, char(31)) FROM (SELECT tag FROM note_tags WHERE note_id = n.id ORDER BY key))";

fn note_from_row(r: &Row) -> rusqlite::Result<Note> {
    let deleted: bool = r.get(7)?;
    let tags: Option<String> = r.get(12)?;
    Ok(Note {
        id: r.get(0)?,
        title: r.get(1)?,
        folder_id: r.get(2)?,
        pinned: r.get(3)?,
        starred: r.get(4)?,
        archived: r.get(5)?,
        locked: r.get(6)?,
        deleted,
        created_ms: r.get::<_, i64>(8)?.max(0) as u64,
        edited_ms: r.get::<_, i64>(9)?.max(0) as u64,
        deleted_ms: deleted.then(|| r.get::<_, i64>(10).map_or(0, |ms| ms.max(0) as u64)),
        attachments: r.get(11)?,
        tags: tags
            .map(|t| t.split('\u{1f}').map(str::to_string).collect())
            .unwrap_or_default(),
    })
}

pub(crate) fn query_notes(conn: &Connection, tail: &str, args: &[&str]) -> Result<Vec<Note>> {
    let sql = format!("SELECT {NOTE_COLUMNS} FROM notes n {tail}");
    let mut stmt = conn.prepare_cached(&sql)?;
    let rows = stmt.query_map(params_from_iter(args.iter()), note_from_row)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// A note that is not deleted for good (S7).
pub(crate) fn get_note(conn: &Connection, id: &str) -> Result<Note> {
    query_notes(conn, "WHERE n.id = ?1 AND n.purged = 0", &[id])?
        .pop()
        .ok_or_else(|| AppError::not_found(format!("note {id}")))
}

/// R28: a locked note keeps its title, tags and attachments as they are.
pub(crate) fn get_unlocked_note(conn: &Connection, id: &str) -> Result<Note> {
    let note = get_note(conn, id)?;
    if note.locked {
        return Err(AppError::invalid("the note is locked"));
    }
    Ok(note)
}

/// R16: pinned notes first, then the order the folder sets; equal notes go by identifier.
fn sort_notes(notes: &mut [Note], sort: FolderSort) {
    notes.sort_by(|a, b| {
        let by = match sort.by {
            SortBy::Edited => a.edited_ms.cmp(&b.edited_ms),
            SortBy::Created => a.created_ms.cmp(&b.created_ms),
            SortBy::Title => a.title.to_lowercase().cmp(&b.title.to_lowercase()),
        };
        let by = if sort.descending { by.reverse() } else { by };
        b.pinned.cmp(&a.pinned).then(by).then_with(|| a.id.cmp(&b.id))
    });
}

const FOLDER_COLUMNS: &str = "f.id, f.name, f.icon, f.eff_parent, f.daily, f.sort, f.show_preview, f.show_date";

fn folder_from_row(r: &Row) -> rusqlite::Result<Folder> {
    Ok(Folder {
        id: r.get(0)?,
        name: r.get(1)?,
        icon: r.get(2)?,
        parent_id: r.get(3)?,
        daily: r.get(4)?,
        sort: FolderSort::parse(&r.get::<_, String>(5)?),
        show_preview: r.get(6)?,
        show_date: r.get(7)?,
        note_count: 0,
    })
}

/// A folder that is not in the Trash, with the notes of its subfolders counted in (R12).
fn live_folders(conn: &Connection) -> Result<Vec<Folder>> {
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT {FOLDER_COLUMNS} FROM folders f WHERE f.deleted = 0 ORDER BY f.pos, f.id"
    ))?;
    let mut folders: Vec<Folder> = stmt.query_map([], folder_from_row)?.collect::<rusqlite::Result<_>>()?;
    let mut own: HashMap<String, u32> = HashMap::new();
    let mut stmt = conn.prepare_cached(
        "SELECT eff_folder, count(*) FROM notes
         WHERE eff_folder IS NOT NULL AND deleted = 0 AND purged = 0 AND archived = 0 GROUP BY eff_folder",
    )?;
    for row in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, u32>(1)?)))? {
        let (id, count) = row?;
        own.insert(id, count);
    }
    let mut children: HashMap<&str, Vec<&str>> = HashMap::new();
    for f in &folders {
        if let Some(parent) = &f.parent_id {
            children.entry(parent.as_str()).or_default().push(f.id.as_str());
        }
    }
    // The tree has no cycle (S9), so the walk ends.
    fn total(id: &str, own: &HashMap<String, u32>, children: &HashMap<&str, Vec<&str>>) -> u32 {
        let below: u32 = children
            .get(id)
            .map_or(0, |ids| ids.iter().map(|child| total(child, own, children)).sum());
        own.get(id).copied().unwrap_or(0) + below
    }
    let counts: Vec<u32> = folders.iter().map(|f| total(&f.id, &own, &children)).collect();
    for (folder, count) in folders.iter_mut().zip(counts) {
        folder.note_count = count;
    }
    Ok(folders)
}

fn get_folder(conn: &Connection, id: &str) -> Result<Folder> {
    live_folders(conn)?
        .into_iter()
        .find(|f| f.id == id)
        .ok_or_else(|| AppError::not_found(format!("folder {id}")))
}

fn folder_is_live(conn: &Connection, id: &str) -> Result<bool> {
    Ok(conn
        .query_row("SELECT 1 FROM folders WHERE id = ?1 AND deleted = 0", [id], |_| Ok(()))
        .optional()?
        .is_some())
}

fn new_id() -> String {
    Uuid::now_v7().to_string()
}

pub(crate) fn last_pos(conn: &Connection, sql: &str, args: &[&str]) -> Result<Option<String>> {
    Ok(conn.query_row(sql, params_from_iter(args.iter()), |r| r.get(0))?)
}

/// The type of a file by its first bytes, as (MIME type, extension), for a name that does not tell it (R85 of Lists).
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

const MS_PER_DAY: u64 = 24 * 60 * 60 * 1000;

impl Store {
    pub(crate) fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub(crate) fn write<T>(&self, f: impl FnOnce(&mut Writer) -> Result<T>) -> Result<T> {
        self.lock().write(f)
    }

    /// S8: sends to the Trash for good every note that has been in it longer than `trash_days`. Called where
    /// the state is whole: at the end of a pass that read the whole storage, and on start without sync.
    /// Returns how many notes were purged.
    pub(crate) fn expire_trash(&self) -> Result<u32> {
        self.write(|w| {
            let days = u64::from(trash_days(w.tx)?);
            let limit = w.now_ms().saturating_sub(days * MS_PER_DAY) as i64;
            let mut stmt =
                w.tx.prepare("SELECT id FROM notes WHERE deleted = 1 AND purged = 0 AND deleted_at < ?1")?;
            let ids: Vec<String> = stmt
                .query_map([limit], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            drop(stmt);
            for id in &ids {
                w.note(id, "purged", json!(true))?;
            }
            Ok(ids.len() as u32)
        })
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

    /// Writes one register of a note the way a version with other rules would.
    #[doc(hidden)]
    pub fn set_note_field_for_tests(&self, id: &str, field: &str, value: Value) {
        self.write(|w| w.note(id, field, value)).unwrap();
    }

    /// Writes one register of a folder the way a version with other rules would.
    #[doc(hidden)]
    pub fn set_folder_field_for_tests(&self, id: &str, field: &str, value: Value) {
        self.write(|w| w.folder(id, field, value)).unwrap();
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
    pub fn data_dir(&self) -> &Path {
        &self.dir
    }
}

pub(crate) fn new_device_id() -> String {
    let simple = Uuid::now_v7().simple().to_string();
    simple[simple.len() - DEVICE_ID_LEN..].to_string()
}

/// R36: the content of the live attachments of one note, as the note quota counts it.
fn note_attachment_bytes(conn: &Connection, note_id: &str) -> Result<u64> {
    let sum: Option<i64> = conn.query_row(
        "SELECT sum(size) FROM attachments WHERE note_id = ?1 AND deleted = 0",
        [note_id],
        |r| r.get(0),
    )?;
    Ok(sum.unwrap_or(0).max(0) as u64)
}

fn too_large_file(size: u64, limit: u64) -> AppError {
    AppError::too_large(format!(
        "the file is {size} bytes; this device takes files up to {limit} bytes"
    ))
}

fn too_large_note(used: u64, size: u64, limit: u64) -> AppError {
    AppError::too_large(format!(
        "the note holds {used} bytes and the file is {size} bytes; this device takes {limit} bytes per note"
    ))
}

/// R36: copies `source` into `tmp` in pieces, hashing each one; `tmp` is removed when anything fails.
pub(crate) fn copy_hashing(source: &Path, tmp: &Path, limit: u64) -> Result<(String, u64)> {
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
        // R142 of Lists: the data folder and the content in it are for this user alone.
        make_private_dir(&dir)?;
        make_private_dir(&dir.join("blobs"))?;
        let database = dir.join("notes.sqlite");
        let conn = Connection::open(&database)?;
        private_file(&database)?;
        crate::backup::close_backups(&dir.join("backups"))?;
        // R92 of Lists: a database of an earlier schema is backed up as it is, before anything in it is changed.
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
        // S4: an older database has a device identifier and no actor yet.
        crate::body::ensure_actor(&conn)?;
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
            history: Mutex::new(Default::default()),
            limits: Mutex::new(AttachmentLimits::default()),
        });
        // An earlier schema has the table the level is kept in as well.
        store.load_log_level()?;
        // R36: read after the migration, which may have created `meta`.
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
        // S8, S26: with no storage there is nothing to arrive, so the state on the device is whole.
        if matches!(store.sync_config()?, crate::model::SyncConfig::Off) {
            store.expire_trash()?;
            store.drop_unneeded_blobs()?;
        }
        Ok(store)
    }

    pub fn device_id(&self) -> String {
        self.lock().clock.device().to_string()
    }

    // ---- notes ----

    /// The notes of a view: pinned first, then in the order of the folder for a folder and by the last edit
    /// elsewhere (R16, R25). Archived notes appear only in Archived and in the Trash (R27).
    pub fn notes(&self, scope: Scope) -> Result<Vec<Note>> {
        const LIVE: &str = "n.deleted = 0 AND n.purged = 0 AND n.archived = 0";
        let inner = self.lock();
        let conn = &inner.conn;
        let mut sort = FolderSort::default();
        let mut notes = match &scope {
            Scope::All => query_notes(conn, &format!("WHERE {LIVE}"), &[])?,
            Scope::Starred => query_notes(conn, &format!("WHERE {LIVE} AND n.starred = 1"), &[])?,
            Scope::Archived => query_notes(conn, "WHERE n.archived = 1 AND n.deleted = 0 AND n.purged = 0", &[])?,
            Scope::Trash => query_notes(conn, "WHERE n.deleted = 1 AND n.purged = 0", &[])?,
            Scope::Untagged => query_notes(
                conn,
                &format!("WHERE {LIVE} AND NOT EXISTS (SELECT 1 FROM note_tags t WHERE t.note_id = n.id)"),
                &[],
            )?,
            Scope::NoFolder => query_notes(conn, &format!("WHERE {LIVE} AND n.eff_folder IS NULL"), &[])?,
            Scope::Folder { id } => {
                sort = get_folder(conn, id)?.sort;
                query_notes(conn, &format!("WHERE {LIVE} AND n.eff_folder = ?1"), &[id])?
            }
            Scope::Tag { name } => query_notes(
                conn,
                &format!("WHERE {LIVE} AND EXISTS (SELECT 1 FROM note_tags t WHERE t.note_id = n.id AND t.key = ?1)"),
                &[&tag::key(name.trim())],
            )?,
            Scope::Filter { id } => {
                let query = filter_query(conn, id)?;
                filter_notes(&inner, &query)?
            }
        };
        if scope == Scope::Trash {
            // R46: the note that has been there least is the one with the most days left.
            notes.sort_by(|a, b| b.deleted_ms.cmp(&a.deleted_ms).then_with(|| a.id.cmp(&b.id)));
        } else {
            sort_notes(&mut notes, sort);
        }
        Ok(notes)
    }

    /// A note that is not deleted for good, in the Trash or not.
    pub fn get_note(&self, id: String) -> Result<Note> {
        get_note(&self.lock().conn, &id)
    }

    /// What the system views of the sidebar hold (R44).
    pub fn counts(&self) -> Result<Counts> {
        const LIVE: &str = "n.deleted = 0 AND n.purged = 0 AND n.archived = 0";
        let inner = self.lock();
        let count = |condition: &str| -> Result<u32> {
            Ok(inner
                .conn
                .query_row(&format!("SELECT count(*) FROM notes n WHERE {condition}"), [], |r| {
                    r.get(0)
                })?)
        };
        Ok(Counts {
            all: count(LIVE)?,
            starred: count(&format!("{LIVE} AND n.starred = 1"))?,
            archived: count("n.archived = 1 AND n.deleted = 0 AND n.purged = 0")?,
            trash: count("n.deleted = 1 AND n.purged = 0")?,
            untagged: count(&format!(
                "{LIVE} AND NOT EXISTS (SELECT 1 FROM note_tags t WHERE t.note_id = n.id)"
            ))?,
            no_folder: count(&format!("{LIVE} AND n.eff_folder IS NULL"))?,
        })
    }

    /// R2: the folder a note goes to when it is created from a view that names none, by `new_note_folder`.
    /// `last_used` is the folder this device created a note in last.
    pub fn default_folder_id(&self, last_used: Option<String>) -> Result<Option<String>> {
        default_folder_id(&self.lock().conn, last_used.as_deref())
    }

    /// R1, R2: a new note in the folder, with the tags and the text it starts with. S3: the document
    /// of the text is made here, by the device that creates the note.
    pub fn create_note(&self, new: NewNote) -> Result<Note> {
        let tags = new.tags.iter().map(|t| tag::clean(t)).collect::<Result<Vec<_>>>()?;
        self.act(|w| {
            if let Some(folder) = &new.folder_id {
                if !folder_is_live(w.tx, folder)? {
                    return Err(AppError::not_found(format!("folder {folder}")));
                }
            }
            let id = new_id();
            w.note(&id, "title", json!(new.title))?;
            w.note(&id, "created", json!(w.now_ms()))?;
            if let Some(folder) = &new.folder_id {
                w.note(&id, "folder", json!(folder))?;
            }
            crate::body::create(w, &id, &new.text)?;
            w.flush()?;
            for name in &tags {
                add_tag_in(w, &id, name)?;
                w.flush()?;
            }
            get_note(w.tx, &id)
        })
    }

    /// R4: a copy with " (copy)" after the title, in the same folder, with the same tags and copies of the
    /// attachments, which refer to the same content. The flags are not copied.
    pub fn duplicate_note(&self, id: String) -> Result<Note> {
        self.act(|w| {
            let original = get_note(w.tx, &id)?;
            let text = crate::body::text(w.tx, &id)?;
            let copy = new_id();
            w.note(&copy, "title", json!(format!("{} (copy)", original.title).trim()))?;
            w.note(&copy, "created", json!(w.now_ms()))?;
            if let Some(folder) = &original.folder_id {
                w.note(&copy, "folder", json!(folder))?;
            }
            crate::body::create(w, &copy, &text)?;
            w.flush()?;
            for name in &original.tags {
                add_tag_in(w, &copy, name)?;
            }
            let mut stmt = w.tx.prepare(
                "SELECT name, mime, size, sha256 FROM attachments WHERE note_id = ?1 AND deleted = 0 ORDER BY id",
            )?;
            let files: Vec<(String, String, i64, String)> = stmt
                .query_map([&id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
                .collect::<rusqlite::Result<_>>()?;
            drop(stmt);
            for (name, mime, size, sha256) in files {
                let file = new_id();
                w.set(KIND_ATTACHMENT, &file, "note", json!(copy))?;
                w.set(KIND_ATTACHMENT, &file, "name", json!(name))?;
                w.set(KIND_ATTACHMENT, &file, "mime", json!(mime))?;
                w.set(KIND_ATTACHMENT, &file, "size", json!(size))?;
                w.set(KIND_ATTACHMENT, &file, "sha256", json!(sha256))?;
            }
            w.flush()?;
            get_note(w.tx, &copy)
        })
    }

    /// R28: refused for a locked note.
    pub fn set_title(&self, id: String, title: String) -> Result<()> {
        self.act_on(Some((id.clone(), "title".into())), |w| {
            if get_unlocked_note(w.tx, &id)?.title != title {
                w.note(&id, "title", json!(title))?;
            }
            Ok(())
        })
    }

    /// R25
    pub fn set_pinned(&self, id: String, pinned: bool) -> Result<()> {
        self.flag(id, "pinned", pinned)
    }

    /// R26
    pub fn set_starred(&self, id: String, starred: bool) -> Result<()> {
        self.flag(id, "starred", starred)
    }

    /// R27
    pub fn set_archived(&self, id: String, archived: bool) -> Result<()> {
        self.flag(id, "archived", archived)
    }

    /// R28: locking and unlocking stay possible on a locked note.
    pub fn set_locked(&self, id: String, locked: bool) -> Result<()> {
        self.flag(id, "locked", locked)
    }

    /// R11: moves the note to the folder, or to the root without one. Moving a locked note stays possible.
    pub fn move_note(&self, id: String, folder_id: Option<String>) -> Result<()> {
        self.act(|w| {
            get_note(w.tx, &id)?;
            if let Some(folder) = &folder_id {
                if !folder_is_live(w.tx, folder)? {
                    return Err(AppError::not_found(format!("folder {folder}")));
                }
            }
            w.note(&id, "folder", opt(folder_id))
        })
    }

    // ---- the Trash ----

    /// S6: the note goes to the Trash with the moment it went. A note already there keeps its moment.
    pub fn delete_note(&self, id: String) -> Result<()> {
        self.act(|w| {
            if !get_note(w.tx, &id)?.deleted {
                w.note(&id, "deleted", json!(w.now_ms()))?;
            }
            Ok(())
        })
    }

    /// S6, S10: takes the note out of the Trash, and its folder and the folders above it when they are in
    /// it as well. The other notes of those folders stay where they are.
    pub fn restore_note(&self, id: String) -> Result<()> {
        self.act(|w| {
            if !get_note(w.tx, &id)?.deleted {
                return Ok(());
            }
            w.note(&id, "deleted", Value::Null)?;
            let mut next: Option<String> =
                w.tx.query_row("SELECT folder_id FROM notes WHERE id = ?1", [&id], |r| r.get(0))?;
            let mut seen = std::collections::HashSet::new();
            while let Some(folder) = next.filter(|f| seen.insert(f.clone())) {
                let row: Option<(bool, Option<String>)> =
                    w.tx.query_row(
                        "SELECT deleted, tree_parent FROM folders WHERE id = ?1",
                        [&folder],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .optional()?;
                let Some((deleted, parent)) = row else { break };
                if deleted {
                    w.folder(&folder, "deleted", Value::Null)?;
                }
                next = parent;
            }
            Ok(())
        })
    }

    /// S7: deletes the note for good. There is no way back: the other devices keep it deleted whatever
    /// they do with it meanwhile. Its attachments are no longer needed (S26).
    pub fn purge_note(&self, id: String) -> Result<()> {
        self.write(|w| {
            get_note(w.tx, &id)?;
            w.note(&id, "purged", json!(true))
        })
    }

    /// R46: deletes for good everything in the Trash. Returns how many notes were deleted.
    pub fn empty_trash(&self) -> Result<u32> {
        self.write(|w| {
            let mut stmt = w.tx.prepare("SELECT id FROM notes WHERE deleted = 1 AND purged = 0")?;
            let ids: Vec<String> = stmt.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
            drop(stmt);
            for id in &ids {
                w.note(id, "purged", json!(true))?;
            }
            Ok(ids.len() as u32)
        })
    }

    // ---- folders ----

    /// The folders that are not in the Trash, in the order of their `pos` (S11); the tree is read from
    /// `parent_id` (S9, S10).
    pub fn folders(&self) -> Result<Vec<Folder>> {
        live_folders(&self.lock().conn)
    }

    pub fn folder(&self, id: String) -> Result<Folder> {
        get_folder(&self.lock().conn, &id)
    }

    /// R10, R15: a folder at the end of its parent, or of the root.
    pub fn create_folder(&self, name: String, parent_id: Option<String>) -> Result<Folder> {
        let name = folder_name(&name)?;
        self.write(|w| {
            if let Some(parent) = &parent_id {
                get_folder(w.tx, parent)?;
            }
            let id = new_id();
            let pos = order::between(last_folder_pos(w.tx, parent_id.as_deref())?.as_deref(), None);
            w.folder(&id, "name", json!(name))?;
            w.folder(&id, "pos", json!(pos))?;
            if let Some(parent) = parent_id {
                w.folder(&id, "parent", json!(parent))?;
            }
            w.flush()?;
            get_folder(w.tx, &id)
        })
    }

    pub fn rename_folder(&self, id: String, name: String) -> Result<()> {
        let name = folder_name(&name)?;
        self.folder_field(id, "name", json!(name))
    }

    pub fn set_folder_icon(&self, id: String, icon: String) -> Result<()> {
        self.folder_field(id, "icon", json!(icon))
    }

    /// R17
    pub fn set_folder_daily(&self, id: String, daily: bool) -> Result<()> {
        self.folder_field(id, "daily", json!(daily))
    }

    /// R16
    pub fn set_folder_sort(&self, id: String, sort: FolderSort) -> Result<()> {
        self.folder_field(id, "sort", json!(sort.as_string()))
    }

    /// R16
    pub fn set_folder_display(&self, id: String, show_preview: bool, show_date: bool) -> Result<()> {
        self.act(|w| {
            get_folder(w.tx, &id)?;
            w.folder(&id, "show_preview", json!(show_preview))?;
            w.folder(&id, "show_date", json!(show_date))
        })
    }

    /// R11, S11: moves the folder under `parent_id` (the root without one), right after `after`, or first
    /// among its brothers without it. A folder cannot go into itself or into its own subfolder.
    pub fn move_folder(&self, id: String, parent_id: Option<String>, after: Option<String>) -> Result<()> {
        self.act(|w| {
            get_folder(w.tx, &id)?;
            if let Some(parent) = &parent_id {
                get_folder(w.tx, parent)?;
                if folder_subtree(w.tx, &id, false)?.contains(parent) {
                    return Err(AppError::invalid(
                        "a folder cannot be moved into itself or its own subfolder",
                    ));
                }
            }
            heal_tree(w)?;
            let siblings = folder_positions(w.tx, parent_id.as_deref(), &id)?;
            w.folder(&id, "pos", json!(pos_after(&siblings, after.as_deref())))?;
            w.folder(&id, "parent", opt(parent_id))
        })
    }

    /// R14: how many notes deleting the folder would send to the Trash, so that the app can name the number.
    pub fn folder_deletion_count(&self, id: String) -> Result<u32> {
        let inner = self.lock();
        get_folder(&inner.conn, &id)?;
        notes_of_folders(&inner.conn, &folder_subtree(&inner.conn, &id, true)?).map(|notes| notes.len() as u32)
    }

    /// S10, R14: the folder, its subfolders and all their notes go to the Trash in one change. Folders and
    /// notes that are there already keep their moment. Returns how many notes went.
    pub fn delete_folder(&self, id: String) -> Result<u32> {
        self.act(|w| {
            get_folder(w.tx, &id)?;
            let folders = folder_subtree(w.tx, &id, true)?;
            let notes = notes_of_folders(w.tx, &folders)?;
            let now = w.now_ms();
            for folder in &folders {
                w.folder(folder, "deleted", json!(now))?;
            }
            for note in &notes {
                w.note(note, "deleted", json!(now))?;
            }
            Ok(notes.len() as u32)
        })
    }

    // ---- tags ----

    /// R22: the tags of the notes that are neither deleted nor archived, with the number of notes, by name.
    /// Names that differ in case are one tag, shown as it was written first (R21).
    pub fn tags(&self) -> Result<Vec<TagCount>> {
        let inner = self.lock();
        let mut stmt = inner.conn.prepare(
            "SELECT t.key, count(*) FROM note_tags t JOIN notes n ON n.id = t.note_id
             WHERE n.deleted = 0 AND n.purged = 0 AND n.archived = 0 GROUP BY t.key",
        )?;
        let counts: Vec<(String, u32)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        let mut tags = Vec::new();
        for (key, count) in counts {
            let name = spelling(&inner.conn, &key)?.unwrap_or_else(|| key.clone());
            tags.push(TagCount { name, count });
        }
        tags.sort_by(|a, b| (tag::key(&a.name), &a.name).cmp(&(tag::key(&b.name), &b.name)));
        Ok(tags)
    }

    /// R20, R21: adds a tag to the note. A note that has the tag already, in any case, keeps it as it is. A
    /// tag that exists on other notes is spelled as it was first written. R28: refused for a locked note.
    pub fn add_tag(&self, id: String, tag: String) -> Result<()> {
        let name = tag::clean(&tag)?;
        self.act(|w| {
            get_unlocked_note(w.tx, &id)?;
            add_tag_in(w, &id, &name)
        })
    }

    /// R20: takes the tag off the note, whatever the case. R28: refused for a locked note.
    pub fn remove_tag(&self, id: String, tag: String) -> Result<()> {
        let key = tag::key(&tag::clean(&tag)?);
        self.act(|w| {
            get_unlocked_note(w.tx, &id)?;
            remove_tag_in(w, &id, &key)
        })
    }

    /// R22: renames the tag on every note; a new name that is a tag already merges the two. Returns how
    /// many notes had the tag.
    pub fn rename_tag(&self, old: String, new: String) -> Result<u32> {
        let (old, new) = (tag::key(&tag::clean(&old)?), tag::clean(&new)?);
        self.write(|w| {
            // A change of case alone is the new spelling; a name that is a tag already is spelled as it is.
            let name = if tag::key(&new) == old {
                new
            } else {
                spelling(w.tx, &tag::key(&new))?.unwrap_or(new)
            };
            let ids = notes_with_tag(w.tx, &old)?;
            for id in &ids {
                remove_tag_in(w, id, &old)?;
                w.flush()?;
                put_tag(w, id, &name)?;
                w.flush()?;
            }
            Ok(ids.len() as u32)
        })
    }

    /// R22: takes the tag off every note. Returns how many notes had it.
    pub fn delete_tag(&self, name: String) -> Result<u32> {
        let key = tag::key(&tag::clean(&name)?);
        self.write(|w| {
            let ids = notes_with_tag(w.tx, &key)?;
            for id in &ids {
                remove_tag_in(w, id, &key)?;
            }
            Ok(ids.len() as u32)
        })
    }

    // ---- filters ----

    /// R45: the saved filters in their order (S11), each with the number of notes it selects now.
    pub fn filters(&self) -> Result<Vec<SavedFilter>> {
        let inner = self.lock();
        let mut stmt = inner
            .conn
            .prepare_cached("SELECT id, name, icon, query FROM filters WHERE deleted = 0 ORDER BY pos, id")?;
        let rows: Vec<(String, String, String, String)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
            .collect::<rusqlite::Result<_>>()?;
        rows.into_iter()
            .map(|(id, name, icon, query)| {
                let query: FilterQuery = serde_json::from_str(&query).unwrap_or_default();
                let count = filter_notes(&inner, &query)?.len() as u32;
                Ok(SavedFilter {
                    id,
                    name,
                    icon,
                    query,
                    count,
                })
            })
            .collect()
    }

    /// What a filter would show, without saving it.
    pub fn preview_filter(&self, query: FilterQuery) -> Result<Vec<Note>> {
        check_query(&query)?;
        let inner = self.lock();
        let mut notes = filter_notes(&inner, &query)?;
        sort_notes(&mut notes, FolderSort::default());
        Ok(notes)
    }

    pub fn create_filter(&self, name: String, icon: String, query: FilterQuery) -> Result<SavedFilter> {
        let name = name.trim().to_string();
        if name.is_empty() {
            return Err(AppError::invalid("filter name is empty"));
        }
        check_query(&query)?;
        let id = self.write(|w| {
            let id = new_id();
            let last = last_pos(w.tx, "SELECT max(pos) FROM filters WHERE deleted = 0", &[])?;
            w.set(KIND_FILTER, &id, "name", json!(name))?;
            w.set(KIND_FILTER, &id, "icon", json!(icon))?;
            w.set(KIND_FILTER, &id, "query", serde_json::to_value(&query)?)?;
            w.set(KIND_FILTER, &id, "pos", json!(order::between(last.as_deref(), None)))?;
            Ok(id)
        })?;
        self.filters()?
            .into_iter()
            .find(|f| f.id == id)
            .ok_or_else(|| AppError::not_found("filter"))
    }

    pub fn update_filter(&self, id: String, name: String, icon: String, query: FilterQuery) -> Result<()> {
        let name = name.trim().to_string();
        if name.is_empty() {
            return Err(AppError::invalid("filter name is empty"));
        }
        check_query(&query)?;
        self.write(|w| {
            let (old_name, old_icon, old_query): (String, String, String) =
                w.tx.query_row(
                    "SELECT name, icon, query FROM filters WHERE id = ?1 AND deleted = 0",
                    [&id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .optional()?
                .ok_or_else(|| AppError::not_found(format!("filter {id}")))?;
            if old_name != name {
                w.set(KIND_FILTER, &id, "name", json!(name))?;
            }
            if old_icon != icon {
                w.set(KIND_FILTER, &id, "icon", json!(icon))?;
            }
            if serde_json::from_str::<FilterQuery>(&old_query).ok().as_ref() != Some(&query) {
                w.set(KIND_FILTER, &id, "query", serde_json::to_value(&query)?)?;
            }
            Ok(())
        })
    }

    /// S11: places the filter right after `after`, or first.
    pub fn move_filter(&self, id: String, after: Option<String>) -> Result<()> {
        self.write(|w| {
            filter_query(w.tx, &id)?;
            let mut stmt =
                w.tx.prepare("SELECT id, pos FROM filters WHERE deleted = 0 AND id != ?1 ORDER BY pos, id")?;
            let siblings: Vec<(String, String)> = stmt
                .query_map([&id], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<_>>()?;
            drop(stmt);
            w.set(KIND_FILTER, &id, "pos", json!(pos_after(&siblings, after.as_deref())))
        })
    }

    pub fn delete_filter(&self, id: String) -> Result<()> {
        self.write(|w| {
            filter_query(w.tx, &id)?;
            w.set(KIND_FILTER, &id, "deleted", json!(true))
        })
    }

    // ---- settings ----

    /// S12: the settings shared by every device.
    pub fn settings(&self) -> Result<AppSettings> {
        read_settings(&self.lock().conn)
    }

    /// S12: saves the settings. Only the fields that differ from what is in force are written, so that
    /// a device that changes one does not take the others from the other devices.
    pub fn set_settings(&self, settings: AppSettings) -> Result<()> {
        if !(1..=MAX_TRASH_DAYS).contains(&settings.trash_days) {
            return Err(AppError::invalid(format!(
                "the days in the Trash are from 1 to {MAX_TRASH_DAYS}"
            )));
        }
        let finite = settings.line_height.is_finite() && settings.paragraph_spacing.is_finite();
        let named = !settings.font_body.is_empty()
            && !settings.font_mono.is_empty()
            && !settings.spell_language.is_empty()
            && !settings.new_note_folder.is_empty();
        if !finite || !named {
            return Err(AppError::invalid("a setting is empty or not a number"));
        }
        self.write(|w| {
            let now = read_settings(w.tx)?;
            let align = |a: TextAlign| json!(if a == TextAlign::Justify { "justify" } else { "left" });
            let fields = [
                (
                    "trash_days",
                    settings.trash_days != now.trash_days,
                    json!(settings.trash_days),
                ),
                (
                    "font_body",
                    settings.font_body != now.font_body,
                    json!(settings.font_body),
                ),
                (
                    "font_mono",
                    settings.font_mono != now.font_mono,
                    json!(settings.font_mono),
                ),
                (
                    "line_height",
                    settings.line_height != now.line_height,
                    json!(settings.line_height),
                ),
                (
                    "paragraph_spacing",
                    settings.paragraph_spacing != now.paragraph_spacing,
                    json!(settings.paragraph_spacing),
                ),
                ("align", settings.align != now.align, align(settings.align)),
                (
                    "hyphenate",
                    settings.hyphenate != now.hyphenate,
                    json!(settings.hyphenate),
                ),
                (
                    "smart_typography",
                    settings.smart_typography != now.smart_typography,
                    json!(settings.smart_typography),
                ),
                (
                    "spellcheck",
                    settings.spellcheck != now.spellcheck,
                    json!(settings.spellcheck),
                ),
                (
                    "spell_language",
                    settings.spell_language != now.spell_language,
                    json!(settings.spell_language),
                ),
                (
                    "new_note_folder",
                    settings.new_note_folder != now.new_note_folder,
                    json!(settings.new_note_folder),
                ),
            ];
            for (field, changed, value) in fields {
                if changed {
                    w.set(KIND_SETTINGS, SETTINGS_ID, field, value)?;
                }
            }
            Ok(())
        })
    }

    // ---- attachments ----

    pub fn attachments(&self, note_id: String) -> Result<Vec<Attachment>> {
        let inner = self.lock();
        let mut stmt = inner.conn.prepare_cached(
            "SELECT id, note_id, name, mime, size, sha256 FROM attachments WHERE note_id = ?1 AND deleted = 0 ORDER BY id",
        )?;
        let rows = stmt.query_map([&note_id], |r| {
            let sha256: String = r.get(5)?;
            let path = self.blob_path(&sha256);
            let present = db::is_sha256(&sha256) && path.exists();
            let (mut name, mut mime): (String, String) = (r.get(2)?, r.get(3)?);
            // R85 of Lists: a name that tells no type is helped by the content, when the content is here.
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
                note_id: r.get(1)?,
                name,
                mime,
                size: r.get(4)?,
                local_path: present.then(|| path.to_string_lossy().into_owned()),
                sha256,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// R36: copies the file at `path` into the store and attaches it to the note.
    /// The content is read and written in pieces, so memory does not grow with the file.
    pub fn add_attachment(&self, note_id: String, path: String, name: Option<String>) -> Result<Attachment> {
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
            // Asked before the file is copied: an unknown note, a locked note and a full note cost nothing.
            {
                let conn = self.lock();
                get_unlocked_note(&conn.conn, &note_id)?;
                let used = note_attachment_bytes(&conn.conn, &note_id)?;
                if over(limits.max_note, used.saturating_add(expected)) {
                    return Err(too_large_note(used, expected, limits.max_note));
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
                get_unlocked_note(w.tx, &note_id)?;
                // Asked again with the size the file really had: it may have grown while it was copied.
                let used = note_attachment_bytes(w.tx, &note_id)?;
                if over(limits.max_note, used.saturating_add(size)) {
                    return Err(too_large_note(used, size, limits.max_note));
                }
                let id = new_id();
                w.set(KIND_ATTACHMENT, &id, "note", json!(note_id))?;
                w.set(KIND_ATTACHMENT, &id, "name", json!(name))?;
                w.set(KIND_ATTACHMENT, &id, "mime", json!(guess_mime(&name)))?;
                w.set(KIND_ATTACHMENT, &id, "size", json!(size))?;
                w.set(KIND_ATTACHMENT, &id, "sha256", json!(sha256))?;
                Ok(id)
            });
            let id = match written {
                Ok(id) => id,
                Err(e) => {
                    // The content came with this call and the note did not take it: it is left to no one.
                    if created {
                        self.drop_blob_if_unused(&sha256);
                    }
                    return Err(e);
                }
            };
            self.attachments(note_id)?
                .into_iter()
                .find(|a| a.id == id)
                .ok_or_else(|| AppError::not_found("attachment"))
        })();
        // R95: the path and the name stay out of the log; the reason goes in.
        if let Err(e) = &added {
            self.note(LogLevel::Error, "attachments", || format!("not added: {e}"));
        }
        added
    }

    /// R36: the limits this device takes.
    pub fn attachment_limits(&self) -> AttachmentLimits {
        *self.limits.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// R36: sets the limits of this device. Zero means no limit. Kept in the database of this
    /// device, not in the shared registers, so it does not travel to other devices.
    pub fn set_attachment_limits(&self, limits: AttachmentLimits) -> Result<()> {
        {
            let conn = self.lock();
            db::meta_set(&conn.conn, MAX_FILE_KEY, &limits.max_file.to_string())?;
            db::meta_set(&conn.conn, MAX_NOTE_KEY, &limits.max_note.to_string())?;
        }
        *self.limits.lock().unwrap_or_else(|p| p.into_inner()) = limits;
        Ok(())
    }

    /// R36: a blob this call wrote is removed when no attachment kept it, as after a failed add.
    pub(crate) fn drop_blob_if_unused(&self, sha256: &str) {
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

    /// R33: removes the attachment from its note. R28: refused for a locked note.
    pub fn remove_attachment(&self, id: String) -> Result<()> {
        self.act(|w| {
            let note: Option<String> =
                w.tx.query_row(
                    "SELECT note_id FROM attachments WHERE id = ?1 AND deleted = 0",
                    [&id],
                    |r| r.get(0),
                )
                .optional()?;
            let note = note.ok_or_else(|| AppError::not_found("attachment"))?;
            // A note not known here yet cannot be locked here.
            if get_note(w.tx, &note).is_ok_and(|n| n.locked) {
                return Err(AppError::invalid("the note is locked"));
            }
            w.set(KIND_ATTACHMENT, &id, "deleted", json!(true))
        })
    }

    /// Writes every attachment of the note into the folder under its own name.
    /// Nothing in the folder is replaced: a name that is taken gets a number. Content that has
    /// not arrived is downloaded first (R35); what cannot be had or written is named in the
    /// result and does not stop the rest.
    pub fn save_attachments(&self, note_id: String, dir: String) -> Result<AttachmentsSaved> {
        let dir = Path::new(&dir);
        let mut report = AttachmentsSaved::default();
        for mut file in self.attachments(note_id)? {
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

impl Store {
    /// A switch of a note: the register is written only when it changes.
    fn flag(&self, id: String, field: &str, value: bool) -> Result<()> {
        self.act(|w| {
            let note = get_note(w.tx, &id)?;
            let now = match field {
                "pinned" => note.pinned,
                "starred" => note.starred,
                "archived" => note.archived,
                _ => note.locked,
            };
            if now != value {
                w.note(&id, field, json!(value))?;
            }
            Ok(())
        })
    }

    fn folder_field(&self, id: String, field: &str, value: Value) -> Result<()> {
        self.act(|w| {
            get_folder(w.tx, &id)?;
            w.folder(&id, field, value)
        })
    }
}

fn opt(value: Option<String>) -> Value {
    value.filter(|v| !v.is_empty()).map_or(Value::Null, Value::String)
}

/// R15: a name is not empty and not longer than 200 characters.
pub(crate) fn folder_name(name: &str) -> Result<String> {
    let name = name.trim();
    if name.is_empty() {
        return Err(AppError::invalid("folder name is empty"));
    }
    if name.chars().count() > MAX_FOLDER_NAME {
        return Err(AppError::invalid(format!(
            "folder name is longer than {MAX_FOLDER_NAME} characters"
        )));
    }
    Ok(name.to_string())
}

const MAX_FOLDER_NAME: usize = 200;

/// The last `pos` among the folders shown under `parent`, or at the root.
pub(crate) fn last_folder_pos(conn: &Connection, parent: Option<&str>) -> Result<Option<String>> {
    Ok(conn.query_row(
        "SELECT max(pos) FROM folders WHERE deleted = 0 AND eff_parent IS ?1",
        [parent],
        |r| r.get(0),
    )?)
}

/// The brothers of a folder in their order, without the folder itself.
fn folder_positions(conn: &Connection, parent: Option<&str>, except: &str) -> Result<Vec<(String, String)>> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, pos FROM folders WHERE deleted = 0 AND eff_parent IS ?1 AND id != ?2 ORDER BY pos, id",
    )?;
    let rows = stmt.query_map(params![parent, except], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Position right after `after` in an ordered list of brothers, or before the first one (S11).
fn pos_after(siblings: &[(String, String)], after: Option<&str>) -> String {
    let index = after.and_then(|a| siblings.iter().position(|(id, _)| id == a));
    let (lo, hi) = match index {
        Some(i) => (Some(siblings[i].1.as_str()), siblings.get(i + 1).map(|s| s.1.as_str())),
        None => (None, siblings.first().map(|s| s.1.as_str())),
    };
    order::between(lo, hi)
}

/// The folder and everything below it in the tree, with or without the ones in the Trash (S9, S10).
fn folder_subtree(conn: &Connection, id: &str, only_live: bool) -> Result<Vec<String>> {
    let mut stmt = conn.prepare_cached(
        "WITH RECURSIVE sub (id) AS (
            SELECT ?1
            UNION ALL
            SELECT f.id FROM folders f JOIN sub ON f.tree_parent = sub.id WHERE (?2 = 0 OR f.deleted = 0)
         ) SELECT id FROM sub",
    )?;
    let rows = stmt.query_map(params![id, only_live], |r| r.get(0))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// The notes of these folders that are not in the Trash and not deleted for good, archived or not.
fn notes_of_folders(conn: &Connection, folders: &[String]) -> Result<Vec<String>> {
    let mut stmt = conn.prepare_cached("SELECT id FROM notes WHERE folder_id = ?1 AND deleted = 0 AND purged = 0")?;
    let mut notes = Vec::new();
    for folder in folders {
        let rows = stmt.query_map([folder], |r| r.get::<_, String>(0))?;
        for row in rows {
            notes.push(row?);
        }
    }
    Ok(notes)
}

/// Writes down the cuts that `rebuild_folders` made to break cycles (S9).
///
/// A cut is only a way of reading the stored parents. Without this, moving one
/// folder of a former cycle would bring the suppressed parent of another back to
/// life and rearrange the tree behind the user's back.
fn heal_tree(w: &mut Writer) -> Result<()> {
    let mut stmt = w.tx.prepare(
        "SELECT id FROM folders WHERE parent_id IS NOT NULL AND tree_parent IS NULL AND parent_id != id
           AND parent_id IN (SELECT id FROM folders)",
    )?;
    let cut: Vec<String> = stmt.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
    drop(stmt);
    w.unnoted(|w| {
        for id in &cut {
            w.folder(id, "parent", Value::Null)?;
        }
        if !cut.is_empty() {
            w.flush()?;
        }
        Ok(())
    })
}

/// R21: the spelling of a tag as it was first written on a note that is not deleted for good.
pub(crate) fn spelling(conn: &Connection, key: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT t.tag FROM note_tags t JOIN notes n ON n.id = t.note_id
             WHERE t.key = ?1 AND n.purged = 0 ORDER BY t.stamp, t.tag LIMIT 1",
            [key],
            |r| r.get(0),
        )
        .optional()?)
}

/// Notes that are not deleted for good and carry the tag.
fn notes_with_tag(conn: &Connection, key: &str) -> Result<Vec<String>> {
    let mut stmt = conn.prepare_cached(
        "SELECT t.note_id FROM note_tags t JOIN notes n ON n.id = t.note_id
         WHERE t.key = ?1 AND n.purged = 0 ORDER BY t.note_id",
    )?;
    let rows = stmt.query_map([key], |r| r.get(0))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// R21: puts the tag on the note unless it has it already, in any case. The name is spelled as the tag is
/// elsewhere when it exists, as it was typed otherwise.
fn add_tag_in(w: &mut Writer, id: &str, name: &str) -> Result<()> {
    let name = spelling(w.tx, &tag::key(name))?.unwrap_or_else(|| name.to_string());
    put_tag(w, id, &name)
}

/// Puts the tag on the note under exactly this spelling, unless the note has the tag already.
fn put_tag(w: &mut Writer, id: &str, name: &str) -> Result<()> {
    let has: bool = w.tx.query_row(
        "SELECT EXISTS (SELECT 1 FROM note_tags WHERE note_id = ?1 AND key = ?2)",
        params![id, tag::key(name)],
        |r| r.get(0),
    )?;
    if has {
        return Ok(());
    }
    w.note(id, &tag::require_field(name)?, json!(true))
}

/// Switches off every register of the note that carries the tag, whatever its case.
fn remove_tag_in(w: &mut Writer, id: &str, key: &str) -> Result<()> {
    let mut stmt =
        w.tx.prepare("SELECT field, value FROM fields WHERE kind = ?1 AND id = ?2 AND field LIKE 'tag:%'")?;
    let on: Vec<String> = stmt
        .query_map(params![KIND_NOTE, id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?
        .filter_map(|row| match row {
            Ok((field, value)) => {
                let name = field.strip_prefix(tag::PREFIX)?;
                (value == "true" && tag::key(name) == key).then(|| Ok(field))
            }
            Err(e) => Some(Err(e)),
        })
        .collect::<rusqlite::Result<_>>()?;
    drop(stmt);
    for field in on {
        w.note(id, &field, json!(false))?;
    }
    Ok(())
}

fn filter_query(conn: &Connection, id: &str) -> Result<FilterQuery> {
    let query: Option<String> = conn
        .query_row("SELECT query FROM filters WHERE id = ?1 AND deleted = 0", [id], |r| {
            r.get(0)
        })
        .optional()?;
    let query = query.ok_or_else(|| AppError::not_found(format!("filter {id}")))?;
    Ok(serde_json::from_str(&query).unwrap_or_default())
}

const DATE_FMT: &str = "%Y-%m-%d";

/// A date a condition names has to read.
fn check_query(query: &FilterQuery) -> Result<()> {
    for condition in &query.conditions {
        if let Condition::Created { when } | Condition::Edited { when } = condition {
            if let DateTest::Before { date } | DateTest::After { date } = when {
                if NaiveDate::parse_from_str(date, DATE_FMT).is_err() {
                    return Err(AppError::invalid(format!("bad date: {date}")));
                }
            }
        }
    }
    Ok(())
}

/// R45: the notes a filter selects, not in the Trash and not archived unless the filter asks for archived
/// ones (R27). A filter without conditions selects all of them.
fn filter_notes(inner: &Inner, query: &FilterQuery) -> Result<Vec<Note>> {
    let conn = &inner.conn;
    let asks_archived = query.conditions.contains(&Condition::Archived);
    let tail = if asks_archived {
        "WHERE n.deleted = 0 AND n.purged = 0"
    } else {
        "WHERE n.deleted = 0 AND n.purged = 0 AND n.archived = 0"
    };
    let notes = query_notes(conn, tail, &[])?;
    // R45: read only when a condition asks for it.
    let texts = if query
        .conditions
        .iter()
        .any(|c| matches!(c, Condition::TextContains { .. }))
    {
        crate::body::texts(conn)?
    } else {
        HashMap::new()
    };
    let mut subtrees: HashMap<&str, Vec<String>> = HashMap::new();
    for condition in &query.conditions {
        if let Condition::InFolder {
            folder_id,
            subfolders: true,
        } = condition
        {
            subtrees.insert(folder_id, folder_subtree(conn, folder_id, true)?);
        }
    }
    let now = inner.now_ms();
    let day_start = |date: &str| -> Option<u64> {
        let day = NaiveDate::parse_from_str(date, DATE_FMT).ok()?;
        Some(inner.ms_of(day.and_time(chrono::NaiveTime::MIN)))
    };
    let on_time = |when: &DateTest, ms: u64| -> bool {
        match when {
            DateTest::Before { date } => day_start(date).is_some_and(|start| ms < start),
            DateTest::After { date } => NaiveDate::parse_from_str(date, DATE_FMT)
                .ok()
                .and_then(|day| day.succ_opt())
                .and_then(|next| day_start(&next.format(DATE_FMT).to_string()))
                .is_some_and(|start| ms >= start),
            DateTest::Within { days } => ms >= now.saturating_sub(u64::from(*days) * MS_PER_DAY),
        }
    };
    let holds = |note: &Note, condition: &Condition| -> bool {
        let has = |name: &str| note.tags.iter().any(|t| tag::key(t) == tag::key(name));
        match condition {
            Condition::InFolder { folder_id, subfolders } => match &note.folder_id {
                None => false,
                Some(folder) if folder == folder_id => true,
                Some(folder) => *subfolders && subtrees.get(folder_id.as_str()).is_some_and(|ids| ids.contains(folder)),
            },
            Condition::HasTag { name } => has(name),
            Condition::NoTags => note.tags.is_empty(),
            Condition::NoneOfTags { names } => !names.iter().any(|name| has(name)),
            Condition::Pinned => note.pinned,
            Condition::Starred => note.starred,
            Condition::Archived => note.archived,
            Condition::HasAttachments => note.attachments > 0,
            Condition::TitleIs { text } => note.title.to_lowercase() == text.to_lowercase(),
            Condition::TitleContains { text } => note.title.to_lowercase().contains(&text.to_lowercase()),
            Condition::TextContains { text } => texts
                .get(&note.id)
                .is_some_and(|body| body.to_lowercase().contains(&text.to_lowercase())),
            Condition::Created { when } => on_time(when, note.created_ms),
            Condition::Edited { when } => on_time(when, note.edited_ms),
        }
    };
    Ok(notes
        .into_iter()
        .filter(|note| {
            query.conditions.is_empty()
                || match query.join {
                    FilterJoin::All => query.conditions.iter().all(|c| holds(note, c)),
                    FilterJoin::Any => query.conditions.iter().any(|c| holds(note, c)),
                }
        })
        .collect())
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

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
