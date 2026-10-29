//! Sync through dumb file storage. Layout and rules: docs/specs/sync.md.
//!
//! ```text
//! lists/vault.json
//! lists/log/<device>-<seq>.jsonl     immutable batch of field changes
//! lists/log/<device>-<seq>.snap      empty: the device has a snapshot as of <seq>
//! lists/snap/<device>-<seq>.jsonl    full state of a device as of its log <seq>, gzip
//! lists/blobs/<aa>/<sha256>          attachment content
//! lists/v1/vault.json                format 3: the storage moved here from lists/v1/
//! ```
//!
//! Format 4 is the same layout with the files sealed under a passphrase (S54, `seal`).
//!
//! A device only ever writes files carrying its own id, so two devices never
//! write the same file and no locking is needed.

mod backups;
pub use backups::{StorageBackup, StorageBackupStatus};
pub mod remote;
pub(crate) mod seal;
pub mod webdav;

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::Path;
use std::sync::Arc;

use chrono::NaiveDateTime;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::LogLevel;
use sha2::{Digest, Sha256};

use crate::db::{
    self, is_sha256, Change, Touched, KIND_ATTACHMENT, KIND_BLOB, KIND_FILTER, KIND_LIST, KIND_SETTINGS, KIND_TASK,
};
use crate::error::{AppError, Result};
use crate::hlc::Clock;
use crate::model::{
    Attachment, AttachmentReport, ConnectionCheck, SyncConfig, SyncReport, SyncSide, SyncStatus, INBOX_ID,
};
use crate::store::{hex, new_device_id, Store};
use remote::{DirRemote, Remote};
use seal::{Keys, SealedRemote, Wrapped};
use webdav::WebDavRemote;

const ROOT: &str = "lists";
/// S49: where the storage lived before #294. Only its `vault.json` stays, and says the storage moved.
const LEGACY: &str = "lists/v1";
/// S49: the format `lists/v1/vault.json` gets after the move: earlier versions stop on it (S14).
const MOVED: u32 = 3;
/// S46: the storage format this version writes; it reads format 1 as well.
const FORMAT: u32 = 2;
/// S54: a storage whose files are sealed under a passphrase.
const SEALED: u32 = 4;
/// S56: the vault id of the encrypted storage this device has synced with.
const SEALED_VAULT: &str = "sealed_vault";
/// S56: files left in the clear when the storage was encrypted, still to be removed.
const CLEAR_FILES: &str = "clear_files";
/// `push/<device>.json` keeps its own number: the storage format does not change it.
const PUSH_FORMAT: u32 = 1;
/// The first bytes of a gzip stream: how a reader tells a compressed snapshot (S46).
const GZIP_MAGIC: [u8; 2] = [0x1f, 0x8b];

/// S45: bytes of this device's log files uploaded since its last snapshot, and the size of that snapshot.
const LOG_BYTES: &str = "log_bytes";
const SNAP_BYTES: &str = "snap_bytes";
/// S45: what a snapshot is taken to weigh before the device has written one.
const SNAP_FLOOR: u64 = 16 * 1024;

/// S47: set while the last run for the fields read everything the storage held.
const WHOLE: &str = "blobs_whole";

/// S47: attachment content still needed: an attachment that is not deleted
/// refers to it, and its task, if known here, is not deleted for good.
const NEEDED: &str = "SELECT DISTINCT a.sha256 FROM attachments a
     WHERE a.deleted = 0 AND NOT EXISTS (SELECT 1 FROM tasks t WHERE t.id = a.task_id AND t.purged = 1)";

/// Set while this device is to put its data over the storage's (S38).
const REPLACE: &str = "replace_remote";
/// What the registers looked like when `kept` was filled (S39).
const KEPT_AT: &str = "replace_kept_at";

#[derive(Serialize, Deserialize)]
struct Vault {
    format: u32,
    vault: String,
    /// S54: format 4 only, the data key wrapped under the passphrase.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    salt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    nonce: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    key: Option<String>,
}

impl Vault {
    fn wrapped(&self) -> Option<Wrapped> {
        Some(Wrapped {
            salt: self.salt.clone()?,
            nonce: self.nonce.clone()?,
            key: self.key.clone()?,
        })
    }
}

/// What the last look at `vault.json` of a storage found (S54).
#[derive(Clone)]
pub(crate) enum Sealing {
    Clear,
    /// Encrypted, and this device cannot open it.
    Locked,
    Open(Arc<Keys>),
}

/// The data key of a storage, with what it was opened from, so that the
/// key derivation runs again only when one of them changes.
pub(crate) struct VaultKey {
    vault: String,
    wrapped: Wrapped,
    passphrase: String,
    data: [u8; 32],
}

const NO_PASSPHRASE: &str = "the storage is encrypted; enter its passphrase in the sync settings";
const WRONG_PASSPHRASE: &str =
    "the passphrase does not open the storage; enter the one it was encrypted or last changed with";
const NOT_SEALED: &str = "the storage was encrypted and is not any more; if it was reset on purpose, \
     turn sync off and connect it again";

/// `push/<device>.json`: where the device takes nudges (S18).
#[derive(Serialize, Deserialize)]
struct PushRecord {
    v: u32,
    url: String,
}

#[derive(Serialize, Deserialize)]
struct Header {
    v: u32,
    device: String,
    seq: u64,
    count: usize,
    /// Snapshots only: last log file of every device already merged into this state.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    covers: BTreeMap<String, u64>,
}

struct Batch {
    header: Header,
    changes: Vec<Change>,
}

fn encode(header: &Header, changes: &[Change]) -> Result<Vec<u8>> {
    let mut out = serde_json::to_vec(header)?;
    for change in changes {
        out.push(b'\n');
        serde_json::to_writer(&mut out, change)?;
    }
    out.push(b'\n');
    Ok(out)
}

/// S46: a snapshot goes to the storage compressed. S50: its header carries the
/// time it was written, so that a tool showing that field does not show 1970.
fn compress(data: &[u8]) -> Result<Vec<u8>> {
    use std::io::Write;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let mut encoder = flate2::GzBuilder::new()
        .mtime(u32::try_from(now).unwrap_or(u32::MAX))
        .write(Vec::new(), flate2::Compression::default());
    encoder.write_all(data)?;
    Ok(encoder.finish()?)
}

/// `None` for a file that is truncated or otherwise unreadable: the caller
/// leaves its cursor where it is and tries again on the next run.
fn decode(data: &[u8]) -> Option<Batch> {
    let unpacked;
    let data = if data.starts_with(&GZIP_MAGIC) {
        use std::io::Read;
        let mut out = Vec::new();
        flate2::read::GzDecoder::new(data).read_to_end(&mut out).ok()?;
        unpacked = out;
        &unpacked[..]
    } else {
        data
    };
    let text = std::str::from_utf8(data).ok()?;
    let mut lines = text.lines();
    let header: Header = serde_json::from_str(lines.next()?).ok()?;
    if header.v == 0 || header.v > FORMAT {
        return None;
    }
    let mut changes = Vec::with_capacity(header.count);
    for line in lines {
        changes.push(serde_json::from_str(line).ok()?);
    }
    (changes.len() == header.count && text.ends_with('\n')).then_some(Batch { header, changes })
}

/// Batches of changes: log files and snapshots.
const DATA: &str = ".jsonl";
/// A mark in `log/` that tells readers about a snapshot without a look into `snap/`.
const MARK: &str = ".snap";

type Index = BTreeMap<String, BTreeSet<u64>>;

fn file_name(device: &str, seq: u64) -> String {
    format!("{device}-{seq:010}{DATA}")
}

fn mark_name(device: &str, seq: u64) -> String {
    format!("{device}-{seq:010}{MARK}")
}

fn parse_name(name: &str, suffix: &str) -> Option<(String, u64)> {
    let (device, seq) = name.strip_suffix(suffix)?.split_once('-')?;
    (seq.len() == 10 && !device.is_empty()).then_some(())?;
    Some((device.to_string(), seq.parse().ok()?))
}

fn index(names: &[String], suffix: &str) -> Index {
    let mut map = Index::new();
    for (device, seq) in names.iter().filter_map(|n| parse_name(n, suffix)) {
        map.entry(device).or_default().insert(seq);
    }
    map
}

fn blob_remote_path(sha256: &str) -> String {
    format!("{ROOT}/blobs/{}/{}", &sha256[..2], sha256)
}

fn legacy_blob_path(sha256: &str) -> String {
    format!("{LEGACY}/blobs/{}/{}", &sha256[..2], sha256)
}

fn read_vault(remote: &dyn Remote, path: &str) -> Result<Option<Vault>> {
    let Some(data) = remote.get(path)? else {
        return Ok(None);
    };
    serde_json::from_slice(&data)
        .map(Some)
        .map_err(|_| AppError::sync("vault.json in the storage is not readable"))
}

fn put_vault(remote: &dyn Remote, path: &str, format: u32, vault: &str) -> Result<()> {
    let vault = Vault {
        format,
        vault: vault.to_string(),
        salt: None,
        nonce: None,
        key: None,
    };
    remote.put(path, &serde_json::to_vec(&vault)?)
}

/// S54: `vault.json` of an encrypted storage.
fn put_sealed_vault(remote: &dyn Remote, vault: &str, wrapped: Wrapped) -> Result<()> {
    let vault = Vault {
        format: SEALED,
        vault: vault.to_string(),
        salt: Some(wrapped.salt),
        nonce: Some(wrapped.nonce),
        key: Some(wrapped.key),
    };
    remote.put(&format!("{ROOT}/vault.json"), &serde_json::to_vec(&vault)?)
}

fn newer_format(format: u32) -> AppError {
    AppError::sync(format!(
        "the storage uses format {format}, this version understands formats up to {SEALED}; update the app"
    ))
}

/// R140: hashes and counts content as it is written, so a large file never sits in memory.
struct HashingWriter<W: Write> {
    inner: W,
    hasher: Sha256,
}

impl<W: Write> HashingWriter<W> {
    fn new(inner: W) -> Self {
        HashingWriter {
            inner,
            hasher: Sha256::new(),
        }
    }
}

impl<W: Write> Write for HashingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let written = self.inner.write(buf)?;
        self.hasher.update(&buf[..written]);
        Ok(written)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

/// R140, R142: a temporary file of the content, readable by its owner only.
fn private_create(path: &Path) -> Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    Ok(options.open(path)?)
}

/// The storage as one run sees it (S55).
enum View<'a> {
    Clear(&'a dyn Remote),
    Sealed(SealedRemote<'a>),
}

impl View<'_> {
    fn remote(&self) -> &dyn Remote {
        match self {
            View::Clear(remote) => *remote,
            View::Sealed(remote) => remote,
        }
    }

    fn sealed(&self) -> bool {
        matches!(self, View::Sealed(_))
    }
}

/// What a download of attachment content came to.
enum Fetched {
    Stored,
    /// The storage has nothing under that name.
    Missing,
    /// The storage has something that is not this content.
    Mismatch,
}

const MISMATCH: &str = "the content in the storage does not match its hash";

/// R102: the record of a file that the pass of attachments could not move:
/// `meta` keeps `since`, `said` and the message under this prefix and the hash.
const TROUBLE: &str = "blob_trouble:";
const TROUBLE_FMT: &str = "%Y-%m-%dT%H:%M:%S";

/// Which way the content was going.
#[derive(Clone, Copy)]
enum Way {
    Up,
    Down,
}

/// Why a file did not move in a pass.
enum Trouble {
    /// The error of the storage or of this device.
    Failed(String),
    /// The storage does not have the content: usually the device that has it has not sent it yet.
    Missing,
}

/// What is known of a file that did not move: since when, when an error line
/// last said so, and the reason it was given.
struct BlobTrouble {
    since: NaiveDateTime,
    said: Option<NaiveDateTime>,
    message: String,
}

impl BlobTrouble {
    fn parse(value: &str) -> Option<Self> {
        let mut parts = value.splitn(3, '\t');
        let time = |s: &str| NaiveDateTime::parse_from_str(s, TROUBLE_FMT).ok();
        let since = time(parts.next()?)?;
        let said = parts.next().and_then(time);
        Some(BlobTrouble {
            since,
            said,
            message: parts.next()?.to_string(),
        })
    }

    fn value(&self) -> String {
        let said = self.said.map(|t| t.format(TROUBLE_FMT).to_string()).unwrap_or_default();
        format!("{}\t{said}\t{}", self.since.format(TROUBLE_FMT), self.message)
    }
}

/// A failure that goes on is said again at the error level after this long.
fn trouble_repeat() -> chrono::Duration {
    chrono::Duration::hours(1)
}

/// How a file is named in the log: its folder and the start of the hash.
fn blob_label(sha256: &str) -> String {
    format!("blobs/{}/{}", &sha256[..2], short_hash(sha256))
}

fn short_hash(sha256: &str) -> String {
    format!("{}…", &sha256[..8])
}

/// An error of the storage names the file by its full hash; the log keeps only its start.
fn hide_hash(message: &str, sha256: &str) -> String {
    message.replace(sha256, &short_hash(sha256))
}

impl Store {
    fn cursor(&self, device: &str) -> Result<u64> {
        Ok(self
            .lock()
            .conn
            .query_row("SELECT seq FROM peers WHERE device = ?1", [device], |r| r.get(0))
            .optional()?
            .unwrap_or(0))
    }

    /// Merges a batch and moves cursors in one transaction. Returns how many registers changed.
    fn merge(&self, batch: &Batch, cursors: &BTreeMap<String, u64>) -> Result<u32> {
        let mut inner = self.lock();
        let inner = &mut *inner;
        let tx = inner
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let mut touched = Touched::new();
        let mut changed = 0;
        for change in batch.changes.iter().filter(|c| c.is_well_formed()) {
            inner.clock.observe(&change.stamp);
            if db::apply(&tx, change, false, &mut touched)? {
                changed += 1;
            }
        }
        db::settle(&tx, &touched)?;
        // S42: what arrived is brought to the rule of subtasks at the end of the run, not here.
        if changed > 0 {
            db::meta_set(&tx, crate::subtasks::HOLD, "1")?;
        }
        for (device, seq) in cursors {
            tx.execute(
                "INSERT INTO peers (device, seq) VALUES (?1, ?2)
                 ON CONFLICT (device) DO UPDATE SET seq = max(seq, excluded.seq)",
                params![device, *seq as i64],
            )?;
        }
        tx.commit()?;
        Ok(changed)
    }

    /// Reads `vault.json` (S14, S46, S49) and, for an encrypted storage, opens
    /// its data key (S54). What it found is kept for the runs that follow.
    fn check_vault(&self, remote: &dyn Remote) -> Result<()> {
        let sealing = self.read_sealing(remote)?;
        *self.sealed.lock().unwrap_or_else(|p| p.into_inner()) = Some((remote.id(), sealing));
        Ok(())
    }

    /// What the last look at `vault.json` of this storage found in this process.
    fn sealing_of(&self, remote: &dyn Remote) -> Option<Sealing> {
        match &*self.sealed.lock().unwrap_or_else(|p| p.into_inner()) {
            Some((id, sealing)) if *id == remote.id() => Some(sealing.clone()),
            _ => None,
        }
    }

    fn read_sealing(&self, remote: &dyn Remote) -> Result<Sealing> {
        let path = format!("{ROOT}/vault.json");
        // S60: a storage this device knows as encrypted is not written in the clear.
        let was_sealed = db::meta_get(&self.lock().conn, SEALED_VAULT)?;
        if let Some(vault) = read_vault(remote, &path)? {
            match vault.format {
                SEALED => {
                    // Locked until the key opens, so that the settings can say why sync stopped.
                    *self.sealed.lock().unwrap_or_else(|p| p.into_inner()) = Some((remote.id(), Sealing::Locked));
                    return self.open_vault(&vault);
                }
                older if older < SEALED && was_sealed.is_some() => return Err(AppError::sync(NOT_SEALED)),
                FORMAT => {}
                // S46: the storage moves on; its files of format 1 stay readable.
                1 => put_vault(remote, &path, FORMAT, &vault.vault)?,
                other => return Err(newer_format(other)),
            }
            // S49: a device of an earlier version may have written to the old place since the move.
            let names = remote.list(&format!("{LEGACY}/log"))?;
            if names
                .iter()
                .any(|n| parse_name(n, DATA).or_else(|| parse_name(n, MARK)).is_some())
            {
                self.move_legacy(remote, false)?;
            }
            return Ok(Sealing::Clear);
        }
        if was_sealed.is_some() {
            return Err(AppError::sync(NOT_SEALED));
        }
        let legacy = format!("{LEGACY}/vault.json");
        let id = match read_vault(remote, &legacy)? {
            Some(vault) if matches!(vault.format, 1 | 2 | MOVED) => {
                self.move_legacy(remote, true)?;
                vault.vault
            }
            Some(vault) => return Err(newer_format(vault.format)),
            None => uuid::Uuid::now_v7().to_string(),
        };
        // S49: the new place is named last but one, the old one is closed last, so a move cut short starts over.
        put_vault(remote, &path, FORMAT, &id)?;
        put_vault(remote, &legacy, MOVED, &id)?;
        Ok(Sealing::Clear)
    }

    /// S58: opens the data key of an encrypted storage with the passphrase of this device.
    fn open_vault(&self, vault: &Vault) -> Result<Sealing> {
        let wrapped = vault
            .wrapped()
            .ok_or_else(|| AppError::sync("vault.json in the storage is not readable"))?;
        let data = self.data_key(&vault.vault, &wrapped)?;
        let keys = Arc::new(Keys::new(&data));
        // S56: the first time this device opens the storage it writes a snapshot of
        // its state, and checks anew which content and which address are there.
        let mut inner = self.lock();
        let tx = inner.conn.transaction()?;
        if db::meta_get(&tx, SEALED_VAULT)?.as_deref() != Some(vault.vault.as_str()) {
            tx.execute("DELETE FROM blobs_uploaded", [])?;
            db::meta_del(&tx, "push_published")?;
            db::meta_set(&tx, "force_snapshot", "1")?;
            db::meta_set(&tx, SEALED_VAULT, &vault.vault)?;
        }
        tx.commit()?;
        Ok(Sealing::Open(keys))
    }

    /// The data key under the passphrase of this device. The key derivation is
    /// slow on purpose, so a key opened once is kept while nothing changes.
    fn data_key(&self, vault: &str, wrapped: &Wrapped) -> Result<[u8; 32]> {
        let passphrase = self
            .sync_passphrase
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
            .ok_or_else(|| AppError::sync(NO_PASSPHRASE))?;
        let mut cached = self.vault_key.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(key) = cached
            .as_ref()
            .filter(|k| k.vault == vault && k.wrapped == *wrapped && k.passphrase == passphrase)
        {
            return Ok(key.data);
        }
        let data = seal::unwrap(&passphrase, vault, wrapped)?.ok_or_else(|| AppError::sync(WRONG_PASSPHRASE))?;
        *cached = Some(VaultKey {
            vault: vault.to_string(),
            wrapped: wrapped.clone(),
            passphrase,
            data,
        });
        Ok(data)
    }

    /// S49: moves the files of `lists/v1/` to the same names under `lists/`.
    /// Attachment content goes along with `blobs`; after the move it is looked
    /// for one file at a time (`download_blob`).
    fn move_legacy(&self, remote: &dyn Remote, blobs: bool) -> Result<()> {
        let mut moved = 0;
        for dir in ["log", "snap", "push"] {
            for name in remote.list(&format!("{LEGACY}/{dir}"))? {
                let ours = match dir {
                    "push" => name.ends_with(".json"),
                    _ => parse_name(&name, DATA).or_else(|| parse_name(&name, MARK)).is_some(),
                };
                if ours && remote.rename(&format!("{LEGACY}/{dir}/{name}"), &format!("{ROOT}/{dir}/{name}"))? {
                    moved += 1;
                }
            }
        }
        if blobs {
            for folder in remote.folders(&format!("{LEGACY}/blobs"))? {
                for name in remote.list(&format!("{LEGACY}/blobs/{folder}"))? {
                    if is_sha256(&name)
                        && name.starts_with(&folder)
                        && remote.rename(&legacy_blob_path(&name), &blob_remote_path(&name))?
                    {
                        moved += 1;
                    }
                }
            }
        }
        self.note(LogLevel::Info, "sync", || {
            format!("moved {moved} files from {LEGACY}/ to {ROOT}/")
        });
        Ok(())
    }

    /// Whether the marks in `log/` leave something unexplained: a device whose
    /// next file is gone although later ones exist, or a known device with
    /// nothing left there. Versions that wrote no marks leave such traces.
    fn marks_fall_short(&self, me: &str, logs: &Index, marks: &Index) -> Result<bool> {
        let cursors: BTreeMap<String, u64> = {
            let inner = self.lock();
            let mut stmt = inner.conn.prepare("SELECT device, seq FROM peers")?;
            let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as u64)))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        let empty = BTreeSet::new();
        Ok(logs
            .keys()
            .chain(cursors.keys())
            .filter(|d| d.as_str() != me)
            .any(|device| {
                let cursor = cursors.get(device).copied().unwrap_or(0);
                let files = logs.get(device).unwrap_or(&empty);
                if files.contains(&(cursor + 1)) || marks.get(device).and_then(|m| m.last()) > Some(&cursor) {
                    return false;
                }
                files.range(cursor + 1..).next().is_some() || (files.is_empty() && !marks.contains_key(device))
            }))
    }

    /// Reads what other devices wrote. Returns how many registers changed and
    /// whether a file could not be read.
    fn pull(&self, remote: &dyn Remote, me: &str, logs: &Index, marks: &Index, first: bool) -> Result<(u32, bool)> {
        // S17: the marks name the snapshots; `snap/` is listed only when they cannot be relied on.
        let mut snaps = marks.clone();
        if first || self.marks_fall_short(me, logs, marks)? {
            for (device, seqs) in index(&remote.list(&format!("{ROOT}/snap"))?, DATA) {
                snaps.entry(device).or_default().extend(seqs);
            }
        }
        let devices: BTreeSet<&String> = logs.keys().chain(snaps.keys()).filter(|d| d.as_str() != me).collect();
        let mut changed = 0;
        let mut unreadable = false;
        for device in devices {
            loop {
                let cursor = self.cursor(device)?;
                let next = cursor + 1;
                if logs.get(device).is_some_and(|seqs| seqs.contains(&next)) {
                    let Some(data) = remote.get(&format!("{ROOT}/log/{}", file_name(device, next)))? else {
                        break; // compacted away since the listing; the snapshot is picked up next run
                    };
                    let Some(batch) = decode(&data).filter(|b| b.header.device == *device && b.header.seq == next)
                    else {
                        unreadable = true;
                        break;
                    };
                    changed += self.merge(&batch, &BTreeMap::from([(device.clone(), next)]))?;
                    continue;
                }
                // The next log file is gone: catch up from the device's snapshot.
                let Some(snap_seq) = snaps.get(device).and_then(|s| s.last()).filter(|s| **s > cursor) else {
                    break;
                };
                let Some(data) = remote.get(&format!("{ROOT}/snap/{}", file_name(device, *snap_seq)))? else {
                    break;
                };
                let Some(batch) = decode(&data).filter(|b| b.header.device == *device) else {
                    unreadable = true;
                    break;
                };
                let mut cursors = batch.header.covers.clone();
                cursors.remove(me);
                cursors.insert(device.clone(), *snap_seq);
                changed += self.merge(&batch, &cursors)?;
            }
        }
        Ok((changed, unreadable))
    }

    /// Moves unsent changes into the outbox as one numbered file, then uploads the outbox.
    /// The file is fixed before the first upload attempt, so a retry sends the same bytes.
    fn push(&self, remote: &dyn Remote, me: &str) -> Result<u32> {
        {
            let mut inner = self.lock();
            let tx = inner
                .conn
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let changes: Vec<Change> = {
                let mut stmt =
                    tx.prepare("SELECT kind, id, field, value, stamp FROM fields WHERE dirty = 1 ORDER BY stamp")?;
                let rows = stmt.query_map([], change_from_row)?;
                rows.collect::<rusqlite::Result<_>>()?
            };
            if !changes.is_empty() {
                let seq = own_seq(&tx)? + 1;
                let header = Header {
                    v: FORMAT,
                    device: me.to_string(),
                    seq,
                    count: changes.len(),
                    covers: BTreeMap::new(),
                };
                tx.execute(
                    "INSERT INTO outbox (seq, body, changes) VALUES (?1, ?2, ?3)",
                    params![seq as i64, encode(&header, &changes)?, changes.len() as i64],
                )?;
                tx.execute("UPDATE fields SET dirty = 0 WHERE dirty = 1", [])?;
                db::meta_set(&tx, "own_seq", &seq.to_string())?;
            }
            tx.commit()?;
        }
        let mut pushed = 0;
        loop {
            let next: Option<(i64, Vec<u8>, u32)> = self
                .lock()
                .conn
                .query_row("SELECT seq, body, changes FROM outbox ORDER BY seq LIMIT 1", [], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?))
                })
                .optional()?;
            let Some((seq, body, changes)) = next else {
                break;
            };
            remote.put(&format!("{ROOT}/log/{}", file_name(me, seq as u64)), &body)?;
            {
                let mut inner = self.lock();
                let tx = inner.conn.transaction()?;
                tx.execute("DELETE FROM outbox WHERE seq = ?1", [seq])?;
                let bytes = meta_u64(&tx, LOG_BYTES)? + body.len() as u64;
                db::meta_set(&tx, LOG_BYTES, &bytes.to_string())?;
                tx.commit()?;
            }
            pushed += changes;
        }
        Ok(pushed)
    }

    /// S18: keeps this device's record in `push/` in step with the address it was given.
    fn publish_push(&self, remote: &dyn Remote, me: &str) -> Result<()> {
        let wanted = self.push_endpoint()?;
        if wanted == db::meta_get(&self.lock().conn, "push_published")? {
            return Ok(());
        }
        let path = format!("{ROOT}/push/{me}.json");
        match &wanted {
            Some(url) => {
                remote.put(
                    &path,
                    &serde_json::to_vec(&PushRecord {
                        v: PUSH_FORMAT,
                        url: url.clone(),
                    })?,
                )?;
                db::meta_set(&self.lock().conn, "push_published", url)
            }
            None => {
                remote.delete(&path)?;
                db::meta_del(&self.lock().conn, "push_published")
            }
        }
    }

    /// S19: tells the other devices that there is something to read. Nothing
    /// here can fail the run.
    fn poke_peers(&self, remote: &dyn Remote, me: &str) {
        let urls = self.peer_addresses(remote, me);
        self.poke(urls.iter().map(String::as_str));
    }

    /// The addresses the other devices take nudges at (S18). A storage that cannot be read names none.
    fn peer_addresses(&self, remote: &dyn Remote, me: &str) -> Vec<String> {
        let own = format!("{me}.json");
        // R102: without the list no device is nudged, and nothing else would say so.
        let names = remote.list(&format!("{ROOT}/push")).unwrap_or_else(|e| {
            self.note(LogLevel::Error, "push", || {
                format!("addresses of the other devices not read: {e}")
            });
            vec![]
        });
        names
            .iter()
            .filter(|name| **name != own && name.ends_with(".json"))
            .filter_map(|name| remote.get(&format!("{ROOT}/push/{name}")).ok().flatten())
            .filter_map(|data| serde_json::from_slice::<PushRecord>(&data).ok())
            .map(|record| record.url)
            .collect()
    }

    /// The full state of this device as a snapshot numbered `seq`, compressed (S10, S46).
    fn snapshot(&self, me: &str, seq: u64) -> Result<Vec<u8>> {
        let inner = self.lock();
        let changes: Vec<Change> = {
            let mut stmt = inner
                .conn
                .prepare("SELECT kind, id, field, value, stamp FROM fields ORDER BY kind, id, field")?;
            let rows = stmt.query_map([], change_from_row)?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        let mut covers: BTreeMap<String, u64> = {
            let mut stmt = inner.conn.prepare("SELECT device, seq FROM peers")?;
            let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as u64)))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        covers.insert(me.to_string(), seq);
        let header = Header {
            v: FORMAT,
            device: me.to_string(),
            seq,
            count: changes.len(),
            covers,
        };
        compress(&encode(&header, &changes)?)
    }

    /// Replaces this device's log files with one snapshot of its full state.
    fn compact(
        &self,
        remote: &dyn Remote,
        me: &str,
        own_logs: &BTreeSet<u64>,
        own_marks: &BTreeSet<u64>,
    ) -> Result<()> {
        let seq = own_seq(&self.lock().conn)?;
        let body = self.snapshot(me, seq)?;
        let old_snaps = index(&remote.list(&format!("{ROOT}/snap"))?, DATA)
            .remove(me)
            .unwrap_or_default();
        remote.put(&format!("{ROOT}/snap/{}", file_name(me, seq)), &body)?;
        // Only after the snapshot and its mark are in place do the files they replace go away.
        remote.put(&format!("{ROOT}/log/{}", mark_name(me, seq)), b"")?;
        for old in own_logs.iter().filter(|s| **s <= seq) {
            remote.delete(&format!("{ROOT}/log/{}", file_name(me, *old)))?;
        }
        for old in own_marks.iter().filter(|s| **s < seq) {
            remote.delete(&format!("{ROOT}/log/{}", mark_name(me, *old)))?;
        }
        for old in old_snaps.iter().filter(|s| **s < seq) {
            remote.delete(&format!("{ROOT}/snap/{}", file_name(me, *old)))?;
        }
        let inner = self.lock();
        db::meta_set(&inner.conn, SNAP_BYTES, &body.len().to_string())?;
        db::meta_set(&inner.conn, LOG_BYTES, "0")?;
        Ok(())
    }

    /// Content the storage and this device may still owe each other: hash and
    /// whether the storage is known to hold it.
    fn blobs_wanted(&self) -> Result<Vec<(String, bool)>> {
        let inner = self.lock();
        let mut stmt = inner.conn.prepare(
            "SELECT DISTINCT a.sha256, EXISTS (SELECT 1 FROM blobs_uploaded b WHERE b.sha256 = a.sha256)
             FROM attachments a JOIN tasks t ON t.id = a.task_id
             WHERE a.deleted = 0 AND t.purged = 0",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        let wanted: Vec<(String, bool)> = rows.collect::<rusqlite::Result<_>>()?;
        Ok(wanted.into_iter().filter(|(s, _)| is_sha256(s)).collect())
    }

    /// S34: how many of them are not in both places yet.
    fn blobs_waiting(&self) -> Result<u32> {
        let wanted = self.blobs_wanted()?;
        Ok(wanted
            .iter()
            .filter(|(sha256, uploaded)| !uploaded || !self.blob_path(sha256).exists())
            .count() as u32)
    }

    fn blob_uploaded(&self, sha256: &str) -> Result<()> {
        self.lock()
            .conn
            .execute("INSERT OR IGNORE INTO blobs_uploaded (sha256) VALUES (?1)", [sha256])?;
        Ok(())
    }

    /// Whether the content was sent; it is not when the storage already has it.
    fn upload_blob(&self, remote: &dyn Remote, sha256: &str) -> Result<bool> {
        let remote_path = blob_remote_path(sha256);
        let sent = !remote.exists(&remote_path)?;
        if sent {
            // R140: the file goes from the disk, not from a copy of it in memory.
            remote.put_file(&remote_path, &self.blob_path(sha256))?;
        }
        self.blob_uploaded(sha256)?;
        Ok(sent)
    }

    /// Whether the content arrived, and why not when it did not. An encrypted
    /// storage has nothing in the old place to look at (`legacy`, S49).
    fn download_blob(&self, remote: &dyn Remote, sha256: &str, legacy: bool) -> Result<Fetched> {
        let path = blob_remote_path(sha256);
        let local = self.blob_path(sha256);
        // A pass and a request (S35) may fetch the same content at once: each writes its own file.
        let tmp = local.with_extension(format!("{}.part", uuid::Uuid::now_v7().simple()));
        let max = self.attachment_limits().max_file;
        let mut out = HashingWriter::new(private_create(&tmp)?);
        let mut found = match remote.get_to(&path, &mut out, max) {
            Ok(found) => found,
            Err(e) => {
                let _ = std::fs::remove_file(&tmp);
                return Err(e);
            }
        };
        // S49: content a device of an earlier version uploaded to the old place after the move.
        if !found && legacy {
            match remote.rename(&legacy_blob_path(sha256), &path) {
                Ok(true) => {
                    found = match remote.get_to(&path, &mut out, max) {
                        Ok(found) => found,
                        Err(e) => {
                            let _ = std::fs::remove_file(&tmp);
                            return Err(e);
                        }
                    };
                }
                Ok(false) => {}
                Err(e) => {
                    let _ = std::fs::remove_file(&tmp);
                    return Err(e);
                }
            }
        }
        if !found {
            let _ = std::fs::remove_file(&tmp);
            return Ok(Fetched::Missing);
        }
        if let Err(e) = out.flush() {
            let _ = std::fs::remove_file(&tmp);
            return Err(e.into());
        }
        let HashingWriter { inner, hasher, .. } = out;
        drop(inner);
        // The name is the hash: content that does not match is dropped, not stored.
        if hex(&hasher.finalize()) != sha256 {
            let _ = std::fs::remove_file(&tmp);
            return Ok(Fetched::Mismatch);
        }
        if let Err(e) = std::fs::rename(&tmp, &local) {
            let _ = std::fs::remove_file(&tmp);
            return Err(e.into());
        }
        crate::store::private_file(&local)?;
        self.blob_uploaded(sha256)?;
        Ok(Fetched::Stored)
    }

    /// S34: one pass over attachment content against the given storage. A file
    /// that fails is left waiting and the rest go on.
    pub fn sync_attachments_with(&self, remote: &dyn Remote) -> Result<AttachmentReport> {
        let _running = match self.blob_lock.try_lock() {
            Ok(guard) => guard,
            Err(std::sync::TryLockError::Poisoned(p)) => p.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => {
                return Ok(AttachmentReport {
                    waiting: self.blobs_waiting()?,
                    ..AttachmentReport::default()
                });
            }
        };
        let view = self.view(remote)?;
        self.attachments_pass(&view)
    }

    /// S34 with the pass already held.
    fn attachments_pass(&self, view: &View) -> Result<AttachmentReport> {
        let remote = view.remote();
        let mut report = AttachmentReport::default();
        let wanted = self.blobs_wanted()?;
        let mut troubles = self.blob_troubles()?;
        for (sha256, uploaded) in &wanted {
            let (what, outcome) = if self.blob_path(sha256).exists() {
                if *uploaded {
                    (Way::Up, Ok(()))
                } else {
                    let sent = self.upload_blob(remote, sha256);
                    report.uploaded += u32::from(matches!(sent, Ok(true)));
                    (Way::Up, sent.map(|_| ()).map_err(|e| Trouble::Failed(e.to_string())))
                }
            } else {
                let outcome = match self.download_blob(remote, sha256, !view.sealed()) {
                    Ok(Fetched::Stored) => {
                        report.downloaded += 1;
                        Ok(())
                    }
                    Ok(Fetched::Missing) => Err(Trouble::Missing),
                    Ok(Fetched::Mismatch) => Err(Trouble::Failed(MISMATCH.into())),
                    Err(e) => Err(Trouble::Failed(e.to_string())),
                };
                (Way::Down, outcome)
            };
            self.note_blob(&mut troubles, sha256, what, outcome)?;
        }
        if db::meta_get(&self.lock().conn, WHOLE)?.is_some() {
            self.remove_unneeded_blobs(remote)?;
            self.drop_unneeded_blobs()?;
        }
        // A file no longer wanted takes its record of trouble with it.
        for gone in troubles
            .keys()
            .filter(|sha256| !wanted.iter().any(|(w, _)| w == *sha256))
        {
            db::meta_del(&self.lock().conn, &format!("{TROUBLE}{gone}"))?;
        }
        report.waiting = self.blobs_waiting()?;
        if report.uploaded > 0 {
            self.poke_peers(remote, &self.device_id());
        }
        Ok(report)
    }

    /// S47: removes content no attachment needs any more from this device. Only
    /// over a whole state: halfway through a run a reference may not have arrived yet.
    pub(crate) fn drop_unneeded_blobs(&self) -> Result<()> {
        let needed: BTreeSet<String> = {
            let inner = self.lock();
            let mut stmt = inner.conn.prepare(NEEDED)?;
            let rows = stmt.query_map([], |r| r.get(0))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        let Ok(entries) = std::fs::read_dir(self.dir.join("blobs")) else {
            return Ok(());
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if is_sha256(&name) && !needed.contains(&name) {
                std::fs::remove_file(entry.path())?;
            }
        }
        Ok(())
    }

    /// S48: removes unneeded content from the storage, unless the data says a
    /// device did so after it became unneeded, and then says it did.
    fn remove_unneeded_blobs(&self, remote: &dyn Remote) -> Result<()> {
        let unneeded: Vec<(String, String, Option<String>)> = {
            let inner = self.lock();
            let mut stmt = inner.conn.prepare(&format!(
                "SELECT a.sha256, max(f.stamp),
                        (SELECT g.stamp FROM fields g WHERE g.kind = ?1 AND g.id = a.sha256 AND g.field = 'gone')
                 FROM attachments a
                 JOIN fields f ON (f.kind = ?2 AND f.id = a.id AND f.field = 'deleted')
                               OR (f.kind = ?3 AND f.id = a.task_id AND f.field = 'purged')
                 WHERE a.sha256 NOT IN ({NEEDED})
                 GROUP BY a.sha256"
            ))?;
            let rows = stmt.query_map(params![KIND_BLOB, KIND_ATTACHMENT, KIND_TASK], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        for (sha256, unneeded_since, gone) in unneeded {
            if !is_sha256(&sha256) || gone.is_some_and(|g| g > unneeded_since) {
                continue;
            }
            // The record goes out with the next log file, after the content is gone (S48).
            match remote.delete(&blob_remote_path(&sha256)) {
                Ok(()) => self.write(|w| w.set(KIND_BLOB, &sha256, "gone", serde_json::json!(true)))?,
                Err(e) => self.note(LogLevel::Error, "attachments", || {
                    format!(
                        "removal of {}: {}",
                        blob_label(&sha256),
                        hide_hash(&e.to_string(), &sha256)
                    )
                }),
            }
        }
        Ok(())
    }

    /// R102: the files the passes before this one could not move.
    fn blob_troubles(&self) -> Result<BTreeMap<String, BlobTrouble>> {
        let inner = self.lock();
        let mut stmt = inner.conn.prepare("SELECT key, value FROM meta WHERE key LIKE ?1")?;
        let rows = stmt.query_map([format!("{TROUBLE}%")], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?;
        let mut found = BTreeMap::new();
        for row in rows {
            let (key, value) = row?;
            if let (Some(sha256), Some(trouble)) = (key.strip_prefix(TROUBLE), BlobTrouble::parse(&value)) {
                found.insert(sha256.to_string(), trouble);
            }
        }
        Ok(found)
    }

    /// R102: says how one file fared in a pass. A new failure is an error line at
    /// once; content the storage does not have is an event until it has waited an
    /// hour; the same failure again is said at the detailed level and becomes an
    /// error line again after an hour; a file that moves after failing is an event.
    fn note_blob(
        &self,
        troubles: &mut BTreeMap<String, BlobTrouble>,
        sha256: &str,
        way: Way,
        outcome: std::result::Result<(), Trouble>,
    ) -> Result<()> {
        let key = format!("{TROUBLE}{sha256}");
        let target = match way {
            Way::Up => format!("upload of {}", blob_label(sha256)),
            Way::Down => format!("download of {}", blob_label(sha256)),
        };
        let before = troubles.remove(sha256);
        let trouble = match outcome {
            Ok(()) => {
                if let Some(before) = before {
                    let since = before.since.format(TROUBLE_FMT);
                    self.note(LogLevel::Info, "attachments", || {
                        format!("{target}: went through after failing since {since}")
                    });
                    db::meta_del(&self.lock().conn, &key)?;
                }
                return Ok(());
            }
            Err(trouble) => trouble,
        };
        let now = self.lock().now();
        let message = match &trouble {
            Trouble::Failed(e) => format!("{target}: {}", hide_hash(e, sha256)),
            Trouble::Missing => format!("{target}: the storage does not have it"),
        };
        let due = |since: NaiveDateTime| now < since || now - since >= trouble_repeat();
        let record = match before.filter(|b| b.message == message) {
            None => {
                match trouble {
                    Trouble::Failed(_) => self.note(LogLevel::Error, "attachments", || message.clone()),
                    Trouble::Missing => self.note(LogLevel::Info, "attachments", || format!("{message} yet")),
                }
                let said = matches!(trouble, Trouble::Failed(_)).then_some(now);
                BlobTrouble {
                    since: now,
                    said,
                    message,
                }
            }
            Some(mut known) => {
                let since = known.since.format(TROUBLE_FMT).to_string();
                if due(known.said.unwrap_or(known.since)) {
                    match trouble {
                        Trouble::Failed(_) => self.note(LogLevel::Error, "attachments", || {
                            format!("{message} (failing since {since})")
                        }),
                        Trouble::Missing => self.note(LogLevel::Error, "attachments", || {
                            format!("{message}, waiting since {since}")
                        }),
                    }
                    known.said = Some(now);
                } else {
                    self.note(LogLevel::Debug, "attachments", || format!("failed again: {message}"));
                }
                known
            }
        };
        db::meta_set(&self.lock().conn, &key, &record.value())?;
        Ok(())
    }

    /// S56 against the given storage; `encrypt_storage` takes the configured one.
    pub fn encrypt_storage_with(&self, raw: &dyn Remote, passphrase: String) -> Result<()> {
        if passphrase.is_empty() {
            return Err(AppError::invalid("the passphrase is empty"));
        }
        let _running = self.sync_lock.lock().unwrap_or_else(|p| p.into_inner());
        self.replacing(|| self.run_with(raw))?;
        if !matches!(self.sealing_of(raw), Some(Sealing::Clear)) {
            return Err(AppError::invalid("the storage is already encrypted"));
        }
        // What could not be read would be removed unread.
        if db::meta_get(&self.lock().conn, WHOLE)?.is_none() {
            return Err(AppError::sync(
                "a file in the storage could not be read; the storage is left as it is",
            ));
        }
        let _blobs = self.blob_lock.lock().unwrap_or_else(|p| p.into_inner());
        // The content this device lacks is fetched now: the copies in the clear are about to go.
        self.attachments_pass(&View::Clear(raw))?;
        self.seal_storage(raw, passphrase)
    }

    /// S59 against the given storage; `change_storage_passphrase` takes the configured one.
    pub fn change_storage_passphrase_with(&self, raw: &dyn Remote, passphrase: String) -> Result<()> {
        if passphrase.is_empty() {
            return Err(AppError::invalid("the passphrase is empty"));
        }
        let _running = self.sync_lock.lock().unwrap_or_else(|p| p.into_inner());
        let vault = read_vault(raw, &format!("{ROOT}/vault.json"))?
            .filter(|v| v.format == SEALED)
            .ok_or_else(|| AppError::invalid("the storage is not encrypted"))?;
        let wrapped = vault
            .wrapped()
            .ok_or_else(|| AppError::sync("vault.json in the storage is not readable"))?;
        let data = self.data_key(&vault.vault, &wrapped)?;
        let rewrapped = seal::wrap(&passphrase, &vault.vault, &data)?;
        put_sealed_vault(raw, &vault.vault, rewrapped.clone())?;
        *self.vault_key.lock().unwrap_or_else(|p| p.into_inner()) = Some(VaultKey {
            vault: vault.vault,
            wrapped: rewrapped,
            passphrase: passphrase.clone(),
            data,
        });
        *self.sync_passphrase.lock().unwrap_or_else(|p| p.into_inner()) = Some(passphrase);
        Ok(())
    }

    /// One run for the fields against the given storage. Attachment content is
    /// left to `sync_attachments_with` (S34).
    pub fn sync_with(&self, remote: &dyn Remote) -> Result<SyncReport> {
        let _running = self.sync_lock.lock().unwrap_or_else(|p| p.into_inner());
        self.replacing(|| self.run_with(remote))
    }

    fn run_with(&self, raw: &dyn Remote) -> Result<SyncReport> {
        let me = self.device_id();
        let mut report = SyncReport::default();

        // S14, S17: the format and the snapshots are looked at once per storage
        // and process, and again when something does not add up.
        let storage = raw.id();
        let first = self.storage_seen.lock().unwrap_or_else(|p| p.into_inner()).as_deref() != Some(storage.as_str());
        if first {
            self.check_vault(raw)?;
        }
        let view = self.view(raw)?;
        let remote = view.remote();
        let names = remote.list(&format!("{ROOT}/log"))?;
        let mut logs = index(&names, DATA);
        let own_marks = index(&names, MARK).remove(&me).unwrap_or_default();
        let (pulled, unreadable) = self.pull(remote, &me, &logs, &index(&names, MARK), first)?;
        if unreadable && !first {
            let sealed = view.sealed();
            self.check_vault(raw)?;
            // S58: the storage was encrypted since the last look: the run starts over with its key.
            if self.view(raw)?.sealed() != sealed {
                *self.storage_seen.lock().unwrap_or_else(|p| p.into_inner()) = None;
                return self.run_with(raw);
            }
        }
        if unreadable {
            db::meta_del(&self.lock().conn, WHOLE)?;
        } else {
            db::meta_set(&self.lock().conn, WHOLE, "1")?;
        }
        *self.storage_seen.lock().unwrap_or_else(|p| p.into_inner()) = Some(storage);
        // S42: everything the storage held is here, so a task of a project has the mark of its project.
        // A file that could not be read leaves the state halfway; the run that reads it does this.
        if !unreadable {
            self.tidy_whole()?;
        }
        report.pulled = pulled;
        report.pushed = self.push(remote, &me)?;

        let mut own_logs = logs.remove(&me).unwrap_or_default();
        let seq = own_seq(&self.lock().conn)?;
        if report.pushed > 0 {
            own_logs.insert(seq);
        }
        // S45: a snapshot once the log since the last one outweighs it, or once the files are too many.
        let limit = *self.compact_after.lock().unwrap_or_else(|p| p.into_inner());
        let (forced, outweighs) = {
            let conn = &self.lock().conn;
            let snapshot = db::meta_get(conn, SNAP_BYTES)?
                .and_then(|s| s.parse().ok())
                .unwrap_or(SNAP_FLOOR);
            (
                db::meta_get(conn, "force_snapshot")?.is_some(),
                meta_u64(conn, LOG_BYTES)? > snapshot,
            )
        };
        if forced || outweighs || own_logs.len() as u32 > limit {
            self.compact(remote, &me, &own_logs, &own_marks)?;
            db::meta_del(&self.lock().conn, "force_snapshot")?;
        }

        // S41: the addresses are read when they are needed anyway, and once after the server was set.
        let unsettled = db::meta_get(&self.lock().conn, crate::push::UNSETTLED_KEY)?.is_some();
        let peers = (report.pushed > 0 || unsettled).then(|| self.peer_addresses(remote, &me));
        if let Some(peers) = &peers {
            self.share_topic(peers.iter().map(String::as_str))?;
            db::meta_del(&self.lock().conn, crate::push::UNSETTLED_KEY)?;
        }
        self.publish_push(remote, &me)?;
        if let Some(peers) = peers.filter(|_| report.pushed > 0) {
            self.poke(peers.iter().map(String::as_str));
        }
        if view.sealed() {
            self.remove_clear_files(raw)?;
        }
        self.backup_to_storage(remote, &me);
        Ok(report)
    }

    /// The storage as sync reads and writes it: through the key when it is encrypted (S55).
    /// Its `vault.json` is read here when no run of this process has read it yet.
    fn view<'a>(&self, raw: &'a dyn Remote) -> Result<View<'a>> {
        let sealing = match self.sealing_of(raw) {
            Some(sealing) => sealing,
            None => {
                self.check_vault(raw)?;
                self.sealing_of(raw).unwrap_or(Sealing::Clear)
            }
        };
        match sealing {
            Sealing::Clear => Ok(View::Clear(raw)),
            Sealing::Open(keys) => Ok(View::Sealed(SealedRemote::new(raw, keys))),
            Sealing::Locked => Err(AppError::sync(NO_PASSPHRASE)),
        }
    }

    /// S56: removes what the storage held in the clear before it was encrypted.
    /// A removal cut short goes on in the next run.
    fn remove_clear_files(&self, raw: &dyn Remote) -> Result<()> {
        let Some(list) = db::meta_get(&self.lock().conn, CLEAR_FILES)? else {
            return Ok(());
        };
        let paths: Vec<String> = serde_json::from_str(&list).unwrap_or_default();
        for path in &paths {
            raw.delete(path)?;
        }
        self.note(LogLevel::Info, "sync", || {
            format!("removed {} files left in the clear", paths.len())
        });
        db::meta_del(&self.lock().conn, CLEAR_FILES)
    }

    /// S56: writes the state of this device and the content it has, sealed
    /// under a new data key, then `vault.json` of format 4, and only then
    /// removes what lay in the clear. The caller holds both passes and has
    /// just read everything the storage held.
    fn seal_storage(&self, raw: &dyn Remote, passphrase: String) -> Result<()> {
        let me = self.device_id();
        let vault = read_vault(raw, &format!("{ROOT}/vault.json"))?
            .ok_or_else(|| AppError::sync("vault.json is missing from the storage"))?;
        let mut clear = Vec::new();
        for dir in ["log", "snap"] {
            for name in raw.list(&format!("{ROOT}/{dir}"))? {
                clear.push(format!("{ROOT}/{dir}/{name}"));
            }
        }
        for folder in raw.folders(&format!("{ROOT}/blobs"))? {
            for name in raw.list(&format!("{ROOT}/blobs/{folder}"))? {
                if is_sha256(&name) {
                    clear.push(format!("{ROOT}/blobs/{folder}/{name}"));
                }
            }
        }
        // S61: the backups that lay in the clear go with the rest.
        for device in raw.folders(&backups::backups_dir())? {
            for name in raw.list(&format!("{}/{device}", backups::backups_dir()))? {
                clear.push(format!("{}/{device}/{name}", backups::backups_dir()));
            }
        }
        let data = seal::random::<32>();
        let wrapped = seal::wrap(&passphrase, &vault.vault, &data)?;
        let keys = Arc::new(Keys::new(&data));
        let sealed = SealedRemote::new(raw, keys.clone());

        // The snapshot takes a number no device has read yet: every device
        // reads it, a device without the key stops on it (S14), and the files
        // of the others it holds may go.
        let seq = own_seq(&self.lock().conn)? + 1;
        let snap = format!("{ROOT}/snap/{}", file_name(&me, seq));
        let mark = format!("{ROOT}/log/{}", mark_name(&me, seq));
        let body = self.snapshot(&me, seq)?;
        let mut written = vec![snap.clone(), mark.clone()];
        let mut uploaded = Vec::new();
        let sent = (|| -> Result<()> {
            sealed.put(&snap, &body)?;
            sealed.put(&mark, b"")?;
            for (sha256, _) in self.blobs_wanted()? {
                let local = self.blob_path(&sha256);
                if local.exists() {
                    let path = blob_remote_path(&sha256);
                    sealed.put_file(&path, &local)?;
                    written.push(sealed.physical(&path));
                    uploaded.push(sha256);
                }
            }
            // The addresses of the devices stay where they are, sealed.
            for name in raw
                .list(&format!("{ROOT}/push"))?
                .iter()
                .filter(|n| n.ends_with(".json"))
            {
                let path = format!("{ROOT}/push/{name}");
                if let Some(record) = raw.get(&path)? {
                    sealed.put(&path, &record)?;
                }
            }
            clear.retain(|path| !written.contains(path));
            db::meta_set(&self.lock().conn, CLEAR_FILES, &serde_json::to_string(&clear)?)?;
            put_sealed_vault(raw, &vault.vault, wrapped.clone())
        })();
        if let Err(e) = sent {
            // The storage stays as it was; what was written for it goes.
            let _ = db::meta_del(&self.lock().conn, CLEAR_FILES);
            for path in &written {
                let _ = raw.delete(path);
            }
            return Err(e);
        }
        {
            let mut inner = self.lock();
            let tx = inner.conn.transaction()?;
            db::meta_set(&tx, "own_seq", &seq.to_string())?;
            db::meta_set(&tx, SNAP_BYTES, &body.len().to_string())?;
            db::meta_set(&tx, LOG_BYTES, "0")?;
            db::meta_set(&tx, SEALED_VAULT, &vault.vault)?;
            tx.execute("DELETE FROM blobs_uploaded", [])?;
            for sha256 in &uploaded {
                tx.execute("INSERT OR IGNORE INTO blobs_uploaded (sha256) VALUES (?1)", [sha256])?;
            }
            tx.commit()?;
        }
        *self.vault_key.lock().unwrap_or_else(|p| p.into_inner()) = Some(VaultKey {
            vault: vault.vault.clone(),
            wrapped,
            passphrase: passphrase.clone(),
            data,
        });
        *self.sync_passphrase.lock().unwrap_or_else(|p| p.into_inner()) = Some(passphrase);
        *self.sealed.lock().unwrap_or_else(|p| p.into_inner()) = Some((raw.id(), Sealing::Open(keys)));
        self.note(LogLevel::Info, "sync", || "the storage is encrypted".to_string());
        // The storage is encrypted now: a removal that fails is left to the next run.
        if let Err(e) = self.remove_clear_files(raw) {
            self.note(LogLevel::Error, "sync", || {
                format!("files left in the clear not removed: {e}")
            });
        }
        Ok(())
    }

    /// Count and greatest stamp of the registers: the same while nothing was written.
    fn registers_mark(conn: &rusqlite::Connection) -> Result<String> {
        Ok(conn.query_row(
            "SELECT count(*) || ':' || coalesce(max(stamp), '') FROM fields",
            [],
            |r| r.get(0),
        )?)
    }

    /// S38, S39: a run that also puts this device's data over the storage's
    /// when that was asked for. The caller holds the sync lock.
    fn replacing(&self, run: impl Fn() -> Result<SyncReport>) -> Result<SyncReport> {
        if db::meta_get(&self.lock().conn, REPLACE)?.is_none() {
            return run();
        }
        {
            let mut inner = self.lock();
            let tx = inner
                .conn
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            if db::meta_get(&tx, KEPT_AT)?.is_none() {
                tx.execute_batch(
                    "DELETE FROM kept; INSERT INTO kept (kind, id, field, value) SELECT kind, id, field, value FROM fields;",
                )?;
                db::meta_set(&tx, KEPT_AT, &Self::registers_mark(&tx)?)?;
            }
            tx.commit()?;
        }
        let first = match run() {
            Ok(report) => report,
            Err(e) => {
                // Nothing was taken in and nothing was edited: the next attempt
                // remembers anew, with whatever is edited until then.
                let inner = self.lock();
                if db::meta_get(&inner.conn, KEPT_AT)? == Some(Self::registers_mark(&inner.conn)?) {
                    inner.conn.execute("DELETE FROM kept", [])?;
                    db::meta_del(&inner.conn, KEPT_AT)?;
                }
                return Err(e);
            }
        };
        self.put_back()?;
        let second = run()?;
        Ok(SyncReport {
            pulled: first.pulled + second.pulled,
            pushed: first.pushed + second.pushed,
            blobs_uploaded: first.blobs_uploaded + second.blobs_uploaded,
            blobs_downloaded: first.blobs_downloaded + second.blobs_downloaded,
        })
    }

    /// S38: gives every register the value this device remembered, under a new
    /// stamp, and sends what it did not have to the trash.
    fn put_back(&self) -> Result<()> {
        self.write(|w| {
            let rows: Vec<(String, String, String, Option<String>)> = {
                let mut stmt = w.tx.prepare(
                    "SELECT f.kind, f.id, f.field, k.value FROM fields f
                     LEFT JOIN kept k ON k.kind = f.kind AND k.id = f.id AND k.field = f.field
                     ORDER BY f.kind, f.id, f.field",
                )?;
                let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?;
                rows.collect::<rusqlite::Result<_>>()?
            };
            let known: BTreeSet<(String, String)> = {
                let mut stmt = w.tx.prepare("SELECT DISTINCT kind, id FROM kept")?;
                let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
                rows.collect::<rusqlite::Result<_>>()?
            };
            // What has a trash to go to; the inbox and the shared settings cannot be deleted.
            let has_trash = |kind: &str, id: &str| match kind {
                KIND_TASK | KIND_ATTACHMENT | KIND_FILTER => true,
                KIND_LIST => id != INBOX_ID,
                _ => false,
            };
            let mut gone: BTreeSet<(String, String)> = BTreeSet::new();
            for (kind, id, field, kept) in rows {
                match kept {
                    Some(value) => {
                        let value = serde_json::from_str(&value).unwrap_or(serde_json::Value::Null);
                        w.set(&kind, &id, &field, value)?;
                    }
                    None if !known.contains(&(kind.clone(), id.clone())) && has_trash(&kind, &id) => {
                        gone.insert((kind, id));
                    }
                    None => w.set(&kind, &id, &field, serde_json::Value::Null)?,
                }
            }
            for (kind, id) in gone {
                w.set(&kind, &id, "deleted", serde_json::Value::Bool(true))?;
            }
            w.tx.execute("DELETE FROM kept", [])?;
            db::meta_del(w.tx, REPLACE)?;
            db::meta_del(w.tx, KEPT_AT)
        })
    }

    fn password(&self, user: &str) -> Result<String> {
        match self.sync_password.lock().unwrap_or_else(|p| p.into_inner()).clone() {
            Some(password) => Ok(password),
            None if user.is_empty() => Ok(String::new()),
            None => Err(AppError::sync(
                "the password is not available on this device; enter it in the sync settings",
            )),
        }
    }

    /// The configured file storage; `None` when sync is off or goes through CalDAV.
    fn file_storage(&self) -> Result<Option<Box<dyn Remote>>> {
        Ok(match self.sync_config()? {
            SyncConfig::Off | SyncConfig::CalDav { .. } => None,
            SyncConfig::Folder { path } => Some(Box::new(DirRemote::new(path))),
            SyncConfig::WebDav { url, user } => Some(Box::new(WebDavRemote::new(&url, &user, &self.password(&user)?)?)),
        })
    }

    /// Runs the configured kind of sync; `None` when sync is off.
    fn run_configured(&self) -> Result<Option<SyncReport>> {
        // The settings are read under the lock: a run never starts with a
        // storage that was replaced while it waited (S36).
        let _running = self.sync_lock.lock().unwrap_or_else(|p| p.into_inner());
        let config = self.sync_config()?;
        Ok(Some(match config {
            SyncConfig::Off => return Ok(None),
            SyncConfig::Folder { .. } | SyncConfig::WebDav { .. } => {
                let Some(remote) = self.file_storage()? else {
                    return Ok(None);
                };
                let report = self.replacing(|| self.run_with(remote.as_ref()))?;
                if matches!(config, SyncConfig::Folder { .. }) {
                    // What this run wrote is not news to it.
                    self.folder_changed();
                }
                report
            }
            SyncConfig::CalDav { url, user } => {
                let password = self.password(&user)?;
                self.replacing(|| crate::caldav::engine::run(self, &url, &user, &password))?
            }
        }))
    }

    /// Whether this device has a task out of the trash or a list of its own (S36, R88).
    pub(crate) fn holds_data(&self) -> Result<bool> {
        Ok(self.lock().conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM tasks WHERE deleted = 0 AND purged = 0)
                 OR EXISTS (SELECT 1 FROM lists WHERE deleted = 0 AND id != ?1)",
            [INBOX_ID],
            |r| r.get(0),
        )?)
    }

    /// S37 without the lock: the caller holds it.
    fn drop_local(&self) -> Result<()> {
        // Over CalDAV the shared settings do not travel (S32): they are this device's own.
        let own_settings = matches!(self.sync_config()?, SyncConfig::CalDav { .. });
        self.renew(None, own_settings)
    }

    /// Replaces the registers and makes this a device the storage has not seen
    /// (S37, S40): with nothing, or with the registers of the database at
    /// `from` (R90). One transaction: a failure leaves what was there. The
    /// caller holds the sync lock.
    pub(crate) fn renew(&self, from: Option<&std::path::Path>, keep_settings: bool) -> Result<()> {
        let mut inner = self.lock();
        let inner = &mut *inner;
        if let Some(path) = from {
            inner
                .conn
                .execute("ATTACH DATABASE ?1 AS restored", [path.to_string_lossy()])?;
        }
        let done = Self::renew_in(inner, from.is_some(), keep_settings);
        if from.is_some() {
            // Whatever happened above, the file is let go.
            let _ = inner.conn.execute("DETACH DATABASE restored", []);
        }
        done?;
        // R112: a copy made before the rule of subtasks holds what does not fit it.
        inner.write(|w| w.tidy())?;
        *self.storage_seen.lock().unwrap_or_else(|p| p.into_inner()) = None;
        *self.folder_seen.lock().unwrap_or_else(|p| p.into_inner()) = None;
        *self.sealed.lock().unwrap_or_else(|p| p.into_inner()) = None;
        Ok(())
    }

    fn renew_in(inner: &mut crate::store::Inner, restore: bool, keep_settings: bool) -> Result<()> {
        let last = db::max_stamp(&inner.conn)?;
        let device = new_device_id();
        let tx = inner
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        if keep_settings {
            tx.execute("DELETE FROM fields WHERE kind != ?1", [KIND_SETTINGS])?;
        } else {
            tx.execute("DELETE FROM fields", [])?;
        }
        tx.execute_batch(
            "DELETE FROM kept;
             DELETE FROM tasks; DELETE FROM task_tags; DELETE FROM attachments; DELETE FROM filters; DELETE FROM lists;
             INSERT INTO lists (id) VALUES ('inbox');
             DELETE FROM peers; DELETE FROM outbox; DELETE FROM blobs_uploaded;
             DELETE FROM caldav_calendars; DELETE FROM caldav_items;
             DELETE FROM meta WHERE key IN ('sync_ok', 'sync_error', 'caldav_filters_sent', 'caldav_home', 'push_published',
                                            'own_seq', 'force_snapshot', 'replace_remote', 'replace_kept_at',
                                            'log_bytes', 'snap_bytes', 'blobs_whole', 'sealed_vault');",
        )?;
        if restore {
            // The file comes from outside: only registers this version would accept from a peer get in.
            let changes: Vec<Change> = {
                let mut stmt = tx.prepare("SELECT kind, id, field, value, stamp FROM restored.fields")?;
                let rows = stmt.query_map([], change_from_row)?;
                rows.collect::<rusqlite::Result<_>>()?
            };
            let mut touched = Touched::new();
            for change in changes.iter().filter(|c| c.is_well_formed()) {
                // Everything is to be uploaded: the storage has not seen this device (S40).
                db::apply(&tx, change, true, &mut touched)?;
            }
            db::settle(&tx, &touched)?;
            db::meta_set(&tx, "force_snapshot", "1")?;
        } else if keep_settings {
            tx.execute("UPDATE fields SET dirty = 1", [])?;
        }
        db::meta_set(&tx, "device", &device)?;
        let newest = db::max_stamp(&tx)?;
        tx.commit()?;
        // The clock stays ahead of every stamp this device has issued or holds.
        inner.clock = Clock::new(&device, last.as_deref());
        if let Some(stamp) = newest {
            inner.clock.observe(&stamp);
        }
        Ok(())
    }

    /// S38 without the lock: the caller holds it.
    pub(crate) fn ask_to_replace_remote(&self) -> Result<()> {
        let inner = self.lock();
        inner.conn.execute("DELETE FROM kept", [])?;
        db::meta_del(&inner.conn, KEPT_AT)?;
        db::meta_set(&inner.conn, REPLACE, "1")
    }

    fn store_config(&self, config: SyncConfig) -> Result<()> {
        if config == self.sync_config()? {
            return Ok(());
        }
        let stored = match config {
            SyncConfig::Off => None,
            SyncConfig::Folder { path } => Some(StoredConfig::Folder { path }),
            SyncConfig::WebDav { url, user } => {
                WebDavRemote::new(&url, &user, "")?;
                Some(StoredConfig::Webdav { url, user })
            }
            SyncConfig::CalDav { url, user } => {
                crate::caldav::client::Client::validate(&url)?;
                Some(StoredConfig::Caldav { url, user })
            }
        };
        let mut inner = self.lock();
        let tx = inner
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        match stored {
            Some(config) => db::meta_set(&tx, "sync", &serde_json::to_string(&config)?)?,
            None => db::meta_del(&tx, "sync")?,
        }
        // A different storage knows nothing of this device. Log numbering goes on
        // (a storage seen before must not get a second file with an old number),
        // and the first run publishes a snapshot so that readers there can start from it.
        tx.execute_batch(
            "DELETE FROM peers; DELETE FROM outbox; DELETE FROM blobs_uploaded;
             DELETE FROM caldav_calendars; DELETE FROM caldav_items; DELETE FROM kept;
             DELETE FROM meta WHERE key IN ('sync_ok', 'sync_error', 'caldav_filters_sent', 'caldav_home', 'push_published',
                                            'replace_remote', 'replace_kept_at', 'log_bytes', 'snap_bytes',
                                            'blobs_whole', 'sealed_vault', 'clear_files');
             UPDATE fields SET dirty = 1;",
        )?;
        db::meta_set(&tx, "force_snapshot", "1")?;
        tx.commit()?;
        drop(inner);
        *self.sealed.lock().unwrap_or_else(|p| p.into_inner()) = None;
        Ok(())
    }
}

fn meta_u64(conn: &rusqlite::Connection, key: &str) -> Result<u64> {
    Ok(db::meta_get(conn, key)?.and_then(|s| s.parse().ok()).unwrap_or(0))
}

fn own_seq(conn: &rusqlite::Connection) -> Result<u64> {
    meta_u64(conn, "own_seq")
}

fn change_from_row(r: &rusqlite::Row) -> rusqlite::Result<Change> {
    let value: String = r.get(3)?;
    Ok(Change {
        kind: r.get(0)?,
        id: r.get(1)?,
        field: r.get(2)?,
        value: serde_json::from_str(&value).unwrap_or(serde_json::Value::Null),
        stamp: r.get(4)?,
    })
}

/// Tests sync settings before they are saved: asks the storage at `config`
/// with `password` and writes nothing, neither there nor here (S26, C26).
#[uniffi::export]
pub fn check_sync_connection(config: SyncConfig, password: String) -> Result<ConnectionCheck> {
    match config {
        SyncConfig::WebDav { url, user } => WebDavRemote::new(&url, &user, &password)?.check(),
        SyncConfig::CalDav { url, user } => {
            crate::caldav::client::Client::check(&url, &user, &password).map(|()| ConnectionCheck::Ready)
        }
        SyncConfig::Off | SyncConfig::Folder { .. } => {
            Err(AppError::invalid("only WebDAV and CalDAV settings can be tested"))
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
enum StoredConfig {
    Folder { path: String },
    Webdav { url: String, user: String },
    Caldav { url: String, user: String },
}

#[uniffi::export]
impl Store {
    pub fn sync_config(&self) -> Result<SyncConfig> {
        let stored = db::meta_get(&self.lock().conn, "sync")?;
        Ok(
            match stored.and_then(|s| serde_json::from_str::<StoredConfig>(&s).ok()) {
                Some(StoredConfig::Folder { path }) => SyncConfig::Folder { path },
                Some(StoredConfig::Webdav { url, user }) => SyncConfig::WebDav { url, user },
                Some(StoredConfig::Caldav { url, user }) => SyncConfig::CalDav { url, user },
                None => SyncConfig::Off,
            },
        )
    }

    /// Keeps the WebDAV password for this process only. It is held in memory and
    /// never reaches the database; persisting it is the job of the platform's
    /// secret storage.
    pub fn set_sync_password(&self, password: Option<String>) {
        *self.sync_password.lock().unwrap_or_else(|p| p.into_inner()) = password;
    }

    /// S58: keeps the passphrase of an encrypted storage for this process only,
    /// the way `set_sync_password` keeps the password. A different one makes
    /// the next run read `vault.json` again and open it with this one.
    pub fn set_sync_passphrase(&self, passphrase: Option<String>) {
        let passphrase = passphrase.filter(|p| !p.is_empty());
        let mut kept = self.sync_passphrase.lock().unwrap_or_else(|p| p.into_inner());
        if *kept == passphrase {
            return;
        }
        *kept = passphrase;
        drop(kept);
        *self.sealed.lock().unwrap_or_else(|p| p.into_inner()) = None;
        *self.storage_seen.lock().unwrap_or_else(|p| p.into_inner()) = None;
    }

    /// S56: encrypts the WebDAV or folder storage with `passphrase`. The device
    /// syncs first, then writes its state and the content of its attachments
    /// sealed, then `vault.json` of format 4, and removes what lay in the clear.
    /// Takes as long as a sync with every attachment: call it off the main thread.
    /// On success the passphrase is kept as with `set_sync_passphrase`; the app
    /// stores it where it stores the password.
    pub fn encrypt_storage(&self, passphrase: String) -> Result<()> {
        let Some(raw) = self.file_storage()? else {
            return Err(AppError::invalid("only a WebDAV or folder storage can be encrypted"));
        };
        self.encrypt_storage_with(raw.as_ref(), passphrase)
    }

    /// S59: wraps the data key of the encrypted storage under a new passphrase
    /// and rewrites only `vault.json`. The other devices ask for the new one
    /// the next time they read `vault.json`.
    pub fn change_storage_passphrase(&self, passphrase: String) -> Result<()> {
        let Some(raw) = self.file_storage()? else {
            return Err(AppError::invalid("only a WebDAV or folder storage can be encrypted"));
        };
        self.change_storage_passphrase_with(raw.as_ref(), passphrase)
    }

    /// Stores where to sync. Switching storage keeps local data; the next run
    /// merges it with whatever the new storage holds.
    pub fn set_sync_config(&self, config: SyncConfig) -> Result<()> {
        self.join_storage(config, SyncSide::Merge)
    }

    /// Stores where to sync and which side to keep there (S36). Replacing a
    /// side waits for a run that is under way and lets none start in between,
    /// so it may take as long as that run: call it off the main thread.
    pub fn join_storage(&self, config: SyncConfig, side: SyncSide) -> Result<()> {
        if side == SyncSide::Merge {
            return self.store_config(config);
        }
        let _running = self.sync_lock.lock().unwrap_or_else(|p| p.into_inner());
        self.store_config(config)?;
        match side {
            SyncSide::Storage => self.drop_local(),
            _ => self.ask_to_replace_remote(),
        }
    }

    /// S36: whether both this device and the storage at `config` hold data, so
    /// that the person has a side to choose. Only reads, here and there.
    pub fn sync_conflict(&self, config: SyncConfig, password: String) -> Result<bool> {
        if !self.holds_data()? {
            return Ok(false);
        }
        // S49: a storage not moved yet holds its files in the old place.
        let files = |remote: &dyn Remote| -> Result<bool> {
            for root in [ROOT, LEGACY] {
                for dir in ["log", "snap"] {
                    let names = remote.list(&format!("{root}/{dir}"))?;
                    if names.iter().any(|name| parse_name(name, DATA).is_some()) {
                        return Ok(true);
                    }
                }
            }
            Ok(false)
        };
        match config {
            SyncConfig::Off => Ok(false),
            SyncConfig::Folder { path } => files(&DirRemote::new(path)),
            SyncConfig::WebDav { url, user } => files(&WebDavRemote::new(&url, &user, &password)?),
            SyncConfig::CalDav { url, user } => crate::caldav::engine::holds_objects(&url, &user, &password),
        }
    }

    /// S37: drops what this device holds and makes it a new device, so that the
    /// next run reads the storage from the start. Changes not uploaded yet are
    /// lost; attachment content on disk stays.
    pub fn replace_local_with_remote(&self) -> Result<()> {
        let _running = self.sync_lock.lock().unwrap_or_else(|p| p.into_inner());
        self.drop_local()
    }

    /// S38: asks the next run to put the data of this device over what the
    /// storage and the other devices hold. Until a run gets through, the
    /// request waits (S39).
    pub fn replace_remote_with_local(&self) -> Result<()> {
        let _running = self.sync_lock.lock().unwrap_or_else(|p| p.into_inner());
        self.ask_to_replace_remote()
    }

    /// For sync through a folder: whether a file appeared in its log or left
    /// it since the last call or the last run. Cheap enough to ask every couple
    /// of seconds; with any other kind of sync the answer is no.
    pub fn folder_changed(&self) -> bool {
        let Ok(SyncConfig::Folder { path }) = self.sync_config() else {
            return false;
        };
        let Ok(mut names) = DirRemote::new(path).list(&format!("{ROOT}/log")) else {
            return false;
        };
        names.sort();
        let mut seen = self.folder_seen.lock().unwrap_or_else(|p| p.into_inner());
        let changed = seen.as_ref() != Some(&names);
        *seen = Some(names);
        changed
    }

    /// Runs one sync. Does nothing when sync is not configured.
    pub fn sync_now(&self) -> Result<SyncReport> {
        let result = match self.run_configured() {
            Ok(None) => return Ok(SyncReport::default()),
            Ok(Some(report)) => Ok(report),
            Err(e) => Err(e),
        };
        // R102: a run that changed nothing is said at the most detailed level only, and so is a
        // failure that repeats the one before it: a device left without a network fails on every run.
        let before = db::meta_get(&self.lock().conn, "sync_error")?;
        match &result {
            Err(e) if before.as_deref() == Some(e.to_string().as_str()) => {
                self.note(LogLevel::Debug, "sync", || format!("failed again: {e}"))
            }
            _ => {
                let quiet = matches!(&result, Ok(report) if report.pulled == 0 && report.pushed == 0);
                let level = if quiet { LogLevel::Debug } else { LogLevel::Info };
                self.note_result("sync", &result, level, |report| {
                    format!("run finished: {} received, {} sent", report.pulled, report.pushed)
                });
            }
        }
        let inner = self.lock();
        match &result {
            Ok(_) => {
                db::meta_set(
                    &inner.conn,
                    "sync_ok",
                    &inner.now().format("%Y-%m-%dT%H:%M").to_string(),
                )?;
                // A run that got through may still have left something unread (C23).
                match crate::caldav::engine::unreadable(&inner.conn)? {
                    Some(problem) => db::meta_set(&inner.conn, "sync_error", &problem)?,
                    None => db::meta_del(&inner.conn, "sync_error")?,
                }
            }
            Err(e) => db::meta_set(&inner.conn, "sync_error", &e.to_string())?,
        }
        result
    }

    /// S34: moves attachment content after a run of `sync_now`: uploads what
    /// the storage lacks and downloads what this device lacks. A file that
    /// fails stays waiting and does not fail the call. Does nothing with
    /// CalDAV, where the content travels inside the task, and while another
    /// pass is going.
    pub fn sync_attachments(&self) -> Result<AttachmentReport> {
        let Some(remote) = self.file_storage()? else {
            return Ok(AttachmentReport::default());
        };
        let result = self.sync_attachments_with(remote.as_ref());
        self.note_result("attachments", &result, LogLevel::Debug, |report| {
            format!(
                "pass finished: {} sent, {} received, {} waiting",
                report.uploaded, report.downloaded, report.waiting
            )
        });
        result
    }

    /// S35: downloads the content of one attachment now, ahead of the pass.
    pub fn fetch_attachment(&self, id: String) -> Result<Attachment> {
        let found = |store: &Store| -> Result<Attachment> {
            let task: Option<String> = store
                .lock()
                .conn
                .query_row(
                    "SELECT task_id FROM attachments WHERE id = ?1 AND deleted = 0",
                    [&id],
                    |r| r.get(0),
                )
                .optional()?;
            let all = match task {
                Some(task) => store.attachments(task)?,
                None => vec![],
            };
            all.into_iter()
                .find(|a| a.id == id)
                .ok_or_else(|| AppError::not_found("attachment"))
        };
        let attachment = found(self)?;
        if attachment.local_path.is_some() {
            return Ok(attachment);
        }
        if !is_sha256(&attachment.sha256) {
            return Err(AppError::sync("the attachment has no content to download"));
        }
        let fetched = (|| {
            let Some(raw) = self.file_storage()? else {
                return Err(AppError::sync("the content of this attachment is not in the storage"));
            };
            let view = self.view(raw.as_ref())?;
            match self.download_blob(view.remote(), &attachment.sha256, !view.sealed())? {
                Fetched::Stored => Ok(()),
                Fetched::Missing => Err(AppError::sync(
                    "the content of this attachment has not reached the storage yet",
                )),
                Fetched::Mismatch => Err(AppError::sync(MISMATCH)),
            }
        })();
        // R102: the app shows that the download failed; the log keeps why.
        if let Err(e) = &fetched {
            let target = blob_label(&attachment.sha256);
            let reason = hide_hash(&e.to_string(), &attachment.sha256);
            self.note(LogLevel::Error, "attachments", || {
                format!("download of {target}: {reason}")
            });
        }
        fetched?;
        found(self)
    }

    pub fn sync_status(&self) -> Result<SyncStatus> {
        let config = self.sync_config()?;
        let configured = config != SyncConfig::Off;
        let files = matches!(config, SyncConfig::Folder { .. } | SyncConfig::WebDav { .. });
        let attachments_waiting = if files { self.blobs_waiting()? } else { 0 };
        let seen_sealed = matches!(
            &*self.sealed.lock().unwrap_or_else(|p| p.into_inner()),
            Some((_, Sealing::Locked | Sealing::Open(_)))
        );
        let inner = self.lock();
        let encrypted = files && (seen_sealed || db::meta_get(&inner.conn, SEALED_VAULT)?.is_some());
        let pending: u32 = inner.conn.query_row(
            "SELECT (SELECT count(*) FROM fields WHERE dirty = 1) + (SELECT coalesce(sum(changes), 0) FROM outbox)",
            [],
            |r| r.get(0),
        )?;
        Ok(SyncStatus {
            configured,
            pending: if configured { pending } else { 0 },
            attachments_waiting,
            last_ok: db::meta_get(&inner.conn, "sync_ok")?,
            last_error: db::meta_get(&inner.conn, "sync_error")?,
            encrypted,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip() {
        assert_eq!(parse_name(&file_name("abc", 42), DATA), Some(("abc".into(), 42)));
        assert_eq!(parse_name("abc-42.jsonl", DATA), None);
        assert_eq!(parse_name("readme.txt", DATA), None);
        // A mark is not a batch and a batch is not a mark.
        assert_eq!(parse_name(&mark_name("abc", 42), MARK), Some(("abc".into(), 42)));
        assert_eq!(parse_name(&mark_name("abc", 42), DATA), None);
        assert_eq!(parse_name(&file_name("abc", 42), MARK), None);
    }

    #[test]
    fn truncated_file_is_rejected() {
        let change = Change {
            kind: "task".into(),
            id: "1".into(),
            field: "title".into(),
            value: serde_json::json!("x"),
            stamp: "000000000001-0000-aaaaaaaaaaaa".into(),
        };
        let header = Header {
            v: 1,
            device: "aaaaaaaaaaaa".into(),
            seq: 1,
            count: 2,
            covers: BTreeMap::new(),
        };
        let full = encode(&header, &[change.clone(), change]).unwrap();
        assert!(decode(&full).is_some());
        assert!(decode(&full[..full.len() - 1]).is_none());
        assert!(decode(&full[..full.len() / 2]).is_none());
        assert!(decode(b"").is_none());
    }
}
