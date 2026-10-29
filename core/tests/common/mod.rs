#![allow(dead_code)]

use std::sync::Arc;

use notes_core::{Folder, NewNote, Note, Scope, Store};
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

pub fn add(store: &Store, title: &str) -> Note {
    store
        .create_note(NewNote {
            title: title.into(),
            ..NewNote::default()
        })
        .unwrap()
}

pub fn add_in(store: &Store, title: &str, folder: &Folder) -> Note {
    store
        .create_note(NewNote {
            title: title.into(),
            folder_id: Some(folder.id.clone()),
            ..NewNote::default()
        })
        .unwrap()
}

pub fn new_folder(store: &Store, name: &str, parent: Option<&Folder>) -> Folder {
    store.create_folder(name.into(), parent.map(|p| p.id.clone())).unwrap()
}

pub fn titles(notes: &[Note]) -> Vec<&str> {
    notes.iter().map(|n| n.title.as_str()).collect()
}

pub fn view(store: &Store, view: Scope) -> Vec<String> {
    store.notes(view).unwrap().into_iter().map(|t| t.title).collect()
}

/// The titles of the notes in view All, the oldest first: the order the notes were made in.
pub fn oldest_first(store: &Store) -> Vec<String> {
    let mut all = store.notes(Scope::All).unwrap();
    all.sort_by(|a, b| (a.created_ms, &a.id).cmp(&(b.created_ms, &b.id)));
    all.into_iter().map(|n| n.title).collect()
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
