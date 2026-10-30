//! Export: R60–R63 in docs/specs/product.md.

mod common;

use common::*;
use notes_core::*;
use serde_json::Value;

fn note_with(d: &Device, title: &str, text: &str, folder: Option<&Folder>) -> Note {
    d.create_note(NewNote {
        title: title.into(),
        text: text.into(),
        folder_id: folder.map(|f| f.id.clone()),
        ..NewNote::default()
    })
    .unwrap()
}

fn attach(d: &Device, note: &Note, name: &str, bytes: &[u8]) -> Attachment {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(name);
    std::fs::write(&path, bytes).unwrap();
    d.add_attachment(note.id.clone(), path.to_string_lossy().into_owned(), None)
        .unwrap()
}

fn read(path: std::path::PathBuf) -> String {
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path:?}: {e}"))
}

/// Every note with its text: what export must leave as it was (R63).
fn everything(d: &Device) -> Vec<(Note, String)> {
    let mut all = d.notes(Scope::All).unwrap();
    all.extend(d.notes(Scope::Archived).unwrap());
    all.extend(d.notes(Scope::Trash).unwrap());
    all.into_iter()
        .map(|n| {
            let text = d.get_text(n.id.clone()).unwrap();
            (n, text)
        })
        .collect()
}

#[test]
fn r60_folders_attachments_and_links_as_obsidian_reads_them() {
    // The scenario of the specification.
    let d = device();
    let folder = new_folder(&d, "Проекты", None);
    let sub = new_folder(&d, "Дом", Some(&folder));
    let second = note_with(&d, "Вторая", "текст", Some(&sub));
    let first = note_with(&d, "План", "", Some(&folder));
    let image = attach(&d, &first, "фото 1.png", b"\x89PNG data");
    d.set_text(
        first.id.clone(),
        format!(
            "Смотри ![фото](attachment:{}) и [вторую заметку](note:{}).\n\n[удалённая](note:нет)",
            image.id, second.id
        ),
    )
    .unwrap();
    d.add_tag(first.id.clone(), "работа".into()).unwrap();
    d.set_pinned(first.id.clone(), true).unwrap();
    let out = tempfile::tempdir().unwrap();
    let report = d
        .export_markdown(ExportScope::All, out.path().to_string_lossy().into_owned())
        .unwrap();
    assert_eq!((report.notes, report.attachments), (2, 1));
    let plan = read(out.path().join("Проекты").join("План.md"));
    assert!(
        plan.starts_with("---\ntitle: \"План\"\ncreated: 2026-10-05T10:00:00Z\n"),
        "{plan}"
    );
    assert!(plan.contains("\ntags: [\"работа\"]\npinned: true\n"), "{plan}");
    assert!(plan.contains(&format!("\nid: {}\n---\n", first.id)), "{plan}");
    assert!(
        plan.contains("Смотри ![фото](_attachments/фото%201.png) и [[Вторая]].\n\nудалённая"),
        "{plan}"
    );
    assert_eq!(
        std::fs::read(out.path().join("Проекты").join("_attachments").join("фото 1.png")).unwrap(),
        b"\x89PNG data"
    );
    assert!(read(out.path().join("Проекты").join("Дом").join("Вторая.md")).ends_with("\n\nтекст\n"));
}

#[test]
fn r60_names_are_cleaned_and_numbered_and_the_trash_stays_out() {
    let d = device();
    note_with(&d, "a/b: c?", "1", None);
    note_with(&d, "Одно", "1", None);
    note_with(&d, "одно", "2", None);
    note_with(&d, "", "без заголовка", None);
    let gone = note_with(&d, "В корзине", "", None);
    d.delete_note(gone.id).unwrap();
    let out = tempfile::tempdir().unwrap();
    d.export_markdown(ExportScope::All, out.path().to_string_lossy().into_owned())
        .unwrap();
    let mut names: Vec<String> = std::fs::read_dir(out.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(names, ["a-b- c-.md", "Одно.md", "без заголовка.md", "одно 2.md"]);
}

#[test]
fn r60_one_note_or_one_folder() {
    let d = device();
    let folder = new_folder(&d, "Папка", None);
    let sub = new_folder(&d, "Внутри", Some(&folder));
    let other = new_folder(&d, "Другая", None);
    let one = note_with(&d, "Одна", "x", Some(&sub));
    note_with(&d, "Снаружи", "x", Some(&other));
    note_with(&d, "В корне", "x", None);

    let out = tempfile::tempdir().unwrap();
    let report = d
        .export_markdown(
            ExportScope::Folder { id: folder.id },
            out.path().to_string_lossy().into_owned(),
        )
        .unwrap();
    assert_eq!(report.notes, 1);
    assert!(out.path().join("Папка").join("Внутри").join("Одна.md").exists());

    let out = tempfile::tempdir().unwrap();
    d.export_markdown(
        ExportScope::Note { id: one.id },
        out.path().to_string_lossy().into_owned(),
    )
    .unwrap();
    assert!(out.path().join("Одна.md").exists());
    assert!(d
        .export_markdown(
            ExportScope::Note { id: "нет".into() },
            out.path().to_string_lossy().into_owned()
        )
        .is_err());
}

#[test]
fn r61_html_with_the_typography_and_the_attachments() {
    let d = device();
    let mut settings = d.settings().unwrap();
    settings.align = TextAlign::Justify;
    settings.font_body = "Literata".into();
    d.set_settings(settings).unwrap();
    let note = note_with(&d, "Отчёт <1>", "", None);
    let image = attach(&d, &note, "схема.png", b"PNG");
    let file = attach(&d, &note, "данные.csv", b"a,b");
    d.set_text(
        note.id.clone(),
        format!(
            "## Итоги\n\nЭто ==важно== и **жирно**.\n\n<script>alert(1)</script>\n\n\
             ![схема](attachment:{}) [данные](attachment:{}) [плохая](javascript:alert(1)) [сайт](https://example.org)\n\n\
             Second paragraph in English.",
            image.id, file.id
        ),
    )
    .unwrap();
    let out = tempfile::tempdir().unwrap();
    let dir = out.path().to_string_lossy().into_owned();
    let path = d.export_html(note.id.clone(), dir.clone(), true).unwrap();
    let html = read(path.into());
    assert!(html.contains("<title>Отчёт &lt;1&gt;</title>"), "{html}");
    assert!(html.contains("<h2>Итоги</h2>"), "{html}");
    assert!(html.contains("<mark>важно</mark>"), "{html}");
    assert!(html.contains("<strong>жирно</strong>"), "{html}");
    assert!(
        html.contains("&lt;script&gt;alert(1)&lt;/script&gt;"),
        "raw HTML is text: {html}"
    );
    assert!(!html.contains("javascript:"), "{html}");
    assert!(html.contains("<a href=\"https://example.org\">сайт</a>"), "{html}");
    assert!(
        html.contains("<img src=\"data:image/png;base64,UE5H\" alt=\"схема\""),
        "{html}"
    );
    assert!(html.contains(";base64,YSxi\">данные</a>"), "{html}");
    assert!(html.contains("text-align: justify; hyphens: auto"), "{html}");
    assert!(html.contains("'Literata'"), "{html}");
    assert!(
        html.contains("<p lang=\"ru\">Это") && html.contains("<p lang=\"en\">Second"),
        "{html}"
    );

    // Next to the file instead of inside it.
    let path = d.export_html(note.id, dir, false).unwrap();
    let html = read(path.into());
    // The address is percent-encoded by the renderer: «Отчёт -1-_files/схема.png».
    assert!(
        html.contains("src=\"%D0%9E%D1%82%D1%87%D1%91%D1%82%20-1-_files/%D1%81%D1%85%D0%B5%D0%BC%D0%B0.png\""),
        "{html}"
    );
    assert_eq!(
        std::fs::read(out.path().join("Отчёт -1-_files").join("схема.png")).unwrap(),
        b"PNG"
    );
}

#[test]
fn r62_json_with_all_fields_reads_back() {
    let d = device();
    let folder = new_folder(&d, "Папка", None);
    let note = note_with(&d, "Заметка", "**текст** заметки", Some(&folder));
    d.add_tag(note.id.clone(), "тег".into()).unwrap();
    d.set_starred(note.id.clone(), true).unwrap();
    attach(&d, &note, "файл.txt", b"content");
    let trashed = note_with(&d, "Удалённая", "x", None);
    d.delete_note(trashed.id.clone()).unwrap();
    d.create_filter(
        "Избранное".into(),
        "⭐".into(),
        FilterQuery {
            join: FilterJoin::All,
            conditions: vec![Condition::Starred],
        },
    )
    .unwrap();
    let out = tempfile::tempdir().unwrap();
    let report = d.export_json(out.path().to_string_lossy().into_owned(), true).unwrap();
    assert_eq!((report.notes, report.attachments), (2, 1));
    let json: Value = serde_json::from_str(&read(out.path().join("notes.json"))).unwrap();
    assert_eq!(json["format"], "notes-export");
    assert_eq!(json["version"], 1);
    let notes = json["notes"].as_array().unwrap();
    let first = notes.iter().find(|n| n["id"] == note.id.as_str()).unwrap();
    assert_eq!(first["title"], "Заметка");
    assert_eq!(first["text"], "**текст** заметки");
    assert_eq!(first["folder_id"], folder.id.as_str());
    assert_eq!(first["tags"], serde_json::json!(["тег"]));
    assert_eq!(first["starred"], true);
    assert_eq!(first["trashed"], Value::Null);
    let file = first["attachments"][0]["file"].as_str().unwrap();
    assert_eq!(std::fs::read(out.path().join(file)).unwrap(), b"content");
    let gone = notes.iter().find(|n| n["id"] == trashed.id.as_str()).unwrap();
    assert_eq!(gone["trashed"], "2026-10-05T10:00:00Z");
    assert_eq!(json["folders"][0]["name"], "Папка");
    assert_eq!(json["filters"][0]["query"]["conditions"][0]["type"], "starred");
    assert_eq!(json["settings"]["trash_days"], 30);
}

#[test]
fn r63_export_changes_nothing() {
    let d = device();
    let folder = new_folder(&d, "Папка", None);
    let note = note_with(&d, "Заметка", "[ссылка](note:x) ==a==", Some(&folder));
    attach(&d, &note, "f.txt", b"x");
    let before = everything(&d);
    let revision = d.undo_revision();
    let out = tempfile::tempdir().unwrap();
    let dir = out.path().to_string_lossy().into_owned();
    d.export_markdown(ExportScope::All, dir.clone()).unwrap();
    d.export_html(note.id, dir.clone(), true).unwrap();
    d.export_json(dir, true).unwrap();
    assert_eq!(everything(&d), before);
    assert_eq!(d.undo_revision(), revision);
}
