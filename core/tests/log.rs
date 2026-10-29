mod common;

use common::*;
use notes_core::{LogLevel, NewTask, Store, SyncConfig};

fn log_of(device: &Device) -> String {
    std::fs::read_to_string(device.dir().join("log.txt")).unwrap_or_default()
}

fn folder(dir: &tempfile::TempDir) -> SyncConfig {
    SyncConfig::Folder {
        path: dir.path().to_string_lossy().into_owned(),
    }
}

/// A WebDAV server that is not there: nothing listens on port 1.
fn unreachable() -> SyncConfig {
    SyncConfig::WebDav {
        url: "http://127.0.0.1:1/dav".into(),
        user: "me".into(),
    }
}

#[test]
fn r102_a_new_device_logs_errors_and_has_no_file() {
    let a = device();
    assert_eq!(a.log_level(), LogLevel::Error);
    assert!(a.log_files().is_empty());
    assert!(!a.dir().join("log.txt").exists());
}

#[test]
fn r102_a_failed_sync_is_an_error_line() {
    let a = device();
    a.set_sync_config(unreachable()).unwrap();
    a.set_sync_password(Some("pw-123".into()));
    assert!(a.sync_now().is_err());
    let log = log_of(&a);
    let line = log.lines().next().expect("a line in the log");
    assert!(line.contains(" ERROR sync: "), "{line}");
    assert_eq!(log.lines().count(), 1);
}

#[test]
fn r102_a_failure_that_repeats_is_an_error_once() {
    let a = device();
    a.set_sync_config(unreachable()).unwrap();
    assert!(a.sync_now().is_err());
    assert!(a.sync_now().is_err());
    assert_eq!(log_of(&a).lines().count(), 1, "{}", log_of(&a));

    a.set_log_level(LogLevel::Debug).unwrap();
    assert!(a.sync_now().is_err());
    assert!(log_of(&a).contains(" DEBUG sync: failed again: "), "{}", log_of(&a));
}

#[test]
fn r102_a_failure_after_a_run_that_went_through_is_an_error_again() {
    let storage = tempfile::tempdir().unwrap();
    let a = device();
    a.set_sync_config(unreachable()).unwrap();
    assert!(a.sync_now().is_err());
    a.set_sync_config(folder(&storage)).unwrap();
    a.sync_now().unwrap();
    a.set_sync_config(unreachable()).unwrap();
    assert!(a.sync_now().is_err());
    assert_eq!(log_of(&a).matches(" ERROR sync: ").count(), 2, "{}", log_of(&a));
}

#[test]
fn r102_a_sync_that_went_through_is_an_event_and_not_an_error() {
    let storage = tempfile::tempdir().unwrap();
    let a = device();
    a.set_sync_config(folder(&storage)).unwrap();
    add(&a, "one");
    a.sync_now().unwrap();
    assert_eq!(log_of(&a), "", "the default level takes errors only");

    a.set_log_level(LogLevel::Info).unwrap();
    add(&a, "two");
    a.sync_now().unwrap();
    let log = log_of(&a);
    assert!(log.contains(" INFO log: level set to info"), "{log}");
    assert!(log.contains(" INFO sync: run finished: 0 received, "), "{log}");
}

#[test]
fn r102_a_run_that_changed_nothing_is_said_at_the_detailed_level_only() {
    let storage = tempfile::tempdir().unwrap();
    let a = device();
    a.set_sync_config(folder(&storage)).unwrap();
    a.set_log_level(LogLevel::Info).unwrap();
    a.sync_now().unwrap();
    assert!(!log_of(&a).contains("sync:"), "{}", log_of(&a));

    a.set_log_level(LogLevel::Debug).unwrap();
    a.sync_now().unwrap();
    assert!(
        log_of(&a).contains(" DEBUG sync: run finished: 0 received, 0 sent"),
        "{}",
        log_of(&a)
    );
}

#[test]
fn r102_nothing_is_written_when_the_log_is_off() {
    let a = device();
    a.set_log_level(LogLevel::Off).unwrap();
    a.set_sync_config(unreachable()).unwrap();
    assert!(a.sync_now().is_err());
    a.log(LogLevel::Error, "app".into(), "a failure".into());
    assert!(a.log_files().is_empty());
}

#[test]
fn r102_the_level_is_kept_on_the_device() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().to_string_lossy().into_owned();
    Store::open(path.clone())
        .unwrap()
        .set_log_level(LogLevel::Debug)
        .unwrap();
    assert_eq!(Store::open(path).unwrap().log_level(), LogLevel::Debug);
}

#[test]
fn r102_the_level_is_not_synced() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b) = (device(), device());
    a.set_sync_config(folder(&storage)).unwrap();
    b.set_sync_config(folder(&storage)).unwrap();
    a.set_log_level(LogLevel::Debug).unwrap();
    add(&a, "one");
    a.sync_now().unwrap();
    b.sync_now().unwrap();
    assert_eq!(b.log_level(), LogLevel::Error);
}

#[test]
fn r102_a_line_from_the_app_follows_the_level_and_stays_one_line() {
    let a = device();
    a.log(LogLevel::Info, "app".into(), "started".into());
    assert_eq!(log_of(&a), "");
    a.log(LogLevel::Error, "app".into(), "first\nsecond".into());
    let log = log_of(&a);
    assert_eq!(log.lines().count(), 1);
    assert!(log.trim_end().ends_with(" ERROR app: first second"), "{log}");
    // The moment comes first: date, T, time to the second.
    let moment = log.split(' ').next().unwrap();
    assert_eq!(moment.len(), "2026-10-05T10:00:00".len());
    assert_eq!(&moment[10..11], "T");
}

#[test]
fn r102_a_file_over_the_limit_becomes_the_previous_one() {
    let a = device();
    let big = "x".repeat(200 * 1024);
    for _ in 0..6 {
        a.log(LogLevel::Error, "app".into(), big.clone());
    }
    let previous = a.dir().join("log.1.txt");
    assert!(!previous.exists(), "six lines of 200 KB cross 1 MB only with the sixth");
    a.log(LogLevel::Error, "app".into(), "after the limit".into());
    assert!(std::fs::metadata(&previous).unwrap().len() > (1 << 20));
    assert_eq!(log_of(&a).lines().count(), 1);
    assert!(log_of(&a).contains("after the limit"));
    let files = a.log_files();
    assert_eq!(files.len(), 2);
    assert!(
        files[0].ends_with("log.1.txt") && files[1].ends_with("log.txt"),
        "{files:?}"
    );
}

#[test]
fn r102_clearing_removes_both_files() {
    let a = device();
    let big = "x".repeat(600 * 1024);
    for _ in 0..3 {
        a.log(LogLevel::Error, "app".into(), big.clone());
    }
    assert_eq!(a.log_files().len(), 2);
    a.clear_log().unwrap();
    assert!(a.log_files().is_empty());
    a.clear_log().unwrap();
}

#[test]
fn r102_the_log_holds_neither_the_tasks_nor_the_password() {
    let storage = tempfile::tempdir().unwrap();
    let a = device();
    a.set_log_level(LogLevel::Debug).unwrap();
    a.create_task(NewTask {
        title: "Секрет".into(),
        notes: "тайная заметка".into(),
        ..NewTask::default()
    })
    .unwrap();
    a.set_sync_config(folder(&storage)).unwrap();
    a.sync_now().unwrap();
    a.sync_attachments().unwrap();
    a.create_backup().unwrap();
    a.set_sync_config(unreachable()).unwrap();
    a.set_sync_password(Some("pw-123".into()));
    assert!(a.sync_now().is_err());

    let log = log_of(&a);
    assert!(log.contains(" INFO sync: run finished"), "{log}");
    assert!(log.contains(" INFO backup: created "), "{log}");
    assert!(log.contains(" ERROR sync: "), "{log}");
    for hidden in ["Секрет", "тайная", "pw-123"] {
        assert!(!log.contains(hidden), "{hidden} is in the log: {log}");
    }
}

#[test]
fn r102_a_backup_does_not_carry_the_log() {
    let a = device();
    add(&a, "one");
    a.log(LogLevel::Error, "app".into(), "kept out of the backup".into());
    let backup = a.create_backup().unwrap();
    let archive = std::fs::read(backup.path).unwrap();
    let needle = b"log.txt";
    assert!(!archive.windows(needle.len()).any(|w| w == needle));
}

// --- R102: attachment content and the calls of the apps (#251) ---

use notes_core::sync::remote::{DirRemote, Remote};
use notes_core::{AppError, Attachment, Result, TaskItem};

fn attach(d: &Device, task: &TaskItem, name: &str, content: &[u8]) -> Attachment {
    let src = tempfile::tempdir().unwrap();
    let file = src.path().join(name);
    std::fs::write(&file, content).unwrap();
    d.add_attachment(task.id.clone(), file.to_string_lossy().into_owned(), None)
        .unwrap()
}

/// A storage that answers requests for attachment content the way a WebDAV
/// server does when it refuses them: `put` and `get` hold the HTTP status.
struct Blobs {
    inner: DirRemote,
    put: Option<u16>,
    get: Option<u16>,
}

impl Blobs {
    fn new(storage: &tempfile::TempDir, put: Option<u16>, get: Option<u16>) -> Self {
        Blobs {
            inner: DirRemote::new(storage.path()),
            put,
            get,
        }
    }
}

impl Remote for Blobs {
    fn id(&self) -> String {
        self.inner.id()
    }
    fn list(&self, dir: &str) -> Result<Vec<String>> {
        self.inner.list(dir)
    }
    fn get(&self, path: &str) -> Result<Option<Vec<u8>>> {
        match self.get {
            Some(code) if path.contains("/blobs/") => Err(AppError::sync(format!("GET {path}: HTTP {code}"))),
            _ => self.inner.get(path),
        }
    }
    fn put(&self, path: &str, data: &[u8]) -> Result<()> {
        match self.put {
            Some(code) if path.contains("/blobs/") => Err(AppError::sync(format!("PUT {path}: HTTP {code}"))),
            _ => self.inner.put(path, data),
        }
    }
    fn delete(&self, path: &str) -> Result<()> {
        self.inner.delete(path)
    }
    fn exists(&self, path: &str) -> Result<bool> {
        self.inner.exists(path)
    }
}

fn errors(log: &str) -> usize {
    log.matches(" ERROR ").count()
}

/// The device after the app was closed and opened again.
fn reopened(d: &Device) -> std::sync::Arc<Store> {
    let store = Store::open(d.dir().to_string_lossy().into_owned()).unwrap();
    store.set_now_for_tests(NOW);
    store
}

#[test]
fn r102_content_the_storage_refuses_is_an_error_line_without_the_task_or_the_hash() {
    let storage = tempfile::tempdir().unwrap();
    let a = device();
    let t = add(&a, "Секрет");
    let att = attach(&a, &t, "тайный.pdf", b"content of the file");
    a.sync_with(&DirRemote::new(storage.path())).unwrap();

    let pass = a.sync_attachments_with(&Blobs::new(&storage, Some(409), None)).unwrap();
    assert_eq!((pass.uploaded, pass.waiting), (0, 1));
    let log = log_of(&a);
    assert_eq!(errors(&log), 1, "{log}");
    let shard = format!("blobs/{}/{}…", &att.sha256[..2], &att.sha256[..8]);
    assert!(
        log.contains(&format!(" ERROR attachments: upload of {shard}: ")),
        "{log}"
    );
    assert!(log.contains("HTTP 409"), "{log}");
    for hidden in [att.sha256.as_str(), "Секрет", "тайный"] {
        assert!(!log.contains(hidden), "{hidden} is in the log: {log}");
    }
}

#[test]
fn r102_content_that_keeps_failing_is_an_error_once_an_hour_and_across_restarts() {
    let storage = tempfile::tempdir().unwrap();
    let a = device();
    let t = add(&a, "с файлом");
    attach(&a, &t, "a.txt", b"content");
    a.sync_with(&DirRemote::new(storage.path())).unwrap();
    let refusing = Blobs::new(&storage, Some(409), None);

    a.sync_attachments_with(&refusing).unwrap();
    a.sync_attachments_with(&refusing).unwrap();
    assert_eq!(errors(&log_of(&a)), 1, "{}", log_of(&a));

    a.set_log_level(LogLevel::Debug).unwrap();
    a.sync_attachments_with(&refusing).unwrap();
    assert!(
        log_of(&a).contains(" DEBUG attachments: failed again: upload of "),
        "{}",
        log_of(&a)
    );
    a.set_log_level(LogLevel::Error).unwrap();

    // Android starts a new process for most runs: a restart does not make the same failure new.
    reopened(&a).sync_attachments_with(&refusing).unwrap();
    assert_eq!(errors(&log_of(&a)), 1, "{}", log_of(&a));

    // A failure that goes on is said again after an hour, with the moment it started.
    a.set_now_for_tests("2026-10-05T11:00");
    a.sync_attachments_with(&refusing).unwrap();
    let log = log_of(&a);
    assert_eq!(errors(&log), 2, "{log}");
    assert!(log.contains("failing since 2026-10-05T10:00:00"), "{log}");
}

#[test]
fn r102_content_that_goes_through_after_failing_is_an_event_and_a_new_failure_an_error() {
    let storage = tempfile::tempdir().unwrap();
    let a = device();
    let t = add(&a, "с файлом");
    attach(&a, &t, "a.txt", b"content");
    a.sync_with(&DirRemote::new(storage.path())).unwrap();
    a.sync_attachments_with(&Blobs::new(&storage, Some(409), None)).unwrap();

    a.set_log_level(LogLevel::Info).unwrap();
    assert_eq!(
        a.sync_attachments_with(&DirRemote::new(storage.path()))
            .unwrap()
            .uploaded,
        1
    );
    let log = log_of(&a);
    assert!(log.contains(" INFO attachments: upload of blobs/"), "{log}");
    assert!(
        log.contains("went through after failing since 2026-10-05T10:00:00"),
        "{log}"
    );

    // The record of the failure went with it.
    let b = device();
    b.sync_with(&DirRemote::new(storage.path())).unwrap();
    b.sync_attachments_with(&Blobs::new(&storage, None, Some(500))).unwrap();
    b.sync_attachments_with(&DirRemote::new(storage.path())).unwrap();
    b.sync_attachments_with(&Blobs::new(&storage, None, Some(503))).unwrap();
    assert_eq!(
        errors(&log_of(&b)),
        1,
        "content now on the device fails no more: {}",
        log_of(&b)
    );
}

#[test]
fn r102_content_that_cannot_be_read_is_an_error_line_at_once() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b) = (device(), device());
    let t = add(&a, "с файлом");
    attach(&a, &t, "a.txt", b"content");
    a.sync_with(&DirRemote::new(storage.path())).unwrap();
    a.sync_attachments_with(&DirRemote::new(storage.path())).unwrap();
    b.sync_with(&DirRemote::new(storage.path())).unwrap();

    assert_eq!(
        b.sync_attachments_with(&Blobs::new(&storage, None, Some(500)))
            .unwrap()
            .waiting,
        1
    );
    let log = log_of(&b);
    assert_eq!(errors(&log), 1, "{log}");
    assert!(log.contains(" ERROR attachments: download of blobs/"), "{log}");
    assert!(log.contains("HTTP 500"), "{log}");
}

#[test]
fn r102_content_that_does_not_match_its_hash_is_an_error_line() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b) = (device(), device());
    let t = add(&a, "с файлом");
    let att = attach(&a, &t, "a.txt", b"content");
    a.sync_with(&DirRemote::new(storage.path())).unwrap();
    a.sync_attachments_with(&DirRemote::new(storage.path())).unwrap();
    let blob = storage
        .path()
        .join(format!("lists/blobs/{}/{}", &att.sha256[..2], att.sha256));
    std::fs::write(blob, b"something else").unwrap();
    b.sync_with(&DirRemote::new(storage.path())).unwrap();

    b.sync_attachments_with(&DirRemote::new(storage.path())).unwrap();
    let log = log_of(&b);
    assert!(log.contains(" ERROR attachments: download of blobs/"), "{log}");
    assert!(log.contains("does not match its hash"), "{log}");
}

#[test]
fn r102_content_missing_from_the_storage_is_an_error_only_after_an_hour() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b) = (device(), device());
    let t = add(&a, "с файлом");
    attach(&a, &t, "a.txt", b"content");
    // The fields arrive and the content does not: the other device has not sent it yet (S34).
    a.sync_with(&DirRemote::new(storage.path())).unwrap();
    b.sync_with(&DirRemote::new(storage.path())).unwrap();

    b.set_log_level(LogLevel::Info).unwrap();
    b.sync_attachments_with(&DirRemote::new(storage.path())).unwrap();
    let log = log_of(&b);
    assert_eq!(errors(&log), 0, "{log}");
    assert!(log.contains(" INFO attachments: download of blobs/"), "{log}");
    assert!(log.contains("the storage does not have it yet"), "{log}");

    b.set_now_for_tests("2026-10-05T10:59");
    b.sync_attachments_with(&DirRemote::new(storage.path())).unwrap();
    assert_eq!(errors(&log_of(&b)), 0, "{}", log_of(&b));

    b.set_now_for_tests("2026-10-05T11:00");
    b.sync_attachments_with(&DirRemote::new(storage.path())).unwrap();
    let log = log_of(&b);
    assert_eq!(errors(&log), 1, "{log}");
    assert!(
        log.contains("the storage does not have it, waiting since 2026-10-05T10:00:00"),
        "{log}"
    );
}

#[test]
fn r102_an_attachment_that_cannot_be_downloaded_on_request_is_an_error_line() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b) = (device(), device());
    b.set_sync_config(folder(&storage)).unwrap();
    let t = add(&a, "с файлом");
    let att = attach(&a, &t, "a.txt", b"content");
    a.sync_with(&DirRemote::new(storage.path())).unwrap();
    b.sync_now().unwrap();

    assert!(b.fetch_attachment(att.id).is_err());
    let log = log_of(&b);
    assert!(log.contains(" ERROR attachments: download of blobs/"), "{log}");
    assert!(log.contains("has not reached the storage yet"), "{log}");
}

#[test]
fn r102_calls_of_the_apps_that_fail_are_error_lines() {
    let a = device();
    let t = add(&a, "Секрет");
    assert!(a
        .add_attachment(t.id.clone(), "/nonexistent/тайный.pdf".into(), None)
        .is_err());
    assert!(a.set_push_endpoint(Some("ftp://example.invalid".into())).is_err());
    let log = log_of(&a);
    assert!(log.contains(" ERROR attachments: not added: "), "{log}");
    assert!(log.contains(" ERROR push: address not kept: "), "{log}");
    for hidden in ["Секрет", "тайный"] {
        assert!(!log.contains(hidden), "{hidden} is in the log: {log}");
    }
    assert_eq!(errors(&log_of(&a)), 2);
}
