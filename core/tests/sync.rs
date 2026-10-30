//! Merge and sync rules: docs/specs/sync.md. Storage is a temporary folder.

mod common;

use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{mpsc, Mutex};

use common::*;
use notes_core::sync::remote::{DirRemote, Remote};
use notes_core::*;
use tempfile::TempDir;

/// The root of the storage.
const ROOT: &str = "notes";

fn sync(d: &Device, storage: &TempDir) -> SyncReport {
    d.sync_with(&DirRemote::new(storage.path())).unwrap()
}

/// S24: the pass that moves attachment content.
fn sync_files(d: &Device, storage: &TempDir) -> AttachmentReport {
    d.sync_attachments_with(&DirRemote::new(storage.path())).unwrap()
}

fn attach(d: &Device, note: &Note, name: &str, content: &[u8]) -> Attachment {
    let src = tempfile::tempdir().unwrap();
    let file = src.path().join(name);
    std::fs::write(&file, content).unwrap();
    d.add_attachment(note.id.clone(), file.to_string_lossy().into_owned(), None)
        .unwrap()
}

fn folder(storage: &TempDir) -> SyncConfig {
    SyncConfig::Folder {
        path: storage.path().to_string_lossy().into_owned(),
    }
}

/// A storage that keeps the fields and refuses attachment content whose path has `refused` in it.
struct NoBlobs {
    inner: DirRemote,
    refused: String,
}

impl NoBlobs {
    fn new(storage: &TempDir, refused: &str) -> NoBlobs {
        NoBlobs {
            inner: DirRemote::new(storage.path()),
            refused: refused.into(),
        }
    }

    fn check(&self, path: &str) -> Result<()> {
        if path.contains("/blobs/") && path.contains(&self.refused) {
            return Err(AppError::Sync {
                msg: "507 Insufficient Storage".into(),
            });
        }
        Ok(())
    }
}

impl Remote for NoBlobs {
    fn id(&self) -> String {
        self.inner.id()
    }
    fn list(&self, dir: &str) -> Result<Vec<String>> {
        self.inner.list(dir)
    }
    fn get(&self, path: &str) -> Result<Option<Vec<u8>>> {
        self.check(path)?;
        self.inner.get(path)
    }
    fn put(&self, path: &str, data: &[u8]) -> Result<()> {
        self.check(path)?;
        self.inner.put(path, data)
    }
    fn delete(&self, path: &str) -> Result<()> {
        self.inner.delete(path)
    }
    fn exists(&self, path: &str) -> Result<bool> {
        self.check(path)?;
        self.inner.exists(path)
    }
}

/// Both devices see everything the other has written.
fn settle(a: &Device, b: &Device, storage: &TempDir) {
    sync(a, storage);
    sync(b, storage);
    sync(a, storage);
}

fn files(storage: &TempDir, dir: &str) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(storage.path().join(ROOT).join(dir))
        .map(|entries| {
            entries
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

#[test]
fn s2_edits_of_different_fields_are_both_kept() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b) = (device(), device());
    let t = add(&a, "исходное");
    settle(&a, &b, &storage);
    assert_eq!(b.get_note(t.id.clone()).unwrap().title, "исходное");

    a.set_title(t.id.clone(), "новое название".into()).unwrap();
    b.set_starred(t.id.clone(), true).unwrap();
    settle(&a, &b, &storage);

    for d in [&a, &b] {
        let got = d.get_note(t.id.clone()).unwrap();
        assert_eq!(got.title, "новое название");
        assert!(got.starred);
    }
}

#[test]
fn s2_same_field_later_write_wins_and_resync_is_a_no_op() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b) = (device(), device());
    let t = add(&a, "исходное");
    settle(&a, &b, &storage);

    a.set_title(t.id.clone(), "от A".into()).unwrap();
    b.set_now_for_tests("2026-10-05T10:05");
    b.set_title(t.id.clone(), "от B, позже".into()).unwrap();
    // Order of delivery must not matter: A, the loser, syncs last here.
    sync(&b, &storage);
    sync(&a, &storage);
    sync(&b, &storage);

    assert_eq!(a.get_note(t.id.clone()).unwrap().title, "от B, позже");
    assert_eq!(b.get_note(t.id).unwrap().title, "от B, позже");
    assert_eq!(sync(&a, &storage), SyncReport::default());
    assert_eq!(sync(&b, &storage), SyncReport::default());
}

#[test]
fn s1_edit_made_after_seeing_a_future_stamp_still_wins() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b) = (device(), device());
    // B's clock runs a day ahead.
    b.set_now_for_tests("2026-10-06T10:00");
    let t = add(&b, "из будущего");
    settle(&b, &a, &storage);

    a.set_title(t.id.clone(), "исправлено на A".into()).unwrap();
    settle(&a, &b, &storage);
    assert_eq!(b.get_note(t.id).unwrap().title, "исправлено на A");
}

#[test]
fn s13_truncated_log_file_is_retried_not_skipped() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b) = (device(), device());
    add(&a, "первая");
    add(&a, "вторая");
    sync(&a, &storage);

    let name = files(&storage, "log").pop().unwrap();
    let path = storage.path().join(ROOT).join("log").join(&name);
    let whole = std::fs::read(&path).unwrap();
    std::fs::write(&path, &whole[..whole.len() * 2 / 3]).unwrap();

    assert_eq!(sync(&b, &storage).pulled, 0);
    assert!(
        oldest_first(&b).is_empty(),
        "nothing from a half-written file is applied"
    );

    std::fs::write(&path, &whole).unwrap();
    assert!(sync(&b, &storage).pulled > 0);
    assert_eq!(oldest_first(&b), ["первая", "вторая"]);
}

#[test]
fn s14_upload_interrupted_before_confirmation_is_repeated_identically() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b) = (device(), device());
    add(&a, "задача");
    sync(&a, &storage);
    let name = files(&storage, "log").pop().unwrap();
    let path = storage.path().join(ROOT).join("log").join(&name);
    let first = std::fs::read(&path).unwrap();

    // Nothing new locally: a second run must not rewrite or renumber anything.
    sync(&a, &storage);
    assert_eq!(files(&storage, "log"), [name]);
    assert_eq!(std::fs::read(&path).unwrap(), first);
    sync(&b, &storage);
    assert_eq!(oldest_first(&b), ["задача"]);
}

#[test]
fn s15_compaction_keeps_late_and_new_devices_complete() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b, c) = (device(), device(), device());
    a.set_compact_after_for_tests(3);

    add(&a, "t1");
    sync(&a, &storage);
    sync(&b, &storage); // B has read A's first file and then falls behind.
    assert_eq!(oldest_first(&b), ["t1"]);
    let from_b = add(&b, "от B");
    sync(&b, &storage);

    for i in 2..=6 {
        add(&a, &format!("t{i}"));
        sync(&a, &storage);
    }
    let a_id = a.device_id();
    let a_logs: Vec<String> = files(&storage, "log")
        .into_iter()
        .filter(|f| f.starts_with(&a_id) && f.ends_with(".jsonl"))
        .collect();
    assert!(a_logs.len() <= 3, "old log files are removed: {a_logs:?}");
    assert_eq!(
        files(&storage, "snap").iter().filter(|f| f.starts_with(&a_id)).count(),
        1
    );

    // B catches up through the snapshot; C starts from nothing.
    sync(&b, &storage);
    sync(&c, &storage);
    for d in [&b, &c] {
        let mut got = oldest_first(d);
        got.sort();
        assert_eq!(got, ["t1", "t2", "t3", "t4", "t5", "t6", "от B"]);
    }
    // The snapshot told C which of B's files it already contains, and edits keep flowing.
    b.set_title(from_b.id.clone(), "от B, изменено".into()).unwrap();
    sync(&b, &storage);
    sync(&c, &storage);
    sync(&a, &storage);
    assert_eq!(c.get_note(from_b.id.clone()).unwrap().title, "от B, изменено");
    assert_eq!(a.get_note(from_b.id).unwrap().title, "от B, изменено");
}

#[test]
fn s15_device_whose_logs_are_all_compacted_is_still_readable() {
    let storage = tempfile::tempdir().unwrap();
    let (a, c) = (device(), device());
    a.set_compact_after_for_tests(0);
    add(&a, "единственная");
    sync(&a, &storage);
    // Everything of A is in its snapshot; what is left in the log is the mark that says so.
    let left = files(&storage, "log");
    assert!(matches!(&left[..], [mark] if mark.ends_with(".snap")), "{left:?}");

    sync(&c, &storage);
    assert_eq!(oldest_first(&c), ["единственная"]);
}

/// Files of one device in a folder of the storage, with their sizes in bytes.
fn own_files(storage: &TempDir, dir: &str, device: &str, suffix: &str) -> Vec<(String, u64)> {
    files(storage, dir)
        .into_iter()
        .filter(|f| f.starts_with(device) && f.ends_with(suffix))
        .map(|f| {
            let size = std::fs::metadata(storage.path().join(ROOT).join(dir).join(&f))
                .unwrap()
                .len();
            (f, size)
        })
        .collect()
}

/// Letters that do not compress: a snapshot holding them weighs about what they do.
fn noise(len: usize, seed: u64) -> String {
    let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (b'a' + (x % 26) as u8) as char
        })
        .collect()
}

#[test]
fn s15_a_compressed_snapshot_carries_the_time_it_was_written() {
    let storage = tempfile::tempdir().unwrap();
    let b = device();
    b.set_compact_after_for_tests(0);
    add(&b, "задача");
    let now = || {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    };
    let before = now();
    sync(&b, &storage);
    let after = now();

    let [(name, _)] = &own_files(&storage, "snap", &b.device_id(), ".jsonl")[..] else {
        panic!("{:?}", files(&storage, "snap"))
    };
    let data = std::fs::read(storage.path().join(ROOT).join("snap").join(name)).unwrap();
    assert!(data.starts_with(&[0x1f, 0x8b]), "the snapshot is compressed");
    // RFC 1952: MTIME is bytes 4-7 of the header, little-endian seconds since 1970.
    let mtime = u64::from(u32::from_le_bytes(data[4..8].try_into().unwrap()));
    assert!(
        (before..=after).contains(&mtime),
        "{mtime} not within {before}..={after}"
    );
}

#[test]
fn s16_joining_a_used_storage_merges_instead_of_replacing() {
    let storage = tempfile::tempdir().unwrap();
    let (a, c) = (device(), device());
    add(&a, "с A");
    sync(&a, &storage);
    add(&c, "с C");
    settle(&c, &a, &storage);
    for d in [&a, &c] {
        let mut got = oldest_first(d);
        got.sort();
        assert_eq!(got, ["с A", "с C"]);
    }
}

#[test]
fn s23_attachment_content_follows_the_record() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b) = (device(), device());
    let t = add(&a, "с фото");
    let att = attach(&a, &t, "photo.png", b"\x89PNG not really");

    sync(&a, &storage);
    assert_eq!(sync_files(&a, &storage).uploaded, 1);
    assert!(Path::new(
        &storage
            .path()
            .join(ROOT)
            .join("blobs")
            .join(&att.sha256[..2])
            .join(&att.sha256)
    )
    .exists());

    sync(&b, &storage);
    assert_eq!(sync_files(&b, &storage).downloaded, 1);
    let got = b.attachments(t.id).unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].sha256, att.sha256);
    assert_eq!(
        std::fs::read(got[0].local_path.as_ref().unwrap()).unwrap(),
        b"\x89PNG not really"
    );

    // Nothing is transferred twice.
    for d in [&a, &b] {
        assert_eq!(sync(d, &storage), SyncReport::default());
        assert_eq!(sync_files(d, &storage), AttachmentReport::default());
    }
}

#[test]
fn s23_corrupted_attachment_content_is_not_stored() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b) = (device(), device());
    let t = add(&a, "с файлом");
    let att = attach(&a, &t, "a.txt", b"original");
    sync(&a, &storage);
    sync_files(&a, &storage);
    std::fs::write(
        storage
            .path()
            .join(ROOT)
            .join("blobs")
            .join(&att.sha256[..2])
            .join(&att.sha256),
        b"tampered",
    )
    .unwrap();

    sync(&b, &storage);
    let pass = sync_files(&b, &storage);
    assert_eq!((pass.downloaded, pass.waiting), (0, 1));
    assert_eq!(b.attachments(t.id).unwrap()[0].local_path, None);
    // Nothing of the attempt is left next to the content.
    assert_eq!(std::fs::read_dir(b.data_dir().join("blobs")).unwrap().count(), 0);
}

#[test]
fn s24_the_fields_arrive_before_the_content() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b) = (device(), device());
    for d in [&a, &b] {
        d.set_sync_config(folder(&storage)).unwrap();
    }
    let t = add(&a, "с файлом");
    attach(&a, &t, "a.txt", b"content");
    assert_eq!(a.sync_status().unwrap().attachments_waiting, 1);

    // The run for the fields leaves the content alone, on both sides.
    let up = a.sync_now().unwrap();
    assert!(up.pushed > 0);
    assert_eq!((up.blobs_uploaded, up.blobs_downloaded), (0, 0));
    assert!(!storage.path().join(ROOT).join("blobs").exists());
    let down = b.sync_now().unwrap();
    assert_eq!((down.blobs_uploaded, down.blobs_downloaded), (0, 0));
    assert_eq!(oldest_first(&b), ["с файлом"]);
    let got = b.attachments(t.id.clone()).unwrap();
    assert_eq!((got.len(), got[0].local_path.as_ref()), (1, None));
    assert_eq!(b.get_note(t.id.clone()).unwrap().attachments, 1);
    assert_eq!(b.sync_status().unwrap().attachments_waiting, 1);

    // B asks before A has uploaded: nothing to take yet, and no failure.
    assert_eq!(
        b.sync_attachments().unwrap(),
        AttachmentReport {
            uploaded: 0,
            downloaded: 0,
            waiting: 1
        }
    );
    assert_eq!(
        a.sync_attachments().unwrap(),
        AttachmentReport {
            uploaded: 1,
            downloaded: 0,
            waiting: 0
        }
    );
    assert_eq!(
        b.sync_attachments().unwrap(),
        AttachmentReport {
            uploaded: 0,
            downloaded: 1,
            waiting: 0
        }
    );
    for d in [&a, &b] {
        let status = d.sync_status().unwrap();
        assert_eq!((status.attachments_waiting, status.last_error), (0, None));
    }
    assert!(b.attachments(t.id).unwrap()[0].local_path.is_some());
}

#[test]
fn s24_refused_content_holds_back_neither_the_note_nor_the_other_files() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b) = (device(), device());
    let t = add(&a, "с двумя файлами");
    let big = attach(&a, &t, "big.bin", b"refused by the storage");
    let small = attach(&a, &t, "small.txt", b"accepted");
    let refusing = NoBlobs::new(&storage, &big.sha256);

    assert!(a.sync_with(&refusing).unwrap().pushed > 0);
    let up = a.sync_attachments_with(&refusing).unwrap();
    assert_eq!((up.uploaded, up.waiting), (1, 1));

    assert!(b.sync_with(&refusing).unwrap().pulled > 0);
    assert_eq!(oldest_first(&b), ["с двумя файлами"]);
    let down = b.sync_attachments_with(&refusing).unwrap();
    assert_eq!((down.downloaded, down.waiting), (1, 1));
    let present: Vec<String> = b
        .attachments(t.id.clone())
        .unwrap()
        .into_iter()
        .filter(|f| f.local_path.is_some())
        .map(|f| f.sha256)
        .collect();
    assert_eq!(present, [small.sha256]);

    // The storage takes the file after all: the next pass moves it without another edit.
    assert_eq!(sync_files(&a, &storage).uploaded, 1);
    let rest = sync_files(&b, &storage);
    assert_eq!((rest.downloaded, rest.waiting), (1, 0));
}

/// A storage where reading attachment content waits until the test lets it go.
struct Held {
    inner: DirRemote,
    entered: Mutex<mpsc::Sender<()>>,
    release: Mutex<mpsc::Receiver<()>>,
}

impl Remote for Held {
    fn id(&self) -> String {
        self.inner.id()
    }
    fn list(&self, dir: &str) -> Result<Vec<String>> {
        self.inner.list(dir)
    }
    fn get(&self, path: &str) -> Result<Option<Vec<u8>>> {
        if path.contains("/blobs/") {
            let _ = self.entered.lock().unwrap().send(());
            // Goes on when the test sends a word or hangs up.
            let _ = self.release.lock().unwrap().recv();
        }
        self.inner.get(path)
    }
    fn put(&self, path: &str, data: &[u8]) -> Result<()> {
        self.inner.put(path, data)
    }
    fn delete(&self, path: &str) -> Result<()> {
        self.inner.delete(path)
    }
    fn exists(&self, path: &str) -> Result<bool> {
        self.inner.exists(path)
    }
}

#[test]
fn s24_s25_a_pass_under_way_holds_back_neither_the_fields_nor_a_request() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b) = (device(), device());
    b.set_sync_config(folder(&storage)).unwrap();
    let t = add(&a, "с файлами");
    let wanted = attach(&a, &t, "first.txt", b"first");
    attach(&a, &t, "second.txt", b"second");
    sync(&a, &storage);
    sync_files(&a, &storage);
    sync(&b, &storage);

    let (entered, inside) = mpsc::channel();
    let (go, release) = mpsc::channel::<()>();
    let held = Held {
        inner: DirRemote::new(storage.path()),
        entered: Mutex::new(entered),
        release: Mutex::new(release),
    };
    let store = b.store.clone();
    let pass = std::thread::spawn(move || store.sync_attachments_with(&held).unwrap());
    inside.recv().unwrap();

    // The pass is in the middle of a download: a run for the fields goes through.
    a.set_title(t.id.clone(), "переименовано".into()).unwrap();
    sync(&a, &storage);
    assert!(sync(&b, &storage).pulled > 0);
    assert_eq!(oldest_first(&b), ["переименовано"]);
    // A second pass does not queue behind the first.
    let second = sync_files(&b, &storage);
    assert_eq!((second.downloaded, second.waiting), (0, 2));
    // And a request for one file gets it.
    let got = b.fetch_attachment(wanted.id).unwrap();
    assert_eq!(std::fs::read(got.local_path.unwrap()).unwrap(), b"first");

    drop(go);
    assert_eq!(pass.join().unwrap().waiting, 0);
    let files = b.attachments(t.id).unwrap();
    assert!(files.iter().all(|f| f.local_path.is_some()));
    // Each download wrote a file of its own and left nothing behind.
    assert_eq!(std::fs::read_dir(b.data_dir().join("blobs")).unwrap().count(), 2);
}

#[test]
fn s24_content_that_cannot_move_does_not_fail_configured_sync() {
    let storage = tempfile::tempdir().unwrap();
    let a = device();
    a.set_sync_config(folder(&storage)).unwrap();
    let t = add(&a, "с файлом");
    let att = attach(&a, &t, "a.txt", b"content");
    // A file where the folder of this content has to be: the upload cannot succeed.
    std::fs::create_dir_all(storage.path().join(ROOT).join("blobs")).unwrap();
    std::fs::write(storage.path().join(ROOT).join("blobs").join(&att.sha256[..2]), b"").unwrap();

    assert!(a.sync_now().unwrap().pushed > 0);
    let pass = a.sync_attachments().unwrap();
    assert_eq!((pass.uploaded, pass.waiting), (0, 1));
    let status = a.sync_status().unwrap();
    assert_eq!((status.pending, status.attachments_waiting), (0, 1));
    assert_eq!(status.last_error, None);
    assert_eq!(status.last_ok.as_deref(), Some(NOW));
}

#[test]
fn s24_attachments_of_purged_notes_and_removed_attachments_do_not_wait() {
    let storage = tempfile::tempdir().unwrap();
    let a = device();
    a.set_sync_config(folder(&storage)).unwrap();
    let t = add(&a, "с файлом");
    let att = attach(&a, &t, "a.txt", b"content");
    assert_eq!(a.sync_status().unwrap().attachments_waiting, 1);
    a.remove_attachment(att.id).unwrap();
    assert_eq!(a.sync_status().unwrap().attachments_waiting, 0);
    assert_eq!(a.sync_attachments().unwrap(), AttachmentReport::default());

    // Without a file storage nothing waits, whatever is attached.
    attach(&a, &t, "b.txt", b"other");
    a.set_sync_config(SyncConfig::Off).unwrap();
    assert_eq!(a.sync_status().unwrap().attachments_waiting, 0);
    assert_eq!(a.sync_attachments().unwrap(), AttachmentReport::default());
}

#[test]
fn s25_one_attachment_is_fetched_on_request() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b) = (device(), device());
    for d in [&a, &b] {
        d.set_sync_config(folder(&storage)).unwrap();
    }
    let t = add(&a, "с двумя файлами");
    let first = attach(&a, &t, "first.txt", b"first");
    let second = attach(&a, &t, "second.txt", b"second");
    a.sync_now().unwrap();
    b.sync_now().unwrap();

    // The content has not reached the storage: the request says so and changes nothing.
    assert!(b.fetch_attachment(second.id.clone()).is_err());
    assert_eq!(b.sync_status().unwrap().attachments_waiting, 2);

    a.sync_attachments().unwrap();
    let got = b.fetch_attachment(second.id.clone()).unwrap();
    assert_eq!((got.id.as_str(), got.name.as_str()), (second.id.as_str(), "second.txt"));
    assert_eq!(std::fs::read(got.local_path.as_ref().unwrap()).unwrap(), b"second");
    let rest = b.attachments(t.id.clone()).unwrap();
    assert_eq!(rest.iter().find(|f| f.id == first.id).unwrap().local_path, None);
    assert_eq!(b.sync_status().unwrap().attachments_waiting, 1);

    // Asking for what is already here costs nothing and gives the same file.
    std::fs::remove_dir_all(storage.path().join(ROOT).join("blobs")).unwrap();
    assert_eq!(b.fetch_attachment(second.id).unwrap(), got);
    assert!(b.fetch_attachment(first.id).is_err());
    assert!(matches!(
        b.fetch_attachment("no such attachment".into()),
        Err(AppError::NotFound { .. })
    ));
}

#[test]
fn s25_content_that_does_not_match_its_hash_is_not_fetched() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b) = (device(), device());
    for d in [&a, &b] {
        d.set_sync_config(folder(&storage)).unwrap();
    }
    let t = add(&a, "с файлом");
    let att = attach(&a, &t, "a.txt", b"original");
    a.sync_now().unwrap();
    a.sync_attachments().unwrap();
    b.sync_now().unwrap();
    std::fs::write(
        storage
            .path()
            .join(ROOT)
            .join("blobs")
            .join(&att.sha256[..2])
            .join(&att.sha256),
        b"tampered",
    )
    .unwrap();

    assert!(b.fetch_attachment(att.id).is_err());
    assert_eq!(b.attachments(t.id).unwrap()[0].local_path, None);
}

/// S25: a storage whose byte methods refuse attachment content: the core must use the file methods.
struct Streams {
    inner: DirRemote,
    sent: AtomicU32,
    taken: AtomicU32,
}

impl Streams {
    fn new(storage: &TempDir) -> Self {
        Streams {
            inner: DirRemote::new(storage.path()),
            sent: AtomicU32::new(0),
            taken: AtomicU32::new(0),
        }
    }
}

impl Remote for Streams {
    fn id(&self) -> String {
        self.inner.id()
    }
    fn list(&self, dir: &str) -> Result<Vec<String>> {
        self.inner.list(dir)
    }
    fn get(&self, path: &str) -> Result<Option<Vec<u8>>> {
        if path.contains("/blobs/") {
            panic!("the content was read whole: {path}");
        }
        self.inner.get(path)
    }
    fn put(&self, path: &str, data: &[u8]) -> Result<()> {
        if path.contains("/blobs/") {
            panic!("the content was written whole: {path}");
        }
        self.inner.put(path, data)
    }
    fn get_to(&self, path: &str, out: &mut dyn std::io::Write, max: u64) -> Result<bool> {
        if path.contains("/blobs/") {
            self.taken.fetch_add(1, Ordering::Relaxed);
        }
        self.inner.get_to(path, out, max)
    }
    fn put_file(&self, path: &str, local: &Path) -> Result<()> {
        if path.contains("/blobs/") {
            self.sent.fetch_add(1, Ordering::Relaxed);
        }
        self.inner.put_file(path, local)
    }
    fn delete(&self, path: &str) -> Result<()> {
        self.inner.delete(path)
    }
    fn exists(&self, path: &str) -> Result<bool> {
        self.inner.exists(path)
    }
}

#[test]
fn s25_content_goes_through_the_files_on_disk() {
    let storage = tempfile::tempdir().unwrap();
    let remote = Streams::new(&storage);
    let (a, b) = (device(), device());
    let content: Vec<u8> = (0..16 * 1024 * 1024).map(|i| (i % 251) as u8).collect();
    let t = add(&a, "с большим файлом");
    attach(&a, &t, "large.bin", &content);

    // A pass for the fields goes byte by byte only for its own small files.
    a.sync_with(&remote).unwrap();
    let report = a.sync_attachments_with(&remote).unwrap();
    assert_eq!((report.uploaded, report.downloaded), (1, 0));
    assert_eq!(remote.sent.load(Ordering::Relaxed), 1, "the file went from the disk");

    b.sync_with(&remote).unwrap();
    let report = b.sync_attachments_with(&remote).unwrap();
    assert_eq!((report.uploaded, report.downloaded), (0, 1));
    assert_eq!(remote.taken.load(Ordering::Relaxed), 1, "the file came to the disk");
    let got = b.attachments(t.id).unwrap().remove(0);
    assert_eq!(std::fs::read(got.local_path.unwrap()).unwrap(), content);
}

#[test]
fn s25_content_above_the_limit_is_not_downloaded() {
    let storage = tempfile::tempdir().unwrap();
    let remote = DirRemote::new(storage.path());
    let (a, b) = (device(), device());
    for d in [&a, &b] {
        d.set_sync_config(folder(&storage)).unwrap();
    }
    let t = add(&a, "с файлом");
    attach(&a, &t, "a.txt", b"twenty bytes of text!");
    a.sync_with(&remote).unwrap();
    a.sync_attachments_with(&remote).unwrap();

    b.set_attachment_limits(AttachmentLimits {
        max_file: 10,
        max_note: 1000,
    })
    .unwrap();
    b.sync_with(&remote).unwrap();
    let report = b.sync_attachments_with(&remote).unwrap();
    assert_eq!((report.uploaded, report.downloaded), (0, 0));
    assert_eq!(b.attachments(t.id.clone()).unwrap()[0].local_path, None);
    assert_eq!(b.sync_status().unwrap().attachments_waiting, 1);
    assert_eq!(
        std::fs::read_dir(b.dir().join("blobs")).unwrap().count(),
        0,
        "nothing was kept"
    );

    // With room for the file, the same content arrives on the next pass.
    b.set_attachment_limits(AttachmentLimits {
        max_file: 1000,
        max_note: 2000,
    })
    .unwrap();
    let report = b.sync_attachments_with(&remote).unwrap();
    assert_eq!(report.downloaded, 1);
    let got = b.attachments(t.id).unwrap().remove(0);
    assert_eq!(
        std::fs::read(got.local_path.unwrap()).unwrap(),
        b"twenty bytes of text!"
    );
}

#[test]
fn r35_content_that_has_not_arrived_is_fetched_before_everything_is_saved() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b) = (device(), device());
    for d in [&a, &b] {
        d.set_sync_config(folder(&storage)).unwrap();
    }
    let t = add(&a, "с тремя файлами");
    attach(&a, &t, "a.txt", b"first");
    let absent = attach(&a, &t, "b.pdf", b"%PDF-1.7 second");
    attach(&a, &t, "c.png", b"third");
    a.sync_now().unwrap();
    a.sync_attachments().unwrap();
    b.sync_now().unwrap();
    // One of the three is on the device already, the other two only in the storage.
    let here = b
        .attachments(t.id.clone())
        .unwrap()
        .into_iter()
        .find(|f| f.name == "a.txt")
        .unwrap();
    b.fetch_attachment(here.id).unwrap();

    let out = tempfile::tempdir().unwrap();
    let report = b
        .save_attachments(t.id.clone(), out.path().to_string_lossy().into_owned())
        .unwrap();

    assert_eq!((report.saved.len(), report.failed.len(), report.fetched), (3, 0, 2));
    let read = |dir: &TempDir, name: &str| std::fs::read(dir.path().join(name)).ok();
    assert_eq!(read(&out, "a.txt").as_deref(), Some(b"first".as_slice()));
    assert_eq!(read(&out, "b.pdf").as_deref(), Some(b"%PDF-1.7 second".as_slice()));
    assert_eq!(read(&out, "c.png").as_deref(), Some(b"third".as_slice()));
    assert_eq!(b.sync_status().unwrap().attachments_waiting, 0);

    // A third device finds one file missing in the storage: it is named, the other two are saved.
    let c = device();
    c.set_sync_config(folder(&storage)).unwrap();
    c.sync_now().unwrap();
    std::fs::remove_file(
        storage
            .path()
            .join(ROOT)
            .join("blobs")
            .join(&absent.sha256[..2])
            .join(&absent.sha256),
    )
    .unwrap();
    let out = tempfile::tempdir().unwrap();
    let report = c
        .save_attachments(t.id, out.path().to_string_lossy().into_owned())
        .unwrap();

    assert_eq!((report.failed, report.fetched), (vec!["b.pdf".to_string()], 2));
    assert_eq!(read(&out, "a.txt").as_deref(), Some(b"first".as_slice()));
    assert_eq!(read(&out, "c.png").as_deref(), Some(b"third".as_slice()));
    assert_eq!(read(&out, "b.pdf"), None);
}

#[test]
fn s17_newer_storage_format_stops_sync_but_not_local_work() {
    let storage = tempfile::tempdir().unwrap();
    let a = device();
    std::fs::create_dir_all(storage.path().join(ROOT)).unwrap();
    std::fs::write(
        storage.path().join(ROOT).join("vault.json"),
        br#"{"format":3,"vault":"x"}"#,
    )
    .unwrap();

    let err = a.sync_with(&DirRemote::new(storage.path())).unwrap_err();
    assert!(err.to_string().contains("format 3"), "{err}");
    add(&a, "локально");
    assert_eq!(oldest_first(&a), ["локально"]);
}

#[test]
fn s17_format_raised_while_the_app_runs_is_noticed_at_the_first_unreadable_file() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b) = (device(), device());
    add(&a, "t1");
    settle(&a, &b, &storage);

    // Another device moved the storage on and wrote a file this version cannot read.
    let root = storage.path().join(ROOT);
    std::fs::write(root.join("vault.json"), br#"{"format":3,"vault":"x"}"#).unwrap();
    let next = format!("{}-{:010}.jsonl", a.device_id(), 2);
    std::fs::write(
        root.join("log").join(next),
        format!("{{\"v\":3,\"device\":\"{}\",\"seq\":2,\"count\":0}}\n", a.device_id()),
    )
    .unwrap();
    let err = b.sync_with(&DirRemote::new(storage.path())).unwrap_err();
    assert!(err.to_string().contains("format 3"), "{err}");
}

#[test]
fn s19_configured_sync_reports_status_and_keeps_changes_after_a_failure() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b) = (device(), device());
    assert_eq!(
        a.sync_now().unwrap(),
        SyncReport::default(),
        "no storage configured: nothing to do"
    );
    assert!(!a.sync_status().unwrap().configured);

    // A path that cannot be a directory: every write fails.
    let blocker = storage.path().join("file");
    std::fs::write(&blocker, b"x").unwrap();
    a.set_sync_config(SyncConfig::Folder {
        path: blocker.join("sub").to_string_lossy().into_owned(),
    })
    .unwrap();
    add(&a, "не потеряется");
    assert!(a.sync_now().is_err());
    let status = a.sync_status().unwrap();
    assert!(status.configured && status.pending > 0 && status.last_error.is_some() && status.last_ok.is_none());

    let good = SyncConfig::Folder {
        path: storage.path().join("ok").to_string_lossy().into_owned(),
    };
    a.set_sync_config(good.clone()).unwrap();
    assert_eq!(a.sync_config().unwrap(), good);
    assert!(a.sync_now().unwrap().pushed > 0);
    let status = a.sync_status().unwrap();
    assert_eq!((status.pending, status.last_error), (0, None));
    assert_eq!(status.last_ok.as_deref(), Some(NOW));

    b.set_sync_config(good).unwrap();
    b.sync_now().unwrap();
    assert_eq!(oldest_first(&b), ["не потеряется"]);
}

#[test]
fn s39_a_file_brought_into_the_sync_folder_is_noticed_once() {
    let storage = tempfile::tempdir().unwrap();
    let folder = SyncConfig::Folder {
        path: storage.path().to_string_lossy().into_owned(),
    };
    let (a, b) = (device(), device());
    a.set_sync_config(folder.clone()).unwrap();
    b.set_sync_config(folder).unwrap();
    add(&a, "от A");
    a.sync_now().unwrap();
    assert!(!a.folder_changed(), "its own file is not news");

    assert!(b.folder_changed(), "A's file is there");
    assert!(!b.folder_changed(), "and is reported once");
    b.sync_now().unwrap();
    assert!(!b.folder_changed());
    // B's first run there published its snapshot and the mark for it.
    assert!(a.folder_changed());
    a.sync_now().unwrap();
    assert!(!a.folder_changed());
    assert!(!b.folder_changed(), "A had nothing to write");

    add(&b, "от B");
    b.sync_now().unwrap();
    assert!(a.folder_changed());

    // Other kinds of sync are not watched.
    assert!(!device().folder_changed());
}

#[test]
fn s16_switching_storage_publishes_the_whole_state_there() {
    let (old, new) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let folder = |d: &TempDir| SyncConfig::Folder {
        path: d.path().to_string_lossy().into_owned(),
    };
    let (a, b, c) = (device(), device(), device());
    a.set_sync_config(folder(&old)).unwrap();
    b.set_sync_config(folder(&old)).unwrap();
    add(&a, "от A");
    a.sync_now().unwrap();
    add(&b, "от B");
    b.sync_now().unwrap();
    a.sync_now().unwrap();

    // A moves to a new storage: it must carry B's note along, not only its own writes.
    a.set_sync_config(folder(&new)).unwrap();
    a.sync_now().unwrap();
    c.set_sync_config(folder(&new)).unwrap();
    c.sync_now().unwrap();
    let mut got = oldest_first(&c);
    got.sort();
    assert_eq!(got, ["от A", "от B"]);

    // Going back to the old storage keeps working for devices that stayed there.
    add(&a, "после возврата");
    a.set_sync_config(folder(&old)).unwrap();
    a.sync_now().unwrap();
    b.sync_now().unwrap();
    assert!(oldest_first(&b).contains(&"после возврата".to_string()));
}

#[test]
fn s23_attachment_hash_from_the_storage_never_becomes_a_path() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b) = (device(), device());
    let t = add(&a, "с файлом");
    let att = attach(&a, &t, "a.txt", b"content");
    sync(&a, &storage);
    sync_files(&a, &storage);

    // A file that exists on B outside its blob folder, and a log in the storage
    // that points the attachment at it.
    let outside = b.data_dir().join("notes.sqlite");
    assert!(outside.exists());
    let log = storage.path().join(ROOT).join("log");
    let snap = storage.path().join(ROOT).join("snap");
    for dir in [log, snap] {
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let text = std::fs::read_to_string(entry.path()).unwrap();
            std::fs::write(entry.path(), text.replace(&att.sha256, "../notes.sqlite")).unwrap();
        }
    }

    sync(&b, &storage);
    assert_eq!(sync_files(&b, &storage), AttachmentReport::default());
    let got = b.attachments(t.id.clone()).unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].sha256, "", "a value that is not a hash is dropped");
    assert_eq!(got[0].local_path, None);
    assert!(b.fetch_attachment(got[0].id.clone()).is_err());
}

/// A storage that cannot be reached while `down` is set.
struct Flaky {
    inner: DirRemote,
    down: std::sync::atomic::AtomicBool,
}

impl Flaky {
    fn new(storage: &TempDir) -> Flaky {
        Flaky {
            inner: DirRemote::new(storage.path()),
            down: std::sync::atomic::AtomicBool::new(true),
        }
    }

    fn check(&self) -> Result<()> {
        if self.down.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(AppError::Sync {
                msg: "connection refused".into(),
            });
        }
        Ok(())
    }
}

impl Remote for Flaky {
    fn id(&self) -> String {
        self.inner.id()
    }
    fn list(&self, dir: &str) -> Result<Vec<String>> {
        self.check()?;
        self.inner.list(dir)
    }
    fn get(&self, path: &str) -> Result<Option<Vec<u8>>> {
        self.check()?;
        self.inner.get(path)
    }
    fn put(&self, path: &str, data: &[u8]) -> Result<()> {
        self.check()?;
        self.inner.put(path, data)
    }
    fn delete(&self, path: &str) -> Result<()> {
        self.check()?;
        self.inner.delete(path)
    }
    fn exists(&self, path: &str) -> Result<bool> {
        self.check()?;
        self.inner.exists(path)
    }
}

fn sorted(mut titles: Vec<String>) -> Vec<String> {
    titles.sort();
    titles
}

/// Where a device keeps attachment content, and where the storage does.
fn on_device(d: &Device, sha256: &str) -> bool {
    d.dir().join("blobs").join(sha256).exists()
}

fn in_storage(storage: &TempDir, sha256: &str) -> bool {
    storage
        .path()
        .join(ROOT)
        .join("blobs")
        .join(&sha256[..2])
        .join(sha256)
        .exists()
}

/// Both passes, the way the apps run them (S24).
fn sync_all(d: &Device, storage: &TempDir) {
    sync(d, storage);
    sync_files(d, storage);
}

/// A storage that counts what is deleted from `blobs/`.
struct Counting {
    inner: DirRemote,
    deleted: Mutex<Vec<String>>,
}

impl Remote for Counting {
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
        self.inner.put(path, data)
    }
    fn delete(&self, path: &str) -> Result<()> {
        if path.contains("/blobs/") {
            self.deleted.lock().unwrap().push(path.into());
        }
        self.inner.delete(path)
    }
    fn exists(&self, path: &str) -> Result<bool> {
        self.inner.exists(path)
    }
}

/// Everything a user can see, in a form two devices can compare.
fn dump(d: &Device) -> Vec<String> {
    let mut out = Vec::new();
    for f in d.folders().unwrap() {
        out.push(format!(
            "# {} | {} | parent={:?} icon={} daily={} sort={:?} {}/{} count={}",
            f.id, f.name, f.parent_id, f.icon, f.daily, f.sort, f.show_preview, f.show_date, f.note_count
        ));
    }
    for scope in [Scope::All, Scope::Archived, Scope::Trash] {
        out.push(format!("# {scope:?}"));
        for n in d.notes(scope).unwrap() {
            out.push(format!(
                "  {} | {} | folder={:?} tags={:?} pinned={} starred={} archived={} locked={} deleted={:?} files={}",
                n.id,
                n.title,
                n.folder_id,
                n.tags,
                n.pinned,
                n.starred,
                n.archived,
                n.locked,
                n.deleted_ms,
                n.attachments
            ));
        }
    }
    for f in d.filters().unwrap() {
        out.push(format!("# filter {} | {} | {:?} | {}", f.id, f.name, f.query, f.count));
    }
    out.push(format!("# settings {:?}", d.settings().unwrap()));
    out
}

// ---- merge: notes, the Trash, folders (S6-S11) ----

#[test]
fn s6_a_note_deleted_on_one_device_and_edited_on_another_stays_deleted_and_the_edit_is_kept() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b) = (device(), device());
    let n = add(&a, "заметка");
    settle(&a, &b, &storage);

    a.delete_note(n.id.clone()).unwrap();
    b.set_title(n.id.clone(), "заголовок от B".into()).unwrap();
    settle(&a, &b, &storage);
    let moment = a.get_note(n.id.clone()).unwrap().deleted_ms;
    assert!(moment.is_some());
    for d in [&a, &b] {
        assert!(view(d, Scope::All).is_empty());
        assert_eq!(view(d, Scope::Trash), ["заголовок от B"]);
        assert_eq!(
            d.get_note(n.id.clone()).unwrap().deleted_ms,
            moment,
            "the moment of deletion travels"
        );
    }

    a.restore_note(n.id.clone()).unwrap();
    settle(&a, &b, &storage);
    assert_eq!(b.get_note(n.id).unwrap().title, "заголовок от B");
    assert_eq!(view(&b, Scope::All), ["заголовок от B"]);
    assert_eq!(dump(&a), dump(&b));
}

#[test]
fn s7_a_note_purged_on_one_device_and_restored_on_another_while_offline_stays_purged() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b) = (device(), device());
    let n = add(&a, "навсегда");
    a.delete_note(n.id.clone()).unwrap();
    settle(&a, &b, &storage);

    a.purge_note(n.id.clone()).unwrap();
    b.set_now_for_tests("2026-10-05T10:30");
    b.restore_note(n.id.clone()).unwrap();
    assert_eq!(view(&b, Scope::All), ["навсегда"]);
    settle(&a, &b, &storage);
    settle(&b, &a, &storage);
    for d in [&a, &b] {
        assert!(matches!(d.get_note(n.id.clone()), Err(AppError::NotFound { .. })));
        assert!(view(d, Scope::All).is_empty());
        assert!(view(d, Scope::Trash).is_empty());
        assert!(d.counts().unwrap().all == 0 && d.counts().unwrap().trash == 0);
    }
    assert_eq!(dump(&a), dump(&b));
}

#[test]
fn s8_a_note_in_the_trash_longer_than_trash_days_is_purged_at_the_end_of_a_pass_that_read_everything() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b) = (device(), device());
    let n = add(&a, "старая");
    a.delete_note(n.id.clone()).unwrap();
    settle(&a, &b, &storage);

    // 29 days later: still in the Trash. 31 days later: purged by the pass, and the purge goes out in it.
    a.set_now_for_tests("2026-11-03T10:00");
    sync(&a, &storage);
    assert!(a.get_note(n.id.clone()).unwrap().deleted);
    a.set_now_for_tests("2026-11-05T10:01");
    sync(&a, &storage);
    assert!(matches!(a.get_note(n.id.clone()), Err(AppError::NotFound { .. })));
    b.set_now_for_tests("2026-10-05T10:30");
    sync(&b, &storage);
    assert!(matches!(b.get_note(n.id), Err(AppError::NotFound { .. })));
}

#[test]
fn s8_the_days_in_the_trash_are_those_of_the_setting_as_it_arrives() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b) = (device(), device());
    let n = add(&a, "в корзине");
    a.delete_note(n.id.clone()).unwrap();
    settle(&a, &b, &storage);

    b.set_settings(AppSettings {
        trash_days: 60,
        ..AppSettings::default()
    })
    .unwrap();
    settle(&b, &a, &storage);
    a.set_now_for_tests("2026-11-10T10:00");
    sync(&a, &storage);
    assert!(a.get_note(n.id.clone()).unwrap().deleted, "36 days with a limit of 60");

    b.set_now_for_tests("2026-11-10T10:00");
    b.set_settings(AppSettings {
        trash_days: 7,
        ..AppSettings::default()
    })
    .unwrap();
    sync(&b, &storage);
    assert!(
        matches!(b.get_note(n.id.clone()), Err(AppError::NotFound { .. })),
        "the device that set it purges at once"
    );
    sync(&a, &storage);
    assert!(
        matches!(a.get_note(n.id), Err(AppError::NotFound { .. })),
        "and the other as the setting arrives"
    );
}

#[test]
fn s8_a_run_that_did_not_read_everything_purges_nothing() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b) = (device(), device());
    let n = add(&a, "старая");
    a.delete_note(n.id.clone()).unwrap();
    settle(&a, &b, &storage);

    // A file of B's log that A cannot read yet: it may hold the edit that brings the note back.
    b.set_now_for_tests("2026-11-10T09:00");
    b.restore_note(n.id.clone()).unwrap();
    sync(&b, &storage);
    let name = files(&storage, "log")
        .into_iter()
        .rfind(|f| f.starts_with(&b.device_id()))
        .unwrap();
    let path = storage.path().join(ROOT).join("log").join(&name);
    let whole = std::fs::read(&path).unwrap();
    std::fs::write(&path, &whole[..whole.len() / 2]).unwrap();

    a.set_now_for_tests("2026-11-10T10:00");
    sync(&a, &storage);
    assert!(
        a.get_note(n.id.clone()).unwrap().deleted,
        "36 days in the Trash, and still there"
    );

    std::fs::write(&path, &whole).unwrap();
    sync(&a, &storage);
    let got = a.get_note(n.id).unwrap();
    assert!(!got.deleted, "the restore arrived first");
}

#[test]
fn s9_concurrent_moves_into_each_other_leave_no_cycle_and_every_device_shows_the_same_tree() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b, c) = (device(), device(), device());
    let x = new_folder(&a, "X", None);
    let y = new_folder(&a, "Y", None);
    let inside = add_in(&a, "в X", &x);
    settle(&a, &b, &storage);

    a.move_folder(x.id.clone(), Some(y.id.clone()), None).unwrap();
    b.set_now_for_tests("2026-10-05T10:05");
    b.move_folder(y.id.clone(), Some(x.id.clone()), None).unwrap();
    settle(&a, &b, &storage);
    sync(&c, &storage);

    // The later move is the one undone: Y stays on top, X under it.
    for d in [&a, &b, &c] {
        assert_eq!(d.folder(y.id.clone()).unwrap().parent_id, None);
        assert_eq!(
            d.folder(x.id.clone()).unwrap().parent_id.as_deref(),
            Some(y.id.as_str())
        );
        assert_eq!(
            d.get_note(inside.id.clone()).unwrap().folder_id.as_deref(),
            Some(x.id.as_str())
        );
    }
    assert_eq!(dump(&a), dump(&b));
    assert_eq!(dump(&a), dump(&c));

    // The cut is written into the field by the next local move of any folder, and that reaches the others.
    let z = new_folder(&b, "Z", None);
    b.move_folder(z.id, None, None).unwrap();
    settle(&b, &a, &storage);
    sync(&c, &storage);
    assert_eq!(dump(&a), dump(&b));
    assert_eq!(dump(&a), dump(&c));
}

#[test]
fn s9_a_longer_cycle_is_cut_at_the_same_folder_whatever_the_order_the_files_are_read_in() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b, c) = (device(), device(), device());
    let folders: Vec<Folder> = ["X", "Y", "Z"].iter().map(|n| new_folder(&a, n, None)).collect();
    settle(&a, &b, &storage);
    sync(&c, &storage);

    // X into Y on A, Y into Z on B, Z into X on C: each device sees only its own move.
    a.move_folder(folders[0].id.clone(), Some(folders[1].id.clone()), None)
        .unwrap();
    b.set_now_for_tests("2026-10-05T10:01");
    b.move_folder(folders[1].id.clone(), Some(folders[2].id.clone()), None)
        .unwrap();
    c.set_now_for_tests("2026-10-05T10:02");
    c.move_folder(folders[2].id.clone(), Some(folders[0].id.clone()), None)
        .unwrap();

    // Delivered in different orders.
    sync(&c, &storage);
    sync(&b, &storage);
    sync(&a, &storage);
    sync(&c, &storage);
    sync(&b, &storage);
    let parents = |d: &Device| {
        folders
            .iter()
            .map(|f| d.folder(f.id.clone()).unwrap().parent_id)
            .collect::<Vec<_>>()
    };
    // The move of C is the latest: Z is at the root, Y under Z, X under Y.
    for d in [&a, &b, &c] {
        assert_eq!(
            parents(d),
            [Some(folders[1].id.clone()), Some(folders[2].id.clone()), None]
        );
    }
    assert_eq!(dump(&a), dump(&b));
    assert_eq!(dump(&a), dump(&c));
}

#[test]
fn s10_a_note_added_to_a_folder_deleted_elsewhere_is_shown_at_the_root_everywhere() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b) = (device(), device());
    let folder = new_folder(&a, "Временная", None);
    settle(&a, &b, &storage);

    a.delete_folder(folder.id.clone()).unwrap();
    add_in(&b, "поздняя", &folder);
    settle(&a, &b, &storage);
    for d in [&a, &b] {
        assert_eq!(view(d, Scope::NoFolder), ["поздняя"]);
        assert!(d.folders().unwrap().is_empty());
        assert_eq!(
            d.get_note(d.notes(Scope::All).unwrap()[0].id.clone())
                .unwrap()
                .folder_id,
            None
        );
    }
    assert_eq!(dump(&a), dump(&b));
}

#[test]
fn s10_deleting_a_folder_reaches_the_other_device_and_restoring_a_note_there_restores_its_chain() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b) = (device(), device());
    let x = new_folder(&a, "X", None);
    let y = new_folder(&a, "Y", Some(&x));
    let n1 = add_in(&a, "n1", &x);
    let n2 = add_in(&a, "n2", &y);
    let n3 = add_in(&a, "n3", &y);
    settle(&a, &b, &storage);

    a.delete_folder(x.id.clone()).unwrap();
    settle(&a, &b, &storage);
    assert!(b.folders().unwrap().is_empty());
    assert_eq!(sorted(view(&b, Scope::Trash)), ["n1", "n2", "n3"]);

    b.restore_note(n2.id.clone()).unwrap();
    settle(&b, &a, &storage);
    sync(&b, &storage);
    for d in [&a, &b] {
        let names: Vec<String> = sorted(d.folders().unwrap().into_iter().map(|f| f.name).collect());
        assert_eq!(names, ["X", "Y"], "the chain is back");
        assert_eq!(view(d, Scope::Folder { id: y.id.clone() }), ["n2"]);
        assert_eq!(sorted(view(d, Scope::Trash)), ["n1", "n3"]);
    }
    let _ = (n1, n3);
    assert_eq!(dump(&a), dump(&b));
}

#[test]
fn s11_folders_and_filters_made_at_the_same_time_come_in_the_same_order_on_every_device() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b) = (device(), device());
    settle(&a, &b, &storage);
    // Both append to an empty list: the keys are the same and the identifier decides.
    new_folder(&a, "от A", None);
    new_folder(&b, "от B", None);
    a.create_filter("фильтр A".into(), String::new(), FilterQuery::default())
        .unwrap();
    b.create_filter("фильтр B".into(), String::new(), FilterQuery::default())
        .unwrap();
    settle(&a, &b, &storage);
    let folders = |d: &Device| d.folders().unwrap().into_iter().map(|f| f.name).collect::<Vec<_>>();
    let filters = |d: &Device| d.filters().unwrap().into_iter().map(|f| f.name).collect::<Vec<_>>();
    assert_eq!(folders(&a).len(), 2);
    assert_eq!(folders(&a), folders(&b));
    assert_eq!(filters(&a), filters(&b));

    // A move on one device puts the folder first on the other.
    let last = a.folders().unwrap().pop().unwrap();
    a.move_folder(last.id.clone(), None, None).unwrap();
    let first = a.filters().unwrap().pop().unwrap();
    a.move_filter(first.id, None).unwrap();
    settle(&a, &b, &storage);
    assert_eq!(b.folders().unwrap()[0].id, last.id);
    assert_eq!(folders(&a), folders(&b));
    assert_eq!(filters(&a), filters(&b));
}

#[test]
fn s11_filters_and_folder_settings_sync_like_everything_else() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b) = (device(), device());
    let folder = new_folder(&a, "Дневник", None);
    a.set_folder_daily(folder.id.clone(), true).unwrap();
    a.set_folder_sort(
        folder.id.clone(),
        FolderSort {
            by: SortBy::Title,
            descending: false,
        },
    )
    .unwrap();
    let query = FilterQuery {
        join: FilterJoin::Any,
        conditions: vec![Condition::Pinned, Condition::HasTag { name: "дом".into() }],
    };
    let f = a.create_filter("Неделя".into(), "star".into(), query.clone()).unwrap();
    settle(&a, &b, &storage);

    let got = b.folder(folder.id.clone()).unwrap();
    assert!(got.daily);
    assert_eq!(got.sort.by, SortBy::Title);
    assert_eq!(b.filters().unwrap()[0].query, query);

    // A rename on one device and a new condition on the other are both kept.
    a.update_filter(f.id.clone(), "Ближайшее".into(), "star".into(), query.clone())
        .unwrap();
    b.set_now_for_tests("2026-10-05T10:05");
    let wider = FilterQuery {
        join: FilterJoin::All,
        conditions: vec![Condition::Starred],
    };
    b.update_filter(f.id.clone(), "Неделя".into(), "star".into(), wider.clone())
        .unwrap();
    settle(&a, &b, &storage);
    for d in [&a, &b] {
        let got = &d.filters().unwrap()[0];
        assert_eq!((got.name.as_str(), &got.query), ("Ближайшее", &wider));
    }
    b.delete_filter(f.id).unwrap();
    settle(&b, &a, &storage);
    assert!(a.filters().unwrap().is_empty());
}

// ---- the shared settings (S12) ----

#[test]
fn s12_the_shared_settings_merge_field_by_field_and_reach_a_new_device() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b) = (device(), device());
    settle(&a, &b, &storage);

    a.set_settings(AppSettings {
        trash_days: 15,
        font_body: "Literata".into(),
        ..AppSettings::default()
    })
    .unwrap();
    b.set_now_for_tests("2026-10-05T10:01");
    b.set_settings(AppSettings {
        trash_days: 45,
        align: TextAlign::Justify,
        ..AppSettings::default()
    })
    .unwrap();
    settle(&a, &b, &storage);
    let expected = AppSettings {
        trash_days: 45,
        font_body: "Literata".into(),
        align: TextAlign::Justify,
        ..AppSettings::default()
    };
    assert_eq!(
        (a.settings().unwrap(), b.settings().unwrap()),
        (expected.clone(), expected.clone())
    );

    let c = device();
    sync(&c, &storage);
    assert_eq!(c.settings().unwrap(), expected);

    // A device that saves what it shows does not take the other fields from its defaults.
    c.set_now_for_tests("2026-10-05T10:02");
    c.set_settings(AppSettings {
        spellcheck: false,
        ..c.settings().unwrap()
    })
    .unwrap();
    settle(&c, &a, &storage);
    sync(&b, &storage);
    for d in [&a, &b, &c] {
        assert_eq!(
            d.settings().unwrap(),
            AppSettings {
                spellcheck: false,
                ..expected.clone()
            }
        );
    }
}

#[test]
fn s12_a_field_a_version_does_not_know_is_kept_and_passed_on_and_a_value_it_cannot_read_gives_the_default() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b, c) = (device(), device(), device());
    a.set_compact_after_for_tests(0);
    b.set_setting_for_tests("future_option", serde_json::json!({"depth": 3}));
    b.set_setting_for_tests("trash_days", serde_json::json!("soon"));
    sync(&b, &storage);
    // A writes something of its own, which makes it publish a snapshot of everything it holds.
    add(&a, "заметка");
    sync(&a, &storage);
    assert_eq!(a.settings().unwrap().trash_days, 30, "unreadable: the default");
    assert_eq!(
        a.setting_for_tests("future_option"),
        Some(serde_json::json!({"depth": 3}))
    );

    // Only the snapshot of A is left to read: the field comes from it.
    for dir in ["log", "snap"] {
        for name in files(&storage, dir) {
            if name.starts_with(&b.device_id()) {
                std::fs::remove_file(storage.path().join(ROOT).join(dir).join(name)).unwrap();
            }
        }
    }
    sync(&c, &storage);
    assert_eq!(
        c.setting_for_tests("future_option"),
        Some(serde_json::json!({"depth": 3}))
    );
    assert_eq!(c.setting_for_tests("trash_days"), Some(serde_json::json!("soon")));
    assert_eq!(c.settings().unwrap(), AppSettings::default());
}

#[test]
fn s5_a_tag_at_the_limit_syncs_and_a_longer_one_from_before_is_not_shown() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b) = (device(), device());
    let n = add(&a, "заметка");
    // Both scripts at the byte limit: 196 Latin letters and 98 Cyrillic ones.
    let latin = "a".repeat(196);
    let cyrillic = "я".repeat(98);
    a.add_tag(n.id.clone(), latin.clone()).unwrap();
    a.add_tag(n.id.clone(), cyrillic.clone()).unwrap();
    let long = "b".repeat(197);

    // A key of 201 bytes is stored in the database all the same, as an earlier version could leave it.
    let a = plant(a, &[("note", &n.id, &format!("tag:{long}"), serde_json::json!(true))]);
    let shown = vec![latin, cyrillic];
    assert_eq!(
        a.get_note(n.id.clone()).unwrap().tags,
        shown,
        "a tag no register can carry is not shown"
    );

    sync(&a, &storage);
    sync(&b, &storage);
    assert_eq!(
        b.get_note(n.id.clone()).unwrap().tags,
        shown,
        "and no other device takes it"
    );
    assert_eq!(a.get_note(n.id).unwrap().tags, shown);
}

#[test]
fn s5_a_change_with_a_key_over_the_limit_is_not_applied() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b) = (device(), device());
    let n = add(&a, "заметка");
    settle(&a, &b, &storage);
    let stamp = "7fffffff0000-0000-cccccccccccc";
    let lines = [
        serde_json::json!({"v": 1, "device": "cccccccccccc", "seq": 1, "count": 3}).to_string(),
        serde_json::json!({"k": "note", "i": n.id, "f": "f".repeat(201), "v": 1, "t": stamp}).to_string(),
        serde_json::json!({"k": "note", "i": "", "f": "title", "v": "пустой", "t": stamp}).to_string(),
        serde_json::json!({"k": "note", "i": n.id, "f": "title", "v": "принято", "t": stamp}).to_string(),
    ];
    std::fs::write(
        storage.path().join(ROOT).join("log/cccccccccccc-0000000001.jsonl"),
        lines.join("\n") + "\n",
    )
    .unwrap();
    sync(&b, &storage);
    assert_eq!(
        b.get_note(n.id).unwrap().title,
        "принято",
        "the well-formed change of the file applies"
    );
    assert_eq!(view(&b, Scope::All), ["принято"], "and the other two made nothing");
}

// ---- compaction: the weight of the log (S15) ----

#[test]
fn s15_a_snapshot_follows_the_weight_of_the_log() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b) = (device(), device());
    let me = a.device_id();
    let log = || {
        own_files(&storage, "log", &me, ".jsonl")
            .iter()
            .map(|(_, n)| n)
            .sum::<u64>()
    };
    let snaps = || own_files(&storage, "snap", &me, ".jsonl");
    let note = add(&a, "заметка");
    sync(&a, &storage);

    // Before its first snapshot the device takes one to weigh 16 KB.
    let big = 8000; // bytes of a title in one edit
    let mut runs = 0;
    while snaps().is_empty() {
        let before = log();
        assert!(before <= 16 * 1024, "the log grew to {before} bytes without a snapshot");
        runs += 1;
        a.set_title(note.id.clone(), noise(big, runs)).unwrap();
        sync(&a, &storage);
        if !snaps().is_empty() {
            assert!(
                before + big as u64 > 16 * 1024,
                "a snapshot came at {before} bytes of log"
            );
        }
    }
    assert!(runs > 1);
    let [(first, weight)] = &snaps()[..] else {
        panic!("{:?}", snaps())
    };

    // Small edits do not make a new one while the log since the snapshot is lighter than it.
    for i in 0..5 {
        a.set_pinned(note.id.clone(), i % 2 == 0).unwrap();
        sync(&a, &storage);
    }
    assert!(log() < *weight, "{} bytes of log against a snapshot of {weight}", log());
    assert_eq!(own_files(&storage, "log", &me, ".jsonl").len(), 5);
    assert_eq!(&snaps()[0].0, first);

    // One upload heavier than the snapshot makes one in the same run.
    a.set_title(note.id.clone(), "ж".repeat(*weight as usize)).unwrap();
    sync(&a, &storage);
    assert_ne!(&snaps()[0].0, first);
    assert!(own_files(&storage, "log", &me, ".jsonl").is_empty());

    sync(&b, &storage);
    let got = b.get_note(note.id).unwrap();
    assert_eq!((got.pinned, got.title.len()), (true, "ж".len() * *weight as usize));
}

// ---- undo reaches the other device (R75) ----

#[test]
fn r75_undo_leaves_a_note_changed_on_another_device_and_reaches_it() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b) = (device(), device());
    let notes: Vec<Note> = ["а", "б", "в"].iter().map(|t| add(&a, t)).collect();
    a.begin_group();
    for n in &notes {
        a.set_title(n.id.clone(), format!("{} от A", n.title)).unwrap();
    }
    a.end_group();
    settle(&a, &b, &storage);
    assert!(!b.can_undo(), "the history stays on its device");
    b.set_title(notes[1].id.clone(), "от B".into()).unwrap();
    settle(&a, &b, &storage);

    assert_eq!(a.undo().unwrap(), 2);
    settle(&a, &b, &storage);
    for d in [&a, &b] {
        let title = |i: usize| d.get_note(notes[i].id.clone()).unwrap().title;
        assert_eq!(
            (title(0), title(1), title(2)),
            ("а".to_string(), "от B".to_string(), "в".to_string())
        );
    }
}

// ---- which side to keep (S33-S37) ----

#[test]
fn s33_s34_there_is_a_side_to_choose_only_when_both_hold_data() {
    let storage = tempfile::tempdir().unwrap();
    let empty = tempfile::tempdir().unwrap();
    let (a, c, fresh) = (device(), device(), device());
    add(&a, "с A");
    sync(&a, &storage);
    let n = add(&c, "с C");

    let before = (files(&storage, "log"), dump(&c));
    assert!(c.sync_conflict(folder(&storage), String::new()).unwrap());
    assert!(!c.sync_conflict(folder(&empty), String::new()).unwrap());
    assert!(!fresh.sync_conflict(folder(&storage), String::new()).unwrap());
    assert!(!c.sync_conflict(SyncConfig::Off, String::new()).unwrap());
    assert_eq!((files(&storage, "log"), dump(&c)), before, "the check writes nothing");
    assert!(files(&empty, "log").is_empty());

    // A folder of its own is data as well; a note in the Trash is not.
    c.delete_note(n.id).unwrap();
    assert!(!c.sync_conflict(folder(&storage), String::new()).unwrap());
    new_folder(&c, "Работа", None);
    assert!(c.sync_conflict(folder(&storage), String::new()).unwrap());
}

#[test]
fn s36_taking_the_storage_drops_what_the_device_held() {
    let storage = tempfile::tempdir().unwrap();
    let (a, c) = (device(), device());
    let shared = add(&a, "общая");
    sync(&a, &storage);
    let held = add(&c, "в");
    new_folder(&c, "своя папка", None);
    c.create_filter("свой фильтр".into(), String::new(), FilterQuery::default())
        .unwrap();
    c.set_settings(AppSettings {
        trash_days: 9,
        ..AppSettings::default()
    })
    .unwrap();
    let was = c.device_id();

    c.replace_local_with_remote().unwrap();
    assert!(view(&c, Scope::All).is_empty(), "empty until the first run");
    assert!(c.folders().unwrap().is_empty() && c.filters().unwrap().is_empty());
    assert_eq!(c.settings().unwrap(), AppSettings::default());
    assert_ne!(c.device_id(), was);
    settle(&c, &a, &storage);
    for d in [&a, &c] {
        assert_eq!(view(d, Scope::All), ["общая"]);
        assert!(view(d, Scope::Trash).is_empty());
        assert!(d.get_note(held.id.clone()).is_err());
    }

    // An edit that was not uploaded is lost, and what the device uploaded before comes back.
    add(&c, "выгруженная");
    settle(&c, &a, &storage);
    c.set_title(shared.id.clone(), "изменено без связи".into()).unwrap();
    c.replace_local_with_remote().unwrap();
    settle(&c, &a, &storage);
    for d in [&a, &c] {
        assert_eq!(sorted(oldest_first(d)), ["выгруженная", "общая"]);
    }

    // The device goes on as a new one: its next change reaches the others.
    add(&c, "после");
    settle(&c, &a, &storage);
    assert_eq!(sorted(oldest_first(&a)), ["выгруженная", "общая", "после"]);
    assert_eq!(dump(&a), dump(&c));
}

#[test]
fn s37_sending_the_device_puts_the_rest_into_the_trash_everywhere() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b, c) = (device(), device(), device());
    let a1 = add(&a, "а1");
    attach(&a, &a1, "a.txt", b"content");
    let work = new_folder(&a, "Работа", None);
    let sub = new_folder(&a, "Вложенная", Some(&work));
    add_in(&a, "а2", &sub);
    a.create_filter("фильтр".into(), String::new(), FilterQuery::default())
        .unwrap();
    settle(&a, &b, &storage);
    add(&c, "в");

    c.replace_remote_with_local().unwrap();
    sync(&c, &storage);
    sync(&a, &storage);
    sync(&b, &storage);
    for d in [&a, &b, &c] {
        assert_eq!(view(d, Scope::All), ["в"]);
        assert_eq!(sorted(view(d, Scope::Trash)), ["а1", "а2"]);
        assert!(d.folders().unwrap().is_empty(), "the folders are gone");
        assert!(d.filters().unwrap().is_empty(), "and the filter");
        assert!(d.attachments(a1.id.clone()).unwrap().is_empty(), "and the attachment");
    }
    assert_eq!(dump(&a), dump(&c));
    assert_eq!(dump(&b), dump(&c));
    // Nothing waits any more: the next run is an ordinary one.
    assert_eq!(sync(&c, &storage), SyncReport::default());

    // What went to the Trash can be taken back.
    a.restore_note(a1.id).unwrap();
    settle(&a, &c, &storage);
    sync(&b, &storage);
    for d in [&a, &b, &c] {
        assert_eq!(sorted(view(d, Scope::All)), ["а1", "в"]);
    }
}

#[test]
fn s37_sending_the_device_undoes_what_it_has_not_seen() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b) = (device(), device());
    let n = add(&a, "один");
    let cleared = add(&a, "очищенная");
    settle(&a, &b, &storage);

    // A goes on: a rename, a tag, a note deleted for good, a new note.
    a.set_title(n.id.clone(), "два".into()).unwrap();
    a.add_tag(n.id.clone(), "дом".into()).unwrap();
    a.delete_note(cleared.id.clone()).unwrap();
    a.empty_trash().unwrap();
    add(&a, "новая на A");
    sync(&a, &storage);

    // B holds the earlier state, like a device restored from a backup.
    b.replace_remote_with_local().unwrap();
    sync(&b, &storage);
    sync(&a, &storage);
    for d in [&a, &b] {
        let note = d.get_note(n.id.clone()).unwrap();
        assert_eq!(note.title, "один");
        assert!(note.tags.is_empty());
        assert_eq!(sorted(view(d, Scope::All)), ["один", "очищенная"]);
        assert_eq!(view(d, Scope::Trash), ["новая на A"]);
    }
    assert_eq!(dump(&a), dump(&b));
}

#[test]
fn s37_an_edit_made_before_and_uploaded_after_loses() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b) = (device(), device());
    let n = add(&a, "один");
    settle(&a, &b, &storage);

    a.set_title(n.id.clone(), "правка без связи".into()).unwrap();
    add(&a, "создана без связи");
    b.replace_remote_with_local().unwrap();
    sync(&b, &storage);
    settle(&a, &b, &storage);
    for d in [&a, &b] {
        assert_eq!(d.get_note(n.id.clone()).unwrap().title, "один");
        // A note the device could not see is not its to delete.
        assert_eq!(sorted(view(d, Scope::All)), ["один", "создана без связи"]);
    }
}

#[test]
fn s37_a_failed_run_leaves_the_replacement_waiting() {
    let storage = tempfile::tempdir().unwrap();
    let (a, c) = (device(), device());
    add(&a, "с A");
    sync(&a, &storage);
    add(&c, "в");

    let remote = Flaky::new(&storage);
    c.replace_remote_with_local().unwrap();
    assert!(c.sync_with(&remote).is_err());
    assert_eq!(view(&c, Scope::All), ["в"]);
    // What is edited while the storage is away belongs to the device's data.
    add(&c, "г");
    assert!(c.sync_with(&remote).is_err());

    remote.down.store(false, std::sync::atomic::Ordering::Relaxed);
    c.sync_with(&remote).unwrap();
    sync(&a, &storage);
    for d in [&a, &c] {
        assert_eq!(sorted(view(d, Scope::All)), ["в", "г"]);
        assert_eq!(view(d, Scope::Trash), ["с A"]);
    }
}

#[test]
fn s35_another_storage_cancels_a_waiting_replacement() {
    let storage = tempfile::tempdir().unwrap();
    let (a, c) = (device(), device());
    add(&a, "с A");
    sync(&a, &storage);
    add(&c, "в");

    c.replace_remote_with_local().unwrap();
    c.set_sync_config(folder(&storage)).unwrap();
    c.sync_now().unwrap();
    assert_eq!(sorted(view(&c, Scope::All)), ["в", "с A"]);
}

#[test]
fn s35_joining_a_storage_and_choosing_a_side_is_one_step() {
    let storage = tempfile::tempdir().unwrap();
    let (a, c, d) = (device(), device(), device());
    add(&a, "с A");
    a.set_sync_config(folder(&storage)).unwrap();
    a.sync_now().unwrap();

    add(&c, "в");
    c.join_storage(folder(&storage), SyncSide::Storage).unwrap();
    c.sync_now().unwrap();
    assert_eq!(view(&c, Scope::All), ["с A"]);

    add(&d, "г");
    d.join_storage(folder(&storage), SyncSide::Device).unwrap();
    d.sync_now().unwrap();
    a.sync_now().unwrap();
    c.sync_now().unwrap();
    for device in [&a, &c, &d] {
        assert_eq!(view(device, Scope::All), ["г"]);
        assert_eq!(view(device, Scope::Trash), ["с A"]);
    }

    // The same storage again: the side still applies.
    add(&c, "без связи");
    c.join_storage(folder(&storage), SyncSide::Storage).unwrap();
    c.sync_now().unwrap();
    assert_eq!(view(&c, Scope::All), ["г"]);
}

// ---- attachments: what is needed, what is removed (S26, S27) ----

#[test]
fn s7_s26_a_note_in_the_trash_keeps_its_content_and_a_purged_one_loses_it_everywhere() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b) = (device(), device());
    let trashed = add(&a, "в корзину");
    let first = attach(&a, &trashed, "1.txt", b"one");
    let second = attach(&a, &trashed, "2.txt", b"two");
    let kept = add(&a, "живая");
    let removed = attach(&a, &kept, "3.txt", b"three");
    let (v, w) = (add(&a, "v"), add(&a, "w"));
    let shared = attach(&a, &v, "4.txt", b"four");
    attach(&a, &w, "4.txt", b"four");
    sync_all(&a, &storage);
    sync_all(&b, &storage);
    let all = [&first.sha256, &second.sha256, &removed.sha256, &shared.sha256];
    assert!(all.iter().all(|h| on_device(&b, h) && in_storage(&storage, h)));

    // A note in the Trash keeps its content everywhere.
    a.delete_note(trashed.id.clone()).unwrap();
    sync_all(&a, &storage);
    sync_all(&b, &storage);
    assert!(all
        .iter()
        .all(|h| on_device(&a, h) && on_device(&b, h) && in_storage(&storage, h)));

    // Deleted for good, or taken off a note: gone; content another attachment refers to stays.
    a.purge_note(trashed.id.clone()).unwrap();
    a.remove_attachment(removed.id.clone()).unwrap();
    a.purge_note(v.id.clone()).unwrap();
    sync_all(&a, &storage);
    sync_all(&b, &storage);
    assert!(b.get_note(trashed.id).is_err(), "the note is gone from B");
    for gone in [&first.sha256, &second.sha256, &removed.sha256] {
        assert!(!on_device(&a, gone) && !on_device(&b, gone) && !in_storage(&storage, gone));
    }
    assert!(on_device(&a, &shared.sha256) && on_device(&b, &shared.sha256) && in_storage(&storage, &shared.sha256));

    // The last note that refers to it goes: so does the content.
    a.purge_note(w.id).unwrap();
    sync_all(&a, &storage);
    sync_all(&b, &storage);
    assert!(!on_device(&a, &shared.sha256) && !on_device(&b, &shared.sha256) && !in_storage(&storage, &shared.sha256));
}

#[test]
fn s26_a_run_that_did_not_read_everything_removes_nothing() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b) = (device(), device());
    let n = add(&a, "с файлом");
    let att = attach(&a, &n, "1.txt", b"one");
    sync_all(&a, &storage);
    sync_all(&b, &storage);

    // A file of A's log that B cannot read yet.
    add(&a, "ещё");
    sync(&a, &storage);
    let name = files(&storage, "log")
        .into_iter()
        .rfind(|n| n.starts_with(&a.device_id()))
        .unwrap();
    let path = storage.path().join(ROOT).join("log").join(&name);
    let whole = std::fs::read(&path).unwrap();
    std::fs::write(&path, &whole[..whole.len() / 2]).unwrap();

    b.remove_attachment(att.id.clone()).unwrap();
    sync_all(&b, &storage);
    assert!(on_device(&b, &att.sha256) && in_storage(&storage, &att.sha256));

    std::fs::write(&path, &whole).unwrap();
    sync_all(&b, &storage);
    assert!(!on_device(&b, &att.sha256) && !in_storage(&storage, &att.sha256));
}

#[test]
fn s26_a_device_without_sync_removes_unneeded_content_when_it_starts() {
    let a = device();
    let n = add(&a, "с файлом");
    let att = attach(&a, &n, "1.txt", b"one");
    a.remove_attachment(att.id).unwrap();
    assert!(on_device(&a, &att.sha256));
    Store::open(a.dir().to_string_lossy().into_owned()).unwrap();
    assert!(!on_device(&a, &att.sha256));
}

#[test]
fn s27_content_attached_again_without_a_connection_comes_back_to_the_storage() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b, c) = (device(), device(), device());
    let n = add(&a, "у A");
    let att = attach(&a, &n, "1.txt", b"one");
    sync_all(&a, &storage);
    sync_all(&b, &storage); // B downloads the content and counts it as in the storage.

    // Without a connection B attaches the same file to a note of its own.
    let u = add(&b, "у B");
    attach(&b, &u, "1.txt", b"one");

    // A deletes its note for good and removes the content from the storage.
    a.delete_note(n.id.clone()).unwrap();
    a.empty_trash().unwrap();
    sync_all(&a, &storage);
    assert!(!in_storage(&storage, &att.sha256));

    // B uploads its note but still counts the content as in the storage.
    sync_all(&b, &storage);
    assert!(!in_storage(&storage, &att.sha256));
    sync(&c, &storage);
    assert_eq!(sync_files(&c, &storage).waiting, 1);

    // A sends the record of the removal and does not remove it again now that B needs it.
    let counting = Counting {
        inner: DirRemote::new(storage.path()),
        deleted: Mutex::new(vec![]),
    };
    a.sync_with(&counting).unwrap();
    a.sync_attachments_with(&counting).unwrap();
    assert!(counting.deleted.lock().unwrap().is_empty());

    // B reads the record and uploads the content again; C gets it.
    sync(&b, &storage);
    assert_eq!(sync_files(&b, &storage).uploaded, 1);
    assert!(in_storage(&storage, &att.sha256));
    sync(&c, &storage);
    assert_eq!(sync_files(&c, &storage).downloaded, 1);
}

#[test]
fn s27_a_device_that_reads_the_removal_does_not_remove_it_again() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b) = (device(), device());
    let n = add(&a, "с файлом");
    let att = attach(&a, &n, "1.txt", b"one");
    sync_all(&a, &storage);
    sync_all(&b, &storage);

    a.remove_attachment(att.id.clone()).unwrap();
    sync_all(&a, &storage);
    sync(&a, &storage); // the record of the removal goes out

    let counting = Counting {
        inner: DirRemote::new(storage.path()),
        deleted: Mutex::new(vec![]),
    };
    b.sync_with(&counting).unwrap();
    b.sync_attachments_with(&counting).unwrap();
    assert!(counting.deleted.lock().unwrap().is_empty());
    assert!(!on_device(&b, &att.sha256));
}

// ---- convergence ----

/// Three devices make random edits and sync in random order; once everyone has
/// synced twice with no edits in between, all three must show the same data.
#[test]
fn devices_converge_whatever_the_order_of_edits_and_syncs() {
    for seed in 1..=8u64 {
        let storage = tempfile::tempdir().unwrap();
        let devices = [device(), device(), device()];
        devices[1].set_compact_after_for_tests(4);
        let mut state = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15);
        let mut rnd = |n: usize| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state % n as u64) as usize
        };
        let mut notes: Vec<String> = Vec::new();
        let mut folders: Vec<String> = Vec::new();
        let mut minute = 0;
        for step in 0..200 {
            let d = &devices[rnd(3)];
            minute += 1;
            d.set_now_for_tests(&format!("2026-10-05T{:02}:{:02}", 10 + minute / 60, minute % 60));
            let note = if notes.is_empty() {
                None
            } else {
                Some(notes[rnd(notes.len())].clone())
            };
            let folder = if folders.is_empty() {
                None
            } else {
                Some(folders[rnd(folders.len())].clone())
            };
            let knows_note = |id: &String| d.get_note(id.clone()).is_ok();
            let knows_folder = |id: &String| d.folder(id.clone()).is_ok();
            match (rnd(17), note, folder) {
                (0..=2, _, _) | (_, None, _) => {
                    let parent = folder_of(&folders, &mut rnd).filter(|f| knows_folder(f));
                    let created = d
                        .create_note(NewNote {
                            title: format!("note {step}"),
                            folder_id: parent,
                            ..NewNote::default()
                        })
                        .unwrap();
                    notes.push(created.id);
                }
                (3, Some(id), _) if knows_note(&id) => d.set_title(id, format!("renamed {step}")).unwrap(),
                (4, Some(id), _) if knows_note(&id) => d.set_pinned(id, rnd(2) == 0).unwrap(),
                (5, Some(id), _) if knows_note(&id) => d.set_starred(id, rnd(2) == 0).unwrap(),
                (6, Some(id), _) if knows_note(&id) => d.add_tag(id, format!("T{}", rnd(3))).unwrap(),
                (7, Some(id), _) if knows_note(&id) => d.remove_tag(id, format!("t{}", rnd(3))).unwrap(),
                (8, Some(id), _) if knows_note(&id) => d.delete_note(id).unwrap(),
                (9, Some(id), _) if knows_note(&id) => d.restore_note(id).unwrap(),
                (10, Some(id), f) if knows_note(&id) => d.move_note(id, f.filter(|f| knows_folder(f))).unwrap(),
                (11, _, _) => {
                    let parent = folder_of(&folders, &mut rnd).filter(|f| knows_folder(f));
                    folders.push(d.create_folder(format!("folder {step}"), parent).unwrap().id);
                }
                (12, _, Some(id)) if knows_folder(&id) => {
                    // Rejected when it would create a cycle locally; that is fine.
                    let parent = folder_of(&folders, &mut rnd).filter(|f| knows_folder(f));
                    d.move_folder(id, parent, None).ok();
                }
                (13, _, Some(id)) if knows_folder(&id) => drop(d.delete_folder(id).unwrap()),
                (14, Some(id), _) if knows_note(&id) => d.set_archived(id, rnd(2) == 0).unwrap(),
                (15, Some(id), _) if knows_note(&id) && rnd(4) == 0 => d.purge_note(id).unwrap(),
                _ => drop(sync(d, &storage)),
            }
        }
        for _ in 0..2 {
            for d in &devices {
                sync(d, &storage);
            }
        }
        for d in &devices {
            d.set_now_for_tests("2026-10-06T10:00");
        }
        let reference = dump(&devices[0]);
        assert!(reference.len() > 8, "seed {seed}: the run produced data");
        for (i, d) in devices.iter().enumerate().skip(1) {
            assert_eq!(dump(d), reference, "seed {seed}: device {i} differs");
        }
        for d in &devices {
            assert_eq!(sync(d, &storage).pulled, 0, "seed {seed}: nothing left to pull");
        }
    }
}

fn folder_of(folders: &[String], rnd: &mut impl FnMut(usize) -> usize) -> Option<String> {
    if folders.is_empty() || rnd(3) == 0 {
        None
    } else {
        Some(folders[rnd(folders.len())].clone())
    }
}
