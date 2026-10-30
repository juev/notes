//! The text of a note as an Automerge document: S3, S4, S7, S13, S15, S20–S22, S36–S38, S70 in
//! docs/specs/sync.md, and R28 and R45 in docs/specs/product.md. Storage is a temporary folder.

mod common;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use automerge::transaction::{CommitOptions, Transactable};
use automerge::{ActorId, AutoCommit, ObjType, ReadDoc, TextEncoding, ROOT};
use base64::Engine as _;
use common::*;
use notes_core::sync::remote::{DirRemote, Remote};
use notes_core::*;
use serde_json::{json, Value};
use tempfile::TempDir;

const ROOT_DIR: &str = "notes";

fn sync(d: &Device, storage: &TempDir) -> SyncReport {
    d.sync_with(&DirRemote::new(storage.path())).unwrap()
}

/// Every device has seen everything the others have written.
fn settle(a: &Device, b: &Device, storage: &TempDir) {
    sync(a, storage);
    sync(b, storage);
    sync(a, storage);
}

fn note_with(d: &Device, title: &str, text: &str) -> Note {
    d.create_note(NewNote {
        title: title.into(),
        text: text.into(),
        ..NewNote::default()
    })
    .unwrap()
}

fn text(d: &Device, note: &Note) -> String {
    d.get_text(note.id.clone()).unwrap()
}

fn units(text: &str) -> u32 {
    text.encode_utf16().count() as u32
}

/// The end of the text, where an editor appends.
fn append(d: &Device, note: &Note, more: &str) {
    let at = units(&text(d, note));
    d.splice_utf16(note.id.clone(), at, 0, more.into()).unwrap();
}

fn storage_path(storage: &TempDir, dir: &str, name: &str) -> PathBuf {
    storage.path().join(ROOT_DIR).join(dir).join(name)
}

/// The log files (not the marks) of the storage, by name.
fn log_files(storage: &TempDir) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(storage.path().join(ROOT_DIR).join("log"))
        .map(|entries| {
            entries
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .filter(|n| n.ends_with(".jsonl"))
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

fn files_of(storage: &TempDir, device: &Device) -> Vec<String> {
    let id = device.device_id();
    log_files(storage).into_iter().filter(|n| n.starts_with(&id)).collect()
}

/// A log file as its header and the lines after it.
fn read_log(path: &PathBuf) -> (Value, Vec<Value>) {
    let text = std::fs::read_to_string(path).unwrap();
    let mut lines = text.lines().map(|l| serde_json::from_str::<Value>(l).unwrap());
    let header = lines.next().unwrap();
    (header, lines.collect())
}

/// Writes a log file again with the header's count set to the lines given.
fn write_log(path: &PathBuf, header: &Value, lines: &[Value]) {
    let mut header = header.clone();
    header["count"] = json!(lines.len());
    let mut out = header.to_string();
    for line in lines {
        out.push('\n');
        out.push_str(&line.to_string());
    }
    out.push('\n');
    std::fs::write(path, out).unwrap();
}

fn is_body(line: &Value) -> bool {
    line.get("crdt").is_some()
}

fn bytes_of(line: &Value) -> Vec<u8> {
    base64::engine::general_purpose::STANDARD
        .decode(line["crdt"].as_str().unwrap())
        .unwrap()
}

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// The lines of the snapshot a device wrote, after its header.
fn snapshot_lines(storage: &TempDir, device: &Device) -> Vec<Value> {
    use std::io::Read;
    let dir = storage.path().join(ROOT_DIR).join("snap");
    let name = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .find(|n| n.starts_with(&device.device_id()))
        .expect("the device wrote a snapshot");
    let data = std::fs::read(dir.join(name)).unwrap();
    let mut out = String::new();
    flate2::read::GzDecoder::new(&data[..])
        .read_to_string(&mut out)
        .unwrap();
    out.lines()
        .skip(1)
        .map(|l| serde_json::from_str::<Value>(l).unwrap())
        .collect()
}

fn millis(moment: &str) -> i64 {
    chrono::NaiveDateTime::parse_from_str(moment, "%Y-%m-%dT%H:%M:%S")
        .unwrap()
        .and_utc()
        .timestamp_millis()
}

fn activity_log(d: &Device) -> String {
    d.log_files()
        .iter()
        .map(|p| std::fs::read_to_string(p).unwrap())
        .collect::<Vec<_>>()
        .join("\n")
}

// ---- S3: merge of the text ----

#[test]
fn s3_a_new_note_has_its_text_and_the_other_device_gets_it() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b) = (device(), device());
    let n = note_with(&a, "заметка", "первая строка\nвторая");
    assert_eq!(text(&a, &n), "первая строка\nвторая");
    assert_eq!(text(&a, &add(&a, "пустая")), "");
    settle(&a, &b, &storage);
    assert_eq!(text(&b, &n), "первая строка\nвторая");
}

#[test]
fn s3_offline_edits_at_different_places_are_both_kept() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b) = (device(), device());
    let n = note_with(&a, "заметка", "one two three");
    settle(&a, &b, &storage);

    a.splice_utf16(n.id.clone(), 0, 0, "alpha ".into()).unwrap();
    append(&b, &n, " omega");
    settle(&a, &b, &storage);
    for d in [&a, &b] {
        assert_eq!(text(d, &n), "alpha one two three omega");
    }
}

#[test]
fn s3_edits_at_the_same_place_are_both_kept_in_the_same_order_on_both_devices() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b) = (device(), device());
    let n = note_with(&a, "заметка", "start end");
    settle(&a, &b, &storage);

    a.splice_utf16(n.id.clone(), 6, 0, "AAA ".into()).unwrap();
    b.splice_utf16(n.id.clone(), 6, 0, "BBB ".into()).unwrap();
    settle(&a, &b, &storage);
    let merged = text(&a, &n);
    assert_eq!(merged, text(&b, &n));
    assert!(merged.contains("AAA ") && merged.contains("BBB "), "{merged}");
    assert!(merged.starts_with("start ") && merged.ends_with("end"), "{merged}");
}

#[test]
fn s3_a_deleted_paragraph_and_a_word_edited_inside_it_converge() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b) = (device(), device());
    let body = "first\n\nsecond paragraph here\n\nthird";
    let n = note_with(&a, "заметка", body);
    settle(&a, &b, &storage);

    // A deletes the whole second paragraph and the blank line after it.
    let start = body.find("second").unwrap() as u32;
    let length = "second paragraph here\n\n".len() as u32;
    a.splice_utf16(n.id.clone(), start, length, String::new()).unwrap();
    // B replaces a word inside it.
    let word = body.find("paragraph").unwrap() as u32;
    b.splice_utf16(n.id.clone(), word, "paragraph".len() as u32, "chapter".into())
        .unwrap();
    settle(&a, &b, &storage);
    let merged = text(&a, &n);
    assert_eq!(merged, text(&b, &n));
    assert!(merged.starts_with("first\n\n") && merged.ends_with("third"), "{merged}");
    assert!(!merged.contains("second") && !merged.contains("here"), "{merged}");
}

#[test]
fn s3_the_same_edits_delivered_in_reverse_order_give_the_same_text() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b, c1, c2) = (device(), device(), device(), device());
    let n = note_with(&a, "заметка", "one two three");
    settle(&a, &b, &storage);
    a.splice_utf16(n.id.clone(), 0, 0, "alpha ".into()).unwrap();
    append(&b, &n, " omega");
    sync(&a, &storage);
    sync(&b, &storage);

    // C1 gets the edit of A before the edit of B, C2 the other way round.
    let a_second = storage_path(&storage, "log", &format!("{}-0000000002.jsonl", a.device_id()));
    let b_second = storage_path(&storage, "log", &format!("{}-0000000001.jsonl", b.device_id()));
    let aside = storage.path().join("aside");
    std::fs::rename(&b_second, &aside).unwrap();
    sync(&c1, &storage);
    std::fs::rename(&aside, &b_second).unwrap();
    sync(&c1, &storage);

    std::fs::rename(&a_second, &aside).unwrap();
    sync(&c2, &storage);
    std::fs::rename(&aside, &a_second).unwrap();
    sync(&c2, &storage);

    sync(&a, &storage);
    for d in [&a, &b, &c1, &c2] {
        assert_eq!(text(d, &n), "alpha one two three omega");
    }
}

#[test]
fn s3_a_snapshot_that_repeats_what_the_log_gave_changes_nothing_twice() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b) = (device(), device());
    let n = note_with(&a, "заметка", "one");
    sync(&a, &storage);
    sync(&b, &storage);
    append(&a, &n, " two");
    a.set_compact_after_for_tests(0);
    sync(&a, &storage);
    assert!(files_of(&storage, &a).is_empty(), "the log of A is in its snapshot now");

    // The snapshot holds the whole document, with the changes B has already: nothing is doubled.
    assert!(sync(&b, &storage).pulled > 0);
    assert_eq!(text(&b, &n), "one two");
    assert_eq!(b.body_changes_for_tests(&n.id), a.body_changes_for_tests(&n.id));
    assert_eq!(sync(&b, &storage).pulled, 0);
    assert_eq!(text(&b, &n), "one two");
}

/// A tiny generator, so that a failure can be repeated.
fn next(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

/// A random edit that falls on a boundary between characters, with emoji among them.
fn random_edit(d: &Device, n: &Note, rnd: &mut u64) {
    let current = text(d, n);
    let chars: Vec<char> = current.chars().collect();
    let offset = |upto: usize| chars[..upto].iter().map(|c| c.len_utf16() as u32).sum::<u32>();
    let from = (next(rnd) as usize) % (chars.len() + 1);
    let to = (from + (next(rnd) as usize) % 3).min(chars.len());
    let alphabet = ["a", "b", "😀", "ж", "\n", " "];
    let insert = alphabet[(next(rnd) as usize) % alphabet.len()].repeat(1 + (next(rnd) as usize) % 2);
    d.splice_utf16(n.id.clone(), offset(from), offset(to) - offset(from), insert)
        .unwrap();
}

#[test]
fn s3_random_offline_edits_converge_whatever_the_order_of_syncs() {
    for seed in 1..=6u64 {
        let storage = tempfile::tempdir().unwrap();
        let devices = [device(), device(), device()];
        let mut rnd = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        let n = note_with(&devices[0], "общая", "начало 😀 конец");
        settle(&devices[0], &devices[1], &storage);
        sync(&devices[2], &storage);
        for _ in 0..40 {
            let d = &devices[(next(&mut rnd) as usize) % 3];
            if next(&mut rnd).is_multiple_of(3) {
                sync(d, &storage);
            } else {
                random_edit(d, &n, &mut rnd);
            }
        }
        for _ in 0..2 {
            for d in &devices {
                sync(d, &storage);
            }
        }
        let first = text(&devices[0], &n);
        for d in &devices {
            assert_eq!(text(d, &n), first, "seed {seed}");
        }
    }
}

#[test]
fn s3_two_body_objects_for_one_note_are_settled_the_same_way_on_every_device() {
    // Two devices that each made a `body` object for the same note: the case the format allows
    // but the core never makes, since only the device that creates a note makes the object.
    fn crafted(actor: u8, text: &str) -> Vec<u8> {
        let mut doc =
            AutoCommit::new_with_encoding(TextEncoding::Utf16CodeUnit).with_actor(ActorId::from(vec![actor; 16]));
        let obj = doc.put_object(ROOT, "body", ObjType::Text).unwrap();
        doc.splice_text(&obj, 0, 0, text).unwrap();
        doc.commit_with(CommitOptions::default().with_time(1));
        doc.save()
    }
    fn plant(storage: &TempDir, device: &str, note: &str, doc: &[u8]) {
        let line = json!({"k": "note", "i": note, "f": "body", "crdt": b64(doc),
                          "t": format!("0000000fffff-0000-{device}")});
        let header = json!({"v": 1, "device": device, "seq": 1, "count": 1});
        write_log(
            &storage_path(storage, "log", &format!("{device}-0000000001.jsonl")),
            &header,
            &[line],
        );
    }

    let storage = tempfile::tempdir().unwrap();
    let a = device();
    let n = note_with(&a, "заметка", "from A");
    sync(&a, &storage);
    // X has the smallest actor, Y the greatest; the object of A lies between them.
    plant(&storage, "000000000001", &n.id, &crafted(0x00, "from X"));
    plant(&storage, "000000000002", &n.id, &crafted(0xff, "from Y"));

    let (b, c1, c2) = (device(), device(), device());
    sync(&b, &storage);
    // C1 reads Y and A before X, C2 reads X alone before the rest.
    let hide = |name: &str| {
        let from = storage_path(&storage, "log", name);
        let to = storage.path().join(format!("aside-{name}"));
        std::fs::rename(&from, &to).unwrap();
        move || std::fs::rename(&to, &from).unwrap()
    };
    let show_x = hide("000000000001-0000000001.jsonl");
    sync(&c1, &storage);
    show_x();
    sync(&c1, &storage);
    let show_a = hide(&format!("{}-0000000001.jsonl", a.device_id()));
    let show_y = hide("000000000002-0000000001.jsonl");
    sync(&c2, &storage);
    show_a();
    show_y();
    sync(&c2, &storage);
    sync(&a, &storage);

    for d in [&a, &b, &c1, &c2] {
        assert_eq!(text(d, &n), "from X");
    }
    // An edit after the merge lands in the object everyone shows.
    append(&a, &n, "!");
    sync(&a, &storage);
    sync(&b, &storage);
    assert_eq!(text(&b, &n), "from X!");
}

#[test]
fn s3_a_device_that_did_not_create_the_text_object_does_not_make_one() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b) = (device(), device());
    let n = note_with(&a, "заметка", "текст");
    sync(&a, &storage);
    // The file reaches B without the line of the text.
    let path = storage_path(&storage, "log", &files_of(&storage, &a)[0]);
    let (header, lines) = read_log(&path);
    let rest: Vec<Value> = lines.into_iter().filter(|l| !is_body(l)).collect();
    write_log(&path, &header, &rest);

    sync(&b, &storage);
    assert_eq!(b.get_note(n.id.clone()).unwrap().title, "заметка");
    assert_eq!(text(&b, &n), "");
    let refused = b.splice_utf16(n.id.clone(), 0, 0, "x".into()).unwrap_err();
    assert!(matches!(refused, AppError::Invalid { .. }), "{refused:?}");
    assert!(b.set_text(n.id.clone(), "x".into()).is_err());
    assert!(!b.has_body_for_tests(&n.id));
}

#[test]
fn r1_r4_a_note_starts_with_its_text_and_a_duplicate_has_it() {
    let d = device();
    let n = note_with(&d, "исходная", "# Заголовок\n\nтекст 😀");
    assert_eq!(text(&d, &n), "# Заголовок\n\nтекст 😀");
    let copy = d.duplicate_note(n.id.clone()).unwrap();
    assert_eq!(text(&d, &copy), "# Заголовок\n\nтекст 😀");
    d.set_text(copy.id.clone(), "другой".into()).unwrap();
    assert_eq!(text(&d, &n), "# Заголовок\n\nтекст 😀");
}

#[test]
fn s13_the_moment_of_the_last_edit_includes_the_text() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b) = (device(), device());
    let n = note_with(&a, "заметка", "x");
    let created = a.get_note(n.id.clone()).unwrap().edited_ms;
    a.set_now_for_tests("2026-10-05T12:30:00");
    append(&a, &n, "y");
    let edited = a.get_note(n.id.clone()).unwrap().edited_ms;
    assert_eq!(edited, millis("2026-10-05T12:30:00") as u64);
    assert!(edited > created);
    settle(&a, &b, &storage);
    assert_eq!(b.get_note(n.id.clone()).unwrap().edited_ms, edited);
}

// ---- S13, S20: the log ----

#[test]
fn s13_a_body_line_is_written_with_the_names_of_the_registers_and_the_header_counts_it() {
    let storage = tempfile::tempdir().unwrap();
    let a = device();
    let n = note_with(&a, "заметка", "текст");
    sync(&a, &storage);
    let (header, lines) = read_log(&storage_path(&storage, "log", &files_of(&storage, &a)[0]));
    assert_eq!(header["count"], lines.len());
    let bodies: Vec<&Value> = lines.iter().filter(|l| is_body(l)).collect();
    assert_eq!(bodies.len(), 1);
    let line = bodies[0];
    assert_eq!(
        (line["k"].as_str(), line["i"].as_str(), line["f"].as_str()),
        (Some("note"), Some(n.id.as_str()), Some("body"))
    );
    assert!(line["t"].as_str().is_some_and(|t| t.len() == 30));
    assert!(!bytes_of(line).is_empty());
    assert!(lines.iter().any(|l| !is_body(l) && l["f"] == "title"));
}

#[test]
fn s13_a_log_file_cut_in_the_middle_of_a_body_line_is_not_passed_and_is_read_when_whole() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b) = (device(), device());
    let n = note_with(&a, "заметка", &"строка текста ".repeat(20));
    sync(&a, &storage);

    let path = storage_path(&storage, "log", &files_of(&storage, &a)[0]);
    let whole = std::fs::read(&path).unwrap();
    let body_at = String::from_utf8_lossy(&whole).find("\"crdt\"").unwrap();
    // Cut inside the base64 of the body line.
    std::fs::write(&path, &whole[..body_at + 40]).unwrap();
    assert_eq!(sync(&b, &storage).pulled, 0);
    assert!(
        b.get_note(n.id.clone()).is_err(),
        "nothing of a half-written file is applied"
    );
    assert!(!b.has_body_for_tests(&n.id));

    std::fs::write(&path, &whole).unwrap();
    assert!(sync(&b, &storage).pulled > 0);
    assert_eq!(text(&b, &n), "строка текста ".repeat(20));
}

#[test]
fn s20_a_pass_uploads_one_body_line_per_edited_note_with_only_the_edits_since_the_last_upload() {
    let storage = tempfile::tempdir().unwrap();
    let a = device();
    let first = note_with(&a, "первая", "основа");
    let second = note_with(&a, "вторая", "другая");
    sync(&a, &storage);
    let files = files_of(&storage, &a);
    let (_, lines) = read_log(&storage_path(&storage, "log", &files[0]));
    assert_eq!(
        lines.iter().filter(|l| is_body(l)).count(),
        2,
        "one line for each new note"
    );
    let base = lines
        .iter()
        .find(|l| l["i"] == first.id.as_str() && is_body(l))
        .unwrap();

    // Two commits on one note become one line; the other note is not uploaded again.
    append(&a, &first, " раз");
    append(&a, &first, " два");
    sync(&a, &storage);
    let files = files_of(&storage, &a);
    assert_eq!(files.len(), 2);
    let (_, lines) = read_log(&storage_path(&storage, "log", &files[1]));
    let bodies: Vec<&Value> = lines.iter().filter(|l| is_body(l)).collect();
    assert_eq!(bodies.len(), 1);
    assert_eq!(bodies[0]["i"], first.id.as_str());
    assert_ne!(bodies[0]["i"], second.id.as_str());

    // Alone, the line depends on the version before it: it holds nothing older.
    let mut alone = AutoCommit::new();
    alone.load_incremental(&bytes_of(bodies[0])).unwrap();
    assert!(!alone.get_missing_deps(&[]).is_empty());
    // On top of the first line it brings exactly the two commits.
    let mut doc = AutoCommit::new();
    doc.load_incremental(&bytes_of(base)).unwrap();
    let before = doc.get_changes_meta(&[]).len();
    doc.load_incremental(&bytes_of(bodies[0])).unwrap();
    assert_eq!(doc.get_changes_meta(&[]).len(), before + 2);
    assert!(doc.get_missing_deps(&[]).is_empty());

    // Nothing edited: no line, no file.
    sync(&a, &storage);
    assert_eq!(files_of(&storage, &a).len(), 2);
}

#[test]
fn s20_a_new_note_reaches_the_other_device_with_its_text_in_one_pass() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b) = (device(), device());
    let n = note_with(&a, "новая", "текст новой заметки");
    sync(&a, &storage);
    assert_eq!(files_of(&storage, &a).len(), 1, "registers and text share a file");

    let report = sync(&b, &storage);
    assert!(report.pulled > 0);
    assert_eq!(b.get_note(n.id.clone()).unwrap().title, "новая");
    assert_eq!(text(&b, &n), "текст новой заметки");
}

/// A storage that refuses writes of log files while `fail` is set.
struct Flaky {
    inner: DirRemote,
    fail: AtomicBool,
}

impl Remote for Flaky {
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
        if path.contains("/log/") && self.fail.load(Ordering::SeqCst) {
            return Err(AppError::Sync {
                msg: "503 Service Unavailable".into(),
            });
        }
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
fn s20_an_upload_that_failed_is_repeated_with_the_same_bytes_and_no_edit_goes_twice() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b) = (device(), device());
    let n = note_with(&a, "заметка", "один");
    let remote = Flaky {
        inner: DirRemote::new(storage.path()),
        fail: AtomicBool::new(true),
    };
    assert!(a.sync_with(&remote).is_err());
    // An edit made while the file waits goes into the next file, not into the one that is fixed.
    append(&a, &n, " два");
    remote.fail.store(false, Ordering::SeqCst);
    a.sync_with(&remote).unwrap();
    assert_eq!(a.sync_status().unwrap().pending, 0);

    let files = files_of(&storage, &a);
    assert_eq!(files.len(), 2, "{files:?}");
    let in_first = read_log(&storage_path(&storage, "log", &files[0]))
        .1
        .iter()
        .filter(|l| is_body(l))
        .count();
    let in_second = read_log(&storage_path(&storage, "log", &files[1]))
        .1
        .iter()
        .filter(|l| is_body(l))
        .count();
    assert_eq!((in_first, in_second), (1, 1));
    sync(&b, &storage);
    assert_eq!(text(&b, &n), "один два");
}

// ---- S15, S21: snapshots ----

#[test]
fn s15_s21_a_snapshot_and_a_log_tail_give_the_same_text_as_the_whole_log() {
    let storage = tempfile::tempdir().unwrap();
    let (a, whole, late) = (device(), device(), device());
    a.set_compact_after_for_tests(3);
    let notes: Vec<Note> = (0..3)
        .map(|i| note_with(&a, &format!("заметка {i}"), &format!("начало {i} 😀")))
        .collect();
    sync(&a, &storage);
    sync(&whole, &storage);
    for round in 0..6 {
        for (i, n) in notes.iter().enumerate() {
            if (round + i) % 2 == 0 {
                append(&a, n, &format!(" правка {round}"));
            }
        }
        sync(&a, &storage);
        sync(&whole, &storage);
    }
    let a_logs = files_of(&storage, &a);
    assert!(a_logs.len() <= 3, "old log files are gone: {a_logs:?}");
    assert!(!snapshot_lines(&storage, &a).is_empty());

    // A device that never synced reads the snapshot and the tail.
    sync(&late, &storage);
    for n in &notes {
        assert_eq!(text(&late, n), text(&a, n));
        assert_eq!(text(&whole, n), text(&a, n));
        // S21: the history is whole in the snapshot, not only the text.
        assert_eq!(late.body_changes_for_tests(&n.id), a.body_changes_for_tests(&n.id));
        assert_eq!(whole.body_changes_for_tests(&n.id), a.body_changes_for_tests(&n.id));
    }
}

#[test]
fn s15_s21_a_snapshot_carries_each_text_whole_and_none_of_a_note_deleted_for_good() {
    let storage = tempfile::tempdir().unwrap();
    let a = device();
    a.set_compact_after_for_tests(0);
    let kept = note_with(&a, "остаётся", "текст остаётся");
    let gone = note_with(&a, "уходит", "текст уходит");
    sync(&a, &storage);
    a.delete_note(gone.id.clone()).unwrap();
    a.purge_note(gone.id.clone()).unwrap();
    sync(&a, &storage);

    let lines = snapshot_lines(&storage, &a);
    let bodies: Vec<&Value> = lines.iter().filter(|l| is_body(l)).collect();
    assert_eq!(bodies.len(), 1);
    assert_eq!(bodies[0]["i"], kept.id.as_str());
    // A whole saved document: it reads without any other file.
    let doc = AutoCommit::load(&bytes_of(bodies[0])).unwrap();
    let obj = doc.get(ROOT, "body").unwrap().unwrap().1;
    assert_eq!(doc.text(&obj).unwrap(), "текст остаётся");
    assert!(doc.get_missing_deps(&[]).is_empty());
}

// ---- S22: damaged bytes and missing dependencies ----

#[test]
fn s22_a_body_change_with_damaged_bytes_is_kept_and_the_rest_of_the_file_applies() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b) = (device(), device());
    let damaged = note_with(&a, "с порчей", "прежний текст");
    let fine = note_with(&a, "целая", "прежний текст");
    settle(&a, &b, &storage);

    append(&a, &damaged, " СЕКРЕТНАЯ ПРАВКА");
    append(&a, &fine, " СЕКРЕТНАЯ ПРАВКА");
    a.set_title(fine.id.clone(), "целая, переименована".into()).unwrap();
    sync(&a, &storage);

    let path = storage_path(&storage, "log", files_of(&storage, &a).last().unwrap());
    let (header, mut lines) = read_log(&path);
    for line in lines.iter_mut().filter(|l| is_body(l) && l["i"] == damaged.id.as_str()) {
        // Valid base64 of bytes that are no Automerge change.
        line["crdt"] = json!(b64(b"these bytes are not automerge changes"));
    }
    write_log(&path, &header, &lines);

    let report = sync(&b, &storage);
    assert!(report.pulled > 0);
    assert_eq!(text(&b, &damaged), "прежний текст", "the note keeps its previous text");
    assert_eq!(
        text(&b, &fine),
        "прежний текст СЕКРЕТНАЯ ПРАВКА",
        "the rest of the file applies"
    );
    assert_eq!(b.get_note(fine.id.clone()).unwrap().title, "целая, переименована");
    assert_eq!(b.body_kept_for_tests(), 1);

    let log = activity_log(&b);
    let line = log
        .lines()
        .find(|l| l.contains(&damaged.id))
        .expect("a line names the note");
    assert!(line.contains(&a.device_id()), "{line}");
    assert!(
        !log.contains("СЕКРЕТНАЯ") && !log.contains("прежний"),
        "no text in the log: {log}"
    );
    assert!(!log.contains(&fine.id));

    // The file was passed: the next edit of A arrives, the kept change is not duplicated or lost.
    append(&a, &fine, "!");
    sync(&a, &storage);
    sync(&b, &storage);
    assert_eq!(text(&b, &fine), "прежний текст СЕКРЕТНАЯ ПРАВКА!");
    assert_eq!(b.body_kept_for_tests(), 1);
}

#[test]
fn s22_a_change_that_is_not_base64_is_kept_as_well_and_survives_a_restart() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b) = (device(), device());
    let n = note_with(&a, "заметка", "было");
    settle(&a, &b, &storage);
    append(&a, &n, " стало");
    sync(&a, &storage);
    let path = storage_path(&storage, "log", files_of(&storage, &a).last().unwrap());
    let (header, mut lines) = read_log(&path);
    for line in lines.iter_mut().filter(|l| is_body(l)) {
        line["crdt"] = json!("!!! not base64 !!!");
    }
    write_log(&path, &header, &lines);

    sync(&b, &storage);
    assert_eq!(text(&b, &n), "было");
    assert_eq!(b.body_kept_for_tests(), 1);
    let b = reopen(b);
    assert_eq!(b.body_kept_for_tests(), 1, "kept on disk, not in memory");
    append(&a, &n, "!");
    sync(&a, &storage);
    sync(&b, &storage);
    assert_eq!(
        b.body_kept_for_tests(),
        1,
        "retried after the next file, still not applicable, not doubled"
    );
}

#[test]
fn s22_a_cut_off_stream_and_a_flipped_byte_are_damaged_changes_too() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b) = (device(), device());
    let n = note_with(&a, "заметка", "было");
    settle(&a, &b, &storage);

    let damage: [fn(Vec<u8>) -> Vec<u8>; 2] = [
        |bytes| bytes[..bytes.len() / 2].to_vec(),
        |mut bytes| {
            let middle = bytes.len() / 2;
            bytes[middle] ^= 0xff;
            bytes
        },
    ];
    for (round, damage) in damage.iter().enumerate() {
        append(&a, &n, &format!(" правка {round}"));
        sync(&a, &storage);
        let path = storage_path(&storage, "log", files_of(&storage, &a).last().unwrap());
        let (header, mut lines) = read_log(&path);
        for line in lines.iter_mut().filter(|l| is_body(l)) {
            line["crdt"] = json!(b64(&damage(bytes_of(line))));
        }
        write_log(&path, &header, &lines);
        sync(&b, &storage);
        assert_eq!(text(&b, &n), "было", "round {round}");
        assert_eq!(b.body_kept_for_tests(), round as u32 + 1);
    }
}

/// A log file of a device that does not exist, with one body line of its own.
fn plant_body(storage: &TempDir, device: &str, note: &str, changes: &[u8]) {
    let line = json!({"k": "note", "i": note, "f": "body", "crdt": b64(changes),
                      "t": format!("0000000fffff-0000-{device}")});
    let header = json!({"v": 1, "device": device, "seq": 1, "count": 1});
    write_log(
        &storage_path(storage, "log", &format!("{device}-0000000001.jsonl")),
        &header,
        &[line],
    );
}

/// The body line of a file of A: the changes of one upload.
fn body_bytes(storage: &TempDir, device: &Device, seq: usize) -> Vec<u8> {
    let name = format!("{}-{seq:010}.jsonl", device.device_id());
    let (_, lines) = read_log(&storage_path(storage, "log", &name));
    bytes_of(lines.iter().find(|l| is_body(l)).unwrap())
}

#[test]
fn s22_changes_with_missing_dependencies_wait_in_the_document_across_a_restart() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b) = (device(), device());
    let n = note_with(&a, "заметка", "первое");
    sync(&a, &storage);
    append(&a, &n, " второе");
    sync(&a, &storage);
    append(&a, &n, " третье");
    sync(&a, &storage);

    // B reads the first file of A. The third edit reaches it by another way, before the second.
    let hidden: Vec<PathBuf> = [2, 3]
        .iter()
        .map(|seq| storage_path(&storage, "log", &format!("{}-{seq:010}.jsonl", a.device_id())))
        .collect();
    let third = body_bytes(&storage, &a, 3);
    let aside: Vec<PathBuf> = (0..2).map(|i| storage.path().join(format!("aside-{i}"))).collect();
    for (from, to) in hidden.iter().zip(&aside) {
        std::fs::rename(from, to).unwrap();
    }
    plant_body(&storage, "000000000003", &n.id, &third);
    sync(&b, &storage);
    assert_eq!(text(&b, &n), "первое", "the text shows what can be applied");
    assert_eq!(b.body_missing_for_tests(&n.id), 1);
    assert_eq!(b.body_kept_for_tests(), 0, "changes that wait are not damaged ones");
    let log = activity_log(&b);
    let line = log
        .lines()
        .find(|l| l.contains(&n.id))
        .expect("the fact is in the activity log");
    assert!(line.contains("000000000003"), "{line}");
    assert!(!log.contains("третье") && !log.contains("первое"), "no text in the log");

    let b = reopen(b);
    assert_eq!(text(&b, &n), "первое");
    assert_eq!(b.body_missing_for_tests(&n.id), 1, "the queue is in the saved document");

    // The second edit arrives: the third, which waited for it, is applied with it.
    std::fs::rename(&aside[0], &hidden[0]).unwrap();
    sync(&b, &storage);
    assert_eq!(text(&b, &n), "первое второе третье");
    assert_eq!(b.body_missing_for_tests(&n.id), 0);
    std::fs::rename(&aside[1], &hidden[1]).unwrap();
    sync(&b, &storage);
    sync(&a, &storage);
    assert_eq!(text(&b, &n), "первое второе третье");
    assert_eq!(text(&a, &n), "первое второе третье");
}

#[test]
fn s22_changes_that_come_before_the_text_they_build_on_wait_for_it() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b) = (device(), device());
    let n = note_with(&a, "заметка", "первое");
    sync(&a, &storage);
    append(&a, &n, " второе");
    sync(&a, &storage);
    let second = body_bytes(&storage, &a, 2);

    // B has nothing of this note yet and gets the second upload of A alone.
    let first = storage_path(&storage, "log", &format!("{}-{:010}.jsonl", a.device_id(), 1));
    let aside = storage.path().join("aside");
    std::fs::rename(&first, &aside).unwrap();
    std::fs::rename(
        storage_path(&storage, "log", &format!("{}-{:010}.jsonl", a.device_id(), 2)),
        storage.path().join("aside-2"),
    )
    .unwrap();
    plant_body(&storage, "000000000003", &n.id, &second);
    sync(&b, &storage);
    assert!(b.get_note(n.id.clone()).is_err(), "no registers of the note yet");
    assert!(b.has_body_for_tests(&n.id));
    assert_eq!(b.body_missing_for_tests(&n.id), 1);

    let b = reopen(b);
    std::fs::rename(&aside, &first).unwrap();
    std::fs::rename(
        storage.path().join("aside-2"),
        storage_path(&storage, "log", &format!("{}-{:010}.jsonl", a.device_id(), 2)),
    )
    .unwrap();
    sync(&b, &storage);
    assert_eq!(text(&b, &n), "первое второе");
    assert_eq!(b.body_missing_for_tests(&n.id), 0);
}

// ---- S4: the actor ----

#[test]
fn s4_the_actor_is_16_random_bytes_kept_across_restarts_and_on_every_edit() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b) = (device(), device());
    let actor = a.actor_id_for_tests();
    assert_eq!(actor.len(), 32);
    assert_ne!(actor, b.actor_id_for_tests());

    let n = note_with(&a, "заметка", "x");
    let a = reopen(a);
    assert_eq!(a.actor_id_for_tests(), actor);
    append(&a, &n, "y");
    sync(&a, &storage);
    sync(&b, &storage);
    // The document of B was loaded from the log; B's own edit carries B's actor.
    append(&b, &n, "z");
    let changes = b.body_changes_for_tests(&n.id);
    assert_eq!(changes.last().unwrap().0, b.actor_id_for_tests());
    assert!(changes
        .iter()
        .all(|(who, _)| *who == actor || *who == b.actor_id_for_tests()));
    assert_eq!(changes.iter().filter(|(who, _)| *who == actor).count(), 2);
}

#[test]
fn s4_s36_a_new_device_identifier_takes_a_new_actor_and_its_edits_merge_with_the_old_ones() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b) = (device(), device());
    let n = note_with(&a, "заметка", "one");
    append(&a, &n, " two");
    settle(&a, &b, &storage);
    let old_actor = a.actor_id_for_tests();
    let old_device = a.device_id();

    a.replace_local_with_remote().unwrap();
    assert_ne!(a.device_id(), old_device);
    assert_ne!(a.actor_id_for_tests(), old_actor);
    assert_eq!(a.actor_id_for_tests().len(), 32);
    assert!(!a.has_body_for_tests(&n.id), "the texts are erased with the rest (S36)");

    append(&b, &n, " three");
    sync(&b, &storage);
    sync(&a, &storage);
    assert_eq!(text(&a, &n), "one two three");
    append(&a, &n, " four");
    settle(&a, &b, &storage);
    for d in [&a, &b] {
        assert_eq!(text(d, &n), "one two three four");
    }
    let who: Vec<String> = a.body_changes_for_tests(&n.id).into_iter().map(|c| c.0).collect();
    assert!(who.contains(&old_actor), "the history of the old actor is read back");
    assert_eq!(who.last().unwrap(), &a.actor_id_for_tests());
}

// ---- S7: deleting for good ----

#[test]
fn s7_a_note_purged_on_one_device_loses_its_document_on_the_other() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b) = (device(), device());
    let gone = note_with(&a, "уходит", "текст уходит");
    let kept = note_with(&a, "остаётся", "текст остаётся");
    settle(&a, &b, &storage);
    assert!(b.has_body_for_tests(&gone.id));

    a.delete_note(gone.id.clone()).unwrap();
    a.purge_note(gone.id.clone()).unwrap();
    assert!(
        !a.has_body_for_tests(&gone.id),
        "the device that purges drops it at once"
    );
    settle(&a, &b, &storage);
    assert!(!b.has_body_for_tests(&gone.id));
    assert!(b.get_text(gone.id.clone()).is_err());
    assert!(b.has_body_for_tests(&kept.id));
    assert_eq!(text(&b, &kept), "текст остаётся");
}

#[test]
fn s7_an_edit_that_arrives_after_the_purge_does_not_bring_the_document_back() {
    let storage = tempfile::tempdir().unwrap();
    let (a, c) = (device(), device());
    let (b1, b2) = (device(), device());
    let n = note_with(&a, "заметка", "текст");
    sync(&a, &storage);
    for d in [&b1, &b2, &c] {
        sync(d, &storage);
    }
    // C edits and uploads; A, which has not read that file yet, purges the note.
    append(&c, &n, " правка с C");
    sync(&c, &storage);
    a.delete_note(n.id.clone()).unwrap();
    a.purge_note(n.id.clone()).unwrap();
    sync(&a, &storage);
    let hide = |name: String| {
        let from = storage_path(&storage, "log", &name);
        let to = storage.path().join(format!("aside-{name}"));
        std::fs::rename(&from, &to).unwrap();
        move || std::fs::rename(&to, &from).unwrap()
    };

    // B1 reads the purge, then the edit; B2 the edit, then the purge.
    let show = hide(format!("{}-0000000001.jsonl", c.device_id()));
    sync(&b1, &storage);
    assert!(!b1.has_body_for_tests(&n.id));
    show();
    sync(&b1, &storage);
    let show = hide(format!("{}-0000000002.jsonl", a.device_id()));
    sync(&b2, &storage);
    show();
    sync(&b2, &storage);
    for d in [&a, &b1, &b2] {
        assert!(!d.has_body_for_tests(&n.id));
        assert!(d.get_text(n.id.clone()).is_err());
    }
}

#[test]
fn s7_s8_the_trash_expiry_drops_the_document() {
    let storage = tempfile::tempdir().unwrap();
    let a = device();
    let n = note_with(&a, "старая", "текст");
    a.delete_note(n.id.clone()).unwrap();
    assert!(a.has_body_for_tests(&n.id), "a note in the Trash keeps its text");
    a.set_now_for_tests("2026-12-05T10:00");
    sync(&a, &storage);
    assert!(view(&a, Scope::Trash).is_empty());
    assert!(!a.has_body_for_tests(&n.id));
}

// ---- S36–S38 with texts ----

#[test]
fn s36_taking_the_storage_erases_the_texts_and_reads_them_again() {
    let storage = tempfile::tempdir().unwrap();
    let (a, c) = (device(), device());
    let shared = note_with(&a, "общая", "текст общей");
    sync(&a, &storage);
    let held = note_with(&c, "в", "текст в");
    let before = c.actor_id_for_tests();

    c.replace_local_with_remote().unwrap();
    assert!(!c.has_body_for_tests(&held.id));
    assert_ne!(c.actor_id_for_tests(), before);
    settle(&c, &a, &storage);
    assert_eq!(text(&c, &shared), "текст общей");
    assert!(c.get_text(held.id.clone()).is_err());

    append(&c, &shared, " + C");
    settle(&c, &a, &storage);
    assert_eq!(text(&a, &shared), "текст общей + C");
}

#[test]
fn s37_sending_the_device_writes_its_remembered_text_over_the_storage() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b) = (device(), device());
    let n = note_with(&a, "общая", "one");
    settle(&a, &b, &storage);
    // A goes on; B holds the earlier text, like a device restored from a backup.
    let only_a = note_with(&a, "только на A", "text only A has");
    a.splice_utf16(n.id.clone(), 0, 0, "A says ".into()).unwrap();
    sync(&a, &storage);
    b.replace_remote_with_local().unwrap();
    sync(&b, &storage);
    sync(&a, &storage);
    for d in [&a, &b] {
        assert_eq!(text(d, &n), "one", "the text of the device wins");
    }
    // What was only elsewhere lies in the Trash with its text.
    assert!(b.get_note(only_a.id.clone()).unwrap().deleted);
    assert_eq!(text(&b, &only_a), "text only A has");
    // Nothing waits any more: the next run is an ordinary one.
    assert_eq!(sync(&b, &storage), SyncReport::default());
}

#[test]
fn s38_a_backup_brings_the_texts_back_marked_for_upload_under_a_new_actor() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b) = (device(), device());
    let n = note_with(&a, "заметка", "alpha");
    let m = note_with(&a, "вторая", "beta");
    let backup = a.create_backup().unwrap();
    append(&a, &n, " gamma");
    let (actor, device_id) = (a.actor_id_for_tests(), a.device_id());

    a.restore_backup(backup.path, false).unwrap();
    assert_ne!(a.actor_id_for_tests(), actor);
    assert_ne!(a.device_id(), device_id);
    assert_eq!(text(&a, &n), "alpha");
    assert_eq!(text(&a, &m), "beta");

    sync(&a, &storage);
    sync(&b, &storage);
    assert_eq!(text(&b, &n), "alpha");
    assert_eq!(text(&b, &m), "beta");
    // An edit after the restore is by the new actor and merges.
    append(&b, &n, " from B");
    append(&a, &n, " from A");
    settle(&a, &b, &storage);
    assert_eq!(text(&a, &n), text(&b, &n));
    assert!(text(&a, &n).contains("from A") && text(&a, &n).contains("from B"));
}

// ---- S70: positions and time ----

#[test]
fn s70_a_position_after_an_emoji_lands_after_it() {
    let d = device();
    let n = note_with(&d, "заметка", "a😀b");
    // U+1F600 is two UTF-16 units: the position after it is 3.
    d.splice_utf16(n.id.clone(), 3, 0, "-".into()).unwrap();
    assert_eq!(text(&d, &n), "a😀-b");
    d.splice_utf16(n.id.clone(), 1, 2, String::new()).unwrap();
    assert_eq!(text(&d, &n), "a-b");
    d.splice_utf16(n.id.clone(), 3, 0, "😀".into()).unwrap();
    assert_eq!(text(&d, &n), "a-b😀");
}

#[test]
fn s70_a_position_inside_a_pair_or_past_the_end_is_refused_and_the_text_does_not_change() {
    let d = device();
    let n = note_with(&d, "заметка", "a😀b");
    let changes = d.body_changes_for_tests(&n.id).len();
    for (pos, len) in [(2, 0), (1, 1), (2, 2), (5, 0), (4, 1), (u32::MAX, 1), (0, u32::MAX)] {
        let refused = d.splice_utf16(n.id.clone(), pos, len, "x".into());
        assert!(
            matches!(refused, Err(AppError::Invalid { .. })),
            "({pos}, {len}): {refused:?}"
        );
        assert_eq!(text(&d, &n), "a😀b");
    }
    assert_eq!(
        d.body_changes_for_tests(&n.id).len(),
        changes,
        "a refused edit commits nothing"
    );
    // The end of the text is a position like any other.
    d.splice_utf16(n.id.clone(), 4, 0, "!".into()).unwrap();
    assert_eq!(text(&d, &n), "a😀b!");
}

#[test]
fn s70_every_edit_is_one_change_with_the_clock_of_the_device_in_milliseconds() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b, late) = (device(), device(), device());
    a.set_now_for_tests("2026-10-05T10:00:00");
    let n = note_with(&a, "заметка", "x");
    append(&a, &n, "y");
    a.set_now_for_tests("2026-10-05T10:00:30");
    append(&a, &n, "z");
    a.set_text(n.id.clone(), "xyz!".into()).unwrap();
    // An edit that changes nothing is no change.
    a.set_text(n.id.clone(), "xyz!".into()).unwrap();
    a.splice_utf16(n.id.clone(), 0, 0, String::new()).unwrap();

    let (t0, t30) = (millis("2026-10-05T10:00:00"), millis("2026-10-05T10:00:30"));
    let times: Vec<i64> = a.body_changes_for_tests(&n.id).into_iter().map(|c| c.1).collect();
    assert_eq!(
        times,
        [t0, t0, t30, t30],
        "the first change makes the text and its first characters"
    );

    // The times travel in the log ...
    sync(&a, &storage);
    sync(&b, &storage);
    assert_eq!(b.body_changes_for_tests(&n.id), a.body_changes_for_tests(&n.id));
    // ... and in the snapshot, which a device that never synced reads.
    a.set_compact_after_for_tests(0);
    sync(&a, &storage);
    assert!(!snapshot_lines(&storage, &a).is_empty());
    assert!(files_of(&storage, &a).is_empty());
    sync(&late, &storage);
    assert_eq!(late.body_changes_for_tests(&n.id), a.body_changes_for_tests(&n.id));
}

#[test]
fn s70_a_commit_never_goes_below_the_greatest_time_in_the_document() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b) = (device(), device());
    a.set_now_for_tests("2026-10-05T10:00:30");
    let n = note_with(&a, "заметка", "x");
    sync(&a, &storage);
    sync(&b, &storage);

    // The clock of B reads half an hour earlier than the last change it received.
    b.set_now_for_tests("2026-10-05T09:30:00");
    append(&b, &n, "y");
    let last = millis("2026-10-05T10:00:30");
    assert_eq!(b.body_changes_for_tests(&n.id).last().unwrap().1, last);
    // A clock that is ahead is used as it reads.
    b.set_now_for_tests("2026-10-05T11:00:00");
    append(&b, &n, "z");
    assert_eq!(
        b.body_changes_for_tests(&n.id).last().unwrap().1,
        millis("2026-10-05T11:00:00")
    );
    // And a clock set back after that does not take it back.
    b.set_now_for_tests("2026-10-05T10:00:00");
    append(&b, &n, "!");
    assert_eq!(
        b.body_changes_for_tests(&n.id).last().unwrap().1,
        millis("2026-10-05T11:00:00")
    );
}

#[test]
fn s70_set_text_turns_the_current_text_into_the_new_one_with_surrogate_pairs_whole() {
    let d = device();
    let n = note_with(&d, "заметка", "one 😀 two");
    d.set_text(n.id.clone(), "one 😀😀 two 😀".into()).unwrap();
    assert_eq!(text(&d, &n), "one 😀😀 two 😀");
    d.set_text(n.id.clone(), String::new()).unwrap();
    assert_eq!(text(&d, &n), "");
}

// ---- R28, R45 ----

#[test]
fn r28_a_locked_note_refuses_edits_of_its_text_but_takes_those_of_other_devices() {
    let storage = tempfile::tempdir().unwrap();
    let (a, b) = (device(), device());
    let n = note_with(&a, "заметка", "text");
    settle(&a, &b, &storage);
    a.set_locked(n.id.clone(), true).unwrap();
    let changes = a.body_changes_for_tests(&n.id).len();
    assert!(matches!(
        a.splice_utf16(n.id.clone(), 0, 0, "x".into()),
        Err(AppError::Invalid { .. })
    ));
    assert!(matches!(
        a.set_text(n.id.clone(), "x".into()),
        Err(AppError::Invalid { .. })
    ));
    assert_eq!(text(&a, &n), "text");
    assert_eq!(a.body_changes_for_tests(&n.id).len(), changes);

    // Edits that come from other devices are applied.
    append(&b, &n, " from B");
    settle(&a, &b, &storage);
    assert_eq!(text(&a, &n), "text from B");
    assert!(a.splice_utf16(n.id.clone(), 0, 0, "x".into()).is_err());

    a.set_locked(n.id.clone(), false).unwrap();
    a.splice_utf16(n.id.clone(), 0, 0, "x".into()).unwrap();
    assert_eq!(text(&a, &n), "xtext from B");
}

fn text_filter(d: &Device, needle: &str) -> Vec<String> {
    let mut found: Vec<String> = d
        .preview_filter(FilterQuery {
            conditions: vec![Condition::TextContains { text: needle.into() }],
            ..FilterQuery::default()
        })
        .unwrap()
        .into_iter()
        .map(|n| n.title)
        .collect();
    found.sort();
    found
}

#[test]
fn r45_the_text_contains_selects_notes_by_their_text_whatever_the_case() {
    let d = device();
    let one = note_with(&d, "первая", "Купить МОЛОКО и хлеб");
    note_with(&d, "вторая", "ничего подходящего");
    note_with(&d, "молоко в названии", "а в тексте нет");
    let empty = add(&d, "без текста");

    assert_eq!(text_filter(&d, "молоко"), ["первая"]);
    assert_eq!(text_filter(&d, "ХЛЕБ"), ["первая"]);
    assert!(text_filter(&d, "сыр").is_empty());

    d.set_text(one.id.clone(), "теперь про сыр".into()).unwrap();
    d.splice_utf16(empty.id.clone(), 0, 0, "и немного сыра".into()).unwrap();
    assert!(text_filter(&d, "молоко").is_empty());
    assert_eq!(text_filter(&d, "сыр"), ["без текста", "первая"]);

    // A saved filter counts them, and a note deleted for good is not selected (S7).
    let saved = d
        .create_filter(
            "про сыр".into(),
            String::new(),
            FilterQuery {
                conditions: vec![Condition::TextContains { text: "сыр".into() }],
                ..FilterQuery::default()
            },
        )
        .unwrap();
    assert_eq!(saved.count, 2);
    d.delete_note(one.id.clone()).unwrap();
    d.purge_note(one.id.clone()).unwrap();
    assert_eq!(d.filters().unwrap()[0].count, 1);
    assert_eq!(view(&d, Scope::Filter { id: saved.id }), ["без текста"]);
}
