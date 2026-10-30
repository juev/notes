//! Behaviour of notes, folders, tags, flags, the Trash, attachments, filters and the shared settings on
//! one device: docs/specs/product.md and docs/specs/sync.md. What needs two devices is in `sync.rs`.

mod common;

use common::*;
use notes_core::*;
use serde_json::json;

fn tag_names(d: &Device) -> Vec<(String, u32)> {
    d.tags().unwrap().into_iter().map(|t| (t.name, t.count)).collect()
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

const DAY_MS: u64 = 24 * 60 * 60 * 1000;

// ---- notes ----

#[test]
fn r1_a_note_has_a_title_a_folder_tags_flags_and_moments() {
    let d = device();
    let folder = new_folder(&d, "Работа", None);
    let note = d
        .create_note(NewNote {
            title: "План".into(),
            folder_id: Some(folder.id.clone()),
            tags: vec!["срочно".into(), "Дом".into()],
            ..NewNote::default()
        })
        .unwrap();
    assert_eq!(note.title, "План");
    assert_eq!(note.folder_id.as_deref(), Some(folder.id.as_str()));
    assert_eq!(note.tags, ["Дом", "срочно"]);
    assert!(!(note.pinned || note.starred || note.archived || note.locked || note.deleted));
    assert_eq!(note.attachments, 0);
    // The moment of creation is the moment of the device; nothing was edited since.
    let created = chrono::NaiveDateTime::parse_from_str(NOW, "%Y-%m-%dT%H:%M")
        .unwrap()
        .and_utc()
        .timestamp_millis() as u64;
    assert_eq!((note.created_ms, note.edited_ms), (created, created));
    assert_eq!(note.deleted_ms, None);

    d.set_now_for_tests("2026-10-05T11:00");
    d.set_title(note.id.clone(), "План на неделю".into()).unwrap();
    let edited = d.get_note(note.id.clone()).unwrap();
    assert_eq!(edited.created_ms, created);
    assert_eq!(
        edited.edited_ms,
        created + 60 * 60 * 1000,
        "the last edit is the greatest stamp"
    );

    assert!(d.get_note("missing".into()).is_err());
    assert!(d
        .create_note(NewNote {
            title: "x".into(),
            folder_id: Some("missing".into()),
            ..NewNote::default()
        })
        .is_err());
}

#[test]
fn r3_a_note_may_have_no_title() {
    let d = device();
    let note = add(&d, "");
    assert_eq!(note.title, "");
    assert_eq!(view(&d, Scope::All), [""]);
}

#[test]
fn r2_the_folder_for_a_new_note_follows_the_setting() {
    let d = device();
    let first = new_folder(&d, "Первая", None);
    let second = new_folder(&d, "Вторая", None);
    let set = |choice: &str| {
        d.set_settings(AppSettings {
            new_note_folder: choice.into(),
            ..d.settings().unwrap()
        })
        .unwrap()
    };
    assert_eq!(
        d.default_folder_id(Some(first.id.clone())).unwrap(),
        None,
        "root is the default"
    );

    set("last");
    assert_eq!(
        d.default_folder_id(Some(first.id.clone())).unwrap(),
        Some(first.id.clone())
    );
    assert_eq!(d.default_folder_id(None).unwrap(), None);

    set(&second.id);
    assert_eq!(
        d.default_folder_id(Some(first.id.clone())).unwrap(),
        Some(second.id.clone())
    );

    // A folder that is gone gives the root.
    d.delete_folder(second.id).unwrap();
    assert_eq!(d.default_folder_id(Some(first.id.clone())).unwrap(), None);
    set("last");
    d.delete_folder(first.id.clone()).unwrap();
    assert_eq!(d.default_folder_id(Some(first.id)).unwrap(), None);
}

#[test]
fn r4_a_copy_keeps_the_folder_the_tags_and_the_content_of_the_attachments() {
    let d = device();
    let folder = new_folder(&d, "Папка", None);
    let note = add_in(&d, "Оригинал", &folder);
    d.add_tag(note.id.clone(), "тег".into()).unwrap();
    d.set_pinned(note.id.clone(), true).unwrap();
    let src = tempfile::tempdir().unwrap();
    let file = src.path().join("Счёт.pdf");
    std::fs::write(&file, b"%PDF-1.7 demo").unwrap();
    let original = d
        .add_attachment(note.id.clone(), file.to_string_lossy().into_owned(), None)
        .unwrap();

    let copy = d.duplicate_note(note.id.clone()).unwrap();
    assert_ne!(copy.id, note.id);
    assert_eq!(copy.title, "Оригинал (copy)");
    assert_eq!(copy.folder_id, note.folder_id);
    assert_eq!(copy.tags, ["тег"]);
    assert!(!copy.pinned, "the flags stay with the original");
    let files = d.attachments(copy.id.clone()).unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].sha256, original.sha256, "the same content");
    assert_ne!(files[0].id, original.id);

    // Taking one away does not touch the other.
    d.remove_attachment(files[0].id.clone()).unwrap();
    assert_eq!(d.attachments(note.id).unwrap().len(), 1);
}

#[test]
fn r28_a_locked_note_keeps_its_title_tags_and_attachments_but_can_be_moved_flagged_and_deleted() {
    let d = device();
    let folder = new_folder(&d, "Папка", None);
    let note = add(&d, "закрыта");
    d.add_tag(note.id.clone(), "тег".into()).unwrap();
    let src = tempfile::tempdir().unwrap();
    let file = src.path().join("a.txt");
    std::fs::write(&file, b"a").unwrap();
    let path = file.to_string_lossy().into_owned();
    let attached = d.add_attachment(note.id.clone(), path.clone(), None).unwrap();
    d.set_locked(note.id.clone(), true).unwrap();

    let refused = |e: AppError| matches!(e, AppError::Invalid { .. });
    assert!(refused(d.set_title(note.id.clone(), "иначе".into()).unwrap_err()));
    assert!(refused(d.add_tag(note.id.clone(), "новый".into()).unwrap_err()));
    assert!(refused(d.remove_tag(note.id.clone(), "тег".into()).unwrap_err()));
    assert!(refused(d.add_attachment(note.id.clone(), path, None).unwrap_err()));
    assert!(refused(d.remove_attachment(attached.id).unwrap_err()));
    let still = d.get_note(note.id.clone()).unwrap();
    assert_eq!(
        (still.title.as_str(), still.tags.clone(), still.attachments),
        ("закрыта", vec!["тег".to_string()], 1)
    );

    d.move_note(note.id.clone(), Some(folder.id.clone())).unwrap();
    d.set_pinned(note.id.clone(), true).unwrap();
    d.set_starred(note.id.clone(), true).unwrap();
    d.set_archived(note.id.clone(), true).unwrap();
    d.delete_note(note.id.clone()).unwrap();
    d.restore_note(note.id.clone()).unwrap();
    let moved = d.get_note(note.id.clone()).unwrap();
    assert_eq!(moved.folder_id.as_deref(), Some(folder.id.as_str()));
    assert!(moved.pinned && moved.starred && moved.archived && moved.locked);

    // Unlocking is possible, and then the note is editable again.
    d.set_locked(note.id.clone(), false).unwrap();
    d.set_title(note.id.clone(), "иначе".into()).unwrap();
    assert_eq!(d.get_note(note.id).unwrap().title, "иначе");
}

#[test]
fn r25_r26_r27_pinned_comes_first_starred_has_a_view_and_archived_is_left_out() {
    let d = device();
    let folder = new_folder(&d, "Папка", None);
    d.set_now_for_tests("2026-10-05T10:00");
    let a = add_in(&d, "a", &folder);
    d.set_now_for_tests("2026-10-05T10:01");
    let b = add_in(&d, "b", &folder);
    d.set_now_for_tests("2026-10-05T10:02");
    let c = add_in(&d, "c", &folder);
    let in_folder = || view(&d, Scope::Folder { id: folder.id.clone() });
    assert_eq!(in_folder(), ["c", "b", "a"], "the last edit first");

    d.set_pinned(a.id.clone(), true).unwrap();
    assert_eq!(in_folder(), ["a", "c", "b"]);
    assert_eq!(view(&d, Scope::All)[0], "a", "in every view");
    d.set_folder_sort(
        folder.id.clone(),
        FolderSort {
            by: SortBy::Title,
            descending: false,
        },
    )
    .unwrap();
    assert_eq!(in_folder(), ["a", "b", "c"]);
    d.set_pinned(c.id.clone(), true).unwrap();
    assert_eq!(
        in_folder(),
        ["a", "c", "b"],
        "pinned first under any sort, in the order of the sort"
    );

    // Starring does not change the order.
    d.set_starred(b.id.clone(), true).unwrap();
    assert_eq!(view(&d, Scope::Starred), ["b"]);
    assert_eq!(in_folder(), ["a", "c", "b"]);

    // Archived is out of the folder, All, the tags and the counters, and in Archived.
    d.add_tag(b.id.clone(), "тег".into()).unwrap();
    d.set_archived(b.id.clone(), true).unwrap();
    assert_eq!(in_folder(), ["a", "c"]);
    assert_eq!(view(&d, Scope::All), ["a", "c"]);
    assert_eq!(view(&d, Scope::Archived), ["b"]);
    assert!(view(&d, Scope::Tag { name: "тег".into() }).is_empty());
    assert!(d.tags().unwrap().is_empty());
    assert_eq!(d.folders().unwrap()[0].note_count, 2);

    d.set_archived(b.id, false).unwrap();
    assert_eq!(in_folder(), ["a", "c", "b"]);
}

#[test]
fn r16_a_folder_sorts_by_edit_creation_or_title_in_either_direction() {
    let d = device();
    let folder = new_folder(&d, "Папка", None);
    d.set_now_for_tests("2026-10-05T10:00");
    let b = add_in(&d, "b", &folder);
    d.set_now_for_tests("2026-10-05T10:01");
    add_in(&d, "A", &folder);
    d.set_now_for_tests("2026-10-05T10:02");
    add_in(&d, "c", &folder);
    d.set_now_for_tests("2026-10-05T10:03");
    d.set_starred(b.id, true).unwrap();
    let sorted = |by, descending| {
        d.set_folder_sort(folder.id.clone(), FolderSort { by, descending })
            .unwrap();
        view(&d, Scope::Folder { id: folder.id.clone() })
    };
    assert_eq!(d.folder(folder.id.clone()).unwrap().sort, FolderSort::default());
    assert_eq!(sorted(SortBy::Edited, true), ["b", "c", "A"]);
    assert_eq!(sorted(SortBy::Edited, false), ["A", "c", "b"]);
    assert_eq!(
        sorted(SortBy::Created, true),
        ["c", "A", "b"],
        "an edit does not move it"
    );
    assert_eq!(sorted(SortBy::Created, false), ["b", "A", "c"]);
    assert_eq!(sorted(SortBy::Title, false), ["A", "b", "c"], "case does not matter");
    assert_eq!(sorted(SortBy::Title, true), ["c", "b", "A"]);
    assert_eq!(
        d.folder(folder.id).unwrap().sort,
        FolderSort {
            by: SortBy::Title,
            descending: true
        }
    );
}

#[test]
fn r16_a_folder_can_hide_the_preview_and_the_date_and_be_a_daily_one() {
    let d = device();
    let folder = new_folder(&d, "Дневник", None);
    assert!(folder.show_preview && folder.show_date && !folder.daily);
    d.set_folder_display(folder.id.clone(), false, true).unwrap();
    d.set_folder_daily(folder.id.clone(), true).unwrap();
    d.set_folder_icon(folder.id.clone(), "📓".into()).unwrap();
    let got = d.folder(folder.id).unwrap();
    assert_eq!(
        (got.show_preview, got.show_date, got.daily, got.icon.as_str()),
        (false, true, true, "📓")
    );
}

// ---- folders ----

#[test]
fn r10_r15_folders_nest_and_names_are_checked() {
    let d = device();
    let a = new_folder(&d, "A", None);
    let b = new_folder(&d, "B", Some(&a));
    let c = new_folder(&d, "C", Some(&b));
    new_folder(&d, "A", None);
    let by_id = |id: &str| d.folder(id.into()).unwrap().parent_id;
    assert_eq!(
        (by_id(&a.id), by_id(&b.id), by_id(&c.id)),
        (None, Some(a.id.clone()), Some(b.id.clone()))
    );
    assert_eq!(
        d.folders().unwrap().iter().filter(|f| f.name == "A").count(),
        2,
        "names may repeat"
    );

    let invalid = |e: AppError| matches!(e, AppError::Invalid { .. });
    assert!(invalid(d.create_folder("  ".into(), None).unwrap_err()));
    assert!(invalid(d.rename_folder(a.id.clone(), "".into()).unwrap_err()));
    assert!(d.create_folder("я".repeat(200), None).is_ok());
    assert!(invalid(d.create_folder("я".repeat(201), None).unwrap_err()));
    assert!(d.create_folder("x".into(), Some("missing".into())).is_err());

    d.rename_folder(a.id.clone(), "  Новое имя ".into()).unwrap();
    assert_eq!(d.folder(a.id).unwrap().name, "Новое имя");
}

#[test]
fn r11_a_folder_cannot_go_into_itself_or_its_own_subfolder() {
    let d = device();
    let a = new_folder(&d, "A", None);
    let b = new_folder(&d, "B", Some(&a));
    let c = new_folder(&d, "C", Some(&b));
    let other = new_folder(&d, "Другая", None);
    for (id, into) in [(&a.id, &a.id), (&a.id, &b.id), (&a.id, &c.id), (&b.id, &c.id)] {
        let error = d.move_folder(id.clone(), Some(into.clone()), None).unwrap_err();
        assert!(matches!(error, AppError::Invalid { .. }), "{error}");
    }
    d.move_folder(c.id.clone(), Some(other.id.clone()), None).unwrap();
    d.move_folder(b.id.clone(), None, None).unwrap();
    let parent = |id: &str| d.folder(id.into()).unwrap().parent_id;
    assert_eq!((parent(&c.id), parent(&b.id)), (Some(other.id), None));
}

#[test]
fn r12_r13_the_tree_counts_the_notes_of_the_subfolders_and_no_folder_lists_the_rest() {
    let d = device();
    let a = new_folder(&d, "A", None);
    let b = new_folder(&d, "B", Some(&a));
    add_in(&d, "в A", &a);
    add_in(&d, "в B 1", &b);
    add_in(&d, "в B 2", &b);
    let hidden = add_in(&d, "в архиве", &b);
    d.set_archived(hidden.id, true).unwrap();
    let gone = add_in(&d, "в корзине", &b);
    d.delete_note(gone.id).unwrap();
    add(&d, "без папки");

    let count = |f: &Folder| d.folder(f.id.clone()).unwrap().note_count;
    assert_eq!((count(&a), count(&b)), (3, 2));
    assert_eq!(view(&d, Scope::NoFolder), ["без папки"]);
    assert_eq!(
        view(&d, Scope::Folder { id: a.id.clone() }),
        ["в A"],
        "the folder lists its own notes"
    );
    assert!(d.notes(Scope::Folder { id: "missing".into() }).is_err());
}

#[test]
fn r11_notes_move_between_folders_and_to_the_root() {
    let d = device();
    let a = new_folder(&d, "A", None);
    let note = add(&d, "н");
    d.move_note(note.id.clone(), Some(a.id.clone())).unwrap();
    assert_eq!(view(&d, Scope::Folder { id: a.id.clone() }), ["н"]);
    d.move_note(note.id.clone(), None).unwrap();
    assert_eq!(view(&d, Scope::NoFolder), ["н"]);
    assert!(d.move_note(note.id, Some("missing".into())).is_err());
}

#[test]
fn s10_a_note_of_a_deleted_or_unknown_folder_and_a_folder_of_a_deleted_parent_are_shown_at_the_root() {
    let d = device();
    let parent = new_folder(&d, "Родитель", None);
    let child = new_folder(&d, "Дочерняя", Some(&parent));
    let in_parent = add_in(&d, "в родителе", &parent);
    let in_child = add_in(&d, "в дочерней", &child);
    let lost = add(&d, "в неизвестной");
    d.set_note_field_for_tests(&lost.id, "folder", json!("no-such-folder"));
    assert_eq!(
        d.get_note(lost.id.clone()).unwrap().folder_id,
        None,
        "unknown folder: the root"
    );

    // Only the parent is deleted, as another device can leave it.
    d.set_folder_field_for_tests(&parent.id, "deleted", json!(1));
    assert_eq!(d.get_note(in_parent.id).unwrap().folder_id, None);
    assert_eq!(
        d.get_note(in_child.id).unwrap().folder_id.as_deref(),
        Some(child.id.as_str())
    );
    assert_eq!(
        d.folder(child.id.clone()).unwrap().parent_id,
        None,
        "the folder of a deleted parent: the root"
    );
    assert!(d.folder(parent.id).is_err(), "a deleted folder is not listed");
    let mut root = view(&d, Scope::NoFolder);
    root.sort();
    assert_eq!(root, ["в неизвестной", "в родителе"]);

    // Once the folder of a note is deleted as well, the note is at the root too.
    d.set_folder_field_for_tests(&child.id, "deleted", json!(1));
    assert!(view(&d, Scope::NoFolder).contains(&"в дочерней".to_string()));
}

#[test]
fn s10_r14_deleting_a_folder_sends_it_its_subfolders_and_their_notes_to_the_trash_in_one_change() {
    let d = device();
    let x = new_folder(&d, "X", None);
    let y = new_folder(&d, "Y", Some(&x));
    let z = new_folder(&d, "Z", None);
    let n1 = add_in(&d, "n1", &x);
    let n2 = add_in(&d, "n2", &x);
    let n3 = add_in(&d, "n3", &y);
    let n4 = add_in(&d, "n4", &y);
    let other = add_in(&d, "в другой", &z);
    d.set_archived(n2.id.clone(), true).unwrap();
    // A note that went to the Trash earlier keeps its own moment.
    d.set_now_for_tests("2026-10-01T09:00");
    d.delete_note(n4.id.clone()).unwrap();
    d.set_now_for_tests(NOW);
    let earlier = d.get_note(n4.id.clone()).unwrap().deleted_ms;

    assert_eq!(
        d.folder_deletion_count(x.id.clone()).unwrap(),
        3,
        "n1, n2 and n3 are told to the user"
    );
    assert_eq!(d.delete_folder(x.id.clone()).unwrap(), 3);
    assert_eq!(
        d.folders().unwrap().iter().map(|f| f.name.as_str()).collect::<Vec<_>>(),
        ["Z"]
    );
    let mut trash = view(&d, Scope::Trash);
    trash.sort();
    assert_eq!(trash, ["n1", "n2", "n3", "n4"]);
    assert_eq!(view(&d, Scope::All), ["в другой"]);
    assert_eq!(d.get_note(n4.id.clone()).unwrap().deleted_ms, earlier);
    assert!(d.get_note(other.id).is_ok_and(|n| !n.deleted));

    // One undo takes the whole action back.
    assert_eq!(d.undo().unwrap(), 1);
    assert_eq!(d.folders().unwrap().len(), 3);
    let mut back = view(&d, Scope::All);
    back.sort();
    assert_eq!(back, ["n1", "n3", "в другой"]);
    assert_eq!(view(&d, Scope::Trash), ["n4"], "and no more than the action");
    let _ = (n1, n3);
}

#[test]
fn s10_restoring_a_note_restores_the_chain_of_its_folders_and_nothing_else() {
    let d = device();
    let x = new_folder(&d, "X", None);
    let y = new_folder(&d, "Y", Some(&x));
    let sibling = new_folder(&d, "Брат", Some(&x));
    let a = add_in(&d, "a", &y);
    let b = add_in(&d, "b", &y);
    let c = add_in(&d, "c", &x);
    let s = add_in(&d, "s", &sibling);
    d.delete_folder(x.id.clone()).unwrap();

    d.restore_note(a.id.clone()).unwrap();
    let names: Vec<String> = d.folders().unwrap().into_iter().map(|f| f.name).collect();
    assert_eq!(names.len(), 2, "X and Y are back: {names:?}");
    assert!(names.contains(&"X".to_string()) && names.contains(&"Y".to_string()));
    assert_eq!(d.get_note(a.id.clone()).unwrap().folder_id, Some(y.id.clone()));
    assert_eq!(d.folder(y.id.clone()).unwrap().parent_id, Some(x.id.clone()));
    let mut trash = view(&d, Scope::Trash);
    trash.sort();
    assert_eq!(trash, ["b", "c", "s"], "the other notes stay in the Trash");
    assert!(
        d.folder(sibling.id.clone()).is_err(),
        "and so does the folder next to the chain"
    );

    // Restoring a note whose folder is out of the Trash changes nothing else, and a note out of it is left alone.
    d.restore_note(c.id).unwrap();
    d.restore_note(a.id).unwrap();
    let mut trash = view(&d, Scope::Trash);
    trash.sort();
    assert_eq!(trash, ["b", "s"]);
    let _ = (b, s);
}

// ---- the Trash ----

#[test]
fn s6_a_note_goes_to_the_trash_with_its_moment_and_comes_back_with_an_empty_value() {
    let d = device();
    let note = add(&d, "н");
    d.set_now_for_tests("2026-10-05T12:00");
    d.delete_note(note.id.clone()).unwrap();
    let deleted_at = chrono::NaiveDateTime::parse_from_str("2026-10-05T12:00", "%Y-%m-%dT%H:%M")
        .unwrap()
        .and_utc()
        .timestamp_millis() as u64;
    let got = d.get_note(note.id.clone()).unwrap();
    assert!(got.deleted);
    assert_eq!(
        got.deleted_ms,
        Some(deleted_at),
        "R46: the Trash counts the days from this"
    );
    assert!(view(&d, Scope::All).is_empty());
    assert_eq!(view(&d, Scope::Trash), ["н"]);

    // Deleting again does not start the days over.
    d.set_now_for_tests("2026-10-07T12:00");
    d.delete_note(note.id.clone()).unwrap();
    assert_eq!(d.get_note(note.id.clone()).unwrap().deleted_ms, Some(deleted_at));

    d.restore_note(note.id.clone()).unwrap();
    let back = d.get_note(note.id).unwrap();
    assert!(!back.deleted && back.deleted_ms.is_none());
    assert_eq!(view(&d, Scope::All), ["н"]);
}

#[test]
fn s7_a_note_deleted_for_good_is_shown_nowhere_and_cannot_be_brought_back() {
    let d = device();
    let folder = new_folder(&d, "Папка", None);
    let note = add_in(&d, "навсегда", &folder);
    d.add_tag(note.id.clone(), "тег".into()).unwrap();
    d.set_starred(note.id.clone(), true).unwrap();
    d.purge_note(note.id.clone()).unwrap();

    for scope in [
        Scope::All,
        Scope::Starred,
        Scope::Archived,
        Scope::Trash,
        Scope::Untagged,
        Scope::NoFolder,
        Scope::Folder { id: folder.id.clone() },
        Scope::Tag { name: "тег".into() },
    ] {
        assert!(view(&d, scope.clone()).is_empty(), "{scope:?}");
    }
    assert!(d.tags().unwrap().is_empty());
    assert_eq!(d.folder(folder.id).unwrap().note_count, 0);
    assert!(matches!(d.get_note(note.id.clone()), Err(AppError::NotFound { .. })));
    assert!(matches!(
        d.restore_note(note.id.clone()),
        Err(AppError::NotFound { .. })
    ));
    assert!(matches!(
        d.set_title(note.id, "x".into()),
        Err(AppError::NotFound { .. })
    ));
}

#[test]
fn r46_the_trash_lists_deleted_notes_and_is_emptied_by_hand() {
    let d = device();
    d.set_now_for_tests("2026-10-05T10:00");
    let a = add(&d, "a");
    let b = add(&d, "b");
    let kept = add(&d, "остаётся");
    d.delete_note(a.id.clone()).unwrap();
    d.set_now_for_tests("2026-10-05T10:05");
    d.delete_note(b.id.clone()).unwrap();
    assert_eq!(view(&d, Scope::Trash), ["b", "a"], "the one that went last first");

    d.purge_note(a.id).unwrap();
    assert_eq!(view(&d, Scope::Trash), ["b"]);
    assert_eq!(d.empty_trash().unwrap(), 1);
    assert!(view(&d, Scope::Trash).is_empty());
    assert!(d.get_note(b.id).is_err());
    assert_eq!(d.empty_trash().unwrap(), 0);
    assert_eq!(view(&d, Scope::All), ["остаётся"]);
    let _ = kept;
}

#[test]
fn s8_the_trash_is_emptied_on_start_by_the_age_of_the_note_and_the_setting() {
    // A device without sync: the check runs at the start, on the real clock.
    let d = device();
    let old = add(&d, "давно");
    let recent = add(&d, "недавно");
    let borderline = add(&d, "почти");
    let live = add(&d, "в работе");
    let planted = |ago_days: u64| json!(now_ms() - ago_days * DAY_MS);
    let d = plant(
        d,
        &[
            ("note", &old.id, "deleted", planted(31)),
            ("note", &recent.id, "deleted", planted(1)),
            (
                "note",
                &borderline.id,
                "deleted",
                json!(now_ms() - 30 * DAY_MS + 60_000),
            ),
        ],
    );
    assert_eq!(d.settings().unwrap().trash_days, 30, "the default");
    assert!(d.get_note(old.id.clone()).is_err(), "31 days: gone");
    assert!(d.get_note(recent.id.clone()).unwrap().deleted, "a day: still there");
    assert!(
        d.get_note(borderline.id).unwrap().deleted,
        "a minute short of 30 days: still there"
    );
    assert!(!d.get_note(live.id.clone()).unwrap().deleted);

    // The setting is the one that counts, from 1 to 3650.
    let d = plant(
        d,
        &[
            ("settings", "app", "trash_days", json!(3)),
            ("note", &recent.id, "deleted", planted(4)),
            ("note", &live.id, "deleted", planted(2)),
        ],
    );
    assert!(d.get_note(recent.id).is_err(), "4 days with a limit of 3");
    assert!(d.get_note(live.id).unwrap().deleted, "2 days with a limit of 3");
}

#[test]
fn s7_the_content_of_a_purged_note_is_no_longer_needed_on_the_device() {
    let d = device();
    let note = add(&d, "с файлом");
    let src = tempfile::tempdir().unwrap();
    let file = src.path().join("a.txt");
    std::fs::write(&file, b"content of the only note").unwrap();
    let a = d
        .add_attachment(note.id.clone(), file.to_string_lossy().into_owned(), None)
        .unwrap();
    let blob = std::path::PathBuf::from(a.local_path.unwrap());
    d.delete_note(note.id.clone()).unwrap();
    let d = reopen(d);
    assert!(blob.exists(), "S26: a note in the Trash keeps its content");
    d.purge_note(note.id).unwrap();
    let d = reopen(d);
    assert!(
        !blob.exists(),
        "S26: the content goes with the note, on start without sync"
    );
    drop(d);
}

// ---- tags ----

#[test]
fn r20_r21_a_tag_is_words_without_comma_and_hash_and_is_one_tag_whatever_the_case() {
    let d = device();
    let a = add(&d, "a");
    let b = add(&d, "b");
    d.add_tag(a.id.clone(), "  Work  plans ".into()).unwrap();
    assert_eq!(
        d.get_note(a.id.clone()).unwrap().tags,
        ["Work plans"],
        "spaces inside are kept as one"
    );
    d.add_tag(b.id.clone(), "work PLANS".into()).unwrap();
    assert_eq!(
        d.get_note(b.id.clone()).unwrap().tags,
        ["Work plans"],
        "the spelling first written is kept"
    );
    assert_eq!(tag_names(&d), [("Work plans".to_string(), 2)], "one tag with two notes");
    assert_eq!(
        view(
            &d,
            Scope::Tag {
                name: "WORK plans".into()
            }
        )
        .len(),
        2
    );

    // The same tag again, in any case, changes nothing.
    d.add_tag(a.id.clone(), "WORK PLANS".into()).unwrap();
    assert_eq!(d.get_note(a.id.clone()).unwrap().tags, ["Work plans"]);

    for bad in ["", "   ", "a,b", "#work", "two #words"] {
        let error = d.add_tag(a.id.clone(), bad.into()).unwrap_err();
        assert!(matches!(error, AppError::Invalid { .. }), "{bad:?}: {error}");
    }

    // Taking it off, in any case.
    d.remove_tag(b.id.clone(), "WORK plans".into()).unwrap();
    assert!(d.get_note(b.id).unwrap().tags.is_empty());
    assert_eq!(tag_names(&d), [("Work plans".to_string(), 1)]);
    assert_eq!(view(&d, Scope::Untagged), ["b"]);
}

#[test]
fn r22_a_tag_is_renamed_merged_and_deleted_for_all_notes() {
    let d = device();
    let a = add(&d, "a");
    let b = add(&d, "b");
    let c = add(&d, "c");
    d.add_tag(a.id.clone(), "дом".into()).unwrap();
    d.add_tag(b.id.clone(), "дом".into()).unwrap();
    d.add_tag(b.id.clone(), "работа".into()).unwrap();
    d.add_tag(c.id.clone(), "Работа".into()).unwrap();
    assert_eq!(
        tag_names(&d),
        [("дом".to_string(), 2), ("работа".to_string(), 2)],
        "first spelling"
    );

    // A change of case alone is the new spelling.
    assert_eq!(d.rename_tag("дом".into(), "Дом".into()).unwrap(), 2);
    assert_eq!(tag_names(&d)[0], ("Дом".to_string(), 2));

    // Into a tag that exists: the two are one, spelled as the one that was there.
    assert_eq!(d.rename_tag("дом".into(), "работа".into()).unwrap(), 2);
    assert_eq!(tag_names(&d), [("работа".to_string(), 3)]);
    assert_eq!(
        d.get_note(b.id.clone()).unwrap().tags,
        ["работа"],
        "b had both: one is left"
    );

    assert_eq!(d.delete_tag("РАБОТА".into()).unwrap(), 3);
    assert!(d.tags().unwrap().is_empty());
    assert_eq!(view(&d, Scope::Untagged).len(), 3);
    assert_eq!(d.rename_tag("нет".into(), "есть".into()).unwrap(), 0);
}

#[test]
fn s5_a_tag_at_the_limit_of_a_field_is_taken_and_a_longer_one_is_refused() {
    let d = device();
    let note = add(&d, "н");
    // `tag:` takes four of the 200 bytes of a key, so a name has 196.
    let latin = "a".repeat(196);
    let cyrillic = "я".repeat(98);
    d.add_tag(note.id.clone(), latin.clone()).unwrap();
    d.add_tag(note.id.clone(), cyrillic.clone()).unwrap();
    for too_long in ["a".repeat(197), "я".repeat(99)] {
        let error = d.add_tag(note.id.clone(), too_long).unwrap_err();
        assert!(matches!(error, AppError::Invalid { .. }), "{error}");
    }
    assert_eq!(d.get_note(note.id.clone()).unwrap().tags, [latin, cyrillic]);
    assert!(matches!(
        d.create_note(NewNote {
            title: "x".into(),
            tags: vec!["я".repeat(99)],
            ..NewNote::default()
        }),
        Err(AppError::Invalid { .. })
    ));
}

#[test]
fn s5_a_tag_from_before_the_limit_leaves_the_view() {
    let d = device();
    let note = add(&d, "н");
    let long = format!("tag:{}", "a".repeat(197));
    // A key of 201 bytes cannot be a register of any device; one that was stored all the same is not shown.
    d.set_note_field_for_tests(&note.id, &long, json!(true));
    let d = plant(d, &[("note", &note.id, "tag:ok", json!(true))]);
    assert_eq!(d.get_note(note.id).unwrap().tags, ["ok"]);
}

// ---- attachments ----

#[test]
fn r30_r33_attachments_are_stored_by_content_and_listed_for_the_note() {
    let d = device();
    let note = add(&d, "с файлом");
    let src = tempfile::tempdir().unwrap();
    let file = src.path().join("Счёт.pdf");
    std::fs::write(&file, b"%PDF-1.7 demo").unwrap();

    let a = d
        .add_attachment(note.id.clone(), file.to_string_lossy().into_owned(), None)
        .unwrap();
    assert_eq!(
        (a.name.as_str(), a.mime.as_str(), a.size, a.note_id.as_str()),
        ("Счёт.pdf", "application/pdf", 13, note.id.as_str())
    );
    std::fs::remove_file(&file).unwrap();
    let stored = a.local_path.clone().expect("content is kept by the store");
    assert_eq!(std::fs::read(stored).unwrap(), b"%PDF-1.7 demo");
    assert_eq!(d.get_note(note.id.clone()).unwrap().attachments, 1);

    d.remove_attachment(a.id).unwrap();
    assert!(d.attachments(note.id.clone()).unwrap().is_empty());
    assert_eq!(d.get_note(note.id).unwrap().attachments, 0);
}

#[test]
fn r34_a_note_in_the_trash_keeps_its_attachments() {
    let d = device();
    let note = add(&d, "с файлом");
    let src = tempfile::tempdir().unwrap();
    let file = src.path().join("a.txt");
    std::fs::write(&file, b"a").unwrap();
    d.add_attachment(note.id.clone(), file.to_string_lossy().into_owned(), None)
        .unwrap();
    d.delete_note(note.id.clone()).unwrap();
    assert_eq!(d.attachments(note.id.clone()).unwrap().len(), 1);
    d.restore_note(note.id.clone()).unwrap();
    assert_eq!(d.get_note(note.id).unwrap().attachments, 1);
}

fn shown_attachment(d: &Device, name: &str, content: &[u8]) -> Attachment {
    let note = add(&d.store, name);
    let src = tempfile::tempdir().unwrap();
    let file = src.path().join(name);
    std::fs::write(&file, content).unwrap();
    d.add_attachment(note.id.clone(), file.to_string_lossy().into_owned(), None)
        .unwrap();
    d.attachments(note.id).unwrap().remove(0)
}

#[test]
fn a_name_without_a_type_takes_the_type_of_the_content() {
    let d = device();
    let jpeg = shown_attachment(&d, "Image", &[0xFF, 0xD8, 0xFF, 0xE0, 0, 16, b'J', b'F', b'I', b'F']);
    assert_eq!((jpeg.name.as_str(), jpeg.mime.as_str()), ("Image.jpg", "image/jpeg"));
    let pdf = shown_attachment(&d, "Scan", b"%PDF-1.7 and the rest");
    assert_eq!((pdf.name.as_str(), pdf.mime.as_str()), ("Scan.pdf", "application/pdf"));
    let png = shown_attachment(&d, "shot", b"\x89PNG\r\n\x1a\n0000");
    assert_eq!((png.name.as_str(), png.mime.as_str()), ("shot.png", "image/png"));
}

#[test]
fn a_name_that_tells_the_type_and_content_that_tells_none_are_left_alone() {
    let d = device();
    // The name decides when it can, whatever the bytes are.
    let named = shown_attachment(&d, "notes.txt", &[0xFF, 0xD8, 0xFF, 0xE0]);
    assert_eq!((named.name.as_str(), named.mime.as_str()), ("notes.txt", "text/plain"));
    // Unknown bytes under an unknown name stay what they were.
    let unknown = shown_attachment(&d, "blob", b"nothing known here");
    assert_eq!(
        (unknown.name.as_str(), unknown.mime.as_str()),
        ("blob", "application/octet-stream")
    );
    // An extension that tells nothing is kept; only the type is learnt from the content.
    let odd = shown_attachment(&d, "export.dat", b"GIF89a....");
    assert_eq!((odd.name.as_str(), odd.mime.as_str()), ("export.dat", "image/gif"));
}

#[test]
fn the_registers_keep_the_name_the_file_came_with() {
    let d = device();
    let shown = shown_attachment(&d, "Image", &[0xFF, 0xD8, 0xFF, 0xE0]);
    assert_eq!(shown.name, "Image.jpg");
    // Without the content the name is what was stored: nothing was written back.
    std::fs::remove_file(shown.local_path.unwrap()).unwrap();
    let bare = d.attachments(shown.note_id).unwrap().remove(0);
    assert_eq!(
        (bare.name.as_str(), bare.mime.as_str()),
        ("Image", "application/octet-stream")
    );
}

/// The files of a folder with their content, by name.
fn folder_content(dir: &std::path::Path) -> Vec<(String, Vec<u8>)> {
    let mut all: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap())
        .map(|e| {
            (
                e.file_name().to_string_lossy().into_owned(),
                std::fs::read(e.path()).unwrap(),
            )
        })
        .collect();
    all.sort();
    all
}

fn attach_named(d: &Device, note: &Note, name: &str, content: &[u8]) -> Attachment {
    let src = tempfile::tempdir().unwrap();
    let file = src.path().join("source");
    std::fs::write(&file, content).unwrap();
    d.add_attachment(note.id.clone(), file.to_string_lossy().into_owned(), Some(name.into()))
        .unwrap()
}

fn named(name: &str, content: &[u8]) -> (String, Vec<u8>) {
    (name.to_string(), content.to_vec())
}

#[test]
fn every_attachment_of_a_note_is_saved_under_its_name() {
    let d = device();
    let n = add(&d, "с тремя файлами");
    attach_named(&d, &n, "a.txt", b"first");
    attach_named(&d, &n, "b.pdf", b"%PDF-1.7 second");
    attach_named(&d, &n, "c.png", b"third");
    let out = tempfile::tempdir().unwrap();

    let report = d
        .save_attachments(n.id.clone(), out.path().to_string_lossy().into_owned())
        .unwrap();

    let mut saved = report.saved.clone();
    saved.sort();
    assert_eq!(saved, ["a.txt", "b.pdf", "c.png"]);
    assert_eq!((report.failed.len(), report.fetched), (0, 0));
    assert_eq!(
        folder_content(out.path()),
        [
            named("a.txt", b"first"),
            named("b.pdf", b"%PDF-1.7 second"),
            named("c.png", b"third")
        ]
    );
    // The content behind the attachments is where it was.
    assert!(d.attachments(n.id).unwrap().iter().all(|f| f.local_path.is_some()));
}

#[test]
fn a_name_that_is_taken_gets_a_number_and_nothing_is_replaced() {
    let d = device();
    let n = add(&d, "с одинаковыми именами");
    attach_named(&d, &n, "a.txt", b"one");
    attach_named(&d, &n, "a.txt", b"two");
    attach_named(&d, &n, "notes", b"three");
    attach_named(&d, &n, ".env", b"four");
    let out = tempfile::tempdir().unwrap();
    for (name, content) in [
        ("a.txt", "was here"),
        ("a 2.txt", "and this"),
        ("notes", "old"),
        (".env", "kept"),
    ] {
        std::fs::write(out.path().join(name), content).unwrap();
    }

    let dir = out.path().to_string_lossy().into_owned();
    let report = d.save_attachments(n.id.clone(), dir.clone()).unwrap();

    assert!(report.failed.is_empty(), "{report:?}");
    let all = folder_content(out.path());
    for kept in [
        named("a.txt", b"was here"),
        named("a 2.txt", b"and this"),
        named("notes", b"old"),
        named(".env", b"kept"),
    ] {
        assert!(all.contains(&kept), "{kept:?} in {all:?}");
    }
    // Two attachments share a name: both are there, each under a name of its own.
    let numbered: Vec<_> = all
        .iter()
        .filter(|(name, _)| name == "a 3.txt" || name == "a 4.txt")
        .collect();
    let mut content: Vec<_> = numbered.iter().map(|(_, c)| c.as_slice()).collect();
    content.sort();
    assert_eq!(content, [b"one".as_slice(), b"two".as_slice()]);
    assert!(all.contains(&named("notes 2", b"three")), "{all:?}");
    assert!(all.contains(&named(".env 2", b"four")), "{all:?}");
    assert_eq!(all.len(), 8);

    // A second run adds a second set and still replaces nothing.
    d.save_attachments(n.id, dir).unwrap();
    assert_eq!(folder_content(out.path()).len(), 12);
}

#[test]
fn a_name_from_another_device_stays_inside_the_folder() {
    let d = device();
    let n = add(&d, "с чужим именем");
    attach_named(&d, &n, "../up/and:out.txt", b"escaped");
    attach_named(&d, &n, "plain.txt", b"plain");
    let parent = tempfile::tempdir().unwrap();
    let out = parent.path().join("chosen");
    std::fs::create_dir(&out).unwrap();

    let report = d.save_attachments(n.id, out.to_string_lossy().into_owned()).unwrap();

    assert!(report.failed.is_empty(), "{report:?}");
    assert_eq!(
        folder_content(&out),
        [named(".._up_and_out.txt", b"escaped"), named("plain.txt", b"plain")]
    );
    assert_eq!(std::fs::read_dir(parent.path()).unwrap().count(), 1);
}

#[test]
fn a_file_that_cannot_be_written_is_named_and_the_rest_are_saved() {
    let d = device();
    let n = add(&d, "с потерянным файлом");
    let lost = attach_named(&d, &n, "lost.txt", b"lost");
    attach_named(&d, &n, "kept.txt", b"kept");
    // The content is gone from the device and there is no storage to take it from.
    std::fs::remove_file(lost.local_path.unwrap()).unwrap();
    let out = tempfile::tempdir().unwrap();

    let report = d
        .save_attachments(n.id.clone(), out.path().to_string_lossy().into_owned())
        .unwrap();

    assert_eq!(
        (report.saved, report.failed, report.fetched),
        (vec!["kept.txt".to_string()], vec!["lost.txt".to_string()], 0)
    );
    assert_eq!(folder_content(out.path()), [named("kept.txt", b"kept")]);

    // A folder that is not there: every file is named, nothing is thrown.
    let nowhere = out.path().join("missing").to_string_lossy().into_owned();
    let report = d.save_attachments(n.id, nowhere).unwrap();
    assert_eq!((report.saved.len(), report.failed.len()), (0, 2));
}

/// Attaches the content to a new note.
fn attach_file(d: &Device, content: &[u8]) -> std::result::Result<Attachment, AppError> {
    let note = add(d, "с файлом");
    let src = tempfile::tempdir().unwrap();
    let file = src.path().join("file.bin");
    std::fs::write(&file, content).unwrap();
    d.add_attachment(note.id, file.to_string_lossy().into_owned(), None)
}

#[test]
fn r36_a_file_above_the_limit_is_not_added() {
    let d = device();
    d.set_attachment_limits(AttachmentLimits {
        max_file: 10,
        max_note: 1000,
    })
    .unwrap();

    let error = attach_file(&d, b"0123456789A").unwrap_err();
    assert!(matches!(error, AppError::TooLarge { .. }), "{error}");

    // The note has no attachment and the content was not left behind.
    let note = d.notes(Scope::All).unwrap().remove(0);
    assert!(d.attachments(note.id).unwrap().is_empty());
    assert_eq!(std::fs::read_dir(d.dir().join("blobs")).unwrap().count(), 0);
}

#[test]
fn r36_the_note_limit_counts_the_live_attachments_of_that_note_only() {
    let d = device();
    d.set_attachment_limits(AttachmentLimits {
        max_file: 100,
        max_note: 10,
    })
    .unwrap();
    let note = add(&d, "с двумя файлами");
    let src = tempfile::tempdir().unwrap();
    let file = src.path().join("six.bin");
    std::fs::write(&file, b"123456").unwrap();
    let path = file.to_string_lossy().into_owned();
    let first = d.add_attachment(note.id.clone(), path.clone(), None).unwrap();

    let error = d.add_attachment(note.id.clone(), path.clone(), None).unwrap_err();
    assert!(matches!(error, AppError::TooLarge { .. }), "{error}");
    assert_eq!(d.attachments(note.id.clone()).unwrap().len(), 1);

    // Another note has room of its own.
    let other = add(&d, "другая");
    d.add_attachment(other.id, path.clone(), None).unwrap();

    // A removed attachment no longer counts, and the same content goes in.
    d.remove_attachment(first.id).unwrap();
    let second = d.add_attachment(note.id.clone(), path, None).unwrap();
    assert_eq!(second.size, 6);
}

#[test]
fn r36_a_large_file_is_copied_in_pieces() {
    let d = device();
    d.set_attachment_limits(AttachmentLimits {
        max_file: 64 * 1024 * 1024,
        max_note: 128 * 1024 * 1024,
    })
    .unwrap();
    let note = add(&d, "с большим файлом");
    let src = tempfile::tempdir().unwrap();
    let file = src.path().join("large.bin");
    // Sparse: the file is large, the disk keeps only the marker at its end.
    {
        use std::io::{Seek, SeekFrom, Write};
        let mut out = std::fs::File::create(&file).unwrap();
        out.set_len(16 * 1024 * 1024).unwrap();
        out.seek(SeekFrom::Start(16 * 1024 * 1024 - 4)).unwrap();
        out.write_all(b"end!").unwrap();
    }

    let added = d
        .add_attachment(note.id, file.to_string_lossy().into_owned(), None)
        .unwrap();
    assert_eq!(added.size, 16 * 1024 * 1024);
    let stored = added.local_path.clone().unwrap();
    assert_eq!(std::fs::metadata(&stored).unwrap().len(), added.size);
    // The hash is of the whole file, the same one a whole read would give.
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    let mut input = std::fs::File::open(&stored).unwrap();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = std::io::Read::read(&mut input, &mut buffer).unwrap();
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    assert_eq!(added.sha256, format!("{:x}", hasher.finalize()));
}

#[test]
fn r36_a_failed_add_does_not_leave_content() {
    let d = device();
    let src = tempfile::tempdir().unwrap();
    let file = src.path().join("file.bin");
    std::fs::write(&file, b"content nobody kept").unwrap();

    let error = d
        .add_attachment("no-such-note".into(), file.to_string_lossy().into_owned(), None)
        .unwrap_err();
    assert!(matches!(error, AppError::NotFound { .. }), "{error}");
    assert_eq!(std::fs::read_dir(d.dir().join("blobs")).unwrap().count(), 0);
}

#[test]
fn r36_the_limits_belong_to_the_device_and_stay() {
    let d = device();
    let dir = d.dir().to_path_buf();
    d.set_attachment_limits(AttachmentLimits {
        max_file: 3,
        max_note: 4,
    })
    .unwrap();

    // A second connection to the same database, as a restart of the app would open it.
    let again = Store::open(dir.to_string_lossy().into_owned()).unwrap();
    assert_eq!(
        again.attachment_limits(),
        AttachmentLimits {
            max_file: 3,
            max_note: 4
        }
    );
}

#[cfg(unix)]
#[test]
fn the_data_folder_is_for_its_owner_alone() {
    use std::os::unix::fs::PermissionsExt;
    let mode = |path: &std::path::Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
    let d = device();
    assert_eq!(mode(d.dir()), 0o700);
    assert_eq!(mode(&d.dir().join("blobs")), 0o700);
    assert_eq!(mode(&d.dir().join("notes.sqlite")), 0o600);

    let note = add(&d, "с файлом");
    let src = tempfile::tempdir().unwrap();
    let file = src.path().join("file.bin");
    std::fs::write(&file, b"content").unwrap();
    let added = d
        .add_attachment(note.id, file.to_string_lossy().into_owned(), None)
        .unwrap();
    assert_eq!(mode(std::path::Path::new(added.local_path.as_ref().unwrap())), 0o600);

    d.log(LogLevel::Error, "attachments".into(), "a reason for a line".into());
    assert_eq!(mode(&d.dir().join("log.txt")), 0o600);
}

// ---- views, counts and filters ----

#[test]
fn r44_the_system_views_and_their_counts() {
    let d = device();
    let folder = new_folder(&d, "Папка", None);
    let plain = add(&d, "обычная");
    let starred = add_in(&d, "со звездой", &folder);
    let archived = add(&d, "в архиве");
    let trashed = add(&d, "в корзине");
    let tagged = add(&d, "с тегом");
    d.set_starred(starred.id.clone(), true).unwrap();
    d.set_archived(archived.id.clone(), true).unwrap();
    d.delete_note(trashed.id.clone()).unwrap();
    d.add_tag(tagged.id.clone(), "тег".into()).unwrap();

    let mut all = view(&d, Scope::All);
    all.sort();
    assert_eq!(all, ["обычная", "с тегом", "со звездой"]);
    assert_eq!(view(&d, Scope::Starred), ["со звездой"]);
    assert_eq!(view(&d, Scope::Archived), ["в архиве"]);
    assert_eq!(view(&d, Scope::Trash), ["в корзине"]);
    let mut untagged = view(&d, Scope::Untagged);
    untagged.sort();
    assert_eq!(untagged, ["обычная", "со звездой"]);
    let mut root = view(&d, Scope::NoFolder);
    root.sort();
    assert_eq!(root, ["обычная", "с тегом"]);
    assert_eq!(
        d.counts().unwrap(),
        Counts {
            all: 3,
            starred: 1,
            archived: 1,
            trash: 1,
            untagged: 2,
            no_folder: 2
        }
    );
    let _ = plain;
}

fn query(join: FilterJoin, conditions: Vec<Condition>) -> FilterQuery {
    FilterQuery { join, conditions }
}

fn selected(d: &Device, q: FilterQuery) -> Vec<String> {
    let mut titles: Vec<String> = d.preview_filter(q).unwrap().into_iter().map(|n| n.title).collect();
    titles.sort();
    titles
}

#[test]
fn r45_a_filter_selects_by_folder_tag_flag_attachment_and_title() {
    let d = device();
    let top = new_folder(&d, "Верх", None);
    let sub = new_folder(&d, "Низ", Some(&top));
    let a = add_in(&d, "Алфавит", &top);
    let b = add_in(&d, "Бета", &sub);
    let c = add(&d, "Гамма");
    d.add_tag(a.id.clone(), "Работа".into()).unwrap();
    d.add_tag(b.id.clone(), "дом".into()).unwrap();
    d.add_tag(b.id.clone(), "работа".into()).unwrap();
    d.set_pinned(c.id.clone(), true).unwrap();
    d.set_starred(a.id.clone(), true).unwrap();
    let src = tempfile::tempdir().unwrap();
    let file = src.path().join("f.txt");
    std::fs::write(&file, b"f").unwrap();
    d.add_attachment(b.id.clone(), file.to_string_lossy().into_owned(), None)
        .unwrap();
    let all = FilterJoin::All;

    let in_folder = |subfolders| Condition::InFolder {
        folder_id: top.id.clone(),
        subfolders,
    };
    assert_eq!(selected(&d, query(all, vec![in_folder(false)])), ["Алфавит"]);
    assert_eq!(selected(&d, query(all, vec![in_folder(true)])), ["Алфавит", "Бета"]);
    let tag = |name: &str| Condition::HasTag { name: name.into() };
    assert_eq!(
        selected(&d, query(all, vec![tag("РАБОТА")])),
        ["Алфавит", "Бета"],
        "case does not matter"
    );
    assert_eq!(selected(&d, query(all, vec![tag("работа"), tag("дом")])), ["Бета"]);
    assert_eq!(selected(&d, query(all, vec![Condition::NoTags])), ["Гамма"]);
    assert_eq!(
        selected(
            &d,
            query(
                all,
                vec![Condition::NoneOfTags {
                    names: vec!["дом".into()]
                }]
            )
        ),
        ["Алфавит", "Гамма"]
    );
    assert_eq!(selected(&d, query(all, vec![Condition::Pinned])), ["Гамма"]);
    assert_eq!(selected(&d, query(all, vec![Condition::Starred])), ["Алфавит"]);
    assert_eq!(selected(&d, query(all, vec![Condition::HasAttachments])), ["Бета"]);
    assert_eq!(
        selected(
            &d,
            query(
                all,
                vec![Condition::TitleIs {
                    text: "бета".into()
                }]
            )
        ),
        ["Бета"]
    );
    assert!(selected(&d, query(all, vec![Condition::TitleIs { text: "бет".into() }])).is_empty());
    assert_eq!(
        selected(&d, query(all, vec![Condition::TitleContains { text: "ЛФ".into() }])),
        ["Алфавит"]
    );
    // Any of, and none at all.
    assert_eq!(
        selected(&d, query(FilterJoin::Any, vec![Condition::Pinned, Condition::Starred])),
        ["Алфавит", "Гамма"]
    );
    assert_eq!(selected(&d, query(all, vec![])).len(), 3);
}

#[test]
fn r45_r27_a_filter_leaves_out_the_archived_and_the_trash_unless_it_asks_for_archived() {
    let d = device();
    let kept = add(&d, "в работе");
    let shelved = add(&d, "в архиве");
    let gone = add(&d, "в корзине");
    d.set_pinned(shelved.id.clone(), true).unwrap();
    d.set_archived(shelved.id.clone(), true).unwrap();
    d.set_pinned(gone.id.clone(), true).unwrap();
    d.delete_note(gone.id).unwrap();
    d.set_pinned(kept.id, true).unwrap();

    assert_eq!(
        selected(&d, query(FilterJoin::All, vec![Condition::Pinned])),
        ["в работе"]
    );
    assert_eq!(
        selected(&d, query(FilterJoin::All, vec![Condition::Pinned, Condition::Archived])),
        ["в архиве"]
    );
    assert_eq!(
        selected(&d, query(FilterJoin::Any, vec![Condition::Pinned, Condition::Archived])),
        ["в архиве", "в работе"]
    );
}

#[test]
fn r45_a_filter_selects_by_the_moments_of_creation_and_edit() {
    let d = device();
    d.set_now_for_tests("2026-09-20T10:00");
    add(&d, "старая");
    d.set_now_for_tests("2026-10-01T10:00");
    let middle = add(&d, "средняя");
    d.set_now_for_tests("2026-10-04T09:00");
    d.set_pinned(middle.id, true).unwrap();
    d.set_now_for_tests("2026-10-05T10:00");
    add(&d, "новая");
    let all = FilterJoin::All;
    let date = |d: &str| d.to_string();

    let created = |when| selected(&d, query(all, vec![Condition::Created { when }]));
    assert_eq!(
        created(DateTest::Before {
            date: date("2026-10-01")
        }),
        ["старая"]
    );
    assert_eq!(
        created(DateTest::After {
            date: date("2026-10-01")
        }),
        ["новая"],
        "after the day is over: the notes of that day are not after it"
    );
    assert_eq!(created(DateTest::Within { days: 3 }), ["новая"]);
    assert_eq!(created(DateTest::Within { days: 10 }), ["новая", "средняя"]);

    let edited = |when| selected(&d, query(all, vec![Condition::Edited { when }]));
    assert_eq!(
        edited(DateTest::Within { days: 2 }),
        ["новая", "средняя"],
        "the edit of 4 October counts"
    );
    assert_eq!(
        edited(DateTest::Before {
            date: date("2026-10-04")
        }),
        ["старая"].map(String::from).to_vec()
    );

    // A date that is not one is refused where the filter is saved.
    let bad = query(
        all,
        vec![Condition::Created {
            when: DateTest::Before {
                date: date("вчера")
            },
        }],
    );
    assert!(matches!(d.preview_filter(bad.clone()), Err(AppError::Invalid { .. })));
    assert!(d.create_filter("плохой".into(), String::new(), bad).is_err());
}

#[test]
fn r45_a_saved_filter_is_named_ordered_counted_and_shown_as_a_view() {
    let d = device();
    let a = add(&d, "a");
    add(&d, "b");
    d.set_pinned(a.id.clone(), true).unwrap();
    let pinned = d
        .create_filter(
            "Закреплённые".into(),
            "pin".into(),
            query(FilterJoin::All, vec![Condition::Pinned]),
        )
        .unwrap();
    let everything = d
        .create_filter("Все".into(), String::new(), query(FilterJoin::All, vec![]))
        .unwrap();
    assert_eq!((pinned.count, everything.count), (1, 2));
    assert_eq!(pinned.icon, "pin");
    assert_eq!(view(&d, Scope::Filter { id: pinned.id.clone() }), ["a"]);

    let names = |d: &Device| d.filters().unwrap().into_iter().map(|f| f.name).collect::<Vec<_>>();
    assert_eq!(names(&d), ["Закреплённые", "Все"], "in the order they were made");
    d.move_filter(everything.id.clone(), None).unwrap();
    assert_eq!(names(&d), ["Все", "Закреплённые"]);
    d.move_filter(everything.id.clone(), Some(pinned.id.clone())).unwrap();
    assert_eq!(names(&d), ["Закреплённые", "Все"]);

    d.update_filter(
        pinned.id.clone(),
        "Важные".into(),
        "star".into(),
        query(FilterJoin::All, vec![Condition::Starred]),
    )
    .unwrap();
    let updated = d.filters().unwrap().remove(0);
    assert_eq!(
        (updated.name.as_str(), updated.icon.as_str(), updated.count),
        ("Важные", "star", 0)
    );

    assert!(d
        .create_filter("  ".into(), String::new(), FilterQuery::default())
        .is_err());
    d.delete_filter(everything.id.clone()).unwrap();
    assert_eq!(names(&d), ["Важные"]);
    assert!(d.notes(Scope::Filter { id: everything.id }).is_err());
}

// ---- order of folders and filters ----

#[test]
fn s11_folders_and_filters_are_ordered_by_pos_and_a_move_puts_one_between_two() {
    let d = device();
    let a = new_folder(&d, "A", None);
    let b = new_folder(&d, "B", None);
    let c = new_folder(&d, "C", None);
    let names = |d: &Device| d.folders().unwrap().into_iter().map(|f| f.name).collect::<Vec<_>>();
    assert_eq!(names(&d), ["A", "B", "C"]);

    d.move_folder(c.id.clone(), None, Some(a.id.clone())).unwrap();
    assert_eq!(names(&d), ["A", "C", "B"], "right after A");
    d.move_folder(b.id.clone(), None, None).unwrap();
    assert_eq!(names(&d), ["B", "A", "C"], "first without an anchor");
    // Between the same two, again and again: a key always exists.
    for _ in 0..30 {
        d.move_folder(c.id.clone(), None, Some(b.id.clone())).unwrap();
        d.move_folder(a.id.clone(), None, Some(b.id.clone())).unwrap();
    }
    assert_eq!(names(&d), ["B", "A", "C"]);

    // Within a parent: the order of the brothers only.
    let child1 = new_folder(&d, "в A 1", Some(&a));
    let child2 = new_folder(&d, "в A 2", Some(&a));
    d.move_folder(child2.id.clone(), Some(a.id.clone()), None).unwrap();
    let kids: Vec<String> = d
        .folders()
        .unwrap()
        .into_iter()
        .filter(|f| f.parent_id.as_deref() == Some(a.id.as_str()))
        .map(|f| f.name)
        .collect();
    assert_eq!(kids, ["в A 2", "в A 1"]);
    let _ = child1;
}

#[test]
fn s11_with_equal_pos_the_identifier_decides() {
    let d = device();
    let seed = new_folder(&d, "seed", None);
    let d = plant(
        d,
        &[
            ("folder", "fb", "name", json!("second by id")),
            ("folder", "fb", "pos", json!("m")),
            ("folder", "fa", "name", json!("first by id")),
            ("folder", "fa", "pos", json!("m")),
            ("filter", "xb", "name", json!("filter second")),
            ("filter", "xb", "pos", json!("m")),
            ("filter", "xa", "name", json!("filter first")),
            ("filter", "xa", "pos", json!("m")),
        ],
    );
    let folders: Vec<String> = d.folders().unwrap().into_iter().map(|f| f.name).collect();
    let position = |name: &str| folders.iter().position(|f| f == name).unwrap();
    assert!(position("first by id") < position("second by id"), "{folders:?}");
    let filters: Vec<String> = d.filters().unwrap().into_iter().map(|f| f.name).collect();
    assert_eq!(filters, ["filter first", "filter second"]);
    let _ = seed;
}

#[test]
fn s9_a_cycle_of_stored_parents_is_read_the_same_way_and_the_next_move_writes_the_cut() {
    let d = device();
    let a = new_folder(&d, "A", None);
    let b = new_folder(&d, "B", None);
    let c = new_folder(&d, "C", None);
    // Two devices moved A into B and B into A while apart; the later `parent` register is B's.
    let d = plant(
        d,
        &[
            ("folder", &a.id, "parent", json!(b.id)),
            ("folder", &b.id, "parent", json!(a.id)),
        ],
    );
    let parent = |id: &str| d.folder(id.into()).unwrap().parent_id;
    assert_eq!(
        parent(&b.id),
        None,
        "B has the greatest stamp: its parent counts as empty"
    );
    assert_eq!(parent(&a.id), Some(b.id.clone()));

    // The stored field is not rewritten by the merge.
    let stored = |id: &str| -> String {
        rusqlite::Connection::open(d.dir().join("notes.sqlite"))
            .unwrap()
            .query_row(
                "SELECT value FROM fields WHERE kind = 'folder' AND id = ?1 AND field = 'parent'",
                [id],
                |r| r.get(0),
            )
            .unwrap()
    };
    assert_eq!(stored(&b.id), json!(a.id).to_string());

    // The next move of any folder writes the cut down, so that moving A away does not bring B's parent back.
    d.move_folder(c.id.clone(), None, None).unwrap();
    assert_eq!(stored(&b.id), "null");
    d.move_folder(a.id.clone(), None, None).unwrap();
    assert_eq!((parent(&a.id), parent(&b.id)), (None, None));
}

// ---- the shared settings ----

#[test]
fn s12_the_settings_have_defaults_and_keep_what_is_saved() {
    let d = device();
    let defaults = d.settings().unwrap();
    assert_eq!(defaults, AppSettings::default());
    assert_eq!(defaults.trash_days, 30);
    assert_eq!(defaults.new_note_folder, "root");
    assert_eq!(defaults.spell_language, "auto");
    assert!(defaults.smart_typography && defaults.spellcheck && !defaults.hyphenate);

    let changed = AppSettings {
        trash_days: 7,
        font_body: "Literata".into(),
        font_mono: "JetBrains Mono".into(),
        line_height: 1.7,
        paragraph_spacing: 0.75,
        align: TextAlign::Justify,
        hyphenate: true,
        smart_typography: false,
        spellcheck: false,
        spell_language: "ru".into(),
        new_note_folder: "last".into(),
    };
    d.set_settings(changed.clone()).unwrap();
    assert_eq!(d.settings().unwrap(), changed);
    let d = reopen(d);
    assert_eq!(d.settings().unwrap(), changed, "kept across a start");
}

#[test]
fn s12_only_the_fields_that_changed_are_written() {
    let d = device();
    d.set_settings(AppSettings::default()).unwrap();
    assert_eq!(
        d.setting_for_tests("trash_days"),
        None,
        "nothing differs from what is in force"
    );
    d.set_settings(AppSettings {
        trash_days: 10,
        ..AppSettings::default()
    })
    .unwrap();
    assert_eq!(d.setting_for_tests("trash_days"), Some(json!(10)));
    assert_eq!(
        d.setting_for_tests("font_body"),
        None,
        "a field not touched is not taken from this device"
    );
}

#[test]
fn s12_a_value_the_version_cannot_read_applies_the_default() {
    let d = device();
    for (field, value) in [
        ("trash_days", json!("many")),
        ("trash_days", json!(0)),
        ("trash_days", json!(3651)),
        ("trash_days", json!(2.5)),
        ("align", json!("center")),
        ("hyphenate", json!("yes")),
        ("line_height", json!("tall")),
        ("font_body", json!(7)),
        ("spell_language", json!("")),
        ("new_note_folder", json!(null)),
    ] {
        d.set_setting_for_tests(field, value.clone());
        assert_eq!(d.settings().unwrap(), AppSettings::default(), "{field} = {value}");
    }
    d.set_setting_for_tests("trash_days", json!(3650));
    assert_eq!(d.settings().unwrap().trash_days, 3650);
    d.set_setting_for_tests("trash_days", json!(1));
    assert_eq!(d.settings().unwrap().trash_days, 1);
}

#[test]
fn s12_the_days_in_the_trash_are_checked_where_they_are_set() {
    let d = device();
    for days in [0, 3651] {
        let error = d
            .set_settings(AppSettings {
                trash_days: days,
                ..AppSettings::default()
            })
            .unwrap_err();
        assert!(matches!(error, AppError::Invalid { .. }), "{error}");
    }
    let error = d
        .set_settings(AppSettings {
            line_height: f64::NAN,
            ..AppSettings::default()
        })
        .unwrap_err();
    assert!(matches!(error, AppError::Invalid { .. }), "{error}");
    let error = d
        .set_settings(AppSettings {
            font_body: String::new(),
            ..AppSettings::default()
        })
        .unwrap_err();
    assert!(matches!(error, AppError::Invalid { .. }), "{error}");
    d.set_settings(AppSettings {
        trash_days: 3650,
        ..AppSettings::default()
    })
    .unwrap();
}

// ---- the store itself ----

#[test]
fn data_survives_reopening() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().to_string_lossy().into_owned();
    let id = {
        let store = Store::open(path.clone()).unwrap();
        store
            .create_note(NewNote {
                title: "помнить".into(),
                ..NewNote::default()
            })
            .unwrap()
            .id
    };
    let store = Store::open(path).unwrap();
    assert_eq!(store.get_note(id.clone()).unwrap().title, "помнить");
    // The clock continues past what is stored, so a new edit wins over the old value.
    store.set_title(id.clone(), "помнить всё".into()).unwrap();
    assert_eq!(store.get_note(id).unwrap().title, "помнить всё");
}

#[test]
fn derived_tables_are_rebuilt_when_their_shape_changes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().to_string_lossy().into_owned();
    let (note, folder) = {
        let store = Store::open(path.clone()).unwrap();
        let folder = store.create_folder("папка".into(), None).unwrap();
        let note = store
            .create_note(NewNote {
                title: "переживёт".into(),
                folder_id: Some(folder.id.clone()),
                ..NewNote::default()
            })
            .unwrap();
        store.add_tag(note.id.clone(), "тег".into()).unwrap();
        (note.id, folder.id)
    };
    // What an older version of the app left behind: derived tables of another shape.
    {
        let conn = rusqlite::Connection::open(dir.path().join("notes.sqlite")).unwrap();
        conn.execute_batch(
            "DROP TABLE notes; CREATE TABLE notes (id TEXT PRIMARY KEY, title TEXT);
             UPDATE meta SET value = '0' WHERE key = 'derived_version';",
        )
        .unwrap();
    }
    let store = Store::open(path).unwrap();
    let got = store.get_note(note).unwrap();
    assert_eq!(got.title, "переживёт");
    assert_eq!(got.tags, ["тег"]);
    assert_eq!(got.folder_id.as_deref(), Some(folder.as_str()));
}
