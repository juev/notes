//! R40–R43: the history of versions of the text of a note, read from the changes of its Automerge
//! document rather than kept apart: every device that holds the same changes lists the same
//! versions. S71: the name of each device, for the list.

use std::collections::HashSet;
use std::str::FromStr;

use automerge::{ChangeHash, PatchAction, ReadDoc};
use rusqlite::OptionalExtension;
use serde_json::json;

use crate::body::{am, ensure_actor, not_arrived, open, stored, text_object};
use crate::error::{AppError, Result};
use crate::markdown::MarkdownRange;
use crate::store::{get_note, get_unlocked_note, Store};

/// S71: the kind of the registers that name the devices, by the actor of each.
pub(crate) const KIND_DEVICE: &str = "device";
/// R40: a version is closed by this pause in editing…
const PAUSE_MS: i64 = 5 * 60 * 1000;
/// …and during editing without such a pause, once this long after it began.
const HOURLY_MS: i64 = 60 * 60 * 1000;

/// One version of the text of a note (R40).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct Version {
    /// What `version_diff` and `restore_version` take.
    pub id: String,
    /// The time of the last edit in the version, in milliseconds since the epoch.
    pub time_ms: u64,
    /// The name of the device of the last edit (S71); empty when the device has none.
    pub device: String,
    /// Whether the last edit was made on this device.
    pub this_device: bool,
    /// UTF-16 units added and removed against the version before it.
    pub added: u32,
    pub removed: u32,
}

/// A version against the current text (R41).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct VersionDiff {
    /// The text of the version.
    pub text: String,
    /// Where the version has text the current one does not, in UTF-16 units of `text`.
    pub changed: Vec<MarkdownRange>,
    /// Where the current text has text the version does not, as positions in `text`.
    pub removed_at: Vec<u32>,
}

/// A change of the document as the history reads it.
struct Change {
    hash: ChangeHash,
    deps: Vec<ChangeHash>,
    time: i64,
    actor: String,
}

/// The heads of the set of changes up to `n` in time order: the set is closed under dependencies,
/// since a change never has a time below those it depends on (S70, `body::commit`).
fn heads(changes: &[Change], n: usize) -> Vec<ChangeHash> {
    let depended: HashSet<ChangeHash> = changes[..=n].iter().flat_map(|c| c.deps.iter().copied()).collect();
    let mut heads: Vec<ChangeHash> = changes[..=n]
        .iter()
        .map(|c| c.hash)
        .filter(|h| !depended.contains(h))
        .collect();
    heads.sort();
    heads
}

fn version_id(heads: &[ChangeHash]) -> String {
    heads.iter().map(ToString::to_string).collect::<Vec<_>>().join(",")
}

fn parse_id(id: &str) -> Result<Vec<ChangeHash>> {
    id.split(',')
        .map(|h| ChangeHash::from_str(h).map_err(|_| AppError::invalid("not a version of this note")))
        .collect()
}

/// What turns the text at `from` into the text at `to`, read off the patches of the library: the
/// units of the result that `from` did not have, and the places where units of `from` went.
fn compare(doc: &mut automerge::AutoCommit, from: &[ChangeHash], to: &[ChangeHash]) -> (Vec<bool>, Vec<bool>, u32) {
    let obj = text_object(doc);
    let start = match &obj {
        Some(obj) => doc.text_at(obj, from).unwrap_or_default(),
        None => String::new(),
    };
    // One entry per UTF-16 unit of the text: whether it is new, and whether units were removed
    // right before it; the last entry stands for the end.
    let mut new: Vec<bool> = vec![false; start.encode_utf16().count()];
    let mut cut: Vec<bool> = vec![false; new.len() + 1];
    let mut removed = 0u32;
    for patch in doc.diff(from, to) {
        if obj.as_ref() != Some(&patch.obj) {
            continue;
        }
        match patch.action {
            PatchAction::SpliceText { index, value, .. } => {
                let inserted = value.make_string().encode_utf16().count();
                let index = index.min(new.len());
                new.splice(index..index, std::iter::repeat_n(true, inserted));
                cut.splice(index..index, std::iter::repeat_n(false, inserted));
            }
            PatchAction::DeleteSeq { index, length } => {
                let end = (index + length).min(new.len());
                let index = index.min(end);
                removed += new[index..end].iter().filter(|n| !**n).count() as u32;
                new.drain(index..end);
                cut.drain(index..end);
                cut[index] = true;
            }
            _ => {}
        }
    }
    (new, cut, removed)
}

#[uniffi::export]
impl Store {
    /// S71: names this device in the history of versions of every device.
    pub fn set_device_name(&self, name: String) -> Result<()> {
        let name = name.trim().to_string();
        self.write(|w| {
            let actor = ensure_actor(w.tx)?.to_hex_string();
            w.set(KIND_DEVICE, &actor, "name", json!(name))
        })
    }

    /// R40: the versions of the text of a note, the latest first. The edits of the last few minutes
    /// are not a version yet: the text is still being written.
    pub fn versions(&self, note: String) -> Result<Vec<Version>> {
        let inner = self.lock();
        get_note(&inner.conn, &note)?;
        let actor = ensure_actor(&inner.conn)?;
        let Some(saved) = stored(&inner.conn, &note)? else {
            return Ok(Vec::new());
        };
        let mut doc = open(&saved, &actor)?;
        let mut changes: Vec<Change> = doc
            .get_changes_meta(&[])
            .into_iter()
            .map(|c| Change {
                hash: c.hash,
                deps: c.deps,
                time: c.timestamp,
                actor: c.actor.to_hex_string(),
            })
            .collect();
        changes.sort_by(|a, b| a.time.cmp(&b.time).then(a.hash.cmp(&b.hash)));
        let now = i64::try_from(inner.now_ms()).unwrap_or(i64::MAX);
        let mut ends = Vec::new();
        let mut began = None;
        for (n, change) in changes.iter().enumerate() {
            let began_at = *began.get_or_insert(change.time);
            let next = changes.get(n + 1).map(|c| c.time);
            let closes = match next {
                Some(next) => next - change.time >= PAUSE_MS || next - began_at >= HOURLY_MS,
                None => now - change.time >= PAUSE_MS,
            };
            if closes {
                ends.push(n);
                began = None;
            }
        }
        let names = |actor: &str| -> Result<String> {
            let value: Option<String> = inner
                .conn
                .query_row(
                    "SELECT value FROM fields WHERE kind = ?1 AND id = ?2 AND field = 'name'",
                    [KIND_DEVICE, actor],
                    |r| r.get(0),
                )
                .optional()?;
            Ok(value
                .and_then(|v| serde_json::from_str::<String>(&v).ok())
                .unwrap_or_default())
        };
        let mut out = Vec::new();
        let mut before: Vec<ChangeHash> = Vec::new();
        for n in ends {
            let at = heads(&changes, n);
            let (new, _, removed) = compare(&mut doc, &before, &at);
            out.push(Version {
                id: version_id(&at),
                time_ms: changes[n].time.max(0) as u64,
                device: names(&changes[n].actor)?,
                this_device: changes[n].actor == actor.to_hex_string(),
                added: new.iter().filter(|n| **n).count() as u32,
                removed,
            });
            before = at;
        }
        out.reverse();
        Ok(out)
    }

    /// R41: a version with what differs from the current text marked.
    pub fn version_diff(&self, note: String, version: String) -> Result<VersionDiff> {
        let at = parse_id(&version)?;
        let inner = self.lock();
        get_note(&inner.conn, &note)?;
        let actor = ensure_actor(&inner.conn)?;
        let mut doc = open(&stored(&inner.conn, &note)?.ok_or_else(not_arrived)?, &actor)?;
        let obj = text_object(&doc).ok_or_else(not_arrived)?;
        let text = doc.text_at(&obj, &at).map_err(am)?;
        let current = doc.get_heads();
        let (new, cut, _) = compare(&mut doc, &current, &at);
        let mut changed: Vec<MarkdownRange> = Vec::new();
        for (unit, is_new) in new.iter().enumerate() {
            if !is_new {
                continue;
            }
            match changed.last_mut() {
                Some(last) if last.end == unit as u32 => last.end += 1,
                _ => changed.push(MarkdownRange {
                    start: unit as u32,
                    end: unit as u32 + 1,
                }),
            }
        }
        let removed_at = cut
            .iter()
            .enumerate()
            .filter(|(_, c)| **c)
            .map(|(at, _)| at as u32)
            .collect();
        Ok(VersionDiff {
            text,
            changed,
            removed_at,
        })
    }

    /// R42: writes into the current text the edit that turns it into the version: an ordinary edit
    /// that syncs, is one action of undo and leaves the later versions in the history. R28: refused
    /// for a locked note.
    pub fn restore_version(&self, note: String, version: String) -> Result<()> {
        let at = parse_id(&version)?;
        let text = {
            let inner = self.lock();
            get_unlocked_note(&inner.conn, &note)?;
            let actor = ensure_actor(&inner.conn)?;
            let doc = open(&stored(&inner.conn, &note)?.ok_or_else(not_arrived)?, &actor)?;
            let obj = text_object(&doc).ok_or_else(not_arrived)?;
            doc.text_at(&obj, &at).map_err(am)?
        };
        self.act(|w| {
            get_unlocked_note(w.tx, &note)?;
            crate::body::replace(w, &note, &text)
        })
    }
}
