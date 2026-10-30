//! The text of a note (S3, S70): one Automerge document per note in `bodies`, outside the
//! registers of `fields`. The document has one text object `body` at its root that holds the
//! Markdown source as typed. Documents are created and loaded with the UTF-16 text encoding, so
//! the positions of the platforms go in without conversion, and every document the core opens
//! gets the actor identifier of this device (S4).
//!
//! What the library does not do is done here: a position inside a surrogate pair or past the end
//! is refused, every commit carries the clock of the device and never a time below the latest in
//! the document, and a conflict between two `body` objects is settled the same way everywhere.

use std::collections::HashMap;

use automerge::transaction::{CommitOptions, Transactable};
use automerge::{
    ActorId, AutoCommit, Automerge, AutomergeError, ChangeHash, LoadOptions, ObjId, ObjType, OnPartialLoad, ReadDoc,
    TextEncoding, Value, ROOT,
};
use base64::Engine as _;
use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};

use crate::db::{self, Touched, KIND_NOTE};
use crate::error::{AppError, Result};
use crate::store::{get_note, get_unlocked_note, hex, Store, Writer};

/// S70: every document is created and loaded with this encoding.
const ENCODING: TextEncoding = TextEncoding::Utf16CodeUnit;
/// S3: the key of the text object at the root of the document.
const FIELD: &str = "body";
/// S4: where `meta` keeps the actor identifier of this device.
const ACTOR_KEY: &str = "actor";
/// The size of a change hash, the unit of the heads kept in `bodies.uploaded`.
const HEAD_LEN: usize = 32;
/// The stamp of a text restored from a file that carries none: it counts for nothing.
const NO_STAMP: &str = "000000000000-0000-000000000000";

fn b64() -> base64::engine::GeneralPurpose {
    base64::engine::general_purpose::STANDARD
}

fn am(e: AutomergeError) -> AppError {
    AppError::Storage {
        msg: format!("text of a note: {e}"),
    }
}

fn not_arrived() -> AppError {
    AppError::invalid("the text of the note has not reached this device yet")
}

/// S4: 16 random bytes in hexadecimal, new whenever the device identifier is new.
pub(crate) fn new_actor() -> String {
    ActorId::random().to_hex_string()
}

/// The actor identifier of this device; one is chosen for a database that has none yet (S4).
pub(crate) fn ensure_actor(conn: &Connection) -> Result<ActorId> {
    let stored = db::meta_get(conn, ACTOR_KEY)?.and_then(|a| ActorId::try_from(a.as_str()).ok());
    match stored {
        Some(actor) => Ok(actor),
        None => {
            let actor = ActorId::random();
            db::meta_set(conn, ACTOR_KEY, &actor.to_hex_string())?;
            Ok(actor)
        }
    }
}

/// A document with no changes yet, as this device holds a note whose text has not arrived.
fn blank(actor: &ActorId) -> AutoCommit {
    AutoCommit::new_with_encoding(ENCODING).with_actor(actor.clone())
}

/// A saved document, loaded with the encoding and the actor of this device (S4, S70).
fn open(bytes: &[u8], actor: &ActorId) -> Result<AutoCommit> {
    let mut doc = AutoCommit::load_with_options(bytes, LoadOptions::new().text_encoding(ENCODING)).map_err(am)?;
    doc.set_actor(actor.clone());
    Ok(doc)
}

/// S3: the text object of the document. Two objects under `body` exist only when two devices
/// created one for the same note; every device then takes the smallest object id, whatever order
/// the changes came in, so that all of them show the same text.
fn text_object(doc: &AutoCommit) -> Option<ObjId> {
    doc.get_all(ROOT, FIELD)
        .ok()?
        .into_iter()
        .filter_map(|(value, id)| matches!(value, Value::Object(ObjType::Text)).then_some(id))
        .min()
}

fn text_of(doc: &AutoCommit) -> Result<String> {
    match text_object(doc) {
        Some(obj) => doc.text(&obj).map_err(am),
        None => Ok(String::new()),
    }
}

/// S70: the greatest time among the changes of the document.
fn latest_time(doc: &mut AutoCommit) -> i64 {
    doc.get_changes_meta(&[])
        .iter()
        .map(|change| change.timestamp)
        .max()
        .unwrap_or(0)
}

/// S70: closes the open edit as one change at the clock of the device, never below `floor`, the
/// greatest time the document held before the edit.
fn commit(doc: &mut AutoCommit, now_ms: u64, floor: i64) {
    let time = i64::try_from(now_ms).unwrap_or(i64::MAX).max(floor);
    doc.commit_with(CommitOptions::default().with_time(time));
}

fn heads_to_bytes(heads: &[ChangeHash]) -> Vec<u8> {
    heads.iter().flat_map(|head| head.0).collect()
}

fn heads_from_bytes(bytes: &[u8]) -> Vec<ChangeHash> {
    let (heads, _) = bytes.as_chunks::<HEAD_LEN>();
    heads.iter().map(|hash| ChangeHash(*hash)).collect()
}

fn stored(conn: &Connection, note: &str) -> Result<Option<Vec<u8>>> {
    Ok(conn
        .query_row("SELECT doc FROM bodies WHERE note_id = ?1", [note], |r| r.get(0))
        .optional()?)
}

/// Writes the saved document of a note with the text read from it. `dirty` marks changes no log
/// file carries yet; a row never goes back to clean here (S20), and its stamp never goes back.
fn write_saved(conn: &Connection, note: &str, saved: &[u8], text: &str, stamp: &str, dirty: bool) -> Result<()> {
    conn.execute(
        "INSERT INTO bodies (note_id, doc, dirty, stamp, text) VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT (note_id) DO UPDATE SET doc = excluded.doc, text = excluded.text,
            dirty = max(dirty, excluded.dirty), stamp = max(stamp, excluded.stamp)",
        params![note, saved, dirty, stamp, text],
    )?;
    Ok(())
}

/// S3: the document of a new note, with the text object that only the device creating the note
/// creates, and the text the note starts with. The first change goes out with the first
/// registers of the note (S20).
pub(crate) fn create(w: &mut Writer, note: &str, text: &str) -> Result<()> {
    let actor = ensure_actor(w.tx)?;
    let mut doc = blank(&actor);
    let obj = doc.put_object(ROOT, FIELD, ObjType::Text).map_err(am)?;
    if !text.is_empty() {
        doc.splice_text(&obj, 0, 0, text).map_err(am)?;
    }
    commit(&mut doc, w.now_ms(), 0);
    let stamp = w.stamp();
    write_saved(w.tx, note, &doc.save(), text, &stamp, true)?;
    w.touch(KIND_NOTE, note);
    Ok(())
}

/// One edit of the text of a note as one change. `change` applies it to the text object and says
/// whether it changed anything; an edit that changes nothing writes nothing.
fn edit(w: &mut Writer, note: &str, change: impl FnOnce(&mut AutoCommit, &ObjId) -> Result<bool>) -> Result<()> {
    let actor = ensure_actor(w.tx)?;
    let mut doc = open(&stored(w.tx, note)?.ok_or_else(not_arrived)?, &actor)?;
    let obj = text_object(&doc).ok_or_else(not_arrived)?;
    // Read before the edit: reading the changes closes the open one.
    let floor = latest_time(&mut doc);
    if !change(&mut doc, &obj)? {
        return Ok(());
    }
    commit(&mut doc, w.now_ms(), floor);
    let stamp = w.stamp();
    write_saved(w.tx, note, &doc.save(), &text_of(&doc)?, &stamp, true)?;
    w.touch(KIND_NOTE, note);
    Ok(())
}

/// S70: a position in UTF-16 units must be inside the text and not between the two halves of a
/// surrogate pair. Automerge would move such a position silently, and so would an editor that
/// counted wrong.
fn check_range(text: &str, pos: u32, delete_len: u32) -> Result<()> {
    let units: Vec<u16> = text.encode_utf16().collect();
    let len = units.len() as u64;
    let end = u64::from(pos) + u64::from(delete_len);
    if u64::from(pos) > len || end > len {
        return Err(AppError::invalid(format!(
            "the range {pos}..{end} is past the end of the text, which is {len} units long"
        )));
    }
    // A valid string holds a low surrogate only as the second half of a pair.
    for at in [pos as usize, end as usize] {
        if units.get(at).is_some_and(|unit| (0xDC00..=0xDFFF).contains(unit)) {
            return Err(AppError::invalid(format!(
                "the position {at} is inside a surrogate pair"
            )));
        }
    }
    Ok(())
}

/// S3, S70: `delete_len` units at `pos` replaced by `insert`.
pub(crate) fn splice(w: &mut Writer, note: &str, pos: u32, delete_len: u32, insert: &str) -> Result<()> {
    edit(w, note, |doc, obj| {
        check_range(&doc.text(obj).map_err(am)?, pos, delete_len)?;
        if delete_len == 0 && insert.is_empty() {
            return Ok(false);
        }
        doc.splice_text(obj, pos as usize, delete_len as isize, insert)
            .map_err(am)?;
        Ok(true)
    })
}

/// S37: the text a device remembered written back over what the note holds now. A note with no
/// document here is left alone.
pub(crate) fn restore(w: &mut Writer, note: &str, text: &str) -> Result<()> {
    if stored(w.tx, note)?.is_none() {
        return Ok(());
    }
    replace(w, note, text)
}

/// The whole text replaced: the library finds the insertions and deletions that turn the current
/// text into the new one.
pub(crate) fn replace(w: &mut Writer, note: &str, text: &str) -> Result<()> {
    edit(w, note, |doc, obj| {
        if doc.text(obj).map_err(am)? == text {
            return Ok(false);
        }
        doc.update_text(obj, text).map_err(am)?;
        Ok(true)
    })
}

/// The text of the note, nothing for a note whose text has not arrived.
pub(crate) fn text(conn: &Connection, note: &str) -> Result<String> {
    Ok(conn
        .query_row("SELECT text FROM bodies WHERE note_id = ?1", [note], |r| r.get(0))
        .optional()?
        .unwrap_or_default())
}

/// The text of every note that has one, for the filter (R45).
pub(crate) fn texts(conn: &Connection) -> Result<HashMap<String, String>> {
    let mut stmt = conn.prepare("SELECT note_id, text FROM bodies")?;
    let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

// ---- the log (S13, S20) ----

/// What a log line of a body carries: the note, the bytes, the stamp of the last edit.
pub(crate) struct Piece {
    pub note: String,
    pub crdt: String,
    pub stamp: String,
}

/// S20: for every note edited since its last upload, the changes since that version, and the new
/// version marked as uploaded. The caller writes them into the file in the same transaction.
pub(crate) fn take_dirty(conn: &Connection) -> Result<Vec<Piece>> {
    let actor = ensure_actor(conn)?;
    let rows: Vec<(String, Vec<u8>, Vec<u8>, String)> = {
        let mut stmt =
            conn.prepare("SELECT note_id, doc, uploaded, stamp FROM bodies WHERE dirty = 1 ORDER BY note_id")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    let mut pieces = Vec::new();
    for (note, saved, uploaded, stamp) in rows {
        let mut doc = open(&saved, &actor)?;
        let changes = doc.save_after(&heads_from_bytes(&uploaded));
        let heads = doc.get_heads();
        conn.execute(
            "UPDATE bodies SET dirty = 0, uploaded = ?2 WHERE note_id = ?1",
            params![note, heads_to_bytes(&heads)],
        )?;
        if !changes.is_empty() {
            pieces.push(Piece {
                note,
                crdt: b64().encode(changes),
                stamp,
            });
        }
    }
    Ok(pieces)
}

/// S15, S21: every text as the whole saved document with its history.
pub(crate) fn all(conn: &Connection) -> Result<Vec<Piece>> {
    let mut stmt = conn.prepare("SELECT note_id, doc, stamp FROM bodies ORDER BY note_id")?;
    let rows = stmt.query_map([], |r| {
        Ok(Piece {
            note: r.get(0)?,
            crdt: b64().encode(r.get::<_, Vec<u8>>(1)?),
            stamp: r.get(2)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// What a body change came to when it was applied.
pub(crate) struct Outcome {
    /// The visible version of the document moved.
    pub changed: bool,
    /// Changes of the document wait for others it does not have (S22).
    pub waiting: bool,
}

/// S22: whether the bytes read as Automerge changes, all of them. The library applies what it can
/// read of a stream and passes over the rest without a word, so garbage, a cut-off stream and a
/// stream with a damaged chunk would all count as applied. Changes that build on others the
/// device does not have are well formed: the library queues them.
fn reads_as_changes(bytes: &[u8]) -> bool {
    let options = LoadOptions::new()
        .text_encoding(ENCODING)
        .on_partial_load(OnPartialLoad::Error);
    matches!(
        Automerge::load_with_options(bytes, options),
        Ok(_) | Err(AutomergeError::MissingDeps)
    )
}

/// S3: applies the changes of a log line to the document of the note. `None` when the bytes are
/// not Automerge changes (S22): the document is left as it was. Changes whose dependencies are
/// missing are queued by the library and saved with the document; the text shows what can be
/// applied.
pub(crate) fn apply(conn: &Connection, note: &str, crdt: &str, stamp: &str) -> Result<Option<Outcome>> {
    let Ok(bytes) = b64().decode(crdt) else {
        return Ok(None);
    };
    if !reads_as_changes(&bytes) {
        return Ok(None);
    }
    let actor = ensure_actor(conn)?;
    let before = stored(conn, note)?;
    let mut doc = match &before {
        Some(saved) => open(saved, &actor)?,
        None => blank(&actor),
    };
    let heads = doc.get_heads();
    if doc.load_incremental(&bytes).is_err() {
        return Ok(None);
    }
    let changed = doc.get_heads() != heads;
    let waiting = !doc.get_missing_deps(&[]).is_empty();
    let saved = doc.save();
    let write = match &before {
        Some(old) => *old != saved,
        None => changed || waiting,
    };
    if write {
        write_saved(conn, note, &saved, &text_of(&doc)?, stamp, false)?;
    }
    Ok(Some(Outcome { changed, waiting }))
}

/// S22: a change that could not be applied, one row for the same bytes of the same note.
pub(crate) struct Kept {
    id: i64,
    pub note: String,
    crdt: String,
    stamp: String,
}

/// S22: keeps the change as it was received. Whether it is new here.
pub(crate) fn keep(conn: &Connection, note: &str, device: &str, crdt: &str, stamp: &str) -> Result<bool> {
    let digest = hex(&Sha256::digest(crdt.as_bytes()));
    let rows = conn.execute(
        "INSERT OR IGNORE INTO body_pending (note_id, device, crdt, stamp, digest) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![note, device, crdt, stamp, digest],
    )?;
    Ok(rows > 0)
}

pub(crate) fn kept(conn: &Connection) -> Result<Vec<Kept>> {
    let mut stmt = conn.prepare("SELECT id, note_id, crdt, stamp FROM body_pending ORDER BY id")?;
    let rows = stmt.query_map([], |r| {
        Ok(Kept {
            id: r.get(0)?,
            note: r.get(1)?,
            crdt: r.get(2)?,
            stamp: r.get(3)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// S22: tries the kept changes again. Those that apply leave the table; the notes they changed
/// are added to `touched`. Returns how many changed a text.
pub(crate) fn retry(conn: &Connection, again: Vec<Kept>, touched: &mut Touched) -> Result<u32> {
    let mut changed = 0;
    for item in again {
        if let Some(outcome) = apply(conn, &item.note, &item.crdt, &item.stamp)? {
            conn.execute("DELETE FROM body_pending WHERE id = ?1", [item.id])?;
            touched.insert((KIND_NOTE.to_string(), item.note));
            changed += u32::from(outcome.changed);
        }
    }
    Ok(changed)
}

/// S38: the texts of the database at `restored` for the notes of the restored registers, every
/// one marked for upload. A file that holds no texts, or a text that is not a document, gives none.
pub(crate) fn import_restored(conn: &Connection, notes: &Touched) -> Result<()> {
    let present: i64 = conn.query_row(
        "SELECT count(*) FROM restored.sqlite_master WHERE type = 'table' AND name = 'bodies'",
        [],
        |r| r.get(0),
    )?;
    if present == 0 {
        return Ok(());
    }
    let Ok(mut stmt) = conn.prepare("SELECT note_id, doc, stamp FROM restored.bodies") else {
        return Ok(());
    };
    let rows: Vec<(String, Vec<u8>, String)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .filter_map(|row| row.ok())
        .collect();
    drop(stmt);
    let actor = ensure_actor(conn)?;
    for (note, saved, stamp) in rows {
        if !notes.contains(&(KIND_NOTE.to_string(), note.clone())) {
            continue;
        }
        let Ok(mut doc) = open(&saved, &actor) else {
            continue;
        };
        let stamp = if crate::hlc::is_valid(&stamp) {
            stamp
        } else {
            NO_STAMP.into()
        };
        write_saved(conn, &note, &doc.save(), &text_of(&doc)?, &stamp, true)?;
    }
    Ok(())
}

impl Store {
    /// S22: the activity log says that a body change was not applied, naming the note and the
    /// device that wrote it and never the text.
    pub(crate) fn note_body_trouble(&self, note: &str, device: &str, what: &str) {
        self.note(crate::LogLevel::Error, "sync", || {
            format!("text of note {note} from device {device}: {what}")
        });
    }
}

#[uniffi::export]
impl Store {
    /// R1: the Markdown source of the note, empty while its text has not arrived.
    pub fn get_text(&self, id: String) -> Result<String> {
        let inner = self.lock();
        get_note(&inner.conn, &id)?;
        text(&inner.conn, &id)
    }

    /// S3, S70: replaces `delete_len` UTF-16 units of the text at `pos` with `insert`, as one change.
    /// A position past the end of the text or inside a surrogate pair is refused and the text does
    /// not change. R28: refused for a locked note.
    pub fn splice_utf16(&self, id: String, pos: u32, delete_len: u32, insert: String) -> Result<()> {
        self.write(|w| {
            get_unlocked_note(w.tx, &id)?;
            splice(w, &id, pos, delete_len, &insert)
        })
    }

    /// Replaces the whole text, the way an editor that holds a string rather than positions does
    /// (the web page, an import). R28: refused for a locked note.
    pub fn set_text(&self, id: String, text: String) -> Result<()> {
        self.write(|w| {
            get_unlocked_note(w.tx, &id)?;
            replace(w, &id, &text)
        })
    }
}

impl Store {
    /// The actor identifier of this device in hexadecimal.
    #[doc(hidden)]
    pub fn actor_id_for_tests(&self) -> String {
        ensure_actor(&self.lock().conn)
            .map(|a| a.to_hex_string())
            .unwrap_or_default()
    }

    /// The changes of the document of a note in the order the library keeps them: the actor in
    /// hexadecimal and the time of each.
    #[doc(hidden)]
    pub fn body_changes_for_tests(&self, id: &str) -> Vec<(String, i64)> {
        let inner = self.lock();
        let Ok(actor) = ensure_actor(&inner.conn) else {
            return vec![];
        };
        let Ok(Some(saved)) = stored(&inner.conn, id) else {
            return vec![];
        };
        let Ok(mut doc) = open(&saved, &actor) else {
            return vec![];
        };
        doc.get_changes_meta(&[])
            .iter()
            .map(|change| (change.actor.to_hex_string(), change.timestamp))
            .collect()
    }

    /// How many changes the document of a note holds that wait for others (S22).
    #[doc(hidden)]
    pub fn body_missing_for_tests(&self, id: &str) -> usize {
        let inner = self.lock();
        let Ok(actor) = ensure_actor(&inner.conn) else {
            return 0;
        };
        let Ok(Some(saved)) = stored(&inner.conn, id) else {
            return 0;
        };
        open(&saved, &actor).map_or(0, |doc| doc.get_missing_deps(&[]).len())
    }

    /// How many body changes are kept as received because they could not be applied (S22).
    #[doc(hidden)]
    pub fn body_kept_for_tests(&self) -> u32 {
        self.lock()
            .conn
            .query_row("SELECT count(*) FROM body_pending", [], |r| r.get(0))
            .unwrap_or(0)
    }

    /// Whether the device holds a document for the note.
    #[doc(hidden)]
    pub fn has_body_for_tests(&self, id: &str) -> bool {
        stored(&self.lock().conn, id).ok().flatten().is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_range_inside_a_pair_or_past_the_end_is_refused() {
        // U+1F600 is two units: 0xD83D 0xDE00.
        let text = "a\u{1F600}b";
        assert!(check_range(text, 0, 4).is_ok());
        assert!(check_range(text, 3, 0).is_ok(), "right after the emoji");
        assert!(check_range(text, 4, 0).is_ok(), "the end");
        assert!(check_range(text, 2, 0).is_err(), "between the halves");
        assert!(
            check_range(text, 1, 1).is_err(),
            "the end of the range falls between them"
        );
        assert!(
            check_range(text, 2, 2).is_err(),
            "the start of the range falls between them"
        );
        assert!(check_range(text, 5, 0).is_err(), "past the end");
        assert!(check_range(text, 3, 2).is_err(), "the range leaves the text");
        assert!(check_range(text, u32::MAX, u32::MAX).is_err(), "no overflow");
    }

    #[test]
    fn a_kept_change_is_applied_when_it_reads_and_leaves_the_table() {
        let conn = Connection::open_in_memory().unwrap();
        db::migrate(&conn).unwrap();
        let mut doc = blank(&ActorId::random());
        let obj = doc.put_object(ROOT, FIELD, ObjType::Text).unwrap();
        doc.splice_text(&obj, 0, 0, "kept").unwrap();
        doc.commit();
        let crdt = b64().encode(doc.save_after(&[]));
        assert!(keep(&conn, "n", "device", &crdt, "000000000001-0000-000000000001").unwrap());
        assert!(
            !keep(&conn, "n", "device", &crdt, "000000000001-0000-000000000001").unwrap(),
            "not twice"
        );

        let mut touched = Touched::new();
        assert_eq!(retry(&conn, kept(&conn).unwrap(), &mut touched).unwrap(), 1);
        assert!(kept(&conn).unwrap().is_empty());
        assert_eq!(text(&conn, "n").unwrap(), "kept");
        assert!(touched.contains(&(KIND_NOTE.to_string(), "n".to_string())));
    }

    #[test]
    fn bytes_that_do_not_read_as_changes_are_told_from_changes_that_wait() {
        let mut doc = blank(&ActorId::random());
        let obj = doc.put_object(ROOT, FIELD, ObjType::Text).unwrap();
        doc.splice_text(&obj, 0, 0, "a").unwrap();
        doc.commit();
        let first = doc.get_heads();
        doc.splice_text(&obj, 1, 0, "b").unwrap();
        doc.commit();
        let whole = doc.save_after(&[]);
        let second = doc.save_after(&first);
        assert!(reads_as_changes(&whole));
        assert!(
            reads_as_changes(&second),
            "a change that waits for others is well formed"
        );
        assert!(reads_as_changes(&[]), "no changes is nothing to apply");
        assert!(!reads_as_changes(b"not changes at all"));
        assert!(!reads_as_changes(&whole[..whole.len() / 2]));
        let mut flipped = whole.clone();
        let last = flipped.len() - 2;
        flipped[last] ^= 0xff;
        assert!(!reads_as_changes(&flipped));
        let mut trailing = whole.clone();
        trailing.extend_from_slice(b"junk");
        assert!(!reads_as_changes(&trailing));
    }

    #[test]
    fn heads_survive_the_trip_through_bytes() {
        let mut doc = blank(&ActorId::random());
        let obj = doc.put_object(ROOT, FIELD, ObjType::Text).unwrap();
        doc.splice_text(&obj, 0, 0, "x").unwrap();
        let heads = doc.get_heads();
        assert_eq!(heads_from_bytes(&heads_to_bytes(&heads)), heads);
        assert!(heads_from_bytes(&[]).is_empty());
    }
}
