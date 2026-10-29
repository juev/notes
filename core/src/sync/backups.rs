//! Backups in the storage: S60 and S61 in docs/specs/sync.md, R153 and R154 in
//! docs/specs/product.md.
//!
//! ```text
//! lists/backups/<device>/Lists_(2026-10-09-11_15_05).listsbackup   R87 without blobs/
//! ```

use chrono::NaiveDateTime;

use super::remote::Remote;
use super::{private_create, ROOT};
use crate::backup::{created_of, SECONDS_FMT};
use crate::db;
use crate::error::{AppError, Result};
use crate::model::SyncConfig;
use crate::store::Store;
use crate::LogLevel;

/// R154: "0" when this device keeps no copies in the storage.
const ENABLED: &str = "storage_backups";
/// S60: when this device last put a copy into the storage, and why the last attempt failed.
const LAST: &str = "storage_backup_last";
const ERROR: &str = "storage_backup_error";

/// A backup in the storage (R154).
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct StorageBackup {
    /// The device that made it; what `restore_storage_backup` takes.
    pub device: String,
    pub name: String,
    /// `YYYY-MM-DDTHH:MM`, local time of the device that made it.
    pub created: String,
    pub this_device: bool,
}

/// What the settings show about the copies in the storage (R154).
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct StorageBackupStatus {
    /// The sync is WebDAV or a folder: there is a storage to keep copies in.
    pub available: bool,
    pub enabled: bool,
    /// `YYYY-MM-DDTHH:MM` of the last copy this device put there.
    pub last: Option<String>,
    /// Why the last attempt failed, until one succeeds.
    pub error: Option<String>,
}

pub(crate) fn backups_dir() -> String {
    format!("{ROOT}/backups")
}

/// A device id names a folder: nothing that could lead elsewhere.
fn device_ok(device: &str) -> bool {
    !device.is_empty() && device.len() <= 64 && device.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

fn name_ok(name: &str) -> bool {
    created_of(name).is_some() && !name.contains(['/', '\\'])
}

impl Store {
    fn storage_backups_enabled(&self) -> Result<bool> {
        Ok(db::meta_get(&self.lock().conn, ENABLED)?.as_deref() != Some("0"))
    }

    /// S60: called at the end of a pass that went through. Puts a copy into the storage when the
    /// time has come and removes this device's copies beyond the number to keep. A failure is
    /// kept for the settings and the log; it does not fail the pass.
    pub(crate) fn backup_to_storage(&self, remote: &dyn Remote, me: &str) {
        let due = (|| -> Result<bool> {
            let every = self.backup_settings()?.every_hours;
            if every == 0 || !self.storage_backups_enabled()? || !self.holds_data()? {
                return Ok(false);
            }
            let inner = self.lock();
            let now = inner.now();
            let last =
                db::meta_get(&inner.conn, LAST)?.and_then(|v| NaiveDateTime::parse_from_str(&v, SECONDS_FMT).ok());
            // A clock set back does not hold the copies off for good.
            Ok(!last.is_some_and(|last| last <= now && now - last < chrono::Duration::hours(every.into())))
        })();
        match due {
            Ok(true) => {}
            Ok(false) => return,
            Err(e) => {
                self.note(LogLevel::Error, "backup", || {
                    format!("copy for the storage not planned: {e}")
                });
                return;
            }
        }
        let now = self.lock().now();
        let result = self.put_storage_backup(remote, me);
        let inner = self.lock();
        let kept = match &result {
            Ok(_) => db::meta_set(&inner.conn, LAST, &now.format(SECONDS_FMT).to_string())
                .and_then(|()| db::meta_del(&inner.conn, ERROR)),
            Err(e) => db::meta_set(&inner.conn, ERROR, &e.to_string()),
        };
        drop(inner);
        if let Err(e) = kept {
            self.note(LogLevel::Error, "backup", || {
                format!("state of the copies not kept: {e}")
            });
        }
        match result {
            Ok(name) => self.note(LogLevel::Info, "backup", || format!("copy {name} put into the storage")),
            Err(e) => self.note(LogLevel::Error, "backup", || {
                format!("copy not put into the storage: {e}")
            }),
        }
    }

    fn put_storage_backup(&self, remote: &dyn Remote, me: &str) -> Result<String> {
        let part = self
            .backups_dir()
            .join(format!(".{}.storage", uuid::Uuid::now_v7().simple()));
        let sent = (|| -> Result<String> {
            let name = self.write_storage_backup(&part)?;
            remote.put_file(&format!("{}/{me}/{name}", backups_dir()), &part)?;
            Ok(name)
        })();
        let _ = std::fs::remove_file(&part);
        let name = sent?;
        // The oldest go first; the copies of other devices are theirs.
        let keep = self.backup_settings()?.keep as usize;
        let mut own: Vec<String> = remote
            .list(&format!("{}/{me}", backups_dir()))?
            .into_iter()
            .filter(|n| name_ok(n))
            .collect();
        own.sort_by(|a, b| b.cmp(a));
        for old in own.iter().skip(keep) {
            remote.delete(&format!("{}/{me}/{old}", backups_dir()))?;
        }
        Ok(name)
    }

    /// The storage as sync sees it, for a call made outside a pass; `None` without one.
    fn storage_view<T>(&self, f: impl FnOnce(&dyn Remote) -> Result<T>) -> Result<Option<T>> {
        // Not at the same time as a pass, which may be moving the storage (S36, S56).
        let _running = self.sync_lock.lock().unwrap_or_else(|p| p.into_inner());
        let Some(raw) = self.file_storage()? else {
            return Ok(None);
        };
        let view = self.view(raw.as_ref())?;
        f(view.remote()).map(Some)
    }
}

#[uniffi::export]
impl Store {
    pub fn storage_backup_status(&self) -> Result<StorageBackupStatus> {
        let available = matches!(
            self.sync_config()?,
            SyncConfig::WebDav { .. } | SyncConfig::Folder { .. }
        );
        let inner = self.lock();
        let last = db::meta_get(&inner.conn, LAST)?
            .and_then(|v| NaiveDateTime::parse_from_str(&v, SECONDS_FMT).ok())
            .map(|t| t.format("%Y-%m-%dT%H:%M").to_string());
        let error = db::meta_get(&inner.conn, ERROR)?;
        drop(inner);
        Ok(StorageBackupStatus {
            available,
            enabled: self.storage_backups_enabled()?,
            last,
            error,
        })
    }

    /// R154: belongs to the device and is not synced. Turning it on puts a copy into the storage
    /// with the next pass when none is there yet from within the schedule.
    pub fn set_storage_backups(&self, enabled: bool) -> Result<()> {
        let inner = self.lock();
        if enabled {
            db::meta_del(&inner.conn, ENABLED)
        } else {
            db::meta_set(&inner.conn, ENABLED, "0")?;
            db::meta_del(&inner.conn, ERROR)
        }
    }

    /// R154: the copies of every device in the storage, newest first. Asks the storage.
    pub fn storage_backups(&self) -> Result<Vec<StorageBackup>> {
        let me = self.device_id();
        let found = self.storage_view(|remote| {
            let mut found = Vec::new();
            for device in remote.folders(&backups_dir())?.into_iter().filter(|d| device_ok(d)) {
                for name in remote.list(&format!("{}/{device}", backups_dir()))? {
                    let Some(created) = name_ok(&name).then(|| created_of(&name)).flatten() else {
                        continue;
                    };
                    found.push(StorageBackup {
                        this_device: device == me,
                        device: device.clone(),
                        created: created.format("%Y-%m-%dT%H:%M").to_string(),
                        name,
                    });
                }
            }
            Ok(found)
        })?;
        let mut found = found.unwrap_or_default();
        found.sort_by(|a, b| (&b.created, &b.name).cmp(&(&a.created, &a.name)));
        Ok(found)
    }

    /// R154: downloads the copy and restores it as `restore_backup` does (R90).
    pub fn restore_storage_backup(&self, device: String, name: String, over_storage: bool) -> Result<()> {
        if !device_ok(&device) || !name_ok(&name) {
            return Err(AppError::invalid("not a backup of this app"));
        }
        let dir = self.backups_dir();
        crate::store::make_private_dir(&dir)?;
        let local = dir.join(format!(".{}.download", uuid::Uuid::now_v7().simple()));
        let fetched = self.storage_view(|remote| {
            let mut out = std::io::BufWriter::new(private_create(&local)?);
            let found = remote.get_to(&format!("{}/{device}/{name}", backups_dir()), &mut out, 0)?;
            std::io::Write::flush(&mut out)?;
            Ok(found)
        });
        let restored = match fetched {
            Ok(Some(true)) => self.restore_backup(local.to_string_lossy().into_owned(), over_storage),
            Ok(Some(false)) => Err(AppError::not_found("backup")),
            Ok(None) => Err(AppError::invalid("sync is not set to WebDAV or a folder")),
            Err(e) => Err(e),
        };
        let _ = std::fs::remove_file(&local);
        restored
    }
}
