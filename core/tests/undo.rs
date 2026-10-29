//! R131: one history of undo for every action that changes a task: docs/specs/product.md.

mod common;

use common::*;
use notes_core::*;

fn weekly() -> Repeat {
    Repeat {
        freq: Freq::Weekly,
        interval: 1,
        weekdays: vec![],
        monthday: None,
        nth: None,
        nth_weekday: None,
        from_done: false,
        count: None,
        until: None,
    }
}

#[test]
fn r131_the_last_action_is_taken_back_first_and_an_empty_history_changes_nothing() {
    let d = device();
    let home = d.create_list("Дом".into()).unwrap();
    let work = d.create_list("Работа".into()).unwrap();
    for title in ["раз", "два"] {
        let t = add(&d, title);
        d.move_to_list(t.id, home.id.clone()).unwrap();
    }
    let a = add(&d, "а");
    d.move_to_list(a.id.clone(), home.id.clone()).unwrap();
    d.set_due(a.id.clone(), Some("2026-10-06".into())).unwrap();
    assert!(d.can_undo());
    let list = |id: &str| view(&d, Scope::List { id: id.into() });
    assert_eq!(list(&home.id), ["раз", "два", "а"]);

    d.set_due(a.id.clone(), Some("2026-10-05".into())).unwrap();
    d.move_task(a.id.clone(), Some(work.id.clone()), None, None).unwrap();
    assert_eq!(list(&work.id), ["а"]);

    assert_eq!(d.undo().unwrap(), 1);
    assert_eq!(list(&home.id), ["раз", "два", "а"], "back in its list, in its place");
    assert_eq!(d.task(a.id.clone()).unwrap().due.as_deref(), Some("2026-10-05"));
    d.undo().unwrap();
    assert_eq!(d.task(a.id.clone()).unwrap().due.as_deref(), Some("2026-10-06"));

    // Down to the creation of the tasks, which go to the trash.
    while d.can_undo() {
        d.undo().unwrap();
    }
    assert_eq!(d.undo().unwrap(), 0);
    assert!(list(&home.id).is_empty());
    assert_eq!(view(&d, Scope::Trash).len(), 3);
}

/// One action on the task.
type Edit<'a> = Box<dyn Fn(&Store) + 'a>;

#[test]
fn r131_every_field_of_a_task_is_taken_back() {
    let d = device();
    let t = add(&d, "а");
    let before = d.task(t.id.clone()).unwrap();
    let edits: Vec<Edit> = vec![
        Box::new(|s| s.set_title(t.id.clone(), "б".into()).unwrap()),
        Box::new(|s| s.set_notes(t.id.clone(), "заметка".into()).unwrap()),
        Box::new(|s| s.set_start(t.id.clone(), Some("2026-10-04".into())).unwrap()),
        Box::new(|s| s.set_due(t.id.clone(), Some("2026-10-07T18:00".into())).unwrap()),
        Box::new(|s| s.set_remind(t.id.clone(), Some("2026-10-07T17:00".into())).unwrap()),
        Box::new(|s| s.set_repeat(t.id.clone(), Some(weekly())).unwrap()),
        Box::new(|s| s.set_priority(t.id.clone(), Priority::High).unwrap()),
        Box::new(|s| s.add_tag(t.id.clone(), "дом".into()).unwrap()),
        Box::new(|s| s.set_project(t.id.clone(), true).unwrap()),
        Box::new(|s| s.delete_task(t.id.clone()).unwrap()),
    ];
    for edit in &edits {
        edit(&d);
    }
    for _ in &edits {
        assert_eq!(d.undo().unwrap(), 1);
    }
    let back = d.task(t.id.clone()).unwrap();
    assert_eq!(
        (back.title, back.notes, back.start, back.due, back.remind, back.repeat),
        (
            before.title,
            before.notes,
            before.start,
            before.due,
            before.remind,
            before.repeat
        )
    );
    assert_eq!(back.priority, Priority::None);
    assert!(back.tags.is_empty());
    assert!(!back.is_project && !back.deleted);

    d.add_tag(t.id.clone(), "дом".into()).unwrap();
    d.remove_tag(t.id.clone(), "дом".into()).unwrap();
    d.undo().unwrap();
    assert_eq!(d.task(t.id.clone()).unwrap().tags, ["дом"]);
    drop(edits);
    d.delete_task(t.id.clone()).unwrap();
    d.restore_task(t.id.clone()).unwrap();
    d.undo().unwrap();
    assert!(d.task(t.id).unwrap().deleted, "restoring is taken back as well");
}

#[test]
fn r131_completion_and_reopening_are_taken_back_whatever_r68_says() {
    let d = device();
    d.set_keep_done(KeepDone::Seconds { seconds: 5 }).unwrap();
    let t = add(&d, "а");
    let early = add_sub(&d, &t, "раньше");
    let late = add_sub(&d, &t, "вместе");
    d.complete_task(early.id.clone()).unwrap();
    d.complete_task(t.id.clone()).unwrap();
    d.undo().unwrap();
    assert!(d.task(t.id.clone()).unwrap().done.is_none());
    assert!(d.task(late.id.clone()).unwrap().done.is_none());
    assert!(
        d.task(early.id.clone()).unwrap().done.is_some(),
        "R11: closed before stays closed"
    );

    d.wont_do_task(t.id.clone()).unwrap();
    d.reopen_task(t.id.clone()).unwrap();
    d.undo().unwrap();
    let closed = d.task(t.id.clone()).unwrap();
    assert!(closed.done.is_some() && closed.wont, "reopening is taken back");
    d.undo().unwrap();
    assert!(d.task(t.id).unwrap().done.is_none());
}

#[test]
fn r131_r128_the_completion_of_a_repeat_is_taken_back() {
    let d = device();
    let t = add(&d, "каждую неделю");
    d.set_due(t.id.clone(), Some("2026-10-05T19:00".into())).unwrap();
    d.set_repeat(t.id.clone(), Some(weekly())).unwrap();
    d.complete_task(t.id.clone()).unwrap();
    assert_eq!(view(&d, Scope::Completed), ["каждую неделю"]);

    assert_eq!(d.undo().unwrap(), 1);
    assert_eq!(d.task(t.id.clone()).unwrap().due.as_deref(), Some("2026-10-05T19:00"));
    assert!(view(&d, Scope::Completed).is_empty(), "the record has gone");
    assert!(view(&d, Scope::Trash).is_empty(), "and not to the trash");

    // The same occurrence completed again comes back as the same record, and goes the same way.
    d.complete_task(t.id.clone()).unwrap();
    assert_eq!(view(&d, Scope::Completed), ["каждую неделю"]);
    d.undo().unwrap();
    assert!(view(&d, Scope::Completed).is_empty());
    assert_eq!(d.task(t.id).unwrap().due.as_deref(), Some("2026-10-05T19:00"));
}

#[test]
fn r131_r133_reopening_a_record_from_completed_is_taken_back() {
    let d = device();
    let t = add(&d, "каждую неделю");
    d.set_due(t.id.clone(), Some("2026-10-05T19:00".into())).unwrap();
    d.set_repeat(t.id.clone(), Some(weekly())).unwrap();
    d.complete_task(t.id.clone()).unwrap();
    let record = d.tasks(Scope::Completed).unwrap().remove(0);
    d.reopen_task(record.id).unwrap();
    assert!(view(&d, Scope::Completed).is_empty());

    assert_eq!(d.undo().unwrap(), 1);
    assert_eq!(view(&d, Scope::Completed), ["каждую неделю"], "completed again");
    assert_eq!(d.task(t.id).unwrap().due.as_deref(), Some("2026-10-12T19:00"));
    assert!(d.tasks(Scope::Completed).unwrap()[0].undoable);
}

#[test]
fn r131_a_new_task_a_copy_a_project_and_an_attachment_are_taken_back() {
    let d = device();
    let t = d.quick_add("а завтра".into(), None, false, None).unwrap();
    let project = d.create_project("п".into(), None).unwrap();
    let copy = d.duplicate_task(t.id.clone()).unwrap();
    let subs = d.add_subtasks(t.id.clone(), vec!["б".into(), "в".into()]).unwrap();
    let file = d.dir().join("note.txt");
    std::fs::write(&file, "x").unwrap();
    d.add_attachment(t.id.clone(), file.to_string_lossy().into_owned(), None)
        .unwrap();

    d.undo().unwrap();
    assert!(d.attachments(t.id.clone()).unwrap().is_empty());
    d.undo().unwrap();
    assert!(
        subs.iter().all(|s| d.task(s.id.clone()).unwrap().deleted),
        "both subtasks in one step"
    );
    d.undo().unwrap();
    assert!(d.task(copy.id).unwrap().deleted);
    d.undo().unwrap();
    assert!(d.task(project.id).unwrap().deleted);
    d.undo().unwrap();
    let gone = d.task(t.id).unwrap();
    assert!(gone.deleted, "a created task goes to the trash");
    assert_eq!(gone.title, "а", "with its title");
}

#[test]
fn r131_edits_of_the_same_text_in_a_row_are_one_step() {
    let d = device();
    let t = add(&d, "а");
    d.set_title(t.id.clone(), "аб".into()).unwrap();
    d.set_title(t.id.clone(), "абв".into()).unwrap();
    d.set_notes(t.id.clone(), "x".into()).unwrap();
    d.set_notes(t.id.clone(), "xy".into()).unwrap();
    d.undo().unwrap();
    assert_eq!(d.task(t.id.clone()).unwrap().notes, "");
    d.undo().unwrap();
    assert_eq!(d.task(t.id.clone()).unwrap().title, "а");
    assert!(!d.task(t.id).unwrap().deleted, "the creation is still there");
}

#[test]
fn r131_an_action_on_several_tasks_is_one_step_and_leaves_a_task_changed_since() {
    let d = device();
    let tasks: Vec<TaskItem> = ["а", "б", "в"].iter().map(|t| add(&d, t)).collect();
    d.begin_group();
    for t in &tasks {
        d.set_due(t.id.clone(), Some("2026-10-12".into())).unwrap();
    }
    d.end_group();
    d.end_group(); // a second end changes nothing
    d.set_priority(tasks[1].id.clone(), Priority::High).unwrap();
    d.set_due(tasks[1].id.clone(), Some("2026-11-01".into())).unwrap();
    // Taken back in turn, the edits of "б" leave the action on all three current again.
    d.undo().unwrap();
    d.undo().unwrap();
    assert_eq!(d.undo().unwrap(), 3);
    assert!(tasks.iter().all(|t| d.task(t.id.clone()).unwrap().due.is_none()));

    d.begin_group();
    for t in &tasks {
        d.set_due(t.id.clone(), Some("2026-10-12".into())).unwrap();
    }
    d.end_group();
    // Changed by something that is not in the history: CalDAV, an older version.
    d.set_task_field_for_tests(&tasks[1].id, "due", serde_json::json!("2026-11-01"));
    assert_eq!(d.undo().unwrap(), 2);
    let due = |i: usize| d.task(tasks[i].id.clone()).unwrap().due;
    assert_eq!((due(0), due(1), due(2)), (None, Some("2026-11-01".into()), None));
}

#[test]
fn r131_the_history_is_in_memory_and_its_own_for_each_actor() {
    let d = device();
    let t = add(&d, "а");
    d.as_actor("A", || d.set_due(t.id.clone(), Some("2026-10-06".into())).unwrap());
    assert!(!d.as_actor("B", || d.can_undo()));
    assert_eq!(d.as_actor("B", || d.undo().unwrap()), 0);
    assert_eq!(d.task(t.id.clone()).unwrap().due.as_deref(), Some("2026-10-06"));
    assert_eq!(d.as_actor("A", || d.undo().unwrap()), 1);
    assert!(d.task(t.id.clone()).unwrap().due.is_none());
    assert!(d.can_undo(), "the creation is in the history of the app");

    let again = Store::open(d.dir().to_string_lossy().into_owned()).unwrap();
    assert!(!again.can_undo(), "a store opened anew has nothing to take back");
    assert_eq!(again.undo().unwrap(), 0);
}

#[test]
fn r131_undo_itself_is_not_taken_back_and_settings_are_not_in_the_history() {
    let d = device();
    let t = add(&d, "а");
    d.set_due(t.id.clone(), Some("2026-10-06".into())).unwrap();
    d.undo().unwrap();
    d.set_keep_done(KeepDone::Seconds { seconds: 0 }).unwrap();
    d.create_list("Дом".into()).unwrap();
    d.undo().unwrap();
    assert!(
        d.task(t.id).unwrap().deleted,
        "the step under the undone one is the creation"
    );
    assert!(!d.can_undo());
}

#[test]
fn r131_the_revision_grows_with_each_action_and_stays_when_a_text_edit_joins_the_last() {
    let d = device();
    assert_eq!(d.undo_revision(), 0);
    let t = add(&d, "а");
    assert_eq!(d.undo_revision(), 1);
    // As the app on macOS makes every change: one group, whatever it holds.
    for title in ["аб", "абв"] {
        d.begin_group();
        d.set_title(t.id.clone(), title.into()).unwrap();
        d.end_group();
    }
    assert_eq!(d.undo_revision(), 2, "the second edit of the title joins the first");
    d.begin_group();
    d.end_group();
    assert_eq!(d.undo_revision(), 2, "an empty group is no action");
    d.begin_group();
    d.set_due(t.id.clone(), Some("2026-10-06".into())).unwrap();
    d.set_priority(t.id.clone(), Priority::High).unwrap();
    d.end_group();
    assert_eq!(d.undo_revision(), 3);
    d.undo().unwrap();
    assert_eq!(d.undo_revision(), 3, "undo takes nothing in");
    let back = d.task(t.id.clone()).unwrap();
    assert_eq!((back.due, back.priority), (None, Priority::None));
    d.undo().unwrap();
    assert_eq!(d.task(t.id).unwrap().title, "а");
}

#[test]
fn r139_a_new_task_that_failed_half_way_is_taken_back_whole_and_saving_again_makes_no_twin() {
    let d = device();
    // As both apps save the card of a new task: one group, taken back at once when a write in it fails.
    let save = |tag: &str| -> std::result::Result<TaskItem, AppError> {
        let revision = d.undo_revision();
        d.begin_group();
        let result = (|| {
            let t = add(&d, "а");
            d.set_notes(t.id.clone(), "заметка".into())?;
            d.add_tag(t.id.clone(), tag.into())?;
            Ok(t)
        })();
        d.end_group();
        if result.is_err() && d.undo_revision() != revision {
            d.undo().unwrap();
        }
        result
    };
    assert!(save(&"т".repeat(99)).is_err(), "a tag of 198 bytes is refused");
    assert!(view(&d, Scope::Inbox).is_empty());
    assert_eq!(view(&d, Scope::Trash), ["а"]);

    let t = save("дом").unwrap();
    assert_eq!(view(&d, Scope::Inbox), ["а"]);
    let back = d.task(t.id).unwrap();
    assert_eq!((back.notes.as_str(), back.tags), ("заметка", vec!["дом".to_string()]));
}
