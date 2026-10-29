//! Backups of the data and restoring from them: R87–R90 in docs/specs/product.md.
//!
//! ```text
//! Lists_(2026-10-09-11_15_05).listsbackup    a zip:
//!   manifest.json     {"format":1,"created":"2026-10-09T11:15:05","fields":…,"blobs":…}
//!   lists.sqlite      table `fields`: every register with its stamp
//!   blobs/<sha256>    attachment content that is on the device
//! ```

use std::collections::BTreeSet;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::db::{self, is_sha256};
use crate::error::{AppError, Result};
use crate::model::SyncConfig;
use crate::store::{hex, Store};
use crate::LogLevel;

const FORMAT: u32 = 1;
const EXT: &str = ".listsbackup";
const PREFIX: &str = "Lists_(";
const NAME_FMT: &str = "%Y-%m-%d-%H_%M_%S";
pub(crate) const SECONDS_FMT: &str = "%Y-%m-%dT%H:%M:%S";
const MANIFEST: &str = "manifest.json";
const DATABASE: &str = "lists.sqlite";
const BLOBS: &str = "blobs/";

/// How often automatic backups are made, in hours; 0 turns them off.
pub const EVERY: [u32; 3] = [0, 24, 48];
/// How many backups are kept at most.
pub const KEEP: [u32; 4] = [5, 10, 20, 30];

#[derive(Serialize, Deserialize)]
struct Manifest {
    format: u32,
    created: String,
    fields: u64,
    blobs: u64,
}

/// A backup in the app's own folder (R89).
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct Backup {
    /// File name; what `delete_backup` takes.
    pub name: String,
    pub path: String,
    /// `YYYY-MM-DDTHH:MM`, local time of the device that made it.
    pub created: String,
    /// Bytes.
    pub size: u64,
}

/// Settings of this device, not synced (R88).
#[derive(Debug, Clone, Copy, PartialEq, uniffi::Record)]
pub struct BackupSettings {
    /// Hours between automatic backups: 0 (off), 24 or 48.
    pub every_hours: u32,
    /// 5, 10, 20 or 30.
    pub keep: u32,
}

fn invalid(msg: impl Into<String>) -> AppError {
    AppError::invalid(msg)
}

pub(crate) fn created_of(name: &str) -> Option<NaiveDateTime> {
    let stem = name.strip_prefix(PREFIX)?.strip_suffix(EXT)?;
    // A second backup within one second carries a counter after the bracket.
    let (moment, _) = stem.split_once(')')?;
    NaiveDateTime::parse_from_str(moment, NAME_FMT).ok()
}

/// R142: `backups/` and the copies in it, as an earlier version may have left them, closed from
/// other users. A folder that is not there yet is left to the first backup.
pub(crate) fn close_backups(dir: &Path) -> Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }
    crate::store::private_dir(dir)?;
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_file() {
            crate::store::private_file(&path)?;
        }
    }
    Ok(())
}

impl Store {
    pub(crate) fn backups_dir(&self) -> PathBuf {
        self.dir.join("backups")
    }

    fn backup_at(&self, name: &str) -> Result<PathBuf> {
        if created_of(name).is_none() || name.contains(['/', '\\']) {
            return Err(invalid("not a backup of this app"));
        }
        Ok(self.backups_dir().join(name))
    }

    /// Writes the archive and returns it; rotation is the caller's.
    fn write_backup(&self) -> Result<Backup> {
        let dir = self.backups_dir();
        crate::store::make_private_dir(&dir)?;
        let now = self.lock().now();
        let moment = now.format(NAME_FMT).to_string();
        let mut name = format!("{PREFIX}{moment}){EXT}");
        let mut n = 1;
        while dir.join(&name).exists() {
            n += 1;
            name = format!("{PREFIX}{moment})_{n}{EXT}");
        }
        let target = dir.join(&name);
        let snapshot = dir.join(format!(".{}.sqlite", uuid::Uuid::now_v7().simple()));
        let part = dir.join(format!(".{}.part", uuid::Uuid::now_v7().simple()));
        let written = self.write_archive(&snapshot, &part, &now, true);
        let _ = std::fs::remove_file(&snapshot);
        let placed = written
            .and_then(|()| Ok(std::fs::rename(&part, &target)?))
            .and_then(|()| Ok(crate::store::private_file(&target)?));
        if let Err(e) = placed {
            let _ = std::fs::remove_file(&part);
            self.note(LogLevel::Error, "backup", || format!("not created: {e}"));
            return Err(e);
        }
        self.note(LogLevel::Info, "backup", || format!("created {name}"));
        Ok(Backup {
            size: std::fs::metadata(&target)?.len(),
            path: target.to_string_lossy().into_owned(),
            created: now.format("%Y-%m-%dT%H:%M").to_string(),
            name,
        })
    }

    /// S60: an archive for the storage written to `part`, without attachment content, and the
    /// name it goes under.
    pub(crate) fn write_storage_backup(&self, part: &Path) -> Result<String> {
        let dir = self.backups_dir();
        crate::store::make_private_dir(&dir)?;
        let now = self.lock().now();
        let snapshot = dir.join(format!(".{}.sqlite", uuid::Uuid::now_v7().simple()));
        let written = self.write_archive(&snapshot, part, &now, false);
        let _ = std::fs::remove_file(&snapshot);
        written?;
        Ok(format!("{PREFIX}{}){EXT}", now.format(NAME_FMT)))
    }

    /// `content`: whether the attachment content on the device goes into the archive.
    fn write_archive(&self, snapshot: &Path, part: &Path, now: &NaiveDateTime, content: bool) -> Result<()> {
        // The registers and the list of content are taken at one moment.
        let (fields, hashes): (u64, BTreeSet<String>) = {
            let inner = self.lock();
            inner
                .conn
                .execute("ATTACH DATABASE ?1 AS snapshot", [snapshot.to_string_lossy()])?;
            let copied = inner.conn.execute(
                "CREATE TABLE snapshot.fields AS SELECT kind, id, field, value, stamp FROM fields",
                [],
            );
            let _ = inner.conn.execute("DETACH DATABASE snapshot", []);
            copied?;
            let fields = inner.conn.query_row("SELECT count(*) FROM fields", [], |r| r.get(0))?;
            // Read from the registers, not from the derived table: before a migration (R92)
            // the derived tables are as an earlier version left them.
            let mut stmt = inner.conn.prepare(
                "SELECT DISTINCT json_extract(value, '$') FROM fields
                 WHERE kind = ?1 AND field = 'sha256' AND json_valid(value) AND json_type(value) = 'text'",
            )?;
            let rows = stmt.query_map([db::KIND_ATTACHMENT], |r| r.get::<_, String>(0))?;
            let hashes = rows.collect::<rusqlite::Result<_>>()?;
            (fields, hashes)
        };
        let present: Vec<String> = hashes
            .into_iter()
            .filter(|h| content && is_sha256(h) && self.blob_path(h).is_file())
            .collect();
        let manifest = Manifest {
            format: FORMAT,
            created: now.format(SECONDS_FMT).to_string(),
            fields,
            blobs: present.len() as u64,
        };
        let zipped = |e: zip::result::ZipError| AppError::Storage { msg: e.to_string() };
        let mut zip = zip::ZipWriter::new(std::fs::File::create(part)?);
        let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        zip.start_file(MANIFEST, options).map_err(zipped)?;
        zip.write_all(&serde_json::to_vec(&manifest)?)?;
        zip.start_file(DATABASE, options).map_err(zipped)?;
        std::io::copy(&mut std::fs::File::open(snapshot)?, &mut zip)?;
        for hash in &present {
            zip.start_file(format!("{BLOBS}{hash}"), options.large_file(true))
                .map_err(zipped)?;
            std::io::copy(&mut std::fs::File::open(self.blob_path(hash))?, &mut zip)?;
        }
        zip.finish().map_err(zipped)?.sync_all()?;
        Ok(())
    }

    /// R88: removes the oldest backups beyond the number to keep.
    fn rotate(&self) -> Result<()> {
        let keep = self.backup_settings()?.keep as usize;
        for old in self.backups()?.into_iter().skip(keep) {
            std::fs::remove_file(old.path)?;
        }
        Ok(())
    }
}

#[uniffi::export]
impl Store {
    pub fn backup_settings(&self) -> Result<BackupSettings> {
        let inner = self.lock();
        let read =
            |key: &str| -> Result<Option<u32>> { Ok(db::meta_get(&inner.conn, key)?.and_then(|v| v.parse().ok())) };
        Ok(BackupSettings {
            every_hours: read("backup_every")?.filter(|v| EVERY.contains(v)).unwrap_or(24),
            keep: read("backup_keep")?.filter(|v| KEEP.contains(v)).unwrap_or(5),
        })
    }

    /// A smaller number to keep takes effect with the next backup, not at once.
    pub fn set_backup_settings(&self, settings: BackupSettings) -> Result<()> {
        if !EVERY.contains(&settings.every_hours) || !KEEP.contains(&settings.keep) {
            return Err(invalid("backup settings out of range"));
        }
        let inner = self.lock();
        db::meta_set(&inner.conn, "backup_every", &settings.every_hours.to_string())?;
        db::meta_set(&inner.conn, "backup_keep", &settings.keep.to_string())
    }

    /// The backups in the app's folder, newest first (R89).
    pub fn backups(&self) -> Result<Vec<Backup>> {
        let entries = match std::fs::read_dir(self.backups_dir()) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
            Err(e) => return Err(e.into()),
        };
        let mut found = Vec::new();
        for entry in entries {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(created) = created_of(&name) else { continue };
            found.push(Backup {
                size: entry.metadata()?.len(),
                path: entry.path().to_string_lossy().into_owned(),
                created: created.format("%Y-%m-%dT%H:%M").to_string(),
                name,
            });
        }
        found.sort_by(|a, b| b.name.cmp(&a.name));
        Ok(found)
    }

    /// "Create backup now" (R89): makes a backup, then drops the oldest beyond the number to keep.
    pub fn create_backup(&self) -> Result<Backup> {
        let backup = self.write_backup()?;
        // R102: the archive is there, but old ones are not removed and the folder grows.
        if let Err(e) = self.rotate() {
            self.note(LogLevel::Error, "backup", || format!("old backups not removed: {e}"));
            return Err(e);
        }
        Ok(backup)
    }

    /// R88: makes the automatic backup when its time has come. Cheap to call often.
    pub fn backup_if_due(&self) -> Result<Option<Backup>> {
        let _looking = self.backup_lock.lock().unwrap_or_else(|p| p.into_inner());
        // R102: a failure of the archive itself is said where it is written; the rest is said here.
        let mut said = false;
        let due = (|| {
            let every = self.backup_settings()?.every_hours;
            if every == 0 || !self.holds_data()? {
                return Ok(None);
            }
            let (now, last) = {
                let inner = self.lock();
                let last = db::meta_get(&inner.conn, "backup_last")?
                    .and_then(|v| NaiveDateTime::parse_from_str(&v, SECONDS_FMT).ok());
                (inner.now(), last)
            };
            // A clock set back does not hold the backups off for good.
            if last.is_some_and(|last| last <= now && now - last < chrono::Duration::hours(every.into())) {
                return Ok(None);
            }
            let backup = self.create_backup().inspect_err(|_| said = true)?;
            db::meta_set(&self.lock().conn, "backup_last", &now.format(SECONDS_FMT).to_string())?;
            Ok(Some(backup))
        })();
        if let Err(e) = &due {
            if !said {
                self.note(LogLevel::Error, "backup", || format!("not created: {e}"));
            }
        }
        due
    }

    pub fn delete_backup(&self, name: String) -> Result<()> {
        match std::fs::remove_file(self.backup_at(&name)?) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
            _ => Ok(()),
        }
    }

    /// R90: replaces the data with what the backup at `path` holds, after a
    /// backup of the present state. With sync on, `over_storage` puts the
    /// restored state over the storage and the other devices (S40); without
    /// it the two are merged.
    pub fn restore_backup(&self, path: String, over_storage: bool) -> Result<()> {
        let result = (|| {
            let refused = |e: zip::result::ZipError| invalid(format!("not a backup of this app: {e}"));
            let mut zip = zip::ZipArchive::new(std::fs::File::open(&path)?).map_err(refused)?;
            let manifest: Manifest = {
                let entry = zip.by_name(MANIFEST).map_err(refused)?;
                serde_json::from_reader(entry.take(1 << 20)).map_err(|_| invalid("not a backup of this app"))?
            };
            if manifest.format != FORMAT {
                return Err(invalid(format!(
                    "the backup uses format {}, this version understands format {FORMAT}; update the app",
                    manifest.format
                )));
            }
            let dir = self.backups_dir();
            crate::store::make_private_dir(&dir)?;
            let database = dir.join(format!(".{}.sqlite", uuid::Uuid::now_v7().simple()));
            let restored = self.restore_from(&mut zip, &database, over_storage);
            let _ = std::fs::remove_file(&database);
            restored
        })();
        self.note_result("backup", &result, LogLevel::Info, |()| "restored from a backup".into());
        result
    }
}

impl Store {
    fn restore_from(
        &self,
        zip: &mut zip::ZipArchive<std::fs::File>,
        database: &Path,
        over_storage: bool,
    ) -> Result<()> {
        let refused = |e: zip::result::ZipError| invalid(format!("not a backup of this app: {e}"));
        std::io::copy(
            &mut zip.by_name(DATABASE).map_err(refused)?,
            &mut std::fs::File::create(database)?,
        )?;
        // Looked at before anything is touched: a file that cannot be read changes nothing.
        rusqlite::Connection::open_with_flags(database, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .and_then(|c| c.query_row("SELECT count(*) FROM fields", [], |r| r.get::<_, i64>(0)))
            .map_err(|_| invalid("the database in the backup cannot be read"))?;

        // The state that is about to go: kept even on a device with nothing but a trash.
        let anything: bool = self
            .lock()
            .conn
            .query_row("SELECT EXISTS (SELECT 1 FROM fields)", [], |r| r.get(0))?;
        if anything {
            self.write_backup()?;
        }

        // Content first: it only adds files named by what they hold.
        for i in 0..zip.len() {
            let mut entry = zip.by_index(i).map_err(refused)?;
            let Some(hash) = entry
                .name()
                .ok()
                .and_then(|n| n.strip_prefix(BLOBS).map(str::to_string))
            else {
                continue;
            };
            let target = self.blob_path(&hash);
            if !is_sha256(&hash) || target.exists() {
                continue;
            }
            let tmp = target.with_extension(format!("{}.part", uuid::Uuid::now_v7().simple()));
            let mut digest = Sha256::new();
            {
                let mut out = std::fs::File::create(&tmp)?;
                let mut buf = [0u8; 64 * 1024];
                loop {
                    let n = entry.read(&mut buf)?;
                    if n == 0 {
                        break;
                    }
                    digest.update(&buf[..n]);
                    out.write_all(&buf[..n])?;
                }
            }
            if hex(&digest.finalize()) == hash {
                std::fs::rename(&tmp, &target)?;
                // R142: as private as content added by hand.
                crate::store::private_file(&target)?;
            } else {
                std::fs::remove_file(&tmp)?;
            }
        }

        let _running = self.sync_lock.lock().unwrap_or_else(|p| p.into_inner());
        self.renew(Some(database), false)?;
        if over_storage && self.sync_config()? != SyncConfig::Off {
            self.ask_to_replace_remote()?;
        }
        Ok(())
    }
}
