//! Import of a backup of Standard Notes: I1–I12 in docs/specs/import.md. The backups are written by
//! the tests; none is taken from the data of the author.

mod common;

use std::io::Write;

use common::*;
use notes_core::sync::remote::DirRemote;
use notes_core::*;
use serde_json::{json, Value};
use tempfile::TempDir;

const TOP: &str = "Standard Notes Backup and Import File.txt";
/// 2025-10-05T10:00:00Z, a year before `NOW` of the tests.
const YEAR_AGO_MS: u64 = 1_759_658_400_000;
const CREATED_MS: u64 = 1_700_000_000_000;

fn item(uuid: &str, content_type: &str, content: Value) -> Value {
    json!({
        "uuid": uuid,
        "content_type": content_type,
        "created_at": "2023-11-14T22:13:20.000Z",
        "updated_at": "2025-10-05T10:00:00.000Z",
        "created_at_timestamp": CREATED_MS * 1000,
        "updated_at_timestamp": YEAR_AGO_MS * 1000,
        "deleted": false,
        "content": content,
    })
}

fn note(uuid: &str, title: &str, text: &str) -> Value {
    item(
        uuid,
        "Note",
        json!({"title": title, "text": text, "noteType": "markdown", "references": []}),
    )
}

fn super_note(uuid: &str, title: &str, blocks: Value) -> Value {
    let text = json!({"root": {"type": "root", "children": blocks}}).to_string();
    item(
        uuid,
        "Note",
        json!({"title": title, "text": text, "noteType": "super", "references": []}),
    )
}

fn paragraph(text: &str) -> Value {
    json!({"type": "paragraph", "children": [{"type": "text", "text": text, "format": 0}]})
}

fn tag(uuid: &str, title: &str, notes: &[&str]) -> Value {
    let references: Vec<Value> = notes
        .iter()
        .map(|n| json!({"uuid": n, "content_type": "Note"}))
        .collect();
    item(uuid, "Tag", json!({"title": title, "references": references}))
}

fn file(uuid: &str, name: &str, size: u64, notes: &[&str]) -> Value {
    let references: Vec<Value> = notes
        .iter()
        .map(|n| json!({"uuid": n, "content_type": "Note", "reference_type": "FileToNote"}))
        .collect();
    item(
        uuid,
        "SN|File",
        json!({"name": name, "mimeType": "image/png", "decryptedSize": size, "references": references}),
    )
}

/// A backup folder with the top file of `items`.
fn backup(items: Vec<Value>) -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    let top = json!({"version": "004", "items": items});
    std::fs::write(dir.path().join(TOP), top.to_string()).unwrap();
    dir
}

fn import(d: &Device, backup: &TempDir) -> ImportReport {
    import_with(d, backup, None, ImportOptions::default())
}

fn import_with(d: &Device, backup: &TempDir, files: Option<&TempDir>, options: ImportOptions) -> ImportReport {
    d.import_standard_notes(
        backup.path().to_string_lossy().into_owned(),
        files.map(|f| f.path().to_string_lossy().into_owned()),
        options,
    )
    .unwrap()
}

fn find(d: &Device, title: &str) -> Note {
    let mut all = d.notes(Scope::All).unwrap();
    all.extend(d.notes(Scope::Archived).unwrap());
    all.extend(d.notes(Scope::Trash).unwrap());
    all.into_iter()
        .find(|n| n.title == title)
        .unwrap_or_else(|| panic!("no note {title}"))
}

fn folder_name(d: &Device, id: &Option<String>) -> Option<String> {
    let id = id.as_ref()?;
    d.folders().unwrap().into_iter().find(|f| &f.id == id).map(|f| f.name)
}

#[test]
fn i1_a_zip_a_folder_and_the_top_file_give_the_same_notes() {
    let dir = backup(vec![note("n1", "Раз", "текст")]);
    let top = dir.path().join(TOP);
    let zip_path = dir.path().join("backup.zip");
    let mut zip = zip::ZipWriter::new(std::fs::File::create(&zip_path).unwrap());
    zip.start_file(TOP, zip::write::SimpleFileOptions::default()).unwrap();
    zip.write_all(&std::fs::read(&top).unwrap()).unwrap();
    zip.finish().unwrap();
    for path in [dir.path().to_path_buf(), top, zip_path] {
        let d = device();
        let report = d
            .import_standard_notes(path.to_string_lossy().into_owned(), None, ImportOptions::default())
            .unwrap();
        assert_eq!(report.notes, 1, "{path:?}");
        assert_eq!(d.get_text(find(&d, "Раз").id).unwrap(), "текст");
    }
}

#[test]
fn i1_an_encrypted_backup_is_refused_with_what_to_do() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join(TOP),
        r#"{"version":"004","keyParams":{},"items":[{"uuid":"n","content_type":"Note","content":"004:x"}]}"#,
    )
    .unwrap();
    let d = device();
    let error = d
        .import_standard_notes(
            dir.path().to_string_lossy().into_owned(),
            None,
            ImportOptions::default(),
        )
        .unwrap_err();
    assert!(error.to_string().contains("export a decrypted backup"), "{error}");
    assert!(d.notes(Scope::All).unwrap().is_empty());
}

#[test]
fn i2_the_import_is_one_step_of_undo() {
    let d = device();
    add(&d, "своя");
    let dir = backup(vec![
        note("n1", "Раз", "a"),
        note("n2", "Два", "b"),
        tag("t1", "Папка", &["n1"]),
    ]);
    import(&d, &dir);
    assert_eq!(d.notes(Scope::All).unwrap().len(), 3);
    d.undo().unwrap();
    assert_eq!(view(&d, Scope::All), ["своя"]);
    assert!(d.folders().unwrap().is_empty());
}

#[test]
fn i3_the_same_backup_twice_adds_nothing() {
    let d = device();
    let dir = backup(vec![
        note("n1", "Раз", "a"),
        note("n2", "Два", "b"),
        tag("t1", "Папка", &["n1"]),
        item(
            "v1",
            "SN|SmartTag",
            json!({"title": "Закреплённые", "predicate": {"keypath": "pinned", "operator": "=", "value": true}}),
        ),
    ]);
    let first = import(&d, &dir);
    assert_eq!(
        (first.notes, first.folders, first.filters, first.already_imported),
        (2, 1, 1, 0)
    );
    // A note sent to the Trash is still there for the second import.
    d.delete_note(find(&d, "Два").id).unwrap();
    let second = import(&d, &dir);
    assert_eq!((second.notes, second.folders, second.filters), (0, 0, 0));
    assert_eq!(second.already_imported, 4);
    assert_eq!(d.notes(Scope::All).unwrap().len(), 1);
    assert_eq!(d.notes(Scope::Trash).unwrap().len(), 1);
}

#[test]
fn i4_empty_notes_are_skipped_and_other_types_kept_as_text() {
    let d = device();
    let mut code = note("n3", "Код", "fn main() {}");
    code["content"]["noteType"] = json!("code");
    let dir = backup(vec![
        note("n1", "", ""),
        super_note("n2", "", json!([])),
        code,
        note("n4", "", "только текст"),
    ]);
    let report = import(&d, &dir);
    assert_eq!(report.notes, 2);
    assert_eq!(report.empty, 2);
    assert_eq!(report.other_types, ["Код"]);
    assert_eq!(d.get_text(find(&d, "Код").id).unwrap(), "fn main() {}");
    assert_eq!(d.get_text(find(&d, "").id).unwrap(), "только текст");
}

#[test]
fn i5_the_note_keeps_the_times_of_standard_notes() {
    // The scenario of the specification.
    let a = device();
    let b = device();
    let storage = tempfile::tempdir().unwrap();
    let dir = backup(vec![note("n1", "Старая", "текст")]);
    import(&a, &dir);
    let imported = find(&a, "Старая");
    assert_eq!(
        imported.edited_ms, YEAR_AGO_MS,
        "the list shows the last edit in Standard Notes"
    );
    assert_eq!(imported.created_ms, CREATED_MS);
    let remote = DirRemote::new(storage.path());
    a.sync_with(&remote).unwrap();
    b.sync_with(&remote).unwrap();
    // A later edit on another device wins over the imported field.
    b.set_title(imported.id.clone(), "Новая".into()).unwrap();
    b.sync_with(&remote).unwrap();
    a.sync_with(&remote).unwrap();
    assert_eq!(a.get_note(imported.id).unwrap().title, "Новая");
}

#[test]
fn i5_two_devices_that_import_the_same_backup_hold_one_text() {
    let a = device();
    let b = device();
    let storage = tempfile::tempdir().unwrap();
    let dir = backup(vec![note("n1", "Общая", "начало")]);
    import(&a, &dir);
    import(&b, &dir);
    let id = find(&a, "Общая").id;
    assert_eq!(id, find(&b, "Общая").id, "the identifier comes from the uuid (I3)");
    // Each device edits its copy before they ever sync.
    a.splice_utf16(id.clone(), 0, 0, "A: ".into()).unwrap();
    let end = b.get_text(id.clone()).unwrap().encode_utf16().count() as u32;
    b.splice_utf16(id.clone(), end, 0, " :B".into()).unwrap();
    let remote = DirRemote::new(storage.path());
    a.sync_with(&remote).unwrap();
    b.sync_with(&remote).unwrap();
    a.sync_with(&remote).unwrap();
    // One document with one text: both edits, the imported text once.
    assert_eq!(a.get_text(id.clone()).unwrap(), "A: начало :B");
    assert_eq!(b.get_text(id).unwrap(), "A: начало :B");
}

#[test]
fn i6_flags_and_the_trash() {
    let d = device();
    let mut pinned = note("n1", "Закреплена", "x");
    pinned["content"]["appData"] = json!({"org.standardnotes.sn": {"pinned": true}});
    let mut flagged = note("n2", "Флаги", "x");
    for flag in ["starred", "archived", "locked"] {
        flagged["content"][flag] = json!(true);
    }
    let mut trashed = note("n3", "В корзине", "x");
    trashed["content"]["trashed"] = json!(true);
    let mut protected = note("n4", "Защищена", "x");
    protected["content"]["protected"] = json!(true);
    let report = import(&d, &backup(vec![pinned, flagged, trashed, protected]));
    assert!(find(&d, "Закреплена").pinned);
    let flags = find(&d, "Флаги");
    assert!(flags.starred && flags.archived && flags.locked);
    let in_trash = find(&d, "В корзине");
    assert!(in_trash.deleted);
    // In the Trash from the moment of the import, so `trash_days` count from it.
    let now = chrono::NaiveDateTime::parse_from_str(NOW, "%Y-%m-%dT%H:%M")
        .unwrap()
        .and_utc()
        .timestamp_millis() as u64;
    assert_eq!(in_trash.deleted_ms, Some(now));
    assert_eq!(report.protected, ["Защищена"]);
    assert!(!find(&d, "Защищена").locked, "an ordinary note");
}

#[test]
fn i7_a_super_note_becomes_markdown() {
    let d = device();
    let blocks = json!([
        {"type": "heading", "tag": "h1", "children": [{"type": "text", "text": "План", "format": 0}]},
        {"type": "paragraph", "children": [
            {"type": "text", "text": "важно", "format": 1},
            {"type": "text", "text": " и ", "format": 0},
            {"type": "text", "text": "подчёркнуто", "format": 8}
        ]},
        {"type": "unencrypted-image", "src": "../_resources/x.jpg", "alt": ""}
    ]);
    let report = import(&d, &backup(vec![super_note("n1", "Супер", blocks)]));
    assert_eq!(
        d.get_text(find(&d, "Супер").id).unwrap(),
        "# План\n\n**важно** и подчёркнуто\n\n\\[Image not imported: ../\\_resources/x.jpg\\]"
    );
    assert_eq!(report.conversion.len(), 1);
    assert!(report.conversion[0].starts_with("Супер: "), "{:?}", report.conversion);
    assert!(report.conversion[0].contains("../_resources/x.jpg"));
}

#[test]
fn i8_tags_become_folders_and_the_smallest_tag_takes_the_note() {
    // The scenario of the specification.
    let d = device();
    let mut items = vec![
        note("both-ab", "AB", "x"),
        note("both-cd", "CD", "x"),
        note("none", "Без тегов", "x"),
    ];
    let a_notes = ["both-ab", "a1", "a2", "a3", "a4"];
    for n in &a_notes[1..] {
        items.push(note(n, n, "x"));
    }
    items.push(note("b1", "b1", "x"));
    items.push(note("c1", "c1", "x"));
    items.push(note("d1", "d1", "x"));
    items.push(tag("ta", "a", &a_notes));
    items.push(tag("tb", "b", &["both-ab", "b1"]));
    items.push(tag("tc", "c", &["both-cd", "c1"]));
    items.push(tag("td", "d", &["both-cd", "d1"]));
    let dir = backup(items);
    let report = import(&d, &dir);
    assert_eq!(report.folders, 4);
    let ab = find(&d, "AB");
    assert_eq!(folder_name(&d, &ab.folder_id).as_deref(), Some("b"));
    assert_eq!(ab.tags, ["a"]);
    let cd = find(&d, "CD");
    assert_eq!(folder_name(&d, &cd.folder_id).as_deref(), Some("c"));
    assert_eq!(cd.tags, ["d"]);
    assert_eq!(find(&d, "Без тегов").folder_id, None);
    assert_eq!(report.several_tags, ["AB → b", "CD → c"]);
    assert_eq!(report.tags, 2);

    // With the option on, everything goes under "Standard Notes".
    let e = device();
    import_with(
        &e,
        &dir,
        None,
        ImportOptions {
            under_one_folder: true,
            ..ImportOptions::default()
        },
    );
    let folders = e.folders().unwrap();
    let root = folders.iter().find(|f| f.name == "Standard Notes").unwrap();
    assert!(folders
        .iter()
        .filter(|f| f.id != root.id)
        .all(|f| f.parent_id.as_ref() == Some(&root.id)));
    assert_eq!(find(&e, "Без тегов").folder_id.as_ref(), Some(&root.id));
}

#[test]
fn i8_an_emoji_icon_is_kept_and_a_symbol_name_is_not() {
    let d = device();
    let mut work = tag("t1", "Работа", &[]);
    work["content"]["iconString"] = json!("💼");
    let mut home = tag("t2", "Дом", &[]);
    home["content"]["iconString"] = json!("home");
    import(&d, &backup(vec![work, home]));
    let icons: Vec<(String, String)> = d.folders().unwrap().into_iter().map(|f| (f.name, f.icon)).collect();
    assert!(icons.contains(&("Работа".into(), "💼".into())), "{icons:?}");
    assert!(icons.contains(&("Дом".into(), String::new())), "{icons:?}");
}

#[test]
fn i9_files_found_by_name_and_size_become_attachments() {
    // The scenario of the specification.
    let d = device();
    let files = tempfile::tempdir().unwrap();
    std::fs::write(files.path().join("фото.png"), b"0123456789").unwrap();
    std::fs::create_dir(files.path().join("a")).unwrap();
    std::fs::create_dir(files.path().join("b")).unwrap();
    std::fs::write(files.path().join("a").join("двойник.png"), b"abcd").unwrap();
    std::fs::write(files.path().join("b").join("двойник.png"), b"wxyz").unwrap();
    std::fs::write(files.path().join("ничей.png"), b"free").unwrap();
    let blocks = json!([
        paragraph("до"),
        {"type": "snfile", "fileUuid": "f1"},
        {"type": "snfile", "fileUuid": "f2"}
    ]);
    let dir = backup(vec![
        super_note("n1", "С файлами", blocks),
        file("f1", "фото.png", 10, &["n1"]),
        file("f2", "двойник.png", 4, &["n1"]),
        file("f3", "ничей.png", 4, &[]),
        file("f4", "потерян.png", 99, &[]),
    ]);
    let report = import_with(
        &d,
        &dir,
        Some(&files),
        ImportOptions {
            collect_unlinked_files: true,
            ..ImportOptions::default()
        },
    );
    let with = find(&d, "С файлами");
    let attachments = d.attachments(with.id.clone()).unwrap();
    assert_eq!(attachments.len(), 1);
    assert_eq!((attachments[0].name.as_str(), attachments[0].size), ("фото.png", 10));
    assert_eq!(
        std::fs::read(attachments[0].local_path.as_ref().unwrap()).unwrap(),
        b"0123456789"
    );
    assert_eq!(
        d.get_text(with.id).unwrap(),
        format!(
            "до\n\n![фото.png](attachment:{})\n\n\\[File not imported: двойник.png\\]",
            attachments[0].id
        )
    );
    assert_eq!(report.files_not_imported, ["двойник.png (С файлами)"]);
    // The file of no note goes into a note of its own; the one with no content stays out.
    let unlinked = find(&d, "Files from Standard Notes");
    let kept = d.attachments(unlinked.id.clone()).unwrap();
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0].name, "ничей.png");
    assert_eq!(report.attachments, 2);
}

#[test]
fn i10_smart_views_become_saved_filters() {
    // The scenario of the specification.
    let d = device();
    let dir = backup(vec![
        note("n1", "x", "a"),
        note("n2", "y", "b"),
        tag("t1", "work", &["n1", "n2"]),
        item(
            "v1",
            "SN|SmartTag",
            json!({"title": "Работа x", "iconString": "⭐", "predicate": {"operator": "and", "value": [
                {"keypath": "tags", "operator": "includes", "value": "work"},
                {"keypath": "title", "operator": "=", "value": "x"}
            ]}}),
        ),
        item(
            "v2",
            "SN|SmartTag",
            json!({"title": "Регулярка", "predicate": {"keypath": "title", "operator": "matches", "value": "^a"}}),
        ),
    ]);
    let report = import(&d, &dir);
    let filters = d.filters().unwrap();
    assert_eq!(filters.len(), 1);
    assert_eq!((filters[0].name.as_str(), filters[0].icon.as_str()), ("Работа x", "⭐"));
    assert_eq!(
        view(
            &d,
            Scope::Filter {
                id: filters[0].id.clone()
            }
        ),
        ["x"]
    );
    assert_eq!(report.filters_not_imported, ["Регулярка"]);
}

#[test]
fn i11_other_items_are_counted_by_type_and_deleted_ones_skipped() {
    let d = device();
    let mut gone = note("n2", "Удалена", "x");
    gone["deleted"] = json!(true);
    let dir = backup(vec![
        note("n1", "Есть", "x"),
        gone,
        item("p1", "SN|UserPreferences", json!({})),
        item("c1", "SN|Component", json!({})),
        item("c2", "SN|Component", json!({})),
    ]);
    let report = import(&d, &dir);
    assert_eq!(report.notes, 1);
    assert_eq!(
        report.ignored,
        [
            IgnoredItems {
                content_type: "SN|Component".into(),
                count: 2
            },
            IgnoredItems {
                content_type: "SN|UserPreferences".into(),
                count: 1
            },
        ]
    );
}

#[test]
fn i12_the_report_reads_as_text_and_the_log_has_numbers_only() {
    let d = device();
    d.set_log_level(LogLevel::Info).unwrap();
    let mut protected = note("n1", "Секретная заметка", "x");
    protected["content"]["protected"] = json!(true);
    let report = import(&d, &backup(vec![protected, note("n2", "", "")]));
    assert!(report
        .text
        .starts_with("Imported: 1 notes, 0 folders, 0 tags, 0 filters, 0 attachments."));
    assert!(report.text.contains("Skipped: 0 already imported, 1 empty."));
    assert!(report.text.contains("- Секретная заметка"));
    let log = std::fs::read_to_string(d.dir().join("log.txt")).unwrap();
    assert!(log.contains("Standard Notes: 1 notes"), "{log}");
    assert!(!log.contains("Секретная"), "no titles in the log");
}

#[test]
fn i5_two_devices_that_import_different_backups_still_agree() {
    let a = device();
    let b = device();
    let storage = tempfile::tempdir().unwrap();
    // A backup of an earlier day on A, a later one on B: one item, two texts.
    import(&a, &backup(vec![note("n1", "Общая", "начало")]));
    let mut later = note("n1", "Общая", "начало и продолжение");
    later["updated_at_timestamp"] = serde_json::json!((YEAR_AGO_MS + 86_400_000) * 1000);
    import(&b, &backup(vec![later]));
    let id = find(&a, "Общая").id;
    let remote = DirRemote::new(storage.path());
    a.sync_with(&remote).unwrap();
    b.sync_with(&remote).unwrap();
    a.sync_with(&remote).unwrap();
    b.splice_utf16(id.clone(), 0, 0, "B: ".into()).unwrap();
    b.sync_with(&remote).unwrap();
    a.sync_with(&remote).unwrap();
    b.sync_with(&remote).unwrap();
    // Both show the same text: one of the two documents, the same one everywhere (S3).
    assert_eq!(a.get_text(id.clone()).unwrap(), b.get_text(id).unwrap());
}
