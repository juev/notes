//! Backups in the storage: S60 and S61 in docs/specs/sync.md, R153 and R154 in
//! docs/specs/product.md. Storage is a temporary folder.

mod common;

use std::path::{Path, PathBuf};

use common::*;
use notes_core::sync::remote::{DirRemote, Remote};
use notes_core::*;
use tempfile::TempDir;

const PASS: &str = "correct horse battery staple";

fn connect(d: &Device, storage: &TempDir) {
    d.set_sync_config(SyncConfig::Folder {
        path: storage.path().to_string_lossy().into_owned(),
    })
    .unwrap();
}

fn sync(d: &Device, storage: &TempDir) -> SyncReport {
    d.sync_with(&DirRemote::new(storage.path())).unwrap()
}

/// The folders of `notes/backups/` and the files in each, sorted.
fn copies(storage: &TempDir) -> Vec<(String, Vec<String>)> {
    let root = storage.path().join("notes/backups");
    let mut out: Vec<(String, Vec<String>)> = std::fs::read_dir(&root)
        .map(|entries| {
            entries
                .map(|e| {
                    let e = e.unwrap();
                    let mut names: Vec<String> = std::fs::read_dir(e.path())
                        .unwrap()
                        .map(|f| f.unwrap().file_name().to_string_lossy().into_owned())
                        .collect();
                    names.sort();
                    (e.file_name().to_string_lossy().into_owned(), names)
                })
                .collect()
        })
        .unwrap_or_default();
    out.sort();
    out
}

fn only_copy(storage: &TempDir) -> PathBuf {
    let found = copies(storage);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].1.len(), 1, "{found:?}");
    storage
        .path()
        .join("notes/backups")
        .join(&found[0].0)
        .join(&found[0].1[0])
}

fn entries(path: &Path) -> Vec<String> {
    let zip = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
    let mut names: Vec<String> = zip.file_names().map(|n| n.unwrap().into_owned()).collect();
    names.sort();
    names
}

fn attach(d: &Device, task: &TaskItem, name: &str, content: &[u8]) -> Attachment {
    let src = tempfile::tempdir().unwrap();
    let file = src.path().join(name);
    std::fs::write(&file, content).unwrap();
    d.add_attachment(task.id.clone(), file.to_string_lossy().into_owned(), None)
        .unwrap()
}

#[test]
fn s60_a_pass_puts_a_copy_without_content_once_per_period() {
    let storage = tempfile::tempdir().unwrap();
    let a = device();
    connect(&a, &storage);
    let task = add(&a, "в копии");
    attach(&a, &task, "a.txt", b"content");
    sync(&a, &storage);
    let copy = only_copy(&storage);
    assert_eq!(
        entries(&copy),
        ["manifest.json", "notes.sqlite"],
        "no blobs/ in a copy for the storage"
    );
    assert!(copy
        .to_string_lossy()
        .ends_with("Notes_(2026-10-05-10_00_00).notesbackup"));

    a.set_now_for_tests("2026-10-05T10:05");
    sync(&a, &storage);
    assert_eq!(copies(&storage)[0].1.len(), 1, "within the period nothing more is put");
    let status = a.storage_backup_status().unwrap();
    assert_eq!(
        (status.available, status.enabled, status.last.as_deref(), status.error),
        (true, true, Some("2026-10-05T10:00"), None)
    );

    a.set_now_for_tests("2026-10-06T10:00");
    sync(&a, &storage);
    assert_eq!(copies(&storage)[0].1.len(), 2);
}

#[test]
fn s60_a_device_keeps_as_many_copies_as_it_keeps_locally_and_leaves_the_others() {
    let storage = tempfile::tempdir().unwrap();
    let a = device();
    connect(&a, &storage);
    add(&a, "задача");
    // A copy of another device.
    let other = storage.path().join("notes/backups/other-device");
    std::fs::create_dir_all(&other).unwrap();
    std::fs::write(other.join("Notes_(2026-01-01-09_00_00).notesbackup"), b"theirs").unwrap();

    for day in 5..12 {
        a.set_now_for_tests(&format!("2026-10-{day:02}T10:00"));
        sync(&a, &storage);
    }
    let found = copies(&storage);
    let own = found.iter().find(|(d, _)| d != "other-device").unwrap();
    assert_eq!(
        own.1,
        (7..12)
            .map(|day| format!("Notes_(2026-10-{day:02}-10_00_00).notesbackup"))
            .collect::<Vec<_>>(),
        "the five newest of the seven"
    );
    assert!(other.join("Notes_(2026-01-01-09_00_00).notesbackup").exists());

    let listed = a.storage_backups().unwrap();
    assert_eq!(listed.len(), 6);
    assert_eq!(listed[0].created, "2026-10-11T10:00");
    assert!(listed[0].this_device);
    let theirs = listed.last().unwrap();
    assert_eq!((theirs.device.as_str(), theirs.this_device), ("other-device", false));
}

/// A storage that refuses whatever goes under `backups/`.
struct NoBackups(DirRemote);

impl Remote for NoBackups {
    fn id(&self) -> String {
        self.0.id()
    }
    fn list(&self, dir: &str) -> Result<Vec<String>> {
        self.0.list(dir)
    }
    fn get(&self, path: &str) -> Result<Option<Vec<u8>>> {
        self.0.get(path)
    }
    fn put(&self, path: &str, data: &[u8]) -> Result<()> {
        if path.contains("/backups/") {
            return Err(AppError::Sync {
                msg: "507 Insufficient Storage".into(),
            });
        }
        self.0.put(path, data)
    }
    fn delete(&self, path: &str) -> Result<()> {
        self.0.delete(path)
    }
    fn exists(&self, path: &str) -> Result<bool> {
        self.0.exists(path)
    }
    fn folders(&self, dir: &str) -> Result<Vec<String>> {
        self.0.folders(dir)
    }
}

#[test]
fn s60_a_failed_copy_does_not_fail_the_pass() {
    let storage = tempfile::tempdir().unwrap();
    let a = device();
    connect(&a, &storage);
    add(&a, "задача");
    a.sync_with(&NoBackups(DirRemote::new(storage.path()))).unwrap();
    let status = a.storage_backup_status().unwrap();
    assert!(status.error.unwrap().contains("507"));
    assert_eq!(status.last, None);
    // The next pass tries again, and a success clears the error.
    sync(&a, &storage);
    let status = a.storage_backup_status().unwrap();
    assert_eq!((status.last.as_deref(), status.error), (Some("2026-10-05T10:00"), None));
}

#[test]
fn s60_r154_no_copies_when_turned_off_without_schedule_or_without_sync() {
    let storage = tempfile::tempdir().unwrap();
    let a = device();
    connect(&a, &storage);
    add(&a, "задача");
    a.set_storage_backups(false).unwrap();
    sync(&a, &storage);
    assert!(copies(&storage).is_empty());
    assert!(!a.storage_backup_status().unwrap().enabled);

    a.set_storage_backups(true).unwrap();
    a.set_backup_settings(BackupSettings {
        every_hours: 0,
        keep: 5,
    })
    .unwrap();
    sync(&a, &storage);
    assert!(copies(&storage).is_empty());

    a.set_backup_settings(BackupSettings {
        every_hours: 24,
        keep: 5,
    })
    .unwrap();
    sync(&a, &storage);
    assert_eq!(
        copies(&storage).len(),
        1,
        "turned on again, a copy comes with the next pass"
    );

    a.set_sync_config(SyncConfig::Off).unwrap();
    assert!(!a.storage_backup_status().unwrap().available);
}

#[test]
fn s61_copies_in_an_encrypted_storage_are_sealed_and_clear_ones_go() {
    let storage = tempfile::tempdir().unwrap();
    let a = device();
    connect(&a, &storage);
    add(&a, "секретная задача");
    sync(&a, &storage);
    let clear = only_copy(&storage);

    a.encrypt_storage_with(&DirRemote::new(storage.path()), PASS.into())
        .unwrap();
    sync(&a, &storage);
    assert!(!clear.exists(), "the copy in the clear is gone");
    a.set_now_for_tests("2026-10-06T10:00");
    sync(&a, &storage);
    let sealed = only_copy(&storage);
    let bytes = std::fs::read(&sealed).unwrap();
    assert!(!bytes.windows("секретная".len()).any(|w| w == "секретная".as_bytes()));
    assert!(
        zip::ZipArchive::new(std::fs::File::open(&sealed).unwrap()).is_err(),
        "not a readable zip"
    );

    // Through the key the copy is listed and restores.
    let listed = a.storage_backups().unwrap();
    assert_eq!(listed.len(), 1);
    add(&a, "после копии");
    a.restore_storage_backup(listed[0].device.clone(), listed[0].name.clone(), false)
        .unwrap();
    assert_eq!(view(&a, Scope::Inbox), ["секретная задача"]);
}

#[test]
fn r153_a_copy_from_the_storage_restores_on_another_device_and_the_content_follows() {
    let storage = tempfile::tempdir().unwrap();
    let a = device();
    connect(&a, &storage);
    let task = add(&a, "с файлом");
    attach(&a, &task, "a.txt", b"the content");
    sync(&a, &storage);
    a.sync_attachments_with(&DirRemote::new(storage.path())).unwrap();

    let b = device();
    connect(&b, &storage);
    let listed = b.storage_backups().unwrap();
    assert_eq!(listed.len(), 1);
    assert!(!listed[0].this_device);
    b.restore_storage_backup(listed[0].device.clone(), listed[0].name.clone(), false)
        .unwrap();
    assert_eq!(view(&b, Scope::Inbox), ["с файлом"]);
    let waiting = b.attachments(task.id.clone()).unwrap();
    assert_eq!(waiting.len(), 1);
    assert!(waiting[0].local_path.is_none(), "not downloaded yet");
    b.sync_attachments_with(&DirRemote::new(storage.path())).unwrap();
    let got = b.attachments(task.id.clone()).unwrap();
    assert_eq!(
        std::fs::read(got[0].local_path.as_ref().unwrap()).unwrap(),
        b"the content"
    );

    assert!(b
        .restore_storage_backup("../escape".into(), listed[0].name.clone(), false)
        .is_err());
    assert!(b
        .restore_storage_backup(listed[0].device.clone(), "../../vault.json".into(), false)
        .is_err());
}
