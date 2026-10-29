//! Merge and sync rules: docs/specs/sync.md. Storage is a temporary folder.

mod common;

use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{mpsc, Mutex};

use common::*;
use notes_core::sync::remote::{DirRemote, Remote};
use notes_core::*;
use tempfile::TempDir;

/// The root of the storage (S49).
const ROOT: &str = "lists";

fn sync(d: &Device, storage: &TempDir) -> SyncReport {
    d.sync_with(&DirRemote::new(storage.path())).unwrap()
}

/// S34: the pass that moves attachment content.
fn sync_files(d: &Device, storage: &TempDir) -> AttachmentReport {
    d.sync_attachments_with(&DirRemote::new(storage.path())).unwrap()
}

fn attach(d: &Device, task: &TaskItem, name: &str, content: &[u8]) -> Attachment {
    let src = tempfile::tempdir().unwrap();
    let file = src.path().join(name);
    std::fs::write(&file, content).unwrap();
    d.add_attachment(task.id.clone(), file.to_string_lossy().into_owned(), None)
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

/// Everything a user can see, in a form two devices can compare.
fn dump(d: &Device) -> Vec<String> {
    fn walk(d: &Device, tasks: Vec<TaskItem>, depth: usize, out: &mut Vec<String>) {
        for t in tasks {
            out.push(format!(
                "{}{} | {} | due={:?} start={:?} prio={:?} done={} tags={:?} repeat={} notes={:?} files={}",
                "  ".repeat(depth),
                t.id,
                t.title,
                t.due,
                t.start,
                t.priority,
                t.done.is_some(),
                t.tags,
                t.repeat.is_some(),
                t.notes,
                t.attachments
            ));
            walk(d, d.subtasks(t.id).unwrap(), depth + 1, out);
        }
    }
    let mut out = Vec::new();
    for list in d.lists().unwrap() {
        out.push(format!(
            "# {} | {} | {} | {:?} | {}",
            list.id, list.name, list.color, list.sort, list.archived
        ));
        walk(d, d.tasks(Scope::List { id: list.id }).unwrap(), 1, &mut out);
    }
    out.push("# trash".into());
    out.extend(
        d.tasks(Scope::Trash)
            .unwrap()
            .into_iter()
            .map(|t| format!("  {} | {}", t.id, t.title)),
    );
    out.push("# completed".into());
    out.extend(
        d.tasks(Scope::Completed)
            .unwrap()
            .into_iter()
            .map(|t| format!("  {} | {} | log={}", t.id, t.title, t.is_log)),
    );
    out
}

#[test]
fn s2_edits_of_different_fields_are_both_kept() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b) = (device(), device());
    let t = add(&a, "исходное");
    settle(&a, &b, &storage);
    assert_eq!(b.task(t.id.clone()).unwrap().title, "исходное");

    a.set_title(t.id.clone(), "новое название".into()).unwrap();
    b.set_due(t.id.clone(), Some("2026-10-09".into())).unwrap();
    settle(&a, &b, &storage);

    for d in [&a, &b] {
        let got = d.task(t.id.clone()).unwrap();
        assert_eq!(got.title, "новое название");
        assert_eq!(got.due.as_deref(), Some("2026-10-09"));
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

    assert_eq!(a.task(t.id.clone()).unwrap().title, "от B, позже");
    assert_eq!(b.task(t.id).unwrap().title, "от B, позже");
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
    assert_eq!(b.task(t.id).unwrap().title, "исправлено на A");
}

#[test]
fn s3_delete_and_concurrent_edit() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b) = (device(), device());
    let t = add(&a, "задача");
    settle(&a, &b, &storage);

    a.delete_task(t.id.clone()).unwrap();
    b.set_notes(t.id.clone(), "заметка от B".into()).unwrap();
    settle(&a, &b, &storage);
    for d in [&a, &b] {
        assert!(view(d, Scope::Inbox).is_empty());
        assert_eq!(view(d, Scope::Trash), ["задача"]);
    }

    a.restore_task(t.id.clone()).unwrap();
    settle(&a, &b, &storage);
    assert_eq!(b.task(t.id).unwrap().notes, "заметка от B");
    assert_eq!(view(&b, Scope::Inbox), ["задача"]);
}

#[test]
fn s4_concurrent_inserts_end_up_in_the_same_order() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b) = (device(), device());
    add(&a, "первая");
    settle(&a, &b, &storage);
    // Both append "at the end": the keys collide and the id breaks the tie.
    add(&a, "от A");
    add(&b, "от B");
    settle(&a, &b, &storage);
    assert_eq!(view(&a, Scope::Inbox).len(), 3);
    assert_eq!(view(&a, Scope::Inbox), view(&b, Scope::Inbox));
}

#[test]
fn s5_concurrent_moves_into_each_other_do_not_leave_a_cycle() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b) = (device(), device());
    let x = add(&a, "X");
    let y = add(&a, "Y");
    settle(&a, &b, &storage);

    // R10: the apps no longer put a task under another one, so the two moves come from devices of an
    // earlier version. The one of the second device is the later.
    let later = 0x7fff_ffff_0000_u64;
    for (device, ms, id, parent) in [
        ("cccccccccccc", later, &x.id, &y.id),
        ("dddddddddddd", later + 1, &y.id, &x.id),
    ] {
        let lines = [
            serde_json::json!({"v": 1, "device": device, "seq": 1, "count": 1}).to_string(),
            serde_json::json!({"k": "task", "i": id, "f": "parent", "v": parent, "t": format!("{ms:012x}-0000-{device}")})
                .to_string(),
        ];
        std::fs::write(
            storage.path().join(format!("{ROOT}/log/{device}-0000000001.jsonl")),
            lines.join("\n") + "\n",
        )
        .unwrap();
    }
    settle(&a, &b, &storage);

    // The later move is the one undone: Y stays on top, X under it.
    for d in [&a, &b] {
        assert_eq!(view(d, Scope::Inbox), ["Y"]);
        assert_eq!(titles(&d.subtasks(y.id.clone()).unwrap()), ["X"]);
        assert!(d.subtasks(x.id.clone()).unwrap().is_empty());
    }
    assert_eq!(dump(&a), dump(&b));
}

#[test]
fn s42_what_an_earlier_version_sends_is_brought_to_the_rule_of_subtasks() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b) = (device(), device());
    let top = add(&a, "а");
    let sub = add_sub(&a, &top, "б");
    settle(&a, &b, &storage);

    // A device of an earlier version: a due date and a tag on the subtask, and a subtask under it.
    let later = 0x7fff_ffff_0000_u64;
    let line = |n: u32, id: &str, field: &str, value: serde_json::Value| {
        serde_json::json!({"k": "task", "i": id, "f": field, "v": value, "t": format!("{later:012x}-{n:04x}-cccccccccccc")})
            .to_string()
    };
    let lines = [
        serde_json::json!({"v": 1, "device": "cccccccccccc", "seq": 1, "count": 6}).to_string(),
        line(0, &sub.id, "due", "2026-10-05".into()),
        line(1, &sub.id, "tag:дом", true.into()),
        line(2, "deep", "title", "в".into()),
        line(3, "deep", "list", INBOX_ID.into()),
        line(4, "deep", "parent", sub.id.clone().into()),
        line(5, "deep", "pos", "m".into()),
    ];
    std::fs::write(
        storage.path().join(ROOT).join("log/cccccccccccc-0000000001.jsonl"),
        lines.join("\n") + "\n",
    )
    .unwrap();

    settle(&a, &b, &storage);
    for d in [&a, &b] {
        assert_eq!(titles(&d.subtasks(top.id.clone()).unwrap()), ["б", "в"]);
        let got = d.task(sub.id.clone()).unwrap();
        assert_eq!((got.due, got.tags.len()), (None, 0));
        assert!(view(d, Scope::Today).is_empty());
    }
    assert_eq!(dump(&a), dump(&b));
}

#[test]
fn s42_a_task_of_a_project_read_before_the_mark_of_its_project_keeps_its_fields() {
    let storage = tempfile::tempdir().unwrap();
    // Three devices wrote in turn: the task, then its mark of a project, then a task of the project
    // with a due date. Their files are read in the order of the names, the task of the project first.
    let file = |device: &str, changes: &[(&str, &str, serde_json::Value)], whole: bool| {
        let mut lines =
            vec![serde_json::json!({"v": 1, "device": device, "seq": 1, "count": changes.len()}).to_string()];
        for (n, (id, field, value)) in changes.iter().enumerate() {
            let stamp = format!("{:012x}-{n:04x}-{device}", 0x0100_0000_0000_u64);
            lines.push(serde_json::json!({"k": "task", "i": id, "f": field, "v": value, "t": stamp}).to_string());
        }
        let text = lines.join("\n") + if whole { "\n" } else { "" };
        std::fs::write(
            storage.path().join(format!("{ROOT}/log/{device}-0000000001.jsonl")),
            text,
        )
        .unwrap();
    };
    let seed = device();
    seed.set_sync_config(folder(&storage)).unwrap();
    sync(&seed, &storage);
    file(
        "aaaaaaaaaaaa",
        &[
            ("t", "title", "т".into()),
            ("t", "list", INBOX_ID.into()),
            ("t", "pos", "m".into()),
            ("t", "parent", "p".into()),
            ("t", "due", "2026-10-05".into()),
            ("t", "notes", "заметка".into()),
        ],
        true,
    );
    file(
        "bbbbbbbbbbbb",
        &[
            ("p", "title", "п".into()),
            ("p", "list", INBOX_ID.into()),
            ("p", "pos", "m".into()),
        ],
        true,
    );
    // The file with the mark is cut short at first: the run cannot tell what it holds.
    file("cccccccccccc", &[("p", "project", true.into())], false);

    let d = device();
    d.set_sync_config(folder(&storage)).unwrap();
    sync(&d, &storage);
    let got = d.task("t".into()).unwrap();
    assert_eq!(
        (got.due.as_deref(), got.notes.as_str()),
        (Some("2026-10-05"), "заметка"),
        "nothing is removed halfway"
    );

    file("cccccccccccc", &[("p", "project", true.into())], true);
    sync(&d, &storage);
    let got = d.task("t".into()).unwrap();
    assert_eq!(
        (got.due.as_deref(), got.notes.as_str()),
        (Some("2026-10-05"), "заметка")
    );
    assert!(d.task("p".into()).unwrap().is_project);

    // A device that reads the three whole files in one run keeps the fields as well.
    let fresh = device();
    fresh.set_sync_config(folder(&storage)).unwrap();
    sync(&fresh, &storage);
    assert_eq!(fresh.task("t".into()).unwrap().due.as_deref(), Some("2026-10-05"));
    assert_eq!(view(&fresh, Scope::Today), ["т"]);
}

#[test]
fn s6_task_added_to_a_list_deleted_elsewhere_lands_in_the_inbox() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b) = (device(), device());
    let list = a.create_list("Временный".into()).unwrap();
    settle(&a, &b, &storage);

    a.delete_list(list.id.clone()).unwrap();
    b.create_task(NewTask {
        title: "поздняя".into(),
        list_id: Some(list.id),
        ..NewTask::default()
    })
    .unwrap();
    settle(&a, &b, &storage);
    for d in [&a, &b] {
        assert_eq!(view(d, Scope::Inbox), ["поздняя"]);
        assert_eq!(d.lists().unwrap().len(), 1);
    }
}

#[test]
fn s7_same_occurrence_completed_on_two_devices_is_recorded_once() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b) = (device(), device());
    let t = add(&a, "зарядка");
    a.set_due(t.id.clone(), Some("2026-10-05".into())).unwrap();
    a.set_repeat(
        t.id.clone(),
        Some(Repeat {
            freq: Freq::Daily,
            interval: 1,
            weekdays: vec![],
            monthday: None,
            nth: None,
            nth_weekday: None,
            from_done: false,
            count: None,
            until: None,
        }),
    )
    .unwrap();
    settle(&a, &b, &storage);

    a.complete_task(t.id.clone()).unwrap();
    b.set_now_for_tests("2026-10-05T21:00");
    b.complete_task(t.id.clone()).unwrap();
    settle(&a, &b, &storage);

    for d in [&a, &b] {
        assert_eq!(view(d, Scope::Completed), ["зарядка"], "one record, not two");
        let got = d.task(t.id.clone()).unwrap();
        assert_eq!(got.due.as_deref(), Some("2026-10-06"), "moved forward once, not twice");
        assert!(got.done.is_none());
    }
}

#[test]
fn r131_undo_leaves_a_task_changed_on_another_device_and_reaches_it() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b) = (device(), device());
    let tasks: Vec<TaskItem> = ["а", "б", "в"].iter().map(|t| add(&a, t)).collect();
    a.begin_group();
    for t in &tasks {
        a.set_due(t.id.clone(), Some("2026-10-12".into())).unwrap();
    }
    a.end_group();
    settle(&a, &b, &storage);
    assert!(!b.can_undo(), "the history stays on its device");
    b.set_due(tasks[1].id.clone(), Some("2026-11-01".into())).unwrap();
    settle(&a, &b, &storage);

    assert_eq!(a.undo().unwrap(), 2);
    settle(&a, &b, &storage);
    for d in [&a, &b] {
        let due = |i: usize| d.task(tasks[i].id.clone()).unwrap().due;
        assert_eq!((due(0), due(1), due(2)), (None, Some("2026-11-01".into()), None));
    }
}

#[test]
fn s44_undo_of_a_completed_repeat_reaches_the_other_device() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b) = (device(), device());
    let t = add(&a, "зарядка");
    a.set_due(t.id.clone(), Some("2026-10-05".into())).unwrap();
    a.set_repeat(
        t.id.clone(),
        Some(Repeat {
            freq: Freq::Daily,
            interval: 1,
            weekdays: vec![],
            monthday: None,
            nth: None,
            nth_weekday: None,
            from_done: false,
            count: None,
            until: None,
        }),
    )
    .unwrap();
    let sub = a
        .create_task(NewTask {
            title: "разминка".into(),
            parent_id: Some(t.id.clone()),
            ..NewTask::default()
        })
        .unwrap();
    a.complete_task(sub.id.clone()).unwrap();
    a.complete_task(t.id.clone()).unwrap();
    settle(&a, &b, &storage);
    assert_eq!(view(&b, Scope::Completed), ["зарядка"]);

    a.undo_close_task(t.id.clone()).unwrap();
    settle(&a, &b, &storage);
    for d in [&a, &b] {
        assert_eq!(d.task(t.id.clone()).unwrap().due.as_deref(), Some("2026-10-05"));
        assert!(
            d.task(sub.id.clone()).unwrap().done.is_some(),
            "the subtask is completed again"
        );
        assert!(view(d, Scope::Completed).is_empty());
    }
}

#[test]
fn s25_clearing_completed_reaches_the_other_device_and_outlives_a_reopen() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b) = (device(), device());
    let mut ids = Vec::new();
    for (day, title) in [
        ("2026-08-01", "давняя"),
        ("2026-09-20", "сентябрьская"),
        ("2026-10-05", "свежая"),
    ] {
        a.set_now_for_tests(&format!("{day}T09:00"));
        let t = add(&a, title);
        a.complete_task(t.id.clone()).unwrap();
        ids.push(t.id);
    }
    // Past the time a completed task stays among the open ones (R68).
    a.set_now_for_tests(NOW);
    settle(&a, &b, &storage);
    assert_eq!(view(&b, Scope::Completed), ["свежая", "сентябрьская", "давняя"]);

    assert_eq!(a.clear_completed(Some("2026-09-05".into())).unwrap(), 1);
    b.reopen_task(ids[0].clone()).unwrap();
    settle(&a, &b, &storage);
    for d in [&a, &b] {
        assert_eq!(view(d, Scope::Completed), ["свежая", "сентябрьская"]);
        assert!(
            view(d, Scope::Inbox).is_empty(),
            "a reopen made without knowing does not bring it back"
        );
        assert!(view(d, Scope::Trash).is_empty());
    }
    assert_eq!(dump(&a), dump(&b));
}

#[test]
fn s8_truncated_log_file_is_retried_not_skipped() {
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
        view(&b, Scope::Inbox).is_empty(),
        "nothing from a half-written file is applied"
    );

    std::fs::write(&path, &whole).unwrap();
    assert!(sync(&b, &storage).pulled > 0);
    assert_eq!(view(&b, Scope::Inbox), ["первая", "вторая"]);
}

#[test]
fn s9_upload_interrupted_before_confirmation_is_repeated_identically() {
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
    assert_eq!(view(&b, Scope::Inbox), ["задача"]);
}

#[test]
fn s10_s11_compaction_keeps_late_and_new_devices_complete() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b, c) = (device(), device(), device());
    a.set_compact_after_for_tests(3);

    add(&a, "t1");
    sync(&a, &storage);
    sync(&b, &storage); // B has read A's first file and then falls behind.
    assert_eq!(view(&b, Scope::Inbox), ["t1"]);
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
        let mut got = view(d, Scope::Inbox);
        got.sort();
        assert_eq!(got, ["t1", "t2", "t3", "t4", "t5", "t6", "от B"]);
    }
    // The snapshot told C which of B's files it already contains, and edits keep flowing.
    b.set_title(from_b.id.clone(), "от B, изменено".into()).unwrap();
    sync(&b, &storage);
    sync(&c, &storage);
    sync(&a, &storage);
    assert_eq!(c.task(from_b.id.clone()).unwrap().title, "от B, изменено");
    assert_eq!(a.task(from_b.id).unwrap().title, "от B, изменено");
}

#[test]
fn s11_device_whose_logs_are_all_compacted_is_still_readable() {
    let storage = tempfile::tempdir().unwrap();
    let (a, c) = (device(), device());
    a.set_compact_after_for_tests(0);
    add(&a, "единственная");
    sync(&a, &storage);
    // Everything of A is in its snapshot; what is left in the log is the mark that says so.
    let left = files(&storage, "log");
    assert!(matches!(&left[..], [mark] if mark.ends_with(".snap")), "{left:?}");

    sync(&c, &storage);
    assert_eq!(view(&c, Scope::Inbox), ["единственная"]);
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
fn s45_a_snapshot_follows_the_weight_of_the_log() {
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
    let task = add(&a, "задача");
    sync(&a, &storage);

    // Before its first snapshot the device takes one to weigh 16 KB.
    let big = 8000; // bytes of notes in one edit
    let mut runs = 0;
    while snaps().is_empty() {
        let before = log();
        assert!(before <= 16 * 1024, "the log grew to {before} bytes without a snapshot");
        runs += 1;
        a.set_notes(task.id.clone(), noise(big, runs)).unwrap();
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
        a.set_title(task.id.clone(), format!("задача {i}")).unwrap();
        sync(&a, &storage);
    }
    assert!(log() < *weight, "{} bytes of log against a snapshot of {weight}", log());
    assert_eq!(own_files(&storage, "log", &me, ".jsonl").len(), 5);
    assert_eq!(&snaps()[0].0, first);

    // One upload heavier than the snapshot makes one in the same run.
    a.set_notes(task.id.clone(), "ж".repeat(*weight as usize)).unwrap();
    sync(&a, &storage);
    assert_ne!(&snaps()[0].0, first);
    assert!(own_files(&storage, "log", &me, ".jsonl").is_empty());

    sync(&b, &storage);
    let got = b.task(task.id).unwrap();
    assert_eq!(
        (got.title.as_str(), got.notes.len()),
        ("задача 4", "ж".len() * *weight as usize)
    );
}

/// Rewrites a storage the way a version of format 1 would have left it:
/// plain snapshots, `"v":1` in every header, format 1 in `vault.json`.
fn to_format_1(storage: &TempDir) -> String {
    use std::io::Read;
    let root = storage.path().join(ROOT);
    for dir in ["log", "snap"] {
        for name in files(storage, dir).iter().filter(|n| n.ends_with(".jsonl")) {
            let path = root.join(dir).join(name);
            let data = std::fs::read(&path).unwrap();
            let mut text = String::new();
            if data.starts_with(&[0x1f, 0x8b]) {
                flate2::read::GzDecoder::new(&data[..])
                    .read_to_string(&mut text)
                    .unwrap();
            } else {
                text = String::from_utf8(data).unwrap();
            }
            std::fs::write(&path, text.replacen("{\"v\":2,", "{\"v\":1,", 1)).unwrap();
        }
    }
    let vault: serde_json::Value = serde_json::from_slice(&std::fs::read(root.join("vault.json")).unwrap()).unwrap();
    let id = vault["vault"].as_str().unwrap().to_string();
    std::fs::write(root.join("vault.json"), format!("{{\"format\":1,\"vault\":\"{id}\"}}")).unwrap();
    id
}

#[test]
fn s46_a_storage_of_format_1_moves_to_format_2() {
    let storage = tempfile::tempdir().unwrap();
    let root = storage.path().join(ROOT);
    let (a, b, c) = (device(), device(), device());
    a.set_compact_after_for_tests(0);
    add(&a, "из снимка");
    sync(&a, &storage);
    a.set_compact_after_for_tests(256);
    add(&a, "из журнала");
    sync(&a, &storage);
    let vault = to_format_1(&storage);
    let a_snap = own_files(&storage, "snap", &a.device_id(), ".jsonl");
    assert!(!std::fs::read(root.join("snap").join(&a_snap[0].0))
        .unwrap()
        .starts_with(&[0x1f, 0x8b]));

    b.set_compact_after_for_tests(0);
    add(&b, "с B");
    sync(&b, &storage);
    let mut got = view(&b, Scope::Inbox);
    got.sort();
    assert_eq!(got, ["из журнала", "из снимка", "с B"]);
    assert_eq!(
        std::fs::read_to_string(root.join("vault.json")).unwrap(),
        format!("{{\"format\":2,\"vault\":\"{vault}\"}}")
    );
    let b_snap = own_files(&storage, "snap", &b.device_id(), ".jsonl");
    let data = std::fs::read(root.join("snap").join(&b_snap[0].0)).unwrap();
    assert!(data.starts_with(&[0x1f, 0x8b]), "the snapshot is compressed");

    // C has no snapshot yet, so its log file stays to be looked at.
    add(&c, "с C");
    sync(&c, &storage);
    let c_log = own_files(&storage, "log", &c.device_id(), ".jsonl");
    let text = std::fs::read_to_string(root.join("log").join(&c_log[0].0)).unwrap();
    assert!(text.starts_with("{\"v\":2,"), "{text}");

    let d = device();
    sync(&d, &storage);
    let mut got = view(&d, Scope::Inbox);
    got.sort();
    assert_eq!(got, ["из журнала", "из снимка", "с B", "с C"]);
}

#[test]
fn s50_a_compressed_snapshot_carries_the_time_it_was_written() {
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

/// The storage again, as an app that was started anew sees it: the format
/// and the old place are looked at once per storage and process (S14, S49).
fn restarted(storage: &TempDir) -> DirRemote {
    DirRemote::new(storage.path().join("."))
}

/// Moves a storage to where versions before #294 kept it, `lists/v1/`.
fn to_legacy(storage: &TempDir) {
    let (root, aside) = (storage.path().join(ROOT), storage.path().join("aside"));
    std::fs::remove_file(root.join("v1/vault.json")).unwrap();
    std::fs::remove_dir(root.join("v1")).unwrap();
    std::fs::rename(&root, &aside).unwrap();
    std::fs::create_dir(&root).unwrap();
    std::fs::rename(&aside, root.join("v1")).unwrap();
}

/// Every file under a folder of the storage, by its path inside that folder.
fn tree(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if entry.path().is_dir() {
            out.extend(tree(&entry.path()).into_iter().map(|n| format!("{name}/{n}")));
        } else {
            out.push(name);
        }
    }
    out.sort();
    out
}

fn vault(path: &Path) -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

/// A storage whose moves give out after a number of them, as when the network goes.
struct CutShort {
    inner: DirRemote,
    left: Mutex<u32>,
}

impl Remote for CutShort {
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
        self.inner.delete(path)
    }
    fn exists(&self, path: &str) -> Result<bool> {
        self.inner.exists(path)
    }
    fn folders(&self, dir: &str) -> Result<Vec<String>> {
        self.inner.folders(dir)
    }
    fn rename(&self, from: &str, to: &str) -> Result<bool> {
        let mut left = self.left.lock().unwrap();
        if *left == 0 {
            return Err(AppError::Sync {
                msg: "connection reset".into(),
            });
        }
        *left -= 1;
        self.inner.rename(from, to)
    }
}

/// A storage of the layout before #294 with a snapshot, a log file, a mark,
/// attachment content and a nudge address of A.
fn legacy_storage(format_1: bool) -> (TempDir, Device, Attachment, String) {
    let storage = tempfile::tempdir().unwrap();
    let a = device();
    // A stands for an earlier version, which kept no copies in the storage (S60).
    a.set_storage_backups(false).unwrap();
    a.set_compact_after_for_tests(0);
    let task = add(&a, "из снимка");
    let att = attach(&a, &task, "a.txt", b"content of A");
    sync(&a, &storage);
    sync_files(&a, &storage);
    a.set_compact_after_for_tests(256);
    add(&a, "из журнала");
    sync(&a, &storage);
    std::fs::create_dir_all(storage.path().join(ROOT).join("push")).unwrap();
    std::fs::write(
        storage
            .path()
            .join(ROOT)
            .join("push")
            .join(format!("{}.json", a.device_id())),
        br#"{"v":1,"url":"https://push.example/a"}"#,
    )
    .unwrap();
    let id = if format_1 {
        to_format_1(&storage)
    } else {
        vault(&storage.path().join(ROOT).join("vault.json"))["vault"]
            .as_str()
            .unwrap()
            .to_string()
    };
    to_legacy(&storage);
    (storage, a, att, id)
}

#[test]
fn s49_a_storage_in_lists_v1_moves_to_lists() {
    for format_1 in [false, true] {
        let (storage, a, att, id) = legacy_storage(format_1);
        let (root, old) = (storage.path().join(ROOT), storage.path().join(ROOT).join("v1"));
        let mut before: Vec<String> = tree(&old).into_iter().filter(|n| n != "vault.json").collect();
        before.sort();
        assert_eq!(
            before
                .iter()
                .filter(|n| n.starts_with("log/") && n.ends_with(".snap"))
                .count(),
            1,
            "{before:?}"
        );

        let b = device();
        add(&b, "с B");
        sync(&b, &storage);
        sync_files(&b, &storage);
        let mut got = view(&b, Scope::Inbox);
        got.sort();
        assert_eq!(got, ["из журнала", "из снимка", "с B"], "format 1: {format_1}");

        // Every file of A is under its old name in the new place, and only the old vault is left behind.
        let moved: Vec<String> = tree(&root)
            .into_iter()
            .filter(|n| !n.starts_with("v1/") && n != "vault.json" && !n.contains(&b.device_id()))
            .collect();
        assert_eq!(moved, before);
        assert_eq!(tree(&old), ["vault.json"]);
        assert_eq!(
            vault(&root.join("vault.json")),
            serde_json::json!({"format": 2, "vault": id})
        );
        assert_eq!(
            vault(&old.join("vault.json")),
            serde_json::json!({"format": 3, "vault": id})
        );
        let files = b
            .attachments(
                b.tasks(Scope::All)
                    .unwrap()
                    .into_iter()
                    .find(|t| t.title == "из снимка")
                    .unwrap()
                    .id,
            )
            .unwrap();
        assert_eq!(files[0].sha256, att.sha256);
        assert_eq!(
            std::fs::read(files[0].local_path.as_ref().unwrap()).unwrap(),
            b"content of A"
        );

        // A, already on the new version, goes on in the new place; C joins from nothing.
        sync(&a, &storage);
        assert!(view(&a, Scope::Inbox).contains(&"с B".to_string()));
        let c = device();
        sync(&c, &storage);
        let mut got = view(&c, Scope::Inbox);
        got.sort();
        assert_eq!(got, ["из журнала", "из снимка", "с B"]);
    }
}

#[test]
fn s49_a_move_cut_short_is_finished_by_the_next_run() {
    let (storage, _a, _att, id) = legacy_storage(false);
    let root = storage.path().join(ROOT);
    let b = device();
    let cut = CutShort {
        inner: DirRemote::new(storage.path()),
        left: Mutex::new(2),
    };
    assert!(b.sync_with(&cut).is_err());
    assert!(
        !root.join("vault.json").exists(),
        "the new place is not named before the move is done"
    );
    assert_eq!(vault(&root.join("v1/vault.json"))["format"], 2);

    sync(&b, &storage);
    let mut got = view(&b, Scope::Inbox);
    got.sort();
    assert_eq!(got, ["из журнала", "из снимка"]);
    assert_eq!(tree(&root.join("v1")), ["vault.json"]);
    assert_eq!(vault(&root.join("vault.json"))["vault"], id.as_str());
}

#[test]
fn s49_two_devices_moving_at_once_lose_nothing() {
    let (storage, _a, _att, _) = legacy_storage(false);
    let before: Vec<String> = tree(&storage.path().join(ROOT).join("v1"))
        .into_iter()
        .filter(|n| n != "vault.json")
        .collect();
    let (b, c) = (device(), device());
    // B lists the old place and moves one file; C moves the rest under its feet; B goes on.
    let cut = CutShort {
        inner: DirRemote::new(storage.path()),
        left: Mutex::new(1),
    };
    assert!(b.sync_with(&cut).is_err());
    sync(&c, &storage);
    sync(&b, &storage);
    // The copies B and C put into the storage are their own (S60), not moved files of A.
    let moved: Vec<String> = tree(&storage.path().join(ROOT))
        .into_iter()
        .filter(|n| !n.starts_with("v1/") && n != "vault.json" && !n.starts_with("backups/"))
        .collect();
    assert_eq!(moved, before);
    assert_eq!(dump(&b), dump(&c));
    assert_eq!(view(&b, Scope::Inbox).len(), 2);
}

#[test]
fn s49_what_an_earlier_version_writes_to_the_old_place_after_the_move_is_fetched() {
    let (storage, a, _att, _) = legacy_storage(false);
    let b = device();
    sync(&b, &storage);
    assert_eq!(sync_files(&b, &storage).downloaded, 1);

    // A is still on the earlier version: its next file and content land in lists/v1/.
    let elsewhere = tempfile::tempdir().unwrap();
    let late = add(&a, "поздняя");
    let att = attach(&a, &late, "late.txt", b"late content");
    sync(&a, &elsewhere);
    sync_files(&a, &elsewhere);
    let old = storage.path().join(ROOT).join("v1");
    // What A wrote there, its log and a snapshot if it took one, goes to the old place.
    let mut written = Vec::new();
    for dir in ["log", "snap"] {
        std::fs::create_dir_all(old.join(dir)).unwrap();
        for name in files(&elsewhere, dir) {
            std::fs::copy(
                elsewhere.path().join(ROOT).join(dir).join(&name),
                old.join(dir).join(&name),
            )
            .unwrap();
            written.push(format!("{dir}/{name}"));
        }
    }
    let blob = format!("blobs/{}/{}", &att.sha256[..2], att.sha256);
    std::fs::create_dir_all(old.join(&blob).parent().unwrap()).unwrap();
    std::fs::copy(elsewhere.path().join(ROOT).join(&blob), old.join(&blob)).unwrap();

    // Without a restart B does not look there.
    sync(&b, &storage);
    assert!(!view(&b, Scope::Inbox).contains(&"поздняя".to_string()));
    b.sync_with(&restarted(&storage)).unwrap();
    assert!(view(&b, Scope::Inbox).contains(&"поздняя".to_string()));
    // The content waits there until it is asked for.
    assert_eq!(tree(&old), [blob.clone(), "vault.json".to_string()]);
    for file in &written {
        assert!(storage.path().join(ROOT).join(file).is_file(), "{file}");
    }

    // The content is found in the old place and moved to the new one.
    assert_eq!(sync_files(&b, &storage).downloaded, 1);
    assert!(storage.path().join(ROOT).join(&blob).is_file());
    assert_eq!(tree(&old), ["vault.json"]);
}

#[test]
fn s49_a_new_storage_closes_the_old_place() {
    let storage = tempfile::tempdir().unwrap();
    let a = device();
    add(&a, "задача");
    sync(&a, &storage);
    let root = storage.path().join(ROOT);
    let id = vault(&root.join("vault.json"))["vault"].clone();
    assert_eq!(
        vault(&root.join("v1/vault.json")),
        serde_json::json!({"format": 3, "vault": id})
    );
    assert_eq!(vault(&root.join("vault.json"))["format"], 2);
}

#[test]
fn s49_the_side_check_sees_a_storage_not_moved_yet_and_moves_nothing() {
    let (storage, _a, _att, _) = legacy_storage(false);
    let before = tree(storage.path());
    let c = device();
    add(&c, "своя");
    assert!(c.sync_conflict(folder(&storage), String::new()).unwrap());
    assert_eq!(tree(storage.path()), before);
}

#[test]
fn s12_joining_a_used_storage_merges_instead_of_replacing() {
    let storage = tempfile::tempdir().unwrap();
    let (a, c) = (device(), device());
    add(&a, "с A");
    sync(&a, &storage);
    add(&c, "с C");
    settle(&c, &a, &storage);
    for d in [&a, &c] {
        let mut got = view(d, Scope::Inbox);
        got.sort();
        assert_eq!(got, ["с A", "с C"]);
    }
}

#[test]
fn s13_attachment_content_follows_the_record() {
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
fn s13_corrupted_attachment_content_is_not_stored() {
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
fn s34_the_fields_arrive_before_the_content() {
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
    assert_eq!(view(&b, Scope::Inbox), ["с файлом"]);
    let got = b.attachments(t.id.clone()).unwrap();
    assert_eq!((got.len(), got[0].local_path.as_ref()), (1, None));
    assert_eq!(b.task(t.id.clone()).unwrap().attachments, 1);
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
fn s34_refused_content_holds_back_neither_the_task_nor_the_other_files() {
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
    assert_eq!(view(&b, Scope::Inbox), ["с двумя файлами"]);
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
fn s34_s35_a_pass_under_way_holds_back_neither_the_fields_nor_a_request() {
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
    assert_eq!(view(&b, Scope::Inbox), ["переименовано"]);
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
fn s34_content_that_cannot_move_does_not_fail_configured_sync() {
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
fn s34_attachments_of_deleted_tasks_and_removed_attachments_do_not_wait() {
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
    a.set_sync_config(SyncConfig::CalDav {
        url: "http://127.0.0.1:9/dav".into(),
        user: String::new(),
    })
    .unwrap();
    assert_eq!(a.sync_status().unwrap().attachments_waiting, 0);
    assert_eq!(a.sync_attachments().unwrap(), AttachmentReport::default());
}

#[test]
fn s35_one_attachment_is_fetched_on_request() {
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
fn s35_content_that_does_not_match_its_hash_is_not_fetched() {
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

/// R140: a storage whose byte methods refuse attachment content: the core must use the file methods.
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
fn s52_content_goes_through_the_files_on_disk() {
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
fn s52_content_above_the_limit_is_not_downloaded() {
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
        max_task: 1000,
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
        max_task: 2000,
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
fn r91_content_that_has_not_arrived_is_fetched_before_everything_is_saved() {
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
fn s14_newer_storage_format_stops_sync_but_not_local_work() {
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
    assert_eq!(view(&a, Scope::Inbox), ["локально"]);
}

#[test]
fn s14_format_raised_while_the_app_runs_is_noticed_at_the_first_unreadable_file() {
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
fn s16_configured_sync_reports_status_and_keeps_changes_after_a_failure() {
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
    assert_eq!(view(&b, Scope::Inbox), ["не потеряется"]);
}

#[test]
fn s15_a_file_brought_into_the_sync_folder_is_noticed_once() {
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
fn switching_storage_publishes_the_whole_state_there() {
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

    // A moves to a new storage: it must carry B's task along, not only its own writes.
    a.set_sync_config(folder(&new)).unwrap();
    a.sync_now().unwrap();
    c.set_sync_config(folder(&new)).unwrap();
    c.sync_now().unwrap();
    let mut got = view(&c, Scope::Inbox);
    got.sort();
    assert_eq!(got, ["от A", "от B"]);

    // Going back to the old storage keeps working for devices that stayed there.
    add(&a, "после возврата");
    a.set_sync_config(folder(&old)).unwrap();
    a.sync_now().unwrap();
    b.sync_now().unwrap();
    assert!(view(&b, Scope::Inbox).contains(&"после возврата".to_string()));
}

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
        let mut ids: Vec<String> = Vec::new();
        let mut minute = 0;
        for step in 0..160 {
            let d = &devices[rnd(3)];
            minute += 1;
            d.set_now_for_tests(&format!("2026-10-05T{:02}:{:02}", 10 + minute / 60, minute % 60));
            let known = |d: &Device, id: &String| d.task(id.clone()).is_ok();
            let pick = if ids.is_empty() {
                None
            } else {
                Some(ids[rnd(ids.len())].clone())
            };
            match (rnd(12), pick) {
                (0..=2, _) | (_, None) => ids.push(add(d, &format!("task {step}")).id),
                (3, Some(id)) if known(d, &id) => d.set_title(id, format!("renamed {step}")).unwrap(),
                (4, Some(id)) if known(d, &id) => d.set_due(id, Some(format!("2026-10-{:02}", 1 + rnd(28)))).unwrap(),
                (5, Some(id)) if known(d, &id) => d.set_priority(id, Priority::from_i64(rnd(4) as i64)).unwrap(),
                (6, Some(id)) if known(d, &id) => drop(d.complete_task(id).unwrap()),
                (7, Some(id)) if known(d, &id) => d.delete_task(id).unwrap(),
                (8, Some(id)) if known(d, &id) => d.add_tag(id, format!("t{}", rnd(3))).unwrap(),
                (9, Some(id)) if known(d, &id) => {
                    let other = ids[rnd(ids.len())].clone();
                    if known(d, &other) {
                        // Rejected when it would create a cycle locally; that is fine.
                        d.move_task(id, None, Some(other), None).ok();
                    }
                }
                (10, Some(id)) if known(d, &id) => d.move_task(id, None, None, None).unwrap(),
                _ => drop(sync(d, &storage)),
            }
        }
        for _ in 0..2 {
            for d in &devices {
                sync(d, &storage);
            }
        }
        // What a view shows depends on the clock (R68): compare the devices at one and the same moment.
        for d in &devices {
            d.set_now_for_tests("2026-10-06T10:00");
        }
        let reference = dump(&devices[0]);
        assert!(reference.len() > 5, "seed {seed}: the run produced data");
        for (i, d) in devices.iter().enumerate().skip(1) {
            assert_eq!(dump(d), reference, "seed {seed}: device {i} differs");
        }
        for d in &devices {
            assert_eq!(sync(d, &storage).pulled, 0, "seed {seed}: nothing left to pull");
        }
    }
}

#[test]
fn s13_attachment_hash_from_the_storage_never_becomes_a_path() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b) = (device(), device());
    let t = add(&a, "с файлом");
    let att = attach(&a, &t, "a.txt", b"content");
    sync(&a, &storage);
    sync_files(&a, &storage);

    // A file that exists on B outside its blob folder, and a log in the storage
    // that points the attachment at it.
    let outside = b.data_dir().join("lists.sqlite");
    assert!(outside.exists());
    let log = storage.path().join(ROOT).join("log");
    let snap = storage.path().join(ROOT).join("snap");
    for dir in [log, snap] {
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let text = std::fs::read_to_string(entry.path()).unwrap();
            std::fs::write(entry.path(), text.replace(&att.sha256, "../lists.sqlite")).unwrap();
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

#[test]
fn projects_and_saved_filters_sync_like_everything_else() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b) = (device(), device());
    let p = add(&a, "проект");
    a.set_project(p.id.clone(), true).unwrap();
    a.quick_add_under("шаг".into(), p.id.clone(), false, None).unwrap();
    let f = a
        .create_filter(
            "Неделя".into(),
            FilterSpec {
                due: DueWindow::Next { days: 7 },
                ..FilterSpec::default()
            },
        )
        .unwrap();
    settle(&a, &b, &storage);

    assert_eq!(titles(&b.projects().unwrap()), ["проект"]);
    assert_eq!(view(&b, Scope::Project { id: p.id }), ["шаг"]);
    assert_eq!(b.filters().unwrap()[0].spec.due, DueWindow::Next { days: 7 });

    // A rename on one device and a new condition on the other are both kept.
    a.update_filter(f.id.clone(), "Ближайшее".into(), f.spec.clone())
        .unwrap();
    b.set_now_for_tests("2026-10-05T10:05");
    b.update_filter(
        f.id.clone(),
        "Неделя".into(),
        FilterSpec {
            due: DueWindow::Next { days: 7 },
            min_priority: Priority::High,
            ..FilterSpec::default()
        },
    )
    .unwrap();
    settle(&a, &b, &storage);
    for d in [&a, &b] {
        let got = &d.filters().unwrap()[0];
        assert_eq!(
            (got.name.as_str(), got.spec.min_priority),
            ("Ближайшее", Priority::High)
        );
    }
    b.delete_filter(f.id).unwrap();
    settle(&b, &a, &storage);
    assert!(a.filters().unwrap().is_empty());
}

#[test]
fn s32_hidden_inbox_and_the_default_list_reach_the_other_device() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b) = (device(), device());
    let home = a.create_list("Дом".into()).unwrap();
    let task = add(&a, "а");
    settle(&a, &b, &storage);

    a.hide_inbox(home.id.clone(), true).unwrap();
    settle(&a, &b, &storage);
    assert!(b.inbox_hidden().unwrap());
    assert_eq!(b.new_task_list().unwrap(), "last");
    assert_eq!(b.task(task.id).unwrap().list_id, home.id);
    assert_eq!(b.default_list_id(None).unwrap(), home.id);

    b.set_now_for_tests("2026-10-05T10:01");
    b.set_new_task_list(home.id.clone()).unwrap();
    settle(&a, &b, &storage);
    assert_eq!(
        (a.new_task_list().unwrap(), b.new_task_list().unwrap()),
        (home.id.clone(), home.id.clone())
    );

    b.show_inbox().unwrap();
    settle(&b, &a, &storage);
    assert!(!a.inbox_hidden().unwrap());
}

#[test]
fn s32_the_shared_setting_merges_like_any_field_and_reaches_a_new_device() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b) = (device(), device());
    settle(&a, &b, &storage);

    let at_once = KeepDone::Seconds { seconds: 0 };
    a.set_keep_done(KeepDone::Seconds { seconds: 15 }).unwrap();
    b.set_now_for_tests("2026-10-05T10:01");
    b.set_keep_done(at_once).unwrap();
    settle(&a, &b, &storage);
    assert_eq!((a.keep_done().unwrap(), b.keep_done().unwrap()), (at_once, at_once));

    let c = device();
    sync(&c, &storage);
    assert_eq!(c.keep_done().unwrap(), at_once);

    // What it means on the device that did not set it: completed there, gone at once here.
    let t = add(&b, "задача");
    b.complete_task(t.id).unwrap();
    settle(&b, &a, &storage);
    assert!(view(&a, Scope::Inbox).is_empty());

    let everywhere = |keep: KeepDone| {
        settle(&a, &b, &storage);
        sync(&c, &storage);
        for device in [&a, &b, &c] {
            assert_eq!(device.keep_done().unwrap(), keep);
        }
    };
    a.set_now_for_tests("2026-10-05T10:02");
    a.set_keep_done(KeepDone::EndOfDay).unwrap();
    everywhere(KeepDone::EndOfDay);

    // A version that knows only minutes chooses fifteen of them.
    b.set_now_for_tests("2026-10-05T10:03");
    b.set_setting_for_tests("keep_done", serde_json::json!(15));
    everywhere(KeepDone::Seconds { seconds: 900 });
}

#[test]
fn s33_the_wont_do_outcome_merges_with_completion_and_survives_an_unaware_version() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b) = (device(), device());
    let plain = add(&a, "брошенная");
    let first = add(&a, "сначала выполнена");
    let second = add(&a, "сначала брошена");
    settle(&a, &b, &storage);

    a.wont_do_task(plain.id.clone()).unwrap();
    // Both close the same task offline: the later `done` decides.
    a.complete_task(first.id.clone()).unwrap();
    a.wont_do_task(second.id.clone()).unwrap();
    b.set_now_for_tests("2026-10-05T11:00");
    b.wont_do_task(first.id.clone()).unwrap();
    b.complete_task(second.id.clone()).unwrap();
    settle(&a, &b, &storage);
    for d in [&a, &b] {
        assert!(d.task(plain.id.clone()).unwrap().wont);
        assert!(d.task(first.id.clone()).unwrap().wont, "marked later");
        let got = d.task(second.id.clone()).unwrap();
        assert!(got.done.is_some() && !got.wont, "completed later");
        assert_eq!(view(d, Scope::WontDo).len(), 2);
    }

    // A version that does not know `wont` reopens by clearing `done` alone.
    b.set_now_for_tests("2026-10-05T12:00");
    b.set_task_field_for_tests(&plain.id, "done", serde_json::Value::Null);
    settle(&a, &b, &storage);
    for d in [&a, &b] {
        let got = d.task(plain.id.clone()).unwrap();
        assert!(got.done.is_none() && !got.wont, "open again");
    }
    // It completes the task in another minute: the moments differ.
    b.set_task_field_for_tests(&plain.id, "done", serde_json::json!("2026-10-05T12:00"));
    settle(&a, &b, &storage);
    for d in [&a, &b] {
        let got = d.task(plain.id.clone()).unwrap();
        assert!(got.done.is_some() && !got.wont, "completed");
    }
    // Reopened and completed here in the minute the mark was made at: the
    // leftover `wont` is cleared, so the task is completed all the same.
    a.set_now_for_tests("2026-10-05T12:30");
    a.set_task_field_for_tests(&plain.id, "done", serde_json::Value::Null);
    a.set_now_for_tests("2026-10-05T10:00");
    assert!(!a.complete_task(plain.id.clone()).unwrap().wont);
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

#[test]
fn s36_there_is_a_side_to_choose_only_when_both_hold_data() {
    let storage = tempfile::tempdir().unwrap();
    let empty = tempfile::tempdir().unwrap();
    let (a, c, fresh) = (device(), device(), device());
    add(&a, "с A");
    sync(&a, &storage);
    let t = add(&c, "с C");

    let before = (files(&storage, "log"), dump(&c));
    assert!(c.sync_conflict(folder(&storage), String::new()).unwrap());
    assert!(!c.sync_conflict(folder(&empty), String::new()).unwrap());
    assert!(!fresh.sync_conflict(folder(&storage), String::new()).unwrap());
    assert!(!c.sync_conflict(SyncConfig::Off, String::new()).unwrap());
    assert_eq!((files(&storage, "log"), dump(&c)), before, "the check writes nothing");
    assert!(files(&empty, "log").is_empty());

    // A list of its own is data as well; tasks in the trash are not.
    c.delete_task(t.id).unwrap();
    assert!(!c.sync_conflict(folder(&storage), String::new()).unwrap());
    c.create_list("Работа".into()).unwrap();
    assert!(c.sync_conflict(folder(&storage), String::new()).unwrap());
}

#[test]
fn s37_taking_the_storage_drops_what_the_device_held() {
    let storage = tempfile::tempdir().unwrap();
    let (a, c) = (device(), device());
    let shared = add(&a, "общая");
    sync(&a, &storage);
    add(&c, "в");
    let was = c.device_id();

    c.replace_local_with_remote().unwrap();
    assert!(view(&c, Scope::All).is_empty(), "empty until the first run");
    assert_ne!(c.device_id(), was);
    settle(&c, &a, &storage);
    for d in [&a, &c] {
        assert_eq!(view(d, Scope::All), ["общая"]);
        assert!(view(d, Scope::Trash).is_empty());
    }

    // An edit that was not uploaded is lost, and what the device uploaded before comes back.
    add(&c, "выгруженная");
    settle(&c, &a, &storage);
    c.set_title(shared.id.clone(), "изменено без связи".into()).unwrap();
    c.replace_local_with_remote().unwrap();
    settle(&c, &a, &storage);
    for d in [&a, &c] {
        assert_eq!(sorted(view(d, Scope::All)), ["выгруженная", "общая"]);
    }

    // The device goes on as a new one: its next change reaches the others.
    add(&c, "после");
    settle(&c, &a, &storage);
    assert_eq!(sorted(view(&a, Scope::All)), ["выгруженная", "общая", "после"]);
    assert_eq!(dump(&a), dump(&c));
}

#[test]
fn s38_sending_the_device_puts_the_rest_into_the_trash_everywhere() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b, c) = (device(), device(), device());
    let a1 = add(&a, "а1");
    add(&a, "а2");
    let work = a.create_list("Работа".into()).unwrap();
    settle(&a, &b, &storage);
    add(&c, "в");

    c.replace_remote_with_local().unwrap();
    sync(&c, &storage);
    sync(&a, &storage);
    sync(&b, &storage);
    for d in [&a, &b, &c] {
        assert_eq!(view(d, Scope::All), ["в"]);
        assert_eq!(sorted(view(d, Scope::Trash)), ["а1", "а2"]);
        assert!(d.lists().unwrap().iter().all(|l| l.id != work.id), "the list is gone");
    }
    assert_eq!(dump(&a), dump(&c));
    assert_eq!(dump(&b), dump(&c));
    // Nothing waits any more: the next run is an ordinary one.
    assert_eq!(sync(&c, &storage), SyncReport::default());

    // What went to the trash can be taken back.
    a.restore_task(a1.id).unwrap();
    settle(&a, &c, &storage);
    sync(&b, &storage);
    for d in [&a, &b, &c] {
        assert_eq!(sorted(view(d, Scope::All)), ["а1", "в"]);
    }
}

#[test]
fn s38_sending_the_device_undoes_what_it_has_not_seen() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b) = (device(), device());
    let t = add(&a, "один");
    let cleared = add(&a, "очищенная");
    settle(&a, &b, &storage);

    // A goes on: a rename, a tag, a task deleted for good, a new task.
    a.set_title(t.id.clone(), "два".into()).unwrap();
    a.add_tag(t.id.clone(), "дом".into()).unwrap();
    a.delete_task(cleared.id.clone()).unwrap();
    a.empty_trash().unwrap();
    add(&a, "новая на A");
    sync(&a, &storage);

    // B holds the earlier state, like a device restored from a backup.
    b.replace_remote_with_local().unwrap();
    sync(&b, &storage);
    sync(&a, &storage);
    for d in [&a, &b] {
        let task = d.task(t.id.clone()).unwrap();
        assert_eq!(task.title, "один");
        assert!(task.tags.is_empty());
        assert_eq!(sorted(view(d, Scope::All)), ["один", "очищенная"]);
        assert_eq!(view(d, Scope::Trash), ["новая на A"]);
    }
    assert_eq!(dump(&a), dump(&b));
}

#[test]
fn s38_an_edit_made_before_and_uploaded_after_loses() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b) = (device(), device());
    let t = add(&a, "один");
    settle(&a, &b, &storage);

    a.set_title(t.id.clone(), "правка без связи".into()).unwrap();
    add(&a, "создана без связи");
    b.replace_remote_with_local().unwrap();
    sync(&b, &storage);
    settle(&a, &b, &storage);
    for d in [&a, &b] {
        assert_eq!(d.task(t.id.clone()).unwrap().title, "один");
        // A task the device could not see is not its to delete.
        assert_eq!(sorted(view(d, Scope::All)), ["один", "создана без связи"]);
    }
}

#[test]
fn s39_a_failed_run_leaves_the_replacement_waiting() {
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
fn s38_another_storage_cancels_a_waiting_replacement() {
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
fn s36_joining_a_storage_and_choosing_a_side_is_one_step() {
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

/// Both passes, the way the apps run them (S34).
fn sync_all(d: &Device, storage: &TempDir) {
    sync(d, storage);
    sync_files(d, storage);
}

#[test]
fn s47_content_stays_while_its_task_can_come_back_and_goes_after() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b) = (device(), device());
    let trashed = add(&a, "в корзину");
    let purged = attach(&a, &trashed, "1.txt", b"one");
    let kept = add(&a, "живая");
    let removed = attach(&a, &kept, "2.txt", b"two");
    let (v, w) = (add(&a, "v"), add(&a, "w"));
    let shared = attach(&a, &v, "3.txt", b"three");
    attach(&a, &w, "3.txt", b"three");
    sync_all(&a, &storage);
    sync_all(&b, &storage);
    let all = [&purged.sha256, &removed.sha256, &shared.sha256];
    assert!(all.iter().all(|h| on_device(&b, h) && in_storage(&storage, h)));

    // A task in the trash keeps its content everywhere.
    a.delete_task(trashed.id.clone()).unwrap();
    sync_all(&a, &storage);
    sync_all(&b, &storage);
    assert!(all
        .iter()
        .all(|h| on_device(&a, h) && on_device(&b, h) && in_storage(&storage, h)));

    // Deleted for good, or taken off a task: gone; content another attachment refers to stays.
    a.empty_trash().unwrap();
    a.remove_attachment(removed.id.clone()).unwrap();
    a.remove_attachment(shared.id.clone()).unwrap();
    sync_all(&a, &storage);
    sync_all(&b, &storage);
    for gone in [&purged.sha256, &removed.sha256] {
        assert!(!on_device(&a, gone) && !on_device(&b, gone) && !in_storage(&storage, gone));
    }
    assert!(on_device(&a, &shared.sha256) && on_device(&b, &shared.sha256) && in_storage(&storage, &shared.sha256));
}

#[test]
fn s47_a_run_that_did_not_read_everything_removes_nothing() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b) = (device(), device());
    let t = add(&a, "с файлом");
    let att = attach(&a, &t, "1.txt", b"one");
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
fn s47_a_device_without_sync_removes_unneeded_content_when_it_starts() {
    let a = device();
    let t = add(&a, "с файлом");
    let att = attach(&a, &t, "1.txt", b"one");
    a.remove_attachment(att.id).unwrap();
    assert!(on_device(&a, &att.sha256));
    Store::open(a.dir().to_string_lossy().into_owned()).unwrap();
    assert!(!on_device(&a, &att.sha256));
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

#[test]
fn s48_content_attached_again_without_a_connection_comes_back_to_the_storage() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b, c) = (device(), device(), device());
    let t = add(&a, "у A");
    let att = attach(&a, &t, "1.txt", b"one");
    sync_all(&a, &storage);
    sync_all(&b, &storage); // B downloads the content and counts it as in the storage.

    // Without a connection B attaches the same file to a task of its own.
    let u = add(&b, "у B");
    attach(&b, &u, "1.txt", b"one");

    // A deletes its task for good and removes the content from the storage.
    a.delete_task(t.id.clone()).unwrap();
    a.empty_trash().unwrap();
    sync_all(&a, &storage);
    assert!(!in_storage(&storage, &att.sha256));

    // B uploads its task but still counts the content as in the storage.
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
fn s48_a_device_that_reads_the_removal_does_not_remove_it_again() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b) = (device(), device());
    let t = add(&a, "с файлом");
    let att = attach(&a, &t, "1.txt", b"one");
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

#[test]
fn s51_a_tag_at_the_limit_syncs_and_a_longer_one_from_before_is_not_shown() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b) = (device(), device());
    let t = add(&a, "задача");
    // Both scripts at the byte limit: 196 Latin letters and 98 Cyrillic ones.
    let latin = "a".repeat(196);
    let cyrillic = "я".repeat(98);
    a.add_tag(t.id.clone(), latin.clone()).unwrap();
    a.add_tag(t.id.clone(), cyrillic.clone()).unwrap();
    let long = "b".repeat(197);

    // An earlier version took any word for a tag and left its field in the database.
    let a = plant(a, &[("task", &t.id, &format!("tag:{long}"), serde_json::json!(true))]);
    let shown = vec![latin, cyrillic];
    assert_eq!(
        a.task(t.id.clone()).unwrap().tags,
        shown,
        "a tag no register can carry is not shown"
    );

    sync(&a, &storage);
    sync(&b, &storage);
    assert_eq!(b.task(t.id.clone()).unwrap().tags, shown);
    assert_eq!(a.task(t.id).unwrap().tags, shown);
}
