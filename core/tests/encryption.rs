//! Encryption of the storage with a passphrase: S54–S60 in docs/specs/sync.md.
//! Storage is a temporary folder; `webdav.rs` repeats the main case over HTTP.

mod common;

use std::path::{Path, PathBuf};

use common::*;
use notes_core::sync::remote::{DirRemote, Remote};
use notes_core::*;
use tempfile::TempDir;

const PASS: &str = "correct horse battery staple";

fn sync(d: &Device, storage: &TempDir) -> SyncReport {
    d.sync_with(&DirRemote::new(storage.path())).unwrap()
}

fn sync_error(d: &Device, storage: &TempDir) -> String {
    d.sync_with(&DirRemote::new(storage.path())).unwrap_err().to_string()
}

fn sync_files(d: &Device, storage: &TempDir) -> AttachmentReport {
    d.sync_attachments_with(&DirRemote::new(storage.path())).unwrap()
}

fn encrypt(d: &Device, storage: &TempDir, passphrase: &str) {
    d.encrypt_storage_with(&DirRemote::new(storage.path()), passphrase.into())
        .unwrap();
}

/// Sync through the folder in the settings, so that the status speaks of it.
fn connect(d: &Device, storage: &TempDir) {
    d.set_sync_config(SyncConfig::Folder {
        path: storage.path().to_string_lossy().into_owned(),
    })
    .unwrap();
}

fn attach(d: &Device, task: &TaskItem, name: &str, content: &[u8]) -> Attachment {
    let src = tempfile::tempdir().unwrap();
    let file = src.path().join(name);
    std::fs::write(&file, content).unwrap();
    d.add_attachment(task.id.clone(), file.to_string_lossy().into_owned(), None)
        .unwrap()
}

/// The content of an attachment as this device holds it, if it does.
fn content(d: &Device, task: &TaskItem, name: &str) -> Option<Vec<u8>> {
    let found = d.attachments(task.id.clone()).unwrap();
    let file = found.iter().find(|a| a.name == name)?;
    std::fs::read(file.local_path.as_ref()?).ok()
}

/// Bytes that are no file's content: a pattern long enough to span several chunks (S55).
fn bulk(len: usize, seed: u8) -> Vec<u8> {
    (0..len)
        .map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed))
        .collect()
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            walk(&path, out);
        } else {
            out.push(path);
        }
    }
}

/// Every file of the storage under its path from the root, with its bytes.
fn stored(storage: &TempDir) -> Vec<(String, Vec<u8>)> {
    let mut paths = Vec::new();
    walk(storage.path(), &mut paths);
    paths.sort();
    paths
        .into_iter()
        .map(|p| {
            let name = p.strip_prefix(storage.path()).unwrap().to_string_lossy().into_owned();
            (name, std::fs::read(&p).unwrap())
        })
        .collect()
}

fn vault(storage: &TempDir) -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(storage.path().join("notes/vault.json")).unwrap()).unwrap()
}

fn holds(data: &[u8], needle: &[u8]) -> bool {
    data.windows(needle.len()).any(|w| w == needle)
}

/// S55: every file is sealed, apart from `vault.json`, the empty marks and the
/// file that says the storage moved (S49); no path and no byte holds a secret.
fn assert_sealed(storage: &TempDir, secrets: &[&str]) {
    let files = stored(storage);
    assert!(files.iter().any(|(p, _)| p.starts_with("notes/log/")));
    for (path, data) in files {
        for secret in secrets {
            assert!(!path.contains(secret), "{path} names {secret}");
            assert!(!holds(&data, secret.as_bytes()), "{path} holds {secret}");
        }
        if path == "notes/vault.json" {
            continue;
        }
        if path.ends_with(".snap") {
            assert!(data.is_empty(), "{path}");
            continue;
        }
        assert!(data.starts_with(b"NSE1"), "{path} is not sealed");
    }
}

fn assert_clear(storage: &TempDir) {
    for (path, data) in stored(storage) {
        assert!(!data.starts_with(b"NSE1"), "{path} is sealed");
    }
}

/// A storage that does something of the test's own before `vault.json` of format 4 is written.
struct OnSeal<'a> {
    inner: DirRemote,
    before: &'a (dyn Fn() -> Result<()> + Sync),
}

impl Remote for OnSeal<'_> {
    fn id(&self) -> String {
        self.inner.id()
    }
    fn list(&self, dir: &str) -> Result<Vec<String>> {
        self.inner.list(dir)
    }
    fn get(&self, path: &str) -> Result<Option<Vec<u8>>> {
        self.inner.get(path)
    }
    fn put(&self, path: &str, data: &[u8]) -> Result<()> {
        if path == "notes/vault.json" && holds(data, b"\"format\":2") {
            (self.before)()?;
        }
        self.inner.put(path, data)
    }
    fn put_file(&self, path: &str, local: &Path) -> Result<()> {
        self.inner.put_file(path, local)
    }
    fn delete(&self, path: &str) -> Result<()> {
        self.inner.delete(path)
    }
    fn exists(&self, path: &str) -> Result<bool> {
        self.inner.exists(path)
    }
    fn folders(&self, dir: &str) -> Result<Vec<String>> {
        self.inner.folders(dir)
    }
}

#[test]
fn s54_two_devices_converge_through_an_encrypted_folder() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b) = (device(), device());
    connect(&a, &storage);
    connect(&b, &storage);
    let t = add(&a, "секретная задача");
    a.set_notes(t.id.clone(), "заметка под замком".into()).unwrap();
    // Larger than a chunk, so that it is sealed and opened in pieces.
    let first = attach(&a, &t, "plan.bin", &bulk(200_000, 1));
    a.set_push_endpoint(Some("https://push.example/topic-a".into()))
        .unwrap();
    sync(&a, &storage);
    sync_files(&a, &storage);
    sync(&b, &storage);
    sync_files(&b, &storage);

    encrypt(&a, &storage, PASS);
    assert_eq!(vault(&storage)["format"], 2);
    assert!(a.sync_status().unwrap().encrypted);

    // S58: B learns it from the snapshot of A, which it cannot read, and stops; its own work goes on.
    let error = sync_error(&b, &storage);
    assert!(error.contains("enter its passphrase"), "{error}");
    assert!(b.sync_status().unwrap().encrypted);
    add(&b, "без связи");
    b.set_sync_passphrase(Some(PASS.into()));
    sync(&b, &storage);
    sync_files(&b, &storage);
    sync(&a, &storage);
    assert_eq!(view(&a, Scope::Inbox), ["секретная задача", "без связи"]);

    let u = add(&a, "после шифрования");
    let second = attach(&a, &u, "second.bin", &bulk(70_000, 2));
    sync(&a, &storage);
    assert_eq!(sync_files(&a, &storage).uploaded, 1);
    sync(&b, &storage);
    assert_eq!(sync_files(&b, &storage).downloaded, 1);
    assert_eq!(content(&b, &u, "second.bin"), Some(bulk(70_000, 2)));
    b.set_notes(u.id.clone(), "правка с B".into()).unwrap();
    sync(&b, &storage);
    sync(&a, &storage);
    assert_eq!(a.task(u.id.clone()).unwrap().notes, "правка с B");

    // A device that comes later with the passphrase reads everything.
    let c = device();
    c.set_sync_passphrase(Some(PASS.into()));
    sync(&c, &storage);
    sync_files(&c, &storage);
    assert_eq!(
        view(&c, Scope::Inbox),
        ["секретная задача", "без связи", "после шифрования"]
    );
    assert_eq!(content(&c, &t, "plan.bin"), Some(bulk(200_000, 1)));
    assert_eq!(content(&c, &u, "second.bin"), Some(bulk(70_000, 2)));

    assert_sealed(
        &storage,
        &[
            "секретная задача",
            "заметка под замком",
            "после шифрования",
            "правка с B",
            "push.example",
            &first.sha256,
            &second.sha256,
        ],
    );
}

#[test]
fn s58_a_missing_or_wrong_passphrase_and_a_newer_format_stop_sync_with_their_own_message() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b) = (device(), device());
    add(&a, "одна");
    sync(&a, &storage);
    sync(&b, &storage);
    encrypt(&a, &storage, PASS);

    // A new process of B reads `vault.json` at its first run.
    let b = reopen(b);
    let missing = sync_error(&b, &storage);
    assert!(missing.contains("enter its passphrase"), "{missing}");
    b.set_sync_passphrase(Some("wrong".into()));
    let wrong = sync_error(&b, &storage);
    assert!(wrong.contains("does not open the storage"), "{wrong}");
    assert_ne!(missing, wrong);
    // Local work goes on.
    add(&b, "локально");
    assert_eq!(view(&b, Scope::Inbox), ["одна", "локально"]);
    b.set_sync_passphrase(Some(PASS.into()));
    sync(&b, &storage);
    sync(&a, &storage);
    assert_eq!(view(&a, Scope::Inbox), ["одна", "локально"]);

    // An earlier version stops on format 4 the same way this one stops on a later one (S14).
    let mut later = vault(&storage);
    later["format"] = 3.into();
    std::fs::write(storage.path().join("notes/vault.json"), later.to_string()).unwrap();
    let b = reopen(b);
    b.set_sync_passphrase(Some(PASS.into()));
    let newer = sync_error(&b, &storage);
    assert!(newer.contains("update the app"), "{newer}");
}

#[test]
fn s57_a_changed_moved_or_plain_file_stays_unread() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b) = (device(), device());
    // A state that outweighs two small log files, so that they stay in the log (S45).
    for i in 0..40 {
        add(&a, &format!("старая задача номер {i}, с названием подлиннее"));
    }
    sync(&a, &storage);
    encrypt(&a, &storage, PASS);
    b.set_sync_passphrase(Some(PASS.into()));
    sync(&b, &storage);

    add(&a, "первая");
    sync(&a, &storage);
    add(&a, "вторая");
    sync(&a, &storage);
    let log = storage.path().join("notes/log");
    let mut own: Vec<PathBuf> = std::fs::read_dir(&log)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "jsonl"))
        .collect();
    own.sort();
    let (first, second) = (own[own.len() - 2].clone(), own[own.len() - 1].clone());
    let (first_bytes, second_bytes) = (std::fs::read(&first).unwrap(), std::fs::read(&second).unwrap());

    // A changed byte: B goes no further than the file.
    let mut changed = first_bytes.clone();
    let middle = changed.len() / 2;
    changed[middle] ^= 1;
    std::fs::write(&first, &changed).unwrap();
    let before = view(&b, Scope::Inbox);
    assert_eq!(sync(&b, &storage).pulled, 0);
    assert_eq!(view(&b, Scope::Inbox), before);

    // The second file put under the name of the first: it was sealed for another path.
    std::fs::write(&first, &second_bytes).unwrap();
    assert_eq!(sync(&b, &storage).pulled, 0);

    // A file in the clear that would read well without encryption.
    let device_id = a.device_id();
    let name = first.file_name().unwrap().to_string_lossy().into_owned();
    let seq: u64 = name
        .trim_end_matches(".jsonl")
        .rsplit('-')
        .next()
        .unwrap()
        .parse()
        .unwrap();
    let injected = format!(
        "{{\"v\":2,\"device\":\"{device_id}\",\"seq\":{seq},\"count\":1}}\n{{\"k\":\"task\",\"i\":\"0192aaaa-0000-7000-8000-000000000001\",\"f\":\"title\",\"v\":\"подложено\",\"t\":\"ffffffffffff-0000-{device_id}\"}}\n"
    );
    std::fs::write(&first, injected).unwrap();
    assert_eq!(sync(&b, &storage).pulled, 0);
    assert_eq!(view(&b, Scope::Inbox), before);

    // The file as it was: both changes arrive.
    std::fs::write(&first, &first_bytes).unwrap();
    sync(&b, &storage);
    let now = view(&b, Scope::Inbox);
    assert_eq!(now.len(), before.len() + 2);
    assert!(now.ends_with(&["первая".to_string(), "вторая".to_string()]));

    // Content swapped for other content: not stored, the attachment waits.
    let t = add(&a, "с файлами");
    let one = attach(&a, &t, "one.bin", &bulk(1000, 3));
    let two = attach(&a, &t, "two.bin", &bulk(1000, 4));
    sync(&a, &storage);
    sync_files(&a, &storage);
    let sealed: Vec<(String, Vec<u8>)> = stored(&storage)
        .into_iter()
        .filter(|(p, _)| p.starts_with("notes/blobs/"))
        .collect();
    assert_eq!(sealed.len(), 2);
    assert!(sealed
        .iter()
        .all(|(p, _)| !p.contains(&one.sha256) && !p.contains(&two.sha256)));
    std::fs::write(storage.path().join(&sealed[1].0), &sealed[0].1).unwrap();
    sync(&b, &storage);
    let report = sync_files(&b, &storage);
    assert_eq!((report.downloaded, report.waiting), (1, 1));
    let kept = [content(&b, &t, "one.bin"), content(&b, &t, "two.bin")];
    assert_eq!(kept.iter().filter(|c| c.is_some()).count(), 1);
}

#[test]
fn s56_a_change_uploaded_in_the_clear_while_the_storage_is_encrypted_is_not_lost() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b) = (device(), device());
    add(&a, "от A");
    sync(&a, &storage);
    sync(&b, &storage);

    // B uploads its change after A has looked at the storage and before A writes `vault.json`.
    add(&b, "от B в момент включения");
    let racing = || {
        sync(&b, &storage);
        Ok(())
    };
    let remote = OnSeal {
        inner: DirRemote::new(storage.path()),
        before: &racing,
    };
    a.encrypt_storage_with(&remote, PASS.into()).unwrap();
    let plain = stored(&storage)
        .into_iter()
        .filter(|(p, d)| p.starts_with("notes/log/") && p.ends_with(".jsonl") && !d.starts_with(b"NSE1"))
        .count();
    assert_eq!(plain, 1, "the file of B is there in the clear");

    let error = sync_error(&b, &storage);
    assert!(error.contains("enter its passphrase"), "{error}");
    b.set_sync_passphrase(Some(PASS.into()));
    sync(&b, &storage);
    sync(&a, &storage);
    assert_eq!(view(&a, Scope::Inbox), ["от A", "от B в момент включения"]);
    let c = device();
    c.set_sync_passphrase(Some(PASS.into()));
    sync(&c, &storage);
    assert_eq!(view(&c, Scope::Inbox), ["от A", "от B в момент включения"]);
    assert_sealed(&storage, &["от A", "от B"]);
}

#[test]
fn s56_encryption_cut_short_leaves_the_storage_as_it_was() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b) = (device(), device());
    let t = add(&a, "до шифрования");
    attach(&a, &t, "file.bin", &bulk(5000, 5));
    sync(&a, &storage);
    sync_files(&a, &storage);
    let before = stored(&storage);

    let refuse = || {
        Err(AppError::Sync {
            msg: "507 Insufficient Storage".into(),
        })
    };
    let remote = OnSeal {
        inner: DirRemote::new(storage.path()),
        before: &refuse,
    };
    let error = a.encrypt_storage_with(&remote, PASS.into()).unwrap_err().to_string();
    assert!(error.contains("507"), "{error}");
    assert_eq!(vault(&storage)["format"], 1);
    assert_clear(&storage);
    assert_eq!(stored(&storage), before);
    assert!(!a.sync_status().unwrap().encrypted);

    // Both devices go on in the clear, and the next attempt gets through.
    add(&a, "после сбоя");
    sync(&a, &storage);
    sync(&b, &storage);
    sync_files(&b, &storage);
    assert_eq!(view(&b, Scope::Inbox), ["до шифрования", "после сбоя"]);
    encrypt(&a, &storage, PASS);
    b.set_sync_passphrase(Some(PASS.into()));
    sync(&b, &storage);
    assert_eq!(view(&b, Scope::Inbox), ["до шифрования", "после сбоя"]);
    assert_sealed(&storage, &["до шифрования", "после сбоя"]);
}

#[test]
fn s56_content_the_encrypting_device_lacks_comes_from_the_device_that_has_it() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b) = (device(), device());
    let t = add(&b, "файл B");
    attach(&b, &t, "big.bin", &bulk(20, 6));
    sync(&b, &storage);
    sync_files(&b, &storage);
    // A takes no file over 10 bytes (R140), so the content stays only with B.
    let small = AttachmentLimits {
        max_file: 10,
        ..a.attachment_limits()
    };
    a.set_attachment_limits(small).unwrap();
    sync(&a, &storage);
    encrypt(&a, &storage, PASS);
    assert!(content(&a, &t, "big.bin").is_none());

    b.set_sync_passphrase(Some(PASS.into()));
    sync(&b, &storage);
    assert_eq!(sync_files(&b, &storage).uploaded, 1);
    a.set_attachment_limits(AttachmentLimits {
        max_file: 1000,
        ..small
    })
    .unwrap();
    assert_eq!(sync_files(&a, &storage).downloaded, 1);
    assert_eq!(content(&a, &t, "big.bin"), Some(bulk(20, 6)));
}

#[test]
fn s59_a_changed_passphrase_works_where_it_was_changed_and_is_asked_for_elsewhere() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b) = (device(), device());
    sync(&a, &storage);
    encrypt(&a, &storage, PASS);
    b.set_sync_passphrase(Some(PASS.into()));
    sync(&b, &storage);
    let before: Vec<(String, Vec<u8>)> = stored(&storage)
        .into_iter()
        .filter(|(p, _)| p != "notes/vault.json")
        .collect();

    a.change_storage_passphrase_with(&DirRemote::new(storage.path()), "new phrase".into())
        .unwrap();
    // Only `vault.json` was written again.
    let after: Vec<(String, Vec<u8>)> = stored(&storage)
        .into_iter()
        .filter(|(p, _)| p != "notes/vault.json")
        .collect();
    assert_eq!(after, before);
    add(&a, "после смены");
    sync(&a, &storage);

    // B, still running, has the same data key.
    sync(&b, &storage);
    assert_eq!(view(&b, Scope::Inbox), ["после смены"]);
    // Started again, it asks for the new passphrase.
    let b = reopen(b);
    b.set_sync_passphrase(Some(PASS.into()));
    let error = sync_error(&b, &storage);
    assert!(error.contains("does not open the storage"), "{error}");
    b.set_sync_passphrase(Some("new phrase".into()));
    sync(&b, &storage);

    let a = reopen(a);
    a.set_sync_passphrase(Some("new phrase".into()));
    add(&a, "после перезапуска");
    sync(&a, &storage);
    sync(&b, &storage);
    assert_eq!(view(&b, Scope::Inbox), ["после смены", "после перезапуска"]);
}

#[test]
fn s62_a_storage_that_is_not_encrypted_any_more_is_not_written_in_the_clear() {
    let storage = tempfile::tempdir().unwrap();
    let a = device();
    a.set_sync_config(SyncConfig::Folder {
        path: storage.path().to_string_lossy().into_owned(),
    })
    .unwrap();
    add(&a, "одна");
    sync(&a, &storage);
    encrypt(&a, &storage, PASS);
    let id = vault(&storage)["vault"].as_str().unwrap().to_string();

    // The storage puts back a `vault.json` in the clear.
    std::fs::write(
        storage.path().join("notes/vault.json"),
        format!("{{\"format\":1,\"vault\":\"{id}\"}}"),
    )
    .unwrap();
    let a = reopen(a);
    a.set_sync_passphrase(Some(PASS.into()));
    add(&a, "не уйдёт открытым");
    let error = sync_error(&a, &storage);
    assert!(error.contains("not any more"), "{error}");
    std::fs::remove_file(storage.path().join("notes/vault.json")).unwrap();
    let error = sync_error(&a, &storage);
    assert!(error.contains("not any more"), "{error}");
    assert_sealed(&storage, &["не уйдёт открытым"]);

    // Connected again on purpose, the device takes the storage as it is.
    a.set_sync_config(SyncConfig::Off).unwrap();
    a.set_sync_config(SyncConfig::Folder {
        path: storage.path().to_string_lossy().into_owned(),
    })
    .unwrap();
    sync(&a, &storage);
}
