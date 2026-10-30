//! R75: one history of undo for every action that changes a note or a folder: docs/specs/product.md.

mod common;

use common::*;
use notes_core::*;

#[test]
fn r75_the_last_action_is_taken_back_first_and_an_empty_history_changes_nothing() {
    let d = device();
    let home = new_folder(&d, "Дом", None);
    let work = new_folder(&d, "Работа", None);
    let a = add_in(&d, "а", &home);
    let in_folder = |f: &Folder| view(&d, Scope::Folder { id: f.id.clone() });
    d.set_starred(a.id.clone(), true).unwrap();
    assert!(d.can_undo());

    d.set_title(a.id.clone(), "б".into()).unwrap();
    d.move_note(a.id.clone(), Some(work.id.clone())).unwrap();
    assert_eq!(in_folder(&work), ["б"]);

    assert_eq!(d.undo().unwrap(), 1);
    assert_eq!(in_folder(&home), ["б"], "back in its folder");
    assert!(d.get_note(a.id.clone()).unwrap().starred);
    d.undo().unwrap();
    assert_eq!(d.get_note(a.id.clone()).unwrap().title, "а");
    d.undo().unwrap();
    assert!(!d.get_note(a.id.clone()).unwrap().starred);

    // Down to the creation of the note, which goes to the Trash.
    while d.can_undo() {
        d.undo().unwrap();
    }
    assert_eq!(d.undo().unwrap(), 0);
    assert!(in_folder(&home).is_empty());
    assert_eq!(view(&d, Scope::Trash), ["а"]);
}

/// One action on the note.
type Edit<'a> = Box<dyn Fn(&Store) + 'a>;

#[test]
fn r75_every_field_of_a_note_is_taken_back() {
    let d = device();
    let folder = new_folder(&d, "Папка", None);
    let n = add(&d, "а");
    let before = d.get_note(n.id.clone()).unwrap();
    let edits: Vec<Edit> = vec![
        Box::new(|s| s.set_title(n.id.clone(), "б".into()).unwrap()),
        Box::new(|s| s.add_tag(n.id.clone(), "дом".into()).unwrap()),
        Box::new(|s| s.set_pinned(n.id.clone(), true).unwrap()),
        Box::new(|s| s.set_starred(n.id.clone(), true).unwrap()),
        Box::new(|s| s.set_archived(n.id.clone(), true).unwrap()),
        Box::new(|s| s.set_locked(n.id.clone(), true).unwrap()),
        Box::new(|s| s.move_note(n.id.clone(), Some(folder.id.clone())).unwrap()),
        Box::new(|s| s.delete_note(n.id.clone()).unwrap()),
    ];
    for edit in &edits {
        edit(&d);
    }
    for _ in &edits {
        assert_eq!(d.undo().unwrap(), 1);
    }
    let back = d.get_note(n.id.clone()).unwrap();
    assert_eq!(
        (back.title, back.folder_id, back.tags, back.pinned, back.starred),
        (before.title, before.folder_id, before.tags, false, false)
    );
    assert!(!(back.archived || back.locked || back.deleted));

    d.add_tag(n.id.clone(), "дом".into()).unwrap();
    d.remove_tag(n.id.clone(), "дом".into()).unwrap();
    d.undo().unwrap();
    assert_eq!(d.get_note(n.id.clone()).unwrap().tags, ["дом"]);
    drop(edits);
    d.delete_note(n.id.clone()).unwrap();
    d.restore_note(n.id.clone()).unwrap();
    d.undo().unwrap();
    assert!(d.get_note(n.id).unwrap().deleted, "restoring is taken back as well");
}

#[test]
fn r75_a_new_note_a_copy_and_an_attachment_are_taken_back() {
    let d = device();
    let n = add(&d, "а");
    let copy = d.duplicate_note(n.id.clone()).unwrap();
    let file = d.dir().join("note.txt");
    std::fs::write(&file, "x").unwrap();
    d.add_attachment(n.id.clone(), file.to_string_lossy().into_owned(), None)
        .unwrap();

    d.undo().unwrap();
    assert!(d.attachments(n.id.clone()).unwrap().is_empty());
    d.undo().unwrap();
    assert!(d.get_note(copy.id).unwrap().deleted);
    d.undo().unwrap();
    let gone = d.get_note(n.id).unwrap();
    assert!(gone.deleted, "a created note goes to the Trash");
    assert_eq!(gone.title, "а", "with its title");
}

#[test]
fn r75_edits_of_the_same_text_in_a_row_are_one_step() {
    let d = device();
    let n = add(&d, "а");
    d.set_title(n.id.clone(), "аб".into()).unwrap();
    d.set_title(n.id.clone(), "абв".into()).unwrap();
    d.undo().unwrap();
    assert_eq!(d.get_note(n.id.clone()).unwrap().title, "а");
    assert!(!d.get_note(n.id).unwrap().deleted, "the creation is still there");
}

#[test]
fn r75_an_action_on_several_notes_is_one_step_and_leaves_a_note_changed_since() {
    let d = device();
    let notes: Vec<Note> = ["а", "б", "в"].iter().map(|t| add(&d, t)).collect();
    d.begin_group();
    for n in &notes {
        d.set_pinned(n.id.clone(), true).unwrap();
    }
    d.end_group();
    d.end_group(); // a second end changes nothing
    d.set_starred(notes[1].id.clone(), true).unwrap();
    d.set_pinned(notes[1].id.clone(), false).unwrap();
    // Taken back in turn, the edits of "б" leave the action on all three current again.
    d.undo().unwrap();
    d.undo().unwrap();
    assert_eq!(d.undo().unwrap(), 3);
    assert!(notes.iter().all(|n| !d.get_note(n.id.clone()).unwrap().pinned));

    d.begin_group();
    for n in &notes {
        d.set_title(n.id.clone(), format!("{} новое", n.title)).unwrap();
    }
    d.end_group();
    // Changed by something that is not in the history: an older version of the app.
    d.set_note_field_for_tests(&notes[1].id, "title", serde_json::json!("чужое"));
    assert_eq!(d.undo().unwrap(), 2);
    let title = |i: usize| d.get_note(notes[i].id.clone()).unwrap().title;
    assert_eq!(
        (title(0), title(1), title(2)),
        ("а".to_string(), "чужое".to_string(), "в".to_string())
    );
}

#[test]
fn r75_the_history_is_in_memory_and_its_own_for_each_actor() {
    let d = device();
    let n = add(&d, "а");
    d.as_actor("A", || d.set_starred(n.id.clone(), true).unwrap());
    assert!(!d.as_actor("B", || d.can_undo()));
    assert_eq!(d.as_actor("B", || d.undo().unwrap()), 0);
    assert!(d.get_note(n.id.clone()).unwrap().starred);
    assert_eq!(d.as_actor("A", || d.undo().unwrap()), 1);
    assert!(!d.get_note(n.id.clone()).unwrap().starred);
    assert!(d.can_undo(), "the creation is in the history of the app");

    let again = Store::open(d.dir().to_string_lossy().into_owned()).unwrap();
    assert!(!again.can_undo(), "a store opened anew has nothing to take back");
    assert_eq!(again.undo().unwrap(), 0);
}

#[test]
fn r75_undo_itself_is_not_taken_back_and_settings_are_not_in_the_history() {
    let d = device();
    let n = add(&d, "а");
    d.set_starred(n.id.clone(), true).unwrap();
    d.undo().unwrap();
    d.set_settings(AppSettings {
        trash_days: 7,
        ..AppSettings::default()
    })
    .unwrap();
    d.create_filter("Все".into(), String::new(), FilterQuery::default())
        .unwrap();
    d.undo().unwrap();
    assert!(
        d.get_note(n.id).unwrap().deleted,
        "the step under the undone one is the creation"
    );
    assert!(!d.can_undo());
    assert_eq!(d.settings().unwrap().trash_days, 7);
}

#[test]
fn r75_the_revision_grows_with_each_action_and_stays_when_a_text_edit_joins_the_last() {
    let d = device();
    assert_eq!(d.undo_revision(), 0);
    let n = add(&d, "а");
    assert_eq!(d.undo_revision(), 1);
    // As the app on macOS makes every change: one group, whatever it holds.
    for title in ["аб", "абв"] {
        d.begin_group();
        d.set_title(n.id.clone(), title.into()).unwrap();
        d.end_group();
    }
    assert_eq!(d.undo_revision(), 2, "the second edit of the title joins the first");
    d.begin_group();
    d.end_group();
    assert_eq!(d.undo_revision(), 2, "an empty group is no action");
    d.begin_group();
    d.set_starred(n.id.clone(), true).unwrap();
    d.set_pinned(n.id.clone(), true).unwrap();
    d.end_group();
    assert_eq!(d.undo_revision(), 3);
    d.undo().unwrap();
    assert_eq!(d.undo_revision(), 3, "undo takes nothing in");
    let back = d.get_note(n.id.clone()).unwrap();
    assert_eq!((back.starred, back.pinned), (false, false));
    d.undo().unwrap();
    assert_eq!(d.get_note(n.id).unwrap().title, "а");
}

#[test]
fn r75_a_new_note_that_failed_half_way_is_taken_back_whole_and_saving_again_makes_no_twin() {
    let d = device();
    // As both apps save the card of a new note: one group, taken back at once when a write in it fails.
    let save = |tag: &str| -> std::result::Result<Note, AppError> {
        let revision = d.undo_revision();
        d.begin_group();
        let result = (|| {
            let n = add(&d, "а");
            d.add_tag(n.id.clone(), tag.into())?;
            Ok(n)
        })();
        d.end_group();
        if result.is_err() && d.undo_revision() != revision {
            d.undo().unwrap();
        }
        result
    };
    assert!(save(&"т".repeat(99)).is_err(), "a tag of 198 bytes is refused");
    assert!(view(&d, Scope::All).is_empty());
    assert_eq!(view(&d, Scope::Trash), ["а"]);

    let n = save("дом").unwrap();
    assert_eq!(view(&d, Scope::All), ["а"]);
    assert_eq!(d.get_note(n.id).unwrap().tags, ["дом"]);
}

#[test]
fn r75_deleting_a_folder_is_one_step_and_one_undo_brings_back_the_folder_and_its_notes() {
    let d = device();
    let folder = new_folder(&d, "Папка", None);
    let sub = new_folder(&d, "Вложенная", Some(&folder));
    add_in(&d, "в папке", &folder);
    add_in(&d, "во вложенной", &sub);
    d.delete_folder(folder.id.clone()).unwrap();
    assert!(d.folders().unwrap().is_empty());
    assert_eq!(view(&d, Scope::Trash).len(), 2);

    assert_eq!(d.undo().unwrap(), 1);
    assert_eq!(d.folders().unwrap().len(), 2);
    assert!(view(&d, Scope::Trash).is_empty());
    assert_eq!(view(&d, Scope::Folder { id: sub.id }), ["во вложенной"]);
}

#[test]
fn r75_redo_puts_back_what_undo_took_and_a_new_action_ends_it() {
    let d = device();
    let note = add(&d, "заметка");
    d.set_starred(note.id.clone(), true).unwrap();
    d.undo().unwrap();
    assert!(!d.get_note(note.id.clone()).unwrap().starred);
    assert!(d.can_redo());
    assert_eq!(d.redo().unwrap(), 1);
    assert!(d.get_note(note.id.clone()).unwrap().starred);
    assert!(!d.can_redo());
    // And undo takes it back again.
    d.undo().unwrap();
    assert!(!d.get_note(note.id.clone()).unwrap().starred);
    d.redo().unwrap();
    assert!(d.get_note(note.id.clone()).unwrap().starred);
    d.undo().unwrap();
    d.set_pinned(note.id.clone(), true).unwrap();
    assert!(!d.can_redo(), "a new action ends what could be put again");
    assert_eq!(d.redo().unwrap(), 0);
}

#[test]
fn r75_redo_of_a_deleted_folder_deletes_it_again() {
    let d = device();
    let folder = new_folder(&d, "Папка", None);
    add_in(&d, "в папке", &folder);
    d.delete_folder(folder.id.clone()).unwrap();
    d.undo().unwrap();
    assert_eq!(d.folders().unwrap().len(), 1);
    d.redo().unwrap();
    assert!(d.folders().unwrap().is_empty());
    assert_eq!(view(&d, Scope::Trash), ["в папке"]);
}

#[test]
fn r75_a_note_made_undone_and_redone_is_back() {
    let d = device();
    let source = add(&d, "исходная");
    d.set_text(source.id.clone(), "тема\nтекст\nдальше".into()).unwrap();
    let made = d.note_from_selection(source.id.clone(), 0, 10).unwrap();
    let linked = d.get_text(source.id.clone()).unwrap();
    d.undo().unwrap();
    assert_eq!(d.get_text(source.id.clone()).unwrap(), "тема\nтекст\nдальше");
    d.redo().unwrap();
    assert_eq!(d.get_text(source.id).unwrap(), linked);
    assert!(!d.get_note(made.id).unwrap().deleted);
}

#[test]
fn r75_two_redos_of_one_field_put_back_both_actions() {
    let d = device();
    let note = add(&d, "заметка");
    d.set_starred(note.id.clone(), true).unwrap();
    d.set_starred(note.id.clone(), false).unwrap();
    d.undo().unwrap();
    d.undo().unwrap();
    assert!(!d.get_note(note.id.clone()).unwrap().starred);
    assert_eq!(d.redo().unwrap(), 1);
    assert!(d.get_note(note.id.clone()).unwrap().starred);
    assert_eq!(d.redo().unwrap(), 1);
    assert!(!d.get_note(note.id.clone()).unwrap().starred);
    // And all the way back again.
    assert_eq!(d.undo().unwrap(), 1);
    assert!(d.get_note(note.id).unwrap().starred);
}

#[test]
fn r75_a_note_deleted_and_restored_is_redone_in_order() {
    let d = device();
    let note = add(&d, "заметка");
    d.delete_note(note.id.clone()).unwrap();
    d.restore_note(note.id.clone()).unwrap();
    d.undo().unwrap();
    d.undo().unwrap();
    d.redo().unwrap();
    assert!(d.get_note(note.id.clone()).unwrap().deleted);
    d.redo().unwrap();
    assert!(!d.get_note(note.id).unwrap().deleted);
}
