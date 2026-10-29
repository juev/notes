#![allow(dead_code)]

use std::sync::Arc;

use notes_core::{NewTask, Scope, Store, TaskItem};
use tempfile::TempDir;

/// 2026-10-05 is a Monday.
pub const NOW: &str = "2026-10-05T10:00";

pub struct Device {
    pub store: Arc<Store>,
    _dir: TempDir,
}

impl std::ops::Deref for Device {
    type Target = Store;
    fn deref(&self) -> &Store {
        &self.store
    }
}

impl Device {
    /// Where the device keeps its database.
    pub fn dir(&self) -> &std::path::Path {
        self._dir.path()
    }
}

pub fn device() -> Device {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path().to_string_lossy().into_owned()).unwrap();
    store.set_now_for_tests(NOW);
    Device { store, _dir: dir }
}

pub fn add(store: &Store, title: &str) -> TaskItem {
    store
        .create_task(NewTask {
            title: title.into(),
            ..NewTask::default()
        })
        .unwrap()
}

pub fn add_sub(store: &Store, parent: &TaskItem, title: &str) -> TaskItem {
    store
        .create_task(NewTask {
            title: title.into(),
            parent_id: Some(parent.id.clone()),
            ..NewTask::default()
        })
        .unwrap()
}

pub fn titles(tasks: &[TaskItem]) -> Vec<&str> {
    tasks.iter().map(|t| t.title.as_str()).collect()
}

pub fn view(store: &Store, view: Scope) -> Vec<String> {
    store.tasks(view).unwrap().into_iter().map(|t| t.title).collect()
}

/// Writes registers straight into the database of a closed device, the way an earlier version of the
/// app could have left them, and opens the folder again: whatever the present version makes of them
/// at the start is in the store that comes back. A register is `(kind, id, field, value)`.
pub fn plant(device: Device, registers: &[(&str, &str, &str, serde_json::Value)]) -> Device {
    let Device { store, _dir: dir } = device;
    drop(store);
    {
        let conn = rusqlite::Connection::open(dir.path().join("notes.sqlite")).unwrap();
        let newest: String = conn
            .query_row("SELECT max(stamp) FROM fields", [], |r| r.get(0))
            .unwrap();
        // A stamp is `<ms>-<counter>-<device>`: the planted ones are later than everything there.
        let ms = u64::from_str_radix(&newest[..12], 16).unwrap() + 1;
        for (index, (kind, id, field, value)) in registers.iter().enumerate() {
            let stamp = format!("{ms:012x}-{index:04x}-{}", &newest[18..]);
            conn.execute(
                "INSERT OR REPLACE INTO fields (kind, id, field, value, stamp, dirty) VALUES (?1, ?2, ?3, ?4, ?5, 1)",
                rusqlite::params![kind, id, field, value.to_string(), stamp],
            )
            .unwrap();
        }
        // The derived tables are built again from the registers at the start.
        conn.execute("DELETE FROM meta WHERE key = 'derived_version'", [])
            .unwrap();
    }
    let store = Store::open(dir.path().to_string_lossy().into_owned()).unwrap();
    store.set_now_for_tests(NOW);
    Device { store, _dir: dir }
}

/// The same device after the app was quit and started again: what the process kept in memory is gone.
pub fn reopen(device: Device) -> Device {
    let Device { store, _dir: dir } = device;
    drop(store);
    let store = Store::open(dir.path().to_string_lossy().into_owned()).unwrap();
    store.set_now_for_tests(NOW);
    Device { store, _dir: dir }
}
