//! The preview, the search and the links between notes: R5, R47 and R50–R53 in
//! docs/specs/product.md.

mod common;

use common::*;
use notes_core::sync::remote::DirRemote;
use notes_core::*;

fn note_with(d: &Device, title: &str, text: &str) -> Note {
    d.create_note(NewNote {
        title: title.into(),
        text: text.into(),
        ..NewNote::default()
    })
    .unwrap()
}

fn cut(text: &str, range: &MarkdownRange) -> String {
    let units: Vec<u16> = text.encode_utf16().collect();
    String::from_utf16(&units[range.start as usize..range.end as usize]).unwrap()
}

fn found(d: &Device, query: &str) -> Vec<String> {
    d.search(query.into(), false, false)
        .unwrap()
        .into_iter()
        .map(|h| h.note.title)
        .collect()
}

#[test]
fn r5_the_preview_is_the_text_without_markup() {
    let d = device();
    let note = note_with(
        &d,
        "a",
        "# Заголовок\n\n**жирный** текст и [ссылка](https://example.org) и ==метка==\n\n- пункт",
    );
    assert_eq!(
        d.get_note(note.id.clone()).unwrap().preview,
        "Заголовок жирный текст и ссылка и метка пункт"
    );
    // It follows the text.
    d.set_text(note.id.clone(), "`код` дальше".into()).unwrap();
    assert_eq!(d.get_note(note.id).unwrap().preview, "код дальше");
    // A long text is cut.
    let long = note_with(&d, "b", &"слово ".repeat(100));
    let preview = d.get_note(long.id).unwrap().preview;
    assert_eq!(preview.chars().count(), 201, "{preview}");
    assert!(preview.ends_with('…'));
}

#[test]
fn r47_any_part_of_a_word_whatever_the_case_and_with_e_for_yo() {
    // The scenario of the specification.
    let d = device();
    note_with(&d, "Слёзы", "текст **слёзы** в тексте");
    note_with(&d, "Другая", "ничего");
    assert_eq!(found(&d, "слезы"), ["Слёзы"]);
    assert_eq!(found(&d, "лёз"), ["Слёзы"]);
    assert_eq!(found(&d, "ЛЕЗ"), ["Слёзы"]);
    // The text is searched without its markup: words across markup are found together.
    note_with(&d, "Разметка", "очень **важное** дело");
    assert_eq!(found(&d, "важное дело"), ["Разметка"]);
    assert_eq!(found(&d, "**"), [] as [&str; 0]);
}

#[test]
fn r47_tags_and_names_of_attachments_are_searched_and_the_title_counts_most() {
    let d = device();
    let in_text = note_with(&d, "Первая", "про отчёт в тексте");
    let in_title = note_with(&d, "Отчёт за год", "");
    let tagged = note_with(&d, "Третья", "");
    d.add_tag(tagged.id.clone(), "отчётность".into()).unwrap();
    let with_file = note_with(&d, "Четвёртая", "");
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("отчёт.pdf");
    std::fs::write(&path, b"%PDF").unwrap();
    d.add_attachment(with_file.id.clone(), path.to_string_lossy().into_owned(), None)
        .unwrap();
    assert_eq!(
        found(&d, "отчет"),
        [in_title.title, tagged.title, with_file.title, in_text.title]
    );
}

#[test]
fn r47_the_found_part_is_marked_in_the_title_and_in_the_snippet() {
    let d = device();
    note_with(&d, "Кот и кот", "Первая строка.\n\nА здесь живёт КОТ, рыжий кот.");
    let hit = d.search("кот".into(), false, false).unwrap().remove(0);
    let title: Vec<String> = hit.title_matches.iter().map(|r| cut(&hit.note.title, r)).collect();
    assert_eq!(title, ["Кот", "кот"]);
    assert!(hit.snippet.starts_with("А здесь живёт"), "{}", hit.snippet);
    let marked: Vec<String> = hit.snippet_matches.iter().map(|r| cut(&hit.snippet, r)).collect();
    assert_eq!(marked, ["КОТ", "кот"]);
    // Only the title matches: the snippet is the preview.
    note_with(&d, "Собака", "лает");
    let hit = d.search("собак".into(), false, false).unwrap().remove(0);
    assert_eq!(hit.snippet, "лает");
    assert!(hit.snippet_matches.is_empty());
}

#[test]
fn r47_archived_notes_and_the_trash_only_on_request() {
    let d = device();
    let archived = note_with(&d, "Архив", "искомое");
    d.set_archived(archived.id, true).unwrap();
    let trashed = note_with(&d, "Корзина", "искомое");
    d.delete_note(trashed.id).unwrap();
    note_with(&d, "Обычная", "искомое");
    let titles = |archived, trash| -> Vec<String> {
        let mut t: Vec<String> = d
            .search("искомое".into(), archived, trash)
            .unwrap()
            .into_iter()
            .map(|h| h.note.title)
            .collect();
        t.sort();
        t
    };
    assert_eq!(titles(false, false), ["Обычная"]);
    assert_eq!(titles(true, false), ["Архив", "Обычная"]);
    assert_eq!(titles(false, true), ["Корзина", "Обычная"]);
    assert!(d.search("   ".into(), true, true).unwrap().is_empty());
}

#[test]
fn r47_the_text_of_a_note_from_another_device_is_searched() {
    let a = device();
    let b = device();
    let storage = tempfile::tempdir().unwrap();
    let remote = DirRemote::new(storage.path());
    note_with(&a, "С другого", "редкое слово");
    a.sync_with(&remote).unwrap();
    b.sync_with(&remote).unwrap();
    assert_eq!(found(&b, "редкое"), ["С другого"]);
}

#[test]
fn r50_the_picker_finds_notes_by_a_part_of_the_title() {
    let d = device();
    note_with(&d, "Старый план", "");
    note_with(&d, "План на год", "");
    let trashed = note_with(&d, "План в корзине", "");
    d.delete_note(trashed.id).unwrap();
    let titles: Vec<String> = d
        .notes_matching_title("план".into(), 10)
        .unwrap()
        .into_iter()
        .map(|n| n.title)
        .collect();
    assert_eq!(
        titles,
        ["План на год", "Старый план"],
        "the title that starts with it first"
    );
    assert_eq!(d.notes_matching_title("план".into(), 1).unwrap().len(), 1);
}

#[test]
fn r51_a_note_lists_the_notes_that_link_to_it() {
    // The scenario of the specification.
    let d = device();
    let target = note_with(&d, "Цель", "");
    let folder = new_folder(&d, "Папка", None);
    let first = note_with(&d, "Первая", &format!("см. [Цель](note:{})", target.id));
    note_with(&d, "Вторая", "без ссылок");
    d.set_title(target.id.clone(), "Новая цель".into()).unwrap();
    d.move_note(target.id.clone(), Some(folder.id)).unwrap();
    let from: Vec<String> = d
        .linked_from(target.id.clone())
        .unwrap()
        .into_iter()
        .map(|n| n.title)
        .collect();
    assert_eq!(from, ["Первая"]);
    // The link text stays as it was written (E31) and still leads to the note.
    assert!(d.get_text(first.id.clone()).unwrap().contains("[Цель]"));
    // A link removed from the text is no longer a link.
    d.set_text(first.id, "ссылки нет".into()).unwrap();
    assert!(d.linked_from(target.id).unwrap().is_empty());
}

#[test]
fn r52_a_link_to_a_note_in_the_trash_or_gone_is_told_apart() {
    let d = device();
    let live = note_with(&d, "Живая", "");
    let trashed = note_with(&d, "В корзине", "");
    d.delete_note(trashed.id.clone()).unwrap();
    let purged = note_with(&d, "Удалена", "");
    d.purge_note(purged.id.clone()).unwrap();
    let states: Vec<(NoteLinkState, String)> = d
        .link_targets(vec![live.id, trashed.id, purged.id, "нет-такой".into()])
        .unwrap()
        .into_iter()
        .map(|t| (t.state, t.title))
        .collect();
    assert_eq!(
        states,
        [
            (NoteLinkState::Live, "Живая".into()),
            (NoteLinkState::InTrash, "В корзине".into()),
            (NoteLinkState::Gone, String::new()),
            (NoteLinkState::Gone, String::new()),
        ]
    );
}

#[test]
fn r53_a_new_note_from_a_selection_leaves_a_link_and_is_one_step_of_undo() {
    let d = device();
    let folder = new_folder(&d, "Папка", None);
    let source = add_in(&d, "Исходная", &folder);
    d.set_text(source.id.clone(), "до\n## Новая тема 😀\nеё текст\nпосле".into())
        .unwrap();
    let text = d.get_text(source.id.clone()).unwrap();
    let start = text[..text.find("##").unwrap()].encode_utf16().count() as u32;
    let end = text[..text.find("\nпосле").unwrap()].encode_utf16().count() as u32;
    let made = d.note_from_selection(source.id.clone(), start, end).unwrap();
    assert_eq!(made.title, "Новая тема 😀");
    assert_eq!(made.folder_id.as_ref(), Some(&folder.id));
    assert_eq!(d.get_text(made.id.clone()).unwrap(), "её текст");
    assert_eq!(
        d.get_text(source.id.clone()).unwrap(),
        format!("до\n[Новая тема 😀](note:{})\nпосле", made.id)
    );
    let from: Vec<String> = d
        .linked_from(made.id.clone())
        .unwrap()
        .into_iter()
        .map(|n| n.title)
        .collect();
    assert_eq!(from, ["Исходная"]);
    // One undo takes back both: the new note and the link in the text.
    d.undo().unwrap();
    assert_eq!(d.get_text(source.id.clone()).unwrap(), text);
    assert!(d.get_note(made.id).unwrap().deleted);
    // A selection inside a surrogate pair is refused.
    let emoji = text.find('😀').unwrap();
    let inside = text[..emoji].encode_utf16().count() as u32 + 1;
    assert!(d.note_from_selection(source.id, 0, inside).is_err());
}

/// The search is a pass over the notes without an index: measured on 5 000 notes with the size of
/// the notes of the backup of the author (about 2 300 characters each). Run by hand:
/// `cargo test --release --test search -- --ignored --nocapture`.
#[test]
#[ignore]
fn r47_measure_the_search_on_5000_notes() {
    let d = device();
    let words = [
        "заметка",
        "проект",
        "встреча",
        "список",
        "идея",
        "код",
        "rust",
        "сервер",
        "план",
    ];
    let started = std::time::Instant::now();
    d.begin_group();
    for n in 0..5000 {
        let mut text = String::new();
        while text.len() < 2300 {
            text.push_str(words[(n + text.len()) % words.len()]);
            text.push_str(if text.len().is_multiple_of(7) { "\n\n" } else { " " });
        }
        note_with(&d, &format!("Заметка {n}"), &text);
    }
    d.end_group();
    println!("5000 notes written in {:?}", started.elapsed());
    for query in ["встреча", "ёлка", "rust сервер", "заметка 4999"] {
        let started = std::time::Instant::now();
        let hits = d.search(query.into(), false, false).unwrap().len();
        println!("{query:?}: {hits} hits in {:?}", started.elapsed());
    }
}
