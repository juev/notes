//! Behaviour of lists, tasks, subtasks and recurrence: docs/specs/product.md.

mod common;

use common::*;
use notes_core::*;
use serde_json::json;

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
fn r1_inbox_always_exists_and_cannot_be_removed() {
    let d = device();
    let lists = d.lists().unwrap();
    assert_eq!(lists.len(), 1);
    assert_eq!(lists[0].id, INBOX_ID);
    assert!(d.delete_list(INBOX_ID.into()).is_err());
    assert!(d.set_list_archived(INBOX_ID.into(), true).is_err());

    add(&d, "без списка");
    assert_eq!(view(&d, Scope::Inbox), ["без списка"]);
}

#[test]
fn r2_list_settings_are_stored_and_applied() {
    let d = device();
    let list = d.create_list("Работа".into()).unwrap();
    d.set_list_color(list.id.clone(), "#3478F6".into()).unwrap();
    d.set_list_icon(list.id.clone(), "briefcase".into()).unwrap();
    d.set_list_defaults(list.id.clone(), Priority::High, true).unwrap();

    let got = d.list(list.id.clone()).unwrap();
    assert_eq!((got.color.as_str(), got.icon.as_str()), ("#3478F6", "briefcase"));

    let in_list = |title: &str| NewTask {
        title: title.into(),
        list_id: Some(list.id.clone()),
        ..NewTask::default()
    };
    let b = d.create_task(in_list("b")).unwrap();
    assert_eq!(b.priority, Priority::High, "list default priority");
    assert_eq!(b.due.as_deref(), Some("2026-10-05"), "list default due");
    let a = d
        .create_task(NewTask {
            priority: Some(Priority::Low),
            due: Some("2026-10-01".into()),
            ..in_list("a")
        })
        .unwrap();
    assert_eq!(a.priority, Priority::Low, "explicit value wins over the default");

    let of = |d: &Device| view(d, Scope::List { id: list.id.clone() });
    assert_eq!(of(&d), ["b", "a"], "manual order is creation order");
    d.set_list_sort(list.id.clone(), SortMode::Title).unwrap();
    assert_eq!(of(&d), ["a", "b"]);
    d.set_list_sort(list.id.clone(), SortMode::Priority).unwrap();
    assert_eq!(of(&d), ["b", "a"]);
    d.set_list_sort(list.id.clone(), SortMode::Due).unwrap();
    assert_eq!(of(&d), ["a", "b"]);

    d.complete_task(a.id).unwrap();
    // A task completed a moment ago stays where it was (R68); this is about the time after that.
    d.set_now_for_tests("2026-10-05T10:30");
    assert_eq!(of(&d), ["b"]);
    d.set_list_show_done(list.id.clone(), true).unwrap();
    assert_eq!(of(&d), ["b", "a"], "completed tasks go last");
    d.set_list_show_done(list.id.clone(), false).unwrap();
    assert_eq!(of(&d), ["b"], "switched off, completed tasks leave the list again");
    assert_eq!(d.list(list.id).unwrap().open_count, 1);
}

#[test]
fn r3_lists_are_reordered_archived_and_deleted() {
    let d = device();
    let a = d.create_list("A".into()).unwrap();
    let b = d.create_list("B".into()).unwrap();
    let c = d.create_list("C".into()).unwrap();
    let names = |d: &Device| d.lists().unwrap().into_iter().map(|l| l.name).collect::<Vec<_>>();
    assert_eq!(names(&d)[1..], ["A", "B", "C"]);
    d.move_list(c.id.clone(), None).unwrap();
    assert_eq!(names(&d)[1..], ["C", "A", "B"]);
    d.move_list(c.id.clone(), Some(a.id.clone())).unwrap();
    assert_eq!(names(&d)[1..], ["A", "C", "B"]);

    let task = d
        .create_task(NewTask {
            title: "в B".into(),
            list_id: Some(b.id.clone()),
            due: Some("2026-10-05".into()),
            ..NewTask::default()
        })
        .unwrap();
    d.set_list_archived(b.id.clone(), true).unwrap();
    assert!(view(&d, Scope::Today).is_empty(), "archived lists stay out of Today");
    d.set_list_archived(b.id.clone(), false).unwrap();
    assert_eq!(view(&d, Scope::Today), ["в B"]);

    d.delete_list(b.id.clone()).unwrap();
    assert_eq!(names(&d)[1..], ["A", "C"]);
    assert_eq!(view(&d, Scope::Trash), ["в B"]);
    // S6: restored without its list, the task shows up in the inbox.
    d.restore_task(task.id).unwrap();
    assert_eq!(view(&d, Scope::Inbox), ["в B"]);
}

#[test]
fn r4_fields_round_trip() {
    let d = device();
    let t = add(&d, "  задача  ");
    assert_eq!(t.title, "задача");
    assert!(d
        .create_task(NewTask {
            title: "   ".into(),
            ..NewTask::default()
        })
        .is_err());

    d.set_notes(t.id.clone(), "заметка".into()).unwrap();
    d.set_start(t.id.clone(), Some("2026-10-06".into())).unwrap();
    d.set_due(t.id.clone(), Some("2026-10-07T09:30".into())).unwrap();
    d.set_priority(t.id.clone(), Priority::Medium).unwrap();
    d.set_remind(t.id.clone(), Some("2026-10-07T09:00".into())).unwrap();
    d.add_tag(t.id.clone(), "#Дом".into()).unwrap();
    d.add_tag(t.id.clone(), "срочно".into()).unwrap();
    d.remove_tag(t.id.clone(), "срочно".into()).unwrap();

    let got = d.task(t.id.clone()).unwrap();
    assert_eq!(got.notes, "заметка");
    assert_eq!(got.start.as_deref(), Some("2026-10-06"));
    assert_eq!(got.due.as_deref(), Some("2026-10-07T09:30"));
    assert_eq!(got.priority, Priority::Medium);
    assert_eq!(got.remind.as_deref(), Some("2026-10-07T09:00"));
    assert_eq!(got.tags, ["дом"]);

    assert!(d.set_due(t.id.clone(), Some("завтра".into())).is_err());
    assert!(d.set_due(t.id.clone(), Some("2026-13-40".into())).is_err());
    d.set_due(t.id.clone(), None).unwrap();
    assert_eq!(d.task(t.id).unwrap().due, None);
}

#[test]
fn r5_start_date_hides_the_task_from_today() {
    let d = device();
    let t = add(&d, "позже");
    d.set_start(t.id.clone(), Some("2026-10-06".into())).unwrap();
    assert!(view(&d, Scope::Today).is_empty());
    assert_eq!(view(&d, Scope::Upcoming), ["позже"]);

    d.set_start(t.id.clone(), Some("2026-10-05".into())).unwrap();
    assert_eq!(view(&d, Scope::Today), ["позже"]);
    assert!(view(&d, Scope::Upcoming).is_empty());

    // Due today but not startable until tomorrow: still hidden.
    d.set_due(t.id.clone(), Some("2026-10-05".into())).unwrap();
    d.set_start(t.id, Some("2026-10-06".into())).unwrap();
    assert!(view(&d, Scope::Today).is_empty());
}

#[test]
fn r7_tags_list_only_those_in_use() {
    let d = device();
    let a = add(&d, "a");
    let b = add(&d, "b");
    d.add_tag(a.id.clone(), "дом".into()).unwrap();
    d.add_tag(b.id.clone(), "дом".into()).unwrap();
    d.add_tag(b.id.clone(), "офис".into()).unwrap();
    let tags = |d: &Device| {
        d.tags()
            .unwrap()
            .into_iter()
            .map(|t| (t.name, t.open_count))
            .collect::<Vec<_>>()
    };
    assert_eq!(tags(&d), [("дом".to_string(), 2), ("офис".to_string(), 1)]);
    assert_eq!(view(&d, Scope::Tag { name: "Дом".into() }), ["a", "b"]);

    d.complete_task(b.id).unwrap();
    assert_eq!(tags(&d), [("дом".to_string(), 1)]);
    assert!(d.add_tag(a.id, "два слова".into()).is_err());
}

#[test]
fn s51_a_tag_at_the_limit_of_a_field_is_taken_and_a_longer_one_is_refused() {
    let d = device();
    let t = add(&d, "задача");
    let latin = "a".repeat(196);
    let cyrillic = "я".repeat(98);
    d.add_tag(t.id.clone(), latin.clone()).unwrap();
    d.add_tag(t.id.clone(), cyrillic.clone()).unwrap();
    assert_eq!(d.task(t.id.clone()).unwrap().tags, [latin.clone(), cyrillic.clone()]);

    // One byte more than the field can carry: 197 Latin letters, 99 Cyrillic ones.
    for too_long in ["a".repeat(197), "я".repeat(99)] {
        let error = d.add_tag(t.id.clone(), too_long.clone()).unwrap_err().to_string();
        assert!(error.contains("196 bytes"), "{error}");
        assert!(
            d.remove_tag(t.id.clone(), too_long).is_err(),
            "no field is written either"
        );
    }
    assert_eq!(d.task(t.id).unwrap().tags, [latin, cyrillic]);
}

#[test]
fn s51_quick_add_refuses_a_tag_that_the_register_cannot_carry() {
    let d = device();
    let too_long = "a".repeat(197);
    let error = d
        .quick_add(format!("задача #{too_long}"), None, true, None)
        .unwrap_err()
        .to_string();
    assert!(error.contains("196 bytes"), "{error}");
    assert!(view(&d, Scope::All).is_empty(), "the task is not created");

    let fits = "я".repeat(98);
    let task = d.quick_add(format!("задача #{fits}"), None, true, None).unwrap();
    assert_eq!(task.tags, [fits]);
}

#[test]
fn s51_a_tag_from_before_the_limit_leaves_the_view_after_an_upgrade() {
    let dir = tempfile::tempdir().unwrap();
    let t = {
        let store = Store::open(dir.path().to_string_lossy().into_owned()).unwrap();
        store.set_now_for_tests(NOW);
        add(&store, "задача")
    };
    let long = "a".repeat(197);
    {
        let conn = rusqlite::Connection::open(dir.path().join("lists.sqlite")).unwrap();
        let stamp: String = conn
            .query_row("SELECT max(stamp) FROM fields", [], |r| r.get(0))
            .unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO fields (kind, id, field, value, stamp, dirty)
             VALUES ('task', ?1, ?2, 'true', ?3, 0)",
            rusqlite::params![t.id, format!("tag:{long}"), stamp],
        )
        .unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO task_tags (task_id, tag) VALUES (?1, ?2)",
            rusqlite::params![t.id, long],
        )
        .unwrap();
        // The derived tables are as version 4, the one before S51, left them.
        conn.execute("UPDATE meta SET value = '4' WHERE key = 'derived_version'", [])
            .unwrap();
    }

    let store = Store::open(dir.path().to_string_lossy().into_owned()).unwrap();
    assert!(
        store.task(t.id).unwrap().tags.is_empty(),
        "a tag no register can carry is not shown"
    );
}

#[test]
fn r9_trash_restore_and_purge() {
    let d = device();
    let t = add(&d, "мусор");
    d.set_project(t.id.clone(), true).unwrap();
    let sub = add_sub(&d, &t, "часть");
    d.set_due(sub.id.clone(), Some("2026-10-05".into())).unwrap();
    assert_eq!(view(&d, Scope::Today), ["часть"]);

    d.delete_task(t.id.clone()).unwrap();
    assert!(view(&d, Scope::Inbox).is_empty());
    assert!(
        view(&d, Scope::Today).is_empty(),
        "subtasks of a deleted task are hidden too"
    );
    assert_eq!(view(&d, Scope::Trash), ["мусор"]);
    assert_eq!(d.counts().unwrap().trash, 1);

    d.restore_task(t.id.clone()).unwrap();
    assert_eq!(
        titles(&d.projects().unwrap()),
        ["мусор"],
        "a project is restored to the sidebar (R120)"
    );
    assert_eq!(view(&d, Scope::Today), ["часть"]);

    d.delete_task(t.id.clone()).unwrap();
    assert_eq!(d.empty_trash().unwrap(), 1);
    assert!(view(&d, Scope::Trash).is_empty());
    assert!(d.task(t.id).is_err());
}

#[test]
fn r46_completed_is_cleared_in_whole_or_before_a_day() {
    let d = device();
    d.set_now_for_tests("2026-08-01T09:00");
    let old = add(&d, "давняя");
    add_sub(&d, &old, "её часть");
    d.complete_task(old.id.clone()).unwrap();
    d.set_now_for_tests("2026-08-01T10:00");
    let rep = add(&d, "зарядка");
    d.set_due(rep.id.clone(), Some("2026-08-01".into())).unwrap();
    d.set_repeat(rep.id.clone(), Some(weekly())).unwrap();
    d.complete_task(rep.id.clone()).unwrap();
    d.set_now_for_tests("2026-09-20T09:00");
    let mid = add(&d, "сентябрьская");
    d.complete_task(mid.id).unwrap();
    d.set_now_for_tests(NOW);
    let fresh = add(&d, "свежая");
    d.complete_task(fresh.id).unwrap();
    let open = add(&d, "в работе");
    let part = add_sub(&d, &open, "сделанная часть");
    d.complete_task(part.id.clone()).unwrap();
    let binned = add(&d, "в корзине");
    d.complete_task(binned.id.clone()).unwrap();
    d.delete_task(binned.id.clone()).unwrap();
    assert_eq!(
        view(&d, Scope::Completed),
        ["свежая", "сентябрьская", "зарядка", "давняя"]
    );

    assert!(d.clear_completed(Some("в августе".into())).is_err());
    assert!(
        d.clear_completed(Some(String::new())).is_err(),
        "an empty day does not mean everything"
    );
    assert_eq!(
        d.clear_completed(Some("2026-08-01".into())).unwrap(),
        0,
        "the day itself is kept"
    );
    assert_eq!(d.clear_completed(Some("2026-09-05".into())).unwrap(), 2);
    assert_eq!(view(&d, Scope::Completed), ["свежая", "сентябрьская"]);
    assert!(d.task(old.id).is_err(), "cleared for good, not moved to the trash");
    assert!(
        view(
            &d,
            Scope::Search {
                text: "её часть".into()
            }
        )
        .is_empty(),
        "subtasks go with their task"
    );
    assert!(
        d.task(rep.id.clone()).unwrap().done.is_none(),
        "the repeating task itself stays"
    );
    assert_eq!(view(&d, Scope::Trash), ["в корзине"]);

    assert_eq!(d.clear_completed(None).unwrap(), 2);
    assert!(view(&d, Scope::Completed).is_empty());
    assert_eq!(view(&d, Scope::Inbox), ["зарядка", "в работе"]);
    assert!(
        d.task(part.id).unwrap().done.is_some(),
        "a completed subtask of an open task stays"
    );
    assert_eq!(view(&d, Scope::Trash), ["в корзине"]);
    d.restore_task(binned.id).unwrap();
    assert_eq!(view(&d, Scope::Completed), ["в корзине"], "the trash was not cleared");
}

#[test]
fn r46_completed_lists_the_whole_log() {
    let d = device();
    let t = add(&d, "зарядка");
    d.set_due(t.id.clone(), Some("2026-10-05".into())).unwrap();
    let mut daily = weekly();
    daily.freq = Freq::Daily;
    d.set_repeat(t.id.clone(), Some(daily)).unwrap();
    for _ in 0..501 {
        d.complete_task(t.id.clone()).unwrap();
    }
    assert_eq!(d.tasks(Scope::Completed).unwrap().len(), 501);
}

#[test]
fn r10_a_subtask_has_a_title_only() {
    let d = device();
    let top = add(&d, "а");
    let other = add(&d, "в");
    let sub = add_sub(&d, &top, "б");

    // No field but the title.
    assert!(d.set_due(sub.id.clone(), Some("2026-10-09".into())).is_err());
    assert!(d.set_start(sub.id.clone(), Some("2026-10-09".into())).is_err());
    assert!(d.set_notes(sub.id.clone(), "заметка".into()).is_err());
    assert!(d.set_priority(sub.id.clone(), Priority::High).is_err());
    assert!(d.set_remind(sub.id.clone(), Some("2026-10-09T09:00".into())).is_err());
    assert!(d.set_repeat(sub.id.clone(), Some(weekly())).is_err());
    assert!(d.add_tag(sub.id.clone(), "дом".into()).is_err());
    let file = d.dir().join("файл.txt");
    std::fs::write(&file, b"1").unwrap();
    assert!(d
        .add_attachment(sub.id.clone(), file.to_string_lossy().into_owned(), None)
        .is_err());
    let with_due = NewTask {
        title: "г".into(),
        parent_id: Some(top.id.clone()),
        due: Some("2026-10-09".into()),
        ..NewTask::default()
    };
    assert!(d.create_task(with_due).is_err());
    // The line typed under a task is the title as it stands: nothing is picked out of it.
    let typed = d
        .quick_add_under("молоко завтра !! #дом".into(), top.id.clone(), false, None)
        .unwrap();
    assert_eq!(typed.title, "молоко завтра !! #дом");
    assert_eq!((typed.due, typed.priority, typed.tags.len()), (None, Priority::None, 0));

    // No subtasks of its own, and it stays with its task.
    assert!(d.quick_add_under("глубже".into(), sub.id.clone(), false, None).is_err());
    assert!(d.move_task(sub.id.clone(), None, None, None).is_err());
    assert!(d.move_task(sub.id.clone(), None, Some(other.id.clone()), None).is_err());
    assert!(d.move_to_list(sub.id.clone(), INBOX_ID.into()).is_err());
    // A task does not become a subtask.
    assert!(d.move_task(other.id.clone(), None, Some(top.id.clone()), None).is_err());
    assert!(d.move_task(other.id.clone(), None, Some(sub.id.clone()), None).is_err());

    // What a line can do: be renamed, reordered, closed and deleted.
    d.set_title(sub.id.clone(), "б2".into()).unwrap();
    assert_eq!(
        titles(&d.subtasks(top.id.clone()).unwrap()),
        ["б2", "молоко завтра !! #дом"]
    );
    d.move_task(sub.id.clone(), None, Some(top.id.clone()), Some(typed.id.clone()))
        .unwrap();
    assert_eq!(
        titles(&d.subtasks(top.id.clone()).unwrap()),
        ["молоко завтра !! #дом", "б2"]
    );
    d.wont_do_task(typed.id).unwrap();
    d.complete_task(sub.id.clone()).unwrap();
    assert_eq!(d.task(top.id.clone()).unwrap().subtasks_done, 1);
    d.delete_task(sub.id).unwrap();
    assert_eq!(d.subtasks(top.id.clone()).unwrap().len(), 1);

    assert_eq!(view(&d, Scope::Inbox), ["а", "в"], "only top-level tasks in a list");
}

#[test]
fn r10_the_tasks_of_a_project_are_full_tasks() {
    let d = device();
    let project = add(&d, "п");
    d.set_project(project.id.clone(), true).unwrap();
    let task = d
        .quick_add_under("т завтра !!".into(), project.id.clone(), false, None)
        .unwrap();
    assert_eq!((task.title.as_str(), task.due.as_deref()), ("т", Some("2026-10-06")));
    d.set_notes(task.id.clone(), "заметка".into()).unwrap();
    d.add_tag(task.id.clone(), "дом".into()).unwrap();
    assert_eq!(view(&d, Scope::Upcoming), ["т"]);

    // A task of a project has subtasks, and those have a title only.
    let sub = add_sub(&d, &task, "у");
    assert!(d.set_due(sub.id.clone(), Some("2026-10-09".into())).is_err());
    assert!(d.quick_add_under("глубже".into(), sub.id, false, None).is_err());

    // A task goes into a project and out of it with its fields.
    let loose = add(&d, "в");
    d.set_due(loose.id.clone(), Some("2026-10-07".into())).unwrap();
    d.move_task(loose.id.clone(), None, Some(project.id.clone()), None)
        .unwrap();
    assert_eq!(d.task(loose.id.clone()).unwrap().due.as_deref(), Some("2026-10-07"));
    d.move_to_list(task.id.clone(), INBOX_ID.into()).unwrap();
    let out = d.task(task.id).unwrap();
    assert_eq!(
        (out.parent_id, out.due.as_deref(), out.notes.as_str()),
        (None, Some("2026-10-06"), "заметка")
    );

    // R32: a project with tasks is not turned back; an empty one is.
    assert!(d.set_project(project.id.clone(), false).is_err());
    let empty = add(&d, "пусто");
    d.set_project(empty.id.clone(), true).unwrap();
    d.set_project(empty.id, false).unwrap();

    // A project does not go into another project, nor does a task with subtasks: their tasks and
    // lines would be a level too deep.
    let second = add(&d, "второй");
    d.set_project(second.id.clone(), true).unwrap();
    assert!(d
        .move_task(project.id.clone(), None, Some(second.id.clone()), None)
        .is_err());
    let holder = add(&d, "с подзадачей");
    add_sub(&d, &holder, "строка");
    assert!(d.move_task(holder.id, None, Some(second.id), None).is_err());
    assert_eq!(d.task(loose.id).unwrap().due.as_deref(), Some("2026-10-07"));
}

#[test]
fn r32_tasks_closed_as_wont_do_or_in_the_trash_are_tasks_of_the_project() {
    let d = device();
    let project = add(&d, "п");
    d.set_project(project.id.clone(), true).unwrap();
    let wont = d
        .quick_add_under("не буду завтра".into(), project.id.clone(), false, None)
        .unwrap();
    let trashed = d
        .quick_add_under("в корзине завтра".into(), project.id.clone(), false, None)
        .unwrap();
    d.wont_do_task(wont.id.clone()).unwrap();
    d.delete_task(trashed.id.clone()).unwrap();
    assert_eq!(
        d.task(project.id.clone()).unwrap().subtasks_total,
        0,
        "neither is counted"
    );

    assert!(d.set_project(project.id, false).is_err());
    d.restore_task(trashed.id.clone()).unwrap();
    for id in [wont.id, trashed.id] {
        assert_eq!(d.task(id).unwrap().due.as_deref(), Some("2026-10-06"));
    }
}

#[test]
fn r126_a_project_is_completed_whatever_the_state_of_its_tasks() {
    let d = device();
    let project = add(&d, "п");
    d.set_project(project.id.clone(), true).unwrap();
    let open = add_sub(&d, &project, "открыта");
    let weekly_task = add_sub(&d, &project, "каждую неделю");
    d.set_due(weekly_task.id.clone(), Some("2026-10-05".into())).unwrap();
    d.set_repeat(weekly_task.id.clone(), Some(weekly())).unwrap();
    assert_eq!(titles(&d.projects().unwrap()), ["п"]);

    d.complete_task(project.id.clone()).unwrap();
    assert!(d.projects().unwrap().is_empty(), "it leaves the sidebar (R32)");
    assert!(d.task(open.id.clone()).unwrap().done.is_some());
    let repeating = d.task(weekly_task.id.clone()).unwrap();
    assert!(repeating.done.is_some(), "closed for good, not moved on");
    assert_eq!(repeating.due.as_deref(), Some("2026-10-05"));

    d.set_now_for_tests("2026-10-05T10:00:06");
    d.undo_close_task(project.id.clone()).unwrap();
    assert_eq!(titles(&d.projects().unwrap()), ["п"]);
    for id in [open.id, weekly_task.id] {
        assert!(d.task(id).unwrap().done.is_none());
    }
}

#[test]
fn r127_a_project_is_created_with_a_title_and_no_defaults_of_its_list() {
    let d = device();
    let work = d.create_list("Работа".into()).unwrap();
    d.set_list_defaults(work.id.clone(), Priority::High, true).unwrap();

    let project = d.create_project("  Отпуск ".into(), Some(work.id.clone())).unwrap();
    assert!(project.is_project);
    assert_eq!(project.title, "Отпуск");
    assert_eq!(project.list_id, work.id);
    assert_eq!(project.due, None, "a due date would become that of every task (R125)");
    assert_eq!(project.start, None);
    assert_eq!(project.priority, Priority::None);
    assert_eq!(titles(&d.projects().unwrap()), ["Отпуск"]);
    assert!(
        view(&d, Scope::List { id: work.id }).is_empty(),
        "not a row of its list (R120)"
    );

    assert!(d.create_project("  ".into(), None).is_err());
    assert_eq!(d.projects().unwrap().len(), 1);
}

#[test]
fn r127_a_project_without_a_list_goes_to_the_default_one() {
    let d = device();
    let home = d.create_list("Дом".into()).unwrap();
    d.set_new_task_list(home.id.clone()).unwrap();
    let project = d.create_project("Ремонт".into(), None).unwrap();
    assert_eq!(project.list_id, home.id);
}

#[test]
fn r10_the_copy_of_a_project_keeps_the_fields_of_its_tasks() {
    let d = device();
    let project = add(&d, "п");
    d.set_project(project.id.clone(), true).unwrap();
    let task = d
        .quick_add_under("т завтра".into(), project.id.clone(), false, None)
        .unwrap();
    add_sub(&d, &task, "строка");

    let copy = d.duplicate_task(project.id).unwrap();
    assert!(copy.is_project);
    let tasks = d.subtasks(copy.id).unwrap();
    assert_eq!(titles(&tasks), ["т"]);
    assert_eq!(tasks[0].due.as_deref(), Some("2026-10-06"));
    assert_eq!(titles(&d.subtasks(tasks[0].id.clone()).unwrap()), ["строка"]);
}

#[test]
fn r112_data_from_before_the_rule_is_brought_to_it() {
    let d = device();
    let top = add(&d, "а");
    let sub = add_sub(&d, &top, "б");
    let last = add_sub(&d, &top, "я");
    let deep = add(&d, "в");
    let deeper = add(&d, "г");
    let project = add(&d, "п");
    d.set_project(project.id.clone(), true).unwrap();
    let dated = add_sub(&d, &project, "т");
    d.set_due(dated.id.clone(), Some("2026-10-05".into())).unwrap();
    d.add_tag(dated.id.clone(), "дом".into()).unwrap();

    let tag = "tag:дом";
    let d = plant(
        d,
        &[
            ("task", &sub.id, "due", json!("2026-10-05")),
            ("task", &sub.id, "start", json!("2026-10-04")),
            ("task", &sub.id, "notes", json!("заметка")),
            ("task", &sub.id, "priority", json!(3)),
            ("task", &sub.id, "remind", json!("2026-10-05T12:00")),
            ("task", &sub.id, "repeat", serde_json::to_value(weekly()).unwrap()),
            ("task", &sub.id, tag, json!(true)),
            ("attachment", "file-1", "task", json!(sub.id)),
            ("attachment", "file-1", "name", json!("файл.txt")),
            // A chain three deep: а → б → в → г.
            ("task", &deep.id, "parent", json!(sub.id)),
            ("task", &deeper.id, "parent", json!(deep.id)),
        ],
    );

    assert_eq!(titles(&d.subtasks(top.id.clone()).unwrap()), ["б", "в", "г", "я"]);
    let _ = last;
    let got = d.task(sub.id.clone()).unwrap();
    assert_eq!(
        (
            got.due,
            got.start,
            got.notes.as_str(),
            got.priority,
            got.remind,
            got.repeat
        ),
        (None, None, "", Priority::None, None, None)
    );
    assert!(got.tags.is_empty());
    assert!(d.attachments(sub.id).unwrap().is_empty());
    assert_eq!(d.task(deeper.id).unwrap().parent_id, Some(top.id));
    // R12: a subtask does not stand on its own; the task of a project does, as before.
    assert_eq!(view(&d, Scope::Today), ["т"]);
    assert_eq!(view(&d, Scope::Tag { name: "дом".into() }), ["т"]);
    assert_eq!(view(&d, Scope::Inbox), ["а"], "the project is in the sidebar (R120)");
    assert!(d
        .planned_notifications(notify_all())
        .unwrap()
        .iter()
        .all(|n| n.title != "б"));
}

#[test]
fn r113_the_subtasks_of_a_new_task_are_added_in_the_order_they_were_typed() {
    let d = device();
    let trip = add(&d, "поездка");
    let typed = vec!["билеты завтра !!".to_string(), "  ".to_string(), " отель ".to_string()];
    let added = d.add_subtasks(trip.id.clone(), typed).unwrap();
    assert_eq!(titles(&added), ["билеты завтра !!", "отель"]);
    let lines = d.subtasks(trip.id.clone()).unwrap();
    assert_eq!(titles(&lines), ["билеты завтра !!", "отель"]);
    assert!(lines.iter().all(|l| l.due.is_none() && l.priority == Priority::None));

    // All or nothing: a line cannot have lines of its own, and nothing of the call stays.
    let refused = d.add_subtasks(lines[0].id.clone(), vec!["а".into(), "б".into()]);
    assert!(refused.is_err());
    assert!(d.subtasks(lines[0].id.clone()).unwrap().is_empty());
    // A task of a project takes them as well; under a project itself they are tasks of the project.
    let project = add(&d, "п");
    d.set_project(project.id.clone(), true).unwrap();
    let task = add_sub(&d, &project, "т");
    assert_eq!(d.add_subtasks(task.id, vec!["у".into()]).unwrap().len(), 1);
    assert!(d.add_subtasks("нет такой".into(), vec!["а".into()]).is_err());
}

#[test]
fn r22_a_subtask_is_found_inside_its_task() {
    let d = device();
    let shop = add(&d, "купить");
    add_sub(&d, &shop, "Молоко");
    add_sub(&d, &shop, "хлеб");
    add(&d, "молоко отдельно");

    let found = d
        .tasks(Scope::Search {
            text: "молоко".into()
        })
        .unwrap();
    assert_eq!(sorted_titles(&found), ["купить", "молоко отдельно"]);
    assert_eq!(
        titles(&d.found_subtasks(shop.id.clone(), "молоко".into()).unwrap()),
        ["Молоко"]
    );
    assert!(d.found_subtasks(shop.id, " ".into()).unwrap().is_empty());
}

fn sorted_titles(tasks: &[TaskItem]) -> Vec<&str> {
    let mut all = titles(tasks);
    all.sort_unstable();
    all
}

fn notify_all() -> NotifySettings {
    NotifySettings {
        enabled: true,
        lead_minutes: vec![0],
        all_day_at: Some("09:00".into()),
        summary_at: None,
    }
}

#[test]
fn r11_completing_a_task_completes_its_open_subtasks() {
    let d = device();
    let top = add(&d, "родитель");
    // Two levels below: the tasks of a project and a subtask of one of them (R10).
    d.set_project(top.id.clone(), true).unwrap();
    let a = add_sub(&d, &top, "a");
    let b = add_sub(&d, &top, "b");
    let deep = add_sub(&d, &b, "deep");

    d.complete_task(top.id.clone()).unwrap();
    for id in [&a.id, &b.id, &deep.id] {
        assert!(d.task(id.clone()).unwrap().done.is_some());
    }
    assert_eq!(d.task(top.id.clone()).unwrap().subtasks_done, 2);
    assert_eq!(view(&d, Scope::Completed), ["родитель"]);

    // Reopening the parent leaves the subtasks alone.
    d.reopen_task(top.id.clone()).unwrap();
    assert!(d.task(top.id).unwrap().done.is_none());
    assert!(d.task(a.id).unwrap().done.is_some());
}

#[test]
fn manual_order_within_a_list() {
    let d = device();
    let a = add(&d, "a");
    let b = add(&d, "b");
    let c = add(&d, "c");
    assert_eq!(view(&d, Scope::Inbox), ["a", "b", "c"]);
    d.move_task(c.id.clone(), None, None, None).unwrap();
    assert_eq!(view(&d, Scope::Inbox), ["c", "a", "b"]);
    d.move_task(c.id.clone(), None, None, Some(a.id.clone())).unwrap();
    assert_eq!(view(&d, Scope::Inbox), ["a", "c", "b"]);
    d.move_task(a.id, None, None, Some(b.id)).unwrap();
    assert_eq!(view(&d, Scope::Inbox), ["c", "b", "a"]);
}

#[test]
fn r17_completing_a_repeating_task_moves_it_forward() {
    let d = device();
    let t = add(&d, "полить цветы");
    d.set_start(t.id.clone(), Some("2026-10-03".into())).unwrap();
    d.set_due(t.id.clone(), Some("2026-10-05T19:00".into())).unwrap();
    d.set_remind(t.id.clone(), Some("2026-10-05T18:45".into())).unwrap();
    d.set_repeat(t.id.clone(), Some(weekly())).unwrap();
    let sub = add_sub(&d, &t, "набрать воды");
    d.complete_task(sub.id.clone()).unwrap();

    let after = d.complete_task(t.id.clone()).unwrap();
    assert!(after.done.is_none(), "the task stays open");
    assert_eq!(after.due.as_deref(), Some("2026-10-12T19:00"));
    assert_eq!(after.start.as_deref(), Some("2026-10-10"));
    assert_eq!(after.remind.as_deref(), Some("2026-10-12T18:45"));
    assert!(
        d.task(sub.id).unwrap().done.is_none(),
        "subtasks are reopened for the next round"
    );

    let log = d.tasks(Scope::Completed).unwrap();
    assert_eq!(titles(&log), ["полить цветы"]);
    assert!(log[0].is_log);
    assert_eq!(log[0].due.as_deref(), Some("2026-10-05T19:00"));
    assert_eq!(
        log[0].start.as_deref(),
        Some("2026-10-03"),
        "the record keeps the start too"
    );
    d.set_now_for_tests("2026-10-05T10:01");
    assert_eq!(
        view(&d, Scope::Inbox),
        ["полить цветы"],
        "the record is not a task of the list"
    );
}

#[test]
fn r17_last_occurrence_completes_the_task() {
    let d = device();
    let t = add(&d, "курс");
    d.set_due(t.id.clone(), Some("2026-10-05".into())).unwrap();
    d.set_repeat(
        t.id.clone(),
        Some(Repeat {
            freq: Freq::Daily,
            count: Some(2),
            ..weekly()
        }),
    )
    .unwrap();

    let first = d.complete_task(t.id.clone()).unwrap();
    assert!(first.done.is_none());
    assert_eq!(first.due.as_deref(), Some("2026-10-06"));
    assert_eq!(first.repeat.unwrap().count, Some(1));

    d.set_now_for_tests("2026-10-06T10:00");
    let second = d.complete_task(t.id).unwrap();
    assert!(second.done.is_some());
    assert_eq!(d.tasks(Scope::Completed).unwrap().len(), 2);
}

/// The repeating task of the R128 scenario, with subtask "а" completed at 9:00 and "б" open.
fn weekly_with_subtasks(d: &Device) -> (TaskItem, TaskItem, TaskItem) {
    let t = add(d, "каждую неделю");
    d.set_start(t.id.clone(), Some("2026-10-03".into())).unwrap();
    d.set_due(t.id.clone(), Some("2026-10-05T19:00".into())).unwrap();
    d.set_remind(t.id.clone(), Some("2026-10-05T18:45".into())).unwrap();
    d.set_repeat(
        t.id.clone(),
        Some(Repeat {
            count: Some(3),
            ..weekly()
        }),
    )
    .unwrap();
    let a = add_sub(d, &t, "а");
    let b = add_sub(d, &t, "б");
    d.set_now_for_tests("2026-10-05T09:00");
    d.complete_task(a.id.clone()).unwrap();
    d.set_now_for_tests(NOW);
    (t, a, b)
}

#[test]
fn r128_undo_gives_back_what_the_completion_of_a_repeat_changed() {
    let d = device();
    let (t, a, b) = weekly_with_subtasks(&d);
    d.complete_task(t.id.clone()).unwrap();
    d.undo_close_task(t.id.clone()).unwrap();

    let back = d.task(t.id.clone()).unwrap();
    assert!(back.done.is_none());
    assert_eq!(back.due.as_deref(), Some("2026-10-05T19:00"));
    assert_eq!(back.start.as_deref(), Some("2026-10-03"));
    assert_eq!(back.remind.as_deref(), Some("2026-10-05T18:45"));
    assert_eq!(
        back.repeat.unwrap().count,
        Some(3),
        "the repeat that was used is given back"
    );
    assert_eq!(
        d.task(a.id).unwrap().done.as_deref(),
        Some("2026-10-05T09:00"),
        "completed before, with its moment"
    );
    assert!(d.task(b.id).unwrap().done.is_none());
    assert!(view(&d, Scope::Completed).is_empty(), "the record has gone");
    assert_eq!(view(&d, Scope::Inbox), ["каждую неделю"]);

    // The same occurrence completed again comes back as the same record (S7).
    d.complete_task(t.id.clone()).unwrap();
    assert_eq!(view(&d, Scope::Completed), ["каждую неделю"]);
    assert_eq!(d.task(t.id).unwrap().due.as_deref(), Some("2026-10-12T19:00"));
}

#[test]
fn r128_the_mark_of_the_record_is_undo() {
    let d = device();
    let (t, a, _) = weekly_with_subtasks(&d);
    d.wont_do_task(t.id.clone()).unwrap();
    d.set_now_for_tests("2026-10-05T10:00:03");
    let record = d.tasks(Scope::Inbox).unwrap().into_iter().find(|r| r.is_log).unwrap();
    assert!(record.wont, "closed as won't do");
    d.reopen_task(record.id).unwrap();
    assert_eq!(d.task(t.id).unwrap().due.as_deref(), Some("2026-10-05T19:00"));
    assert_eq!(d.task(a.id).unwrap().done.as_deref(), Some("2026-10-05T09:00"));
    assert!(view(&d, Scope::WontDo).is_empty());
    assert_eq!(view(&d, Scope::Inbox), ["каждую неделю"]);
}

#[test]
fn r128_undo_changes_nothing_once_the_task_has_left_that_date() {
    let d = device();
    let (t, _, _) = weekly_with_subtasks(&d);
    d.complete_task(t.id.clone()).unwrap();
    d.set_due(t.id.clone(), Some("2026-10-20".into())).unwrap();
    d.undo_close_task(t.id.clone()).unwrap();
    assert_eq!(d.task(t.id.clone()).unwrap().due.as_deref(), Some("2026-10-20"));
    assert_eq!(view(&d, Scope::Completed), ["каждую неделю"]);

    // Completed once more: the first completion can no longer be taken back, the second can.
    let first = d.tasks(Scope::Completed).unwrap().remove(0);
    d.set_now_for_tests("2026-10-05T10:01");
    d.complete_task(t.id.clone()).unwrap();
    d.reopen_task(first.id).unwrap();
    assert_eq!(d.tasks(Scope::Completed).unwrap().len(), 2, "the first record stays");
    d.undo_close_task(t.id.clone()).unwrap();
    assert_eq!(d.task(t.id).unwrap().due.as_deref(), Some("2026-10-20"));
    assert_eq!(view(&d, Scope::Completed), ["каждую неделю"]);
}

#[test]
fn r128_a_record_written_before_undo_existed_cannot_be_reopened() {
    let d = device();
    let t = due_today(&d, "старый повтор");
    d.set_repeat(t.id.clone(), Some(weekly())).unwrap();
    d.complete_task(t.id.clone()).unwrap();
    let record = d.tasks(Scope::Completed).unwrap().remove(0);
    let d = plant(d, &[("task", record.id.as_str(), "undo", serde_json::Value::Null)]);
    assert!(d.reopen_task(record.id).is_err());
    d.undo_close_task(t.id.clone()).unwrap();
    assert_eq!(
        d.task(t.id).unwrap().due.as_deref(),
        Some("2026-10-12"),
        "nothing to give back"
    );
}

#[test]
fn r133_the_mark_of_a_record_in_completed_reopens_while_the_completion_can_be_undone() {
    let d = device();
    let (t, a, _) = weekly_with_subtasks(&d);
    d.complete_task(t.id.clone()).unwrap();
    d.set_now_for_tests("2026-10-06T10:00");
    let record = d.tasks(Scope::Completed).unwrap().remove(0);
    assert!(
        record.undoable,
        "the task stands on the date the completion moved it to"
    );
    assert!(!d.task(t.id.clone()).unwrap().undoable, "only a record");
    d.reopen_task(record.id).unwrap();
    assert_eq!(d.task(t.id.clone()).unwrap().due.as_deref(), Some("2026-10-05T19:00"));
    assert_eq!(d.task(a.id).unwrap().done.as_deref(), Some("2026-10-05T09:00"));
    assert!(view(&d, Scope::Completed).is_empty(), "the record has left Completed");

    // Completed twice: the first completion can no longer be taken back, the last one can.
    d.complete_task(t.id.clone()).unwrap();
    d.wont_do_task(t.id.clone()).unwrap();
    let records = d.tasks(Scope::Completed).unwrap();
    let wont = records.iter().find(|r| r.wont).unwrap();
    assert!(wont.undoable);
    assert!(!records.iter().find(|r| !r.wont).unwrap().undoable);
    assert!(d.tasks(Scope::WontDo).unwrap()[0].undoable, "in Won't do as well");

    // A changed date, and the task completed for good, leave nothing to take back.
    d.set_due(t.id.clone(), Some("2026-10-30".into())).unwrap();
    assert!(d.tasks(Scope::Completed).unwrap().iter().all(|r| !r.undoable));
    d.set_due(t.id.clone(), Some("2026-10-19T19:00".into())).unwrap();
    assert!(d.task(wont.id.clone()).unwrap().undoable, "back on that date");
    d.set_repeat(t.id.clone(), None).unwrap();
    d.complete_task(t.id.clone()).unwrap();
    assert!(d.tasks(Scope::Completed).unwrap().iter().all(|r| !r.undoable));
}

#[test]
fn r133_a_record_written_before_undo_existed_has_no_mark() {
    let d = device();
    let t = due_today(&d, "старый повтор");
    d.set_repeat(t.id.clone(), Some(weekly())).unwrap();
    d.complete_task(t.id.clone()).unwrap();
    let record = d.tasks(Scope::Completed).unwrap().remove(0);
    let d = plant(d, &[("task", record.id.as_str(), "undo", serde_json::Value::Null)]);
    assert!(!d.task(record.id.clone()).unwrap().undoable);
    let d = plant(d, &[("task", record.id.as_str(), "undo", serde_json::json!("{broken"))]);
    assert!(
        !d.task(record.id).unwrap().undoable,
        "a value that is not an object reads as none"
    );
}

#[test]
fn r18_monthly_on_the_31st() {
    let d = device();
    d.set_now_for_tests("2026-01-31T10:00");
    let t = add(&d, "отчёт");
    d.set_due(t.id.clone(), Some("2026-01-31".into())).unwrap();
    d.set_repeat(
        t.id.clone(),
        Some(Repeat {
            freq: Freq::Monthly,
            ..weekly()
        }),
    )
    .unwrap();
    assert_eq!(
        d.complete_task(t.id.clone()).unwrap().due.as_deref(),
        Some("2026-02-28")
    );
    d.set_now_for_tests("2026-02-28T10:00");
    assert_eq!(d.complete_task(t.id).unwrap().due.as_deref(), Some("2026-03-31"));
}

#[test]
fn repeating_task_without_dates_gets_a_due_date() {
    let d = device();
    let t = add(&d, "зарядка");
    d.set_repeat(
        t.id.clone(),
        Some(Repeat {
            freq: Freq::Daily,
            ..weekly()
        }),
    )
    .unwrap();
    assert_eq!(d.complete_task(t.id).unwrap().due.as_deref(), Some("2026-10-06"));
}

#[test]
fn r19_attachments_are_stored_by_content() {
    let d = device();
    let t = add(&d, "с файлом");
    let src = tempfile::tempdir().unwrap();
    let file = src.path().join("Счёт.pdf");
    std::fs::write(&file, b"%PDF-1.7 demo").unwrap();

    let a = d
        .add_attachment(t.id.clone(), file.to_string_lossy().into_owned(), None)
        .unwrap();
    assert_eq!(
        (a.name.as_str(), a.mime.as_str(), a.size),
        ("Счёт.pdf", "application/pdf", 13)
    );
    std::fs::remove_file(&file).unwrap();
    let stored = a.local_path.expect("content is kept by the store");
    assert_eq!(std::fs::read(stored).unwrap(), b"%PDF-1.7 demo");
    assert_eq!(d.task(t.id.clone()).unwrap().attachments, 1);

    let copy = d.duplicate_task(t.id.clone()).unwrap();
    assert_eq!(d.attachments(copy.id).unwrap().len(), 1);

    d.remove_attachment(a.id).unwrap();
    assert!(d.attachments(t.id).unwrap().is_empty());
}

#[test]
fn r21_smart_views_and_counts() {
    let d = device();
    let overdue = add(&d, "просрочено");
    d.set_due(overdue.id, Some("2026-10-01".into())).unwrap();
    let today = add(&d, "сегодня");
    d.set_due(today.id, Some("2026-10-05T23:00".into())).unwrap();
    let later = add(&d, "потом");
    d.set_due(later.id, Some("2026-10-20".into())).unwrap();
    let sooner = add(&d, "скоро");
    d.set_due(sooner.id, Some("2026-10-06".into())).unwrap();
    add(&d, "без даты");

    assert_eq!(view(&d, Scope::Today), ["просрочено", "сегодня"]);
    assert_eq!(view(&d, Scope::Upcoming), ["скоро", "потом"]);
    assert_eq!(view(&d, Scope::All).len(), 5);
    let c = d.counts().unwrap();
    assert_eq!((c.inbox, c.today, c.overdue, c.upcoming, c.trash), (5, 2, 1, 2, 0));
}

#[test]
fn r93_counts_say_which_built_in_views_hold_something() {
    let d = device();
    let c = d.counts().unwrap();
    assert_eq!((c.all, c.completed, c.wont_do, c.trash), (0, 0, 0, 0));

    let parent = add(&d, "открытая");
    add_sub(&d, &parent, "подзадача");
    let done = add(&d, "сделанная");
    d.complete_task(done.id).unwrap();
    let dropped = add(&d, "не буду");
    d.wont_do_task(dropped.id.clone()).unwrap();
    let gone = add(&d, "удалённая");
    d.delete_task(gone.id).unwrap();

    let c = d.counts().unwrap();
    // A subtask is not a row of All; "won't do" is a row of Completed as well.
    assert_eq!((c.all, c.completed, c.wont_do, c.trash), (1, 2, 1, 1));
    assert_eq!(c.completed as usize, view(&d, Scope::Completed).len());
    assert_eq!(c.wont_do as usize, view(&d, Scope::WontDo).len());

    d.reopen_task(dropped.id).unwrap();
    let c = d.counts().unwrap();
    assert_eq!((c.all, c.completed, c.wont_do), (2, 1, 0));
}

#[test]
fn r121_hiding_inbox_moves_its_open_tasks_and_takes_the_default_list_along() {
    let d = device();
    let a = add(&d, "а");
    add_sub(&d, &a, "а1");
    let b = add(&d, "б");
    d.complete_task(b.id).unwrap();
    let p = add(&d, "п");
    d.set_project(p.id.clone(), true).unwrap();
    let home = d.create_list("Дом".into()).unwrap();
    let own = add(&d, "своя");
    d.move_to_list(own.id, home.id.clone()).unwrap();
    let old = d.create_list("Старое".into()).unwrap();
    d.set_list_archived(old.id.clone(), true).unwrap();
    d.set_new_task_list(home.id.clone()).unwrap();

    assert!(d.hide_inbox(old.id.clone(), true).is_err());
    assert!(d.hide_inbox(INBOX_ID.into(), true).is_err());
    assert!(!d.inbox_hidden().unwrap());

    let hiding = d.hide_inbox(home.id.clone(), true).unwrap();
    assert_eq!(hiding.moved.len(), 2);
    assert_eq!(hiding.new_task_list, home.id);
    assert!(d.inbox_hidden().unwrap());
    assert_eq!(d.new_task_list().unwrap(), "last");
    // The moved ones go to the end, in their order; the project is shown in the sidebar only (R120).
    assert_eq!(view(&d, Scope::List { id: home.id.clone() }), ["своя", "а"]);
    assert!(d
        .projects()
        .unwrap()
        .iter()
        .any(|t| t.title == "п" && t.list_id == home.id));
    assert_eq!(
        d.tasks(Scope::Inbox)
            .unwrap()
            .iter()
            .filter(|t| t.done.is_none())
            .count(),
        0
    );
    assert_eq!(d.counts().unwrap().inbox, 0);
    // The subtask went along with its task.
    let moved = d
        .tasks(Scope::List { id: home.id.clone() })
        .unwrap()
        .into_iter()
        .find(|t| t.title == "а")
        .unwrap();
    assert_eq!(moved.list_id, home.id);
    assert_eq!(titles(&d.subtasks(moved.id).unwrap()), ["а1"]);
    assert!(d.subtasks(a.id.clone()).unwrap().iter().all(|t| t.list_id == home.id));

    // R40: the list used last, and while Inbox is hidden it does not count.
    assert_eq!(d.default_list_id(None).unwrap(), home.id);
    assert_eq!(d.default_list_id(Some(INBOX_ID.into())).unwrap(), home.id);
    assert_eq!(d.default_list_id(Some(old.id.clone())).unwrap(), home.id);

    d.undo_hide_inbox(hiding).unwrap();
    assert!(!d.inbox_hidden().unwrap());
    assert_eq!(d.new_task_list().unwrap(), home.id);
    assert_eq!(view(&d, Scope::List { id: home.id.clone() }), ["своя"]);
    assert!(view(&d, Scope::Inbox).contains(&"а".to_string()));
    assert!(d
        .projects()
        .unwrap()
        .iter()
        .any(|t| t.title == "п" && t.list_id == INBOX_ID));
}

#[test]
fn r121_inbox_hidden_without_moving_keeps_its_tasks_and_is_shown_again_unchanged() {
    let d = device();
    add(&d, "а");
    let home = d.create_list("Дом".into()).unwrap();
    let hiding = d.hide_inbox(home.id.clone(), false).unwrap();
    assert!(hiding.moved.is_empty());
    assert!(d.inbox_hidden().unwrap());
    // The row comes back by its counter while open tasks are there.
    assert_eq!(d.counts().unwrap().inbox, 1);
    let a = d.tasks(Scope::Inbox).unwrap().remove(0);
    d.move_to_list(a.id, home.id.clone()).unwrap();
    assert_eq!(d.counts().unwrap().inbox, 0);

    d.set_new_task_list(INBOX_ID.into()).unwrap();
    d.show_inbox().unwrap();
    assert!(!d.inbox_hidden().unwrap());
    assert_eq!(d.new_task_list().unwrap(), INBOX_ID);
    assert_eq!(d.default_list_id(None).unwrap(), INBOX_ID);
}

#[test]
fn r40_default_list_falls_back_to_inbox_or_while_it_is_hidden_to_the_first_list() {
    let d = device();
    assert_eq!(d.new_task_list().unwrap(), INBOX_ID);
    assert_eq!(d.default_list_id(None).unwrap(), INBOX_ID);
    let work = d.create_list("Работа".into()).unwrap();
    let home = d.create_list("Дом".into()).unwrap();

    d.set_new_task_list(home.id.clone()).unwrap();
    assert_eq!(d.default_list_id(Some(work.id.clone())).unwrap(), home.id);
    d.set_new_task_list("last".into()).unwrap();
    assert_eq!(d.default_list_id(Some(work.id.clone())).unwrap(), work.id);
    assert_eq!(d.default_list_id(None).unwrap(), INBOX_ID);
    d.set_list_archived(work.id.clone(), true).unwrap();
    assert_eq!(d.default_list_id(Some(work.id.clone())).unwrap(), INBOX_ID);
    d.delete_list(home.id.clone()).unwrap();
    d.set_new_task_list(home.id.clone()).unwrap();
    assert_eq!(d.default_list_id(None).unwrap(), INBOX_ID);

    let other = d.create_list("Другое".into()).unwrap();
    d.hide_inbox(other.id.clone(), true).unwrap();
    d.set_new_task_list(INBOX_ID.into()).unwrap();
    assert_eq!(d.default_list_id(None).unwrap(), other.id);
    // Not even Inbox fits: it is all there is.
    d.set_list_archived(other.id, true).unwrap();
    assert_eq!(d.default_list_id(None).unwrap(), INBOX_ID);
}

#[test]
fn r40_the_choice_kept_on_the_device_is_adopted_only_into_an_empty_setting() {
    let d = device();
    let home = d.create_list("Дом".into()).unwrap();
    d.adopt_new_task_list(INBOX_ID.into()).unwrap();
    assert_eq!(d.setting_for_tests("new_task_list"), None);
    d.adopt_new_task_list("last".into()).unwrap();
    assert_eq!(d.new_task_list().unwrap(), "last");
    d.adopt_new_task_list(home.id.clone()).unwrap();
    assert_eq!(d.new_task_list().unwrap(), "last");
    d.set_setting_for_tests("new_task_list", json!(5));
    assert_eq!(d.new_task_list().unwrap(), INBOX_ID);
}

#[test]
fn r22_search_ignores_case_in_any_script() {
    let d = device();
    let t = add(&d, "Купить МОЛОКО");
    d.set_notes(t.id, "в Перекрёстке".into()).unwrap();
    add(&d, "другое");
    assert_eq!(
        view(
            &d,
            Scope::Search {
                text: "молоко".into()
            }
        ),
        ["Купить МОЛОКО"]
    );
    assert_eq!(
        view(
            &d,
            Scope::Search {
                text: "ПЕРЕКРЁСТКЕ".into()
            }
        ),
        ["Купить МОЛОКО"]
    );
    assert!(view(&d, Scope::Search { text: "  ".into() }).is_empty());
}

#[test]
fn r23_quick_add_creates_the_task_the_line_describes() {
    let d = device();
    let shop = d.create_list("Покупки".into()).unwrap();
    let t = d
        .quick_add("купить молоко завтра 18:30 !! #дом @покупки".into(), None, false, None)
        .unwrap();
    assert_eq!(t.title, "купить молоко");
    assert_eq!(t.due.as_deref(), Some("2026-10-06T18:30"));
    assert_eq!(t.priority, Priority::Medium);
    assert_eq!(t.tags, ["дом"]);
    assert_eq!(t.list_id, shop.id);

    // An unknown @word is not a list: it stays in the title, the fallback list is used.
    let other = d
        .quick_add("написать @ivan".into(), Some(shop.id.clone()), false, None)
        .unwrap();
    assert_eq!(other.title, "написать @ivan");
    assert_eq!(other.list_id, shop.id);

    assert!(
        d.quick_add("#тег !!".into(), None, false, None).is_err(),
        "a title is required"
    );
}

#[test]
fn duplicate_copies_the_subtree_next_to_the_original() {
    let d = device();
    let a = add(&d, "a");
    add(&d, "z");
    add_sub(&d, &a, "sub");
    d.set_repeat(a.id.clone(), Some(weekly())).unwrap();

    let copy = d.duplicate_task(a.id).unwrap();
    assert_eq!(view(&d, Scope::Inbox), ["a", "a", "z"]);
    assert!(copy.repeat.is_some());
    let copied_sub = d.subtasks(copy.id).unwrap();
    assert_eq!(titles(&copied_sub), ["sub"]);
}

#[test]
fn data_survives_reopening() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().to_string_lossy().into_owned();
    let id = {
        let store = Store::open(path.clone()).unwrap();
        store
            .create_task(NewTask {
                title: "помнить".into(),
                ..NewTask::default()
            })
            .unwrap()
            .id
    };
    let store = Store::open(path).unwrap();
    assert_eq!(store.task(id.clone()).unwrap().title, "помнить");
    // The clock continues past what is stored, so a new edit wins over the old value.
    store.set_title(id.clone(), "помнить всё".into()).unwrap();
    assert_eq!(store.task(id).unwrap().title, "помнить всё");
}

#[test]
fn r8_reminders_list_open_tasks_in_time_order() {
    let d = device();
    let late = add(&d, "поздно");
    d.set_remind(late.id, Some("2026-10-07T09:00".into())).unwrap();
    let early = add(&d, "рано");
    d.set_remind(early.id.clone(), Some("2026-10-06T08:00".into())).unwrap();
    let done = add(&d, "уже сделано");
    d.set_remind(done.id.clone(), Some("2026-10-06T07:00".into())).unwrap();
    d.complete_task(done.id).unwrap();
    add(&d, "без напоминания");

    assert_eq!(titles(&d.reminders().unwrap()), ["рано", "поздно"]);
    d.delete_task(early.id).unwrap();
    assert_eq!(titles(&d.reminders().unwrap()), ["поздно"]);
}

#[test]
fn projects_are_tasks_with_a_view_of_their_own() {
    let d = device();
    let work = d.create_list("Работа".into()).unwrap();
    let launch = d
        .create_task(NewTask {
            title: "Запуск сайта".into(),
            list_id: Some(work.id.clone()),
            ..NewTask::default()
        })
        .unwrap();
    add(&d, "обычная задача");
    assert!(d.projects().unwrap().is_empty());

    d.set_project(launch.id.clone(), true).unwrap();
    let design = d
        .quick_add_under("Макеты завтра !! #дизайн".into(), launch.id.clone(), false, None)
        .unwrap();
    assert_eq!(
        (design.title.as_str(), design.due.as_deref(), design.priority),
        ("Макеты", Some("2026-10-06"), Priority::Medium)
    );
    assert_eq!(
        design.list_id, work.id,
        "a task of a project lives in the project's list"
    );
    let copy = add_sub(&d, &launch, "Тексты");
    d.complete_task(copy.id).unwrap();

    let projects = d.projects().unwrap();
    assert_eq!(titles(&projects), ["Запуск сайта"]);
    assert!(projects[0].is_project);
    assert_eq!((projects[0].subtasks_total, projects[0].subtasks_done), (2, 1));
    assert_eq!(
        view(&d, Scope::Project { id: launch.id.clone() }),
        ["Макеты", "Тексты"],
        "open first, completed last"
    );
    assert_eq!(
        view(&d, Scope::Upcoming),
        ["Макеты"],
        "project tasks show up in the date views"
    );

    // A task of a project cannot be a project; a project with tasks is not turned back (R32).
    assert!(d.set_project(design.id, true).is_err());
    assert!(d.set_project(launch.id.clone(), false).is_err());
    assert_eq!(d.subtasks(launch.id.clone()).unwrap().len(), 2);

    // A completed project leaves the sidebar.
    d.complete_task(launch.id).unwrap();
    assert!(d.projects().unwrap().is_empty());
}

#[test]
fn r120_a_project_is_not_a_row_of_the_task_views() {
    let d = device();
    add(&d, "а");
    let b = add(&d, "б");
    assert_eq!(d.counts().unwrap().inbox, 2);

    d.set_project(b.id.clone(), true).unwrap();
    assert_eq!(view(&d, Scope::Inbox), ["а"], "the project leaves its list");
    assert_eq!(d.counts().unwrap().inbox, 1);
    assert_eq!(titles(&d.projects().unwrap()), ["б"], "it stays in the sidebar");

    // Its own date, tag and priority do not bring it back.
    d.set_due(b.id.clone(), Some("2026-10-05".into())).unwrap();
    d.add_tag(b.id.clone(), "дом".into()).unwrap();
    d.set_priority(b.id.clone(), Priority::High).unwrap();
    let wide = d
        .create_filter(
            "всё".into(),
            FilterSpec {
                status: FilterStatus::All,
                ..spec()
            },
        )
        .unwrap();
    for scope in [
        Scope::Today,
        Scope::Upcoming,
        Scope::All,
        Scope::Tag { name: "дом".into() },
        Scope::Filter { id: wide.id.clone() },
    ] {
        assert!(!view(&d, scope.clone()).contains(&"б".to_string()), "{scope:?}");
    }
    let counts = d.counts().unwrap();
    assert_eq!((counts.today, counts.overdue, counts.all), (0, 0, 1));
    assert!(d.tags().unwrap().iter().all(|t| t.name != "дом" || t.open_count == 0));
    assert_eq!(d.filters().unwrap()[0].open_count, 1, "only «а»");
    assert_eq!(
        view(&d, Scope::Search { text: "б".into() }),
        ["б"],
        "search finds the project"
    );

    // The trash shows it, so it can be restored.
    d.delete_task(b.id.clone()).unwrap();
    assert_eq!(view(&d, Scope::Trash), ["б"]);
    d.restore_task(b.id.clone()).unwrap();

    // An empty project turned back into a task returns to its place.
    d.set_project(b.id.clone(), false).unwrap();
    assert_eq!(view(&d, Scope::Inbox), ["а", "б"]);
    assert_eq!(d.counts().unwrap().inbox, 2);

    // A project with a dated task: the task is in Today, the project is not.
    let p = add(&d, "п");
    d.set_project(p.id.clone(), true).unwrap();
    let t = add_sub(&d, &p, "т");
    d.set_due(t.id.clone(), Some("2026-10-05".into())).unwrap();
    d.set_due(p.id.clone(), Some("2026-10-05".into())).unwrap();
    assert_eq!(
        view(&d, Scope::Today),
        ["б", "т"],
        "«б» is a task again, «п» is not a row"
    );
    assert_eq!(d.counts().unwrap().today, 2);

    // A completed project is not a row of its list, even with completed tasks shown, but is in Completed.
    d.complete_task(t.id).unwrap();
    d.complete_task(p.id.clone()).unwrap();
    d.set_now_for_tests("2026-10-05T10:30");
    d.set_list_show_done(INBOX_ID.into(), true).unwrap();
    assert_eq!(view(&d, Scope::Inbox), ["а", "б"]);
    assert!(view(&d, Scope::Completed).contains(&"п".to_string()));
}

fn spec() -> FilterSpec {
    FilterSpec::default()
}

#[test]
fn saved_filters_select_by_date_list_tag_priority_status_and_text() {
    let d = device();
    let work = d.create_list("Работа".into()).unwrap();
    let mk = |title: &str, due: Option<&str>, list: Option<&str>| {
        d.create_task(NewTask {
            title: title.into(),
            due: due.map(str::to_string),
            list_id: list.map(str::to_string),
            ..NewTask::default()
        })
        .unwrap()
    };
    let late = mk("просрочено", Some("2026-10-01"), None);
    mk("сегодня", Some("2026-10-05T18:00"), Some(&work.id));
    let soon = mk("через три дня", Some("2026-10-08"), Some(&work.id));
    mk("через неделю", Some("2026-10-12"), None);
    mk("без даты", None, Some(&work.id));
    let done = mk("сделано", Some("2026-10-05"), None);
    d.complete_task(done.id).unwrap();
    // Past the time a completed task stays among the open ones (R68).
    d.set_now_for_tests("2026-10-05T10:30");
    d.add_tag(soon.id.clone(), "важное".into()).unwrap();
    d.set_priority(soon.id.clone(), Priority::High).unwrap();
    d.set_priority(late.id, Priority::Low).unwrap();
    d.set_notes(soon.id, "позвонить Ивану".into()).unwrap();

    let show = |s: FilterSpec| {
        titles(&d.preview_filter(s).unwrap())
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        show(FilterSpec {
            due: DueWindow::Next { days: 7 },
            ..spec()
        }),
        ["просрочено", "сегодня", "через три дня"]
    );
    assert_eq!(
        show(FilterSpec {
            due: DueWindow::Next { days: 8 },
            ..spec()
        })
        .len(),
        4,
        "the eighth day is the 12th"
    );
    assert_eq!(
        show(FilterSpec {
            due: DueWindow::Today,
            ..spec()
        }),
        ["сегодня"]
    );
    assert_eq!(
        show(FilterSpec {
            due: DueWindow::Overdue,
            ..spec()
        }),
        ["просрочено"]
    );
    assert_eq!(
        show(FilterSpec {
            due: DueWindow::NoDate,
            ..spec()
        }),
        ["без даты"]
    );
    assert_eq!(
        show(FilterSpec {
            list_ids: vec![work.id.clone()],
            ..spec()
        }),
        ["сегодня", "через три дня", "без даты"]
    );
    assert_eq!(
        show(FilterSpec {
            tags: vec!["#Важное".into()],
            ..spec()
        }),
        ["через три дня"]
    );
    assert_eq!(
        show(FilterSpec {
            min_priority: Priority::Low,
            ..spec()
        }),
        ["просрочено", "через три дня"]
    );
    assert_eq!(
        show(FilterSpec {
            min_priority: Priority::High,
            ..spec()
        }),
        ["через три дня"]
    );
    assert_eq!(
        show(FilterSpec {
            status: FilterStatus::Done,
            ..spec()
        }),
        ["сделано"]
    );
    assert_eq!(
        show(FilterSpec {
            status: FilterStatus::All,
            due: DueWindow::Today,
            ..spec()
        }),
        ["сегодня", "сделано"]
    );
    assert_eq!(
        show(FilterSpec {
            text: "ИВАНУ позвонить".into(),
            ..spec()
        }),
        ["через три дня"]
    );
    assert_eq!(
        show(FilterSpec {
            due: DueWindow::Next { days: 7 },
            list_ids: vec![work.id],
            min_priority: Priority::Medium,
            ..spec()
        }),
        ["через три дня"]
    );
}

#[test]
fn saved_filters_are_named_stored_and_shown_as_views() {
    let d = device();
    add(&d, "без даты");
    let t = add(&d, "скоро");
    d.set_due(t.id, Some("2026-10-07".into())).unwrap();
    assert!(d.create_filter("  ".into(), spec()).is_err());

    let week = d
        .create_filter(
            "Неделя".into(),
            FilterSpec {
                due: DueWindow::Next { days: 7 },
                ..spec()
            },
        )
        .unwrap();
    assert_eq!((week.name.as_str(), week.open_count), ("Неделя", 1));
    assert_eq!(view(&d, Scope::Filter { id: week.id.clone() }), ["скоро"]);

    d.update_filter(
        week.id.clone(),
        "Без срока".into(),
        FilterSpec {
            due: DueWindow::NoDate,
            ..spec()
        },
    )
    .unwrap();
    let all = d.filters().unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(
        (all[0].name.as_str(), all[0].spec.due),
        ("Без срока", DueWindow::NoDate)
    );
    assert_eq!(view(&d, Scope::Filter { id: week.id.clone() }), ["без даты"]);

    d.delete_filter(week.id.clone()).unwrap();
    assert!(d.filters().unwrap().is_empty());
    assert!(d.tasks(Scope::Filter { id: week.id }).is_err());
}

#[test]
fn derived_tables_are_rebuilt_when_their_shape_changes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().to_string_lossy().into_owned();
    let id = {
        let store = Store::open(path.clone()).unwrap();
        let t = store
            .create_task(NewTask {
                title: "переживёт".into(),
                ..NewTask::default()
            })
            .unwrap();
        store.add_tag(t.id.clone(), "тег".into()).unwrap();
        store.set_project(t.id.clone(), true).unwrap();
        t.id
    };
    // What an older version of the app left behind: derived tables of another shape.
    {
        let conn = rusqlite::Connection::open(dir.path().join("lists.sqlite")).unwrap();
        conn.execute_batch("DROP TABLE tasks; CREATE TABLE tasks (id TEXT PRIMARY KEY, title TEXT); UPDATE meta SET value = '1' WHERE key = 'derived_version';").unwrap();
    }
    let store = Store::open(path).unwrap();
    let got = store.task(id).unwrap();
    assert_eq!((got.title.as_str(), got.is_project), ("переживёт", true));
    assert_eq!(got.tags, ["тег"]);
}

fn notify() -> NotifySettings {
    NotifySettings {
        enabled: true,
        lead_minutes: vec![15],
        all_day_at: Some("09:00".into()),
        summary_at: None,
    }
}

#[test]
fn notifications_follow_reminders_due_dates_and_settings() {
    let d = device(); // now is 2026-10-05T10:00
    let mk = |title: &str, due: Option<&str>, remind: Option<&str>| {
        let t = d
            .create_task(NewTask {
                title: title.into(),
                due: due.map(str::to_string),
                ..NewTask::default()
            })
            .unwrap();
        if let Some(r) = remind {
            d.set_remind(t.id.clone(), Some(r.into())).unwrap();
        }
        t
    };
    mk(
        "со своим напоминанием",
        Some("2026-10-06T18:00"),
        Some("2026-10-06T12:00"),
    );
    mk("срок со временем", Some("2026-10-05T18:30"), None);
    mk("срок на день", Some("2026-10-07"), None);
    mk("срок сегодня без времени", Some("2026-10-05"), None);
    mk("напоминание в прошлом", None, Some("2026-10-05T09:00"));
    let done = mk("выполнена", Some("2026-10-06T10:00"), None);
    d.complete_task(done.id).unwrap();
    mk("без дат", None, None);

    let plan = |s: NotifySettings| {
        d.planned_notifications(s)
            .unwrap()
            .into_iter()
            .map(|n| (n.title, n.at, n.kind))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        plan(notify()),
        [
            ("срок со временем".to_string(), "2026-10-05T18:15".to_string(), NotificationKind::Due),
            ("со своим напоминанием".to_string(), "2026-10-06T12:00".to_string(), NotificationKind::Reminder),
            ("срок на день".to_string(), "2026-10-07T09:00".to_string(), NotificationKind::Due),
        ],
        "past moments, completed tasks and tasks without dates produce nothing; a reminder of its own replaces the derived one"
    );
    // Each kind of derived reminder can be turned off; a task's own reminder stays.
    assert_eq!(
        plan(NotifySettings {
            lead_minutes: vec![],
            all_day_at: None,
            ..notify()
        })
        .len(),
        1
    );
    assert_eq!(
        plan(NotifySettings {
            lead_minutes: vec![0],
            ..notify()
        })[0]
            .1,
        "2026-10-05T18:30"
    );
    assert!(plan(NotifySettings {
        enabled: false,
        ..notify()
    })
    .is_empty());
    // Several lead times give several notifications for one task; those already past are dropped.
    let several = d
        .planned_notifications(NotifySettings {
            lead_minutes: vec![60, 15, 15, 1440],
            all_day_at: None,
            ..notify()
        })
        .unwrap()
        .into_iter()
        .filter(|n| n.title == "срок со временем")
        .map(|n| (n.at, n.key.rsplit(':').next().unwrap_or_default().to_string()))
        .collect::<Vec<_>>();
    assert_eq!(
        several,
        [
            ("2026-10-05T17:30".to_string(), "60".to_string()),
            ("2026-10-05T18:15".to_string(), "15".to_string()),
        ]
    );

    let summary: Vec<(String, u32)> = d
        .planned_notifications(NotifySettings {
            lead_minutes: vec![],
            all_day_at: None,
            summary_at: Some("08:00".into()),
            ..notify()
        })
        .unwrap()
        .into_iter()
        .filter(|n| n.kind == NotificationKind::Summary)
        .map(|n| (n.at, n.count))
        .collect();
    assert_eq!(
        summary[0],
        ("2026-10-06T08:00".to_string(), 3),
        "today's eight o'clock has passed; tomorrow counts everything due by then"
    );
    assert_eq!(summary[1], ("2026-10-07T08:00".to_string(), 4));
    assert_eq!(summary.len(), 6);
}

fn due_today(d: &Device, title: &str) -> TaskItem {
    d.create_task(NewTask {
        title: title.into(),
        due: Some("2026-10-05".into()),
        ..NewTask::default()
    })
    .unwrap()
}

#[test]
fn r68_a_completed_task_stays_in_place_for_the_set_time() {
    let d = device();
    assert_eq!(
        d.keep_done().unwrap(),
        KeepDone::Seconds { seconds: 5 },
        "five seconds unless set"
    );
    due_today(&d, "а");
    let b = due_today(&d, "б");
    due_today(&d, "в");
    d.add_tag(b.id.clone(), "дом".into()).unwrap();
    let tag = || Scope::Tag { name: "дом".into() };

    d.complete_task(b.id.clone()).unwrap();
    d.set_now_for_tests("2026-10-05T10:00:04");
    for scope in [Scope::Today, Scope::Inbox, Scope::All] {
        let tasks = d.tasks(scope.clone()).unwrap();
        assert_eq!(titles(&tasks), ["а", "б", "в"], "{scope:?}: where it was");
        assert!(tasks[1].done.is_some(), "{scope:?}: shown as completed");
    }
    assert_eq!(view(&d, tag()), ["б"]);
    let counts = d.counts().unwrap();
    assert_eq!((counts.today, counts.inbox), (2, 2), "not counted as open");
    assert_eq!(d.tags().unwrap().len(), 0, "nor under its tag");

    // Completed at 10:00:00, kept through 10:00:05.
    d.set_now_for_tests("2026-10-05T10:00:05");
    assert_eq!(view(&d, Scope::Today), ["а", "б", "в"]);
    d.set_now_for_tests("2026-10-05T10:00:06");
    for scope in [Scope::Today, Scope::Inbox, Scope::All] {
        assert_eq!(view(&d, scope.clone()), ["а", "в"], "{scope:?}: gone after the time");
    }
    assert!(view(&d, tag()).is_empty());
    assert_eq!(view(&d, Scope::Completed), ["б"]);
}

#[test]
fn r68_reopening_a_kept_task_leaves_it_where_it_was() {
    let d = device();
    due_today(&d, "а");
    let b = due_today(&d, "б");
    due_today(&d, "в");
    d.complete_task(b.id.clone()).unwrap();
    d.reopen_task(b.id).unwrap();
    d.set_now_for_tests("2026-10-05T12:00");
    assert_eq!(view(&d, Scope::Today), ["а", "б", "в"]);
    assert_eq!(d.counts().unwrap().today, 3);
}

#[test]
fn r68_the_time_is_a_setting_and_zero_removes_at_once() {
    let d = device();
    due_today(&d, "а");
    let b = due_today(&d, "б");
    d.complete_task(b.id).unwrap();

    d.set_keep_done(KeepDone::Seconds { seconds: 0 }).unwrap();
    assert_eq!(d.keep_done().unwrap(), KeepDone::Seconds { seconds: 0 });
    assert_eq!(view(&d, Scope::Today), ["а"], "at once");

    d.set_keep_done(KeepDone::Seconds { seconds: 3600 }).unwrap();
    d.set_now_for_tests("2026-10-05T10:59");
    assert_eq!(
        view(&d, Scope::Today),
        ["а", "б"],
        "the setting applies to what is already completed"
    );
    d.set_now_for_tests("2026-10-05T11:01");
    assert_eq!(view(&d, Scope::Today), ["а"]);

    d.set_keep_done(KeepDone::Seconds { seconds: 6_000_000 }).unwrap();
    assert_eq!(
        d.keep_done().unwrap(),
        KeepDone::Seconds { seconds: 86_400 },
        "a day at most"
    );
    for choice in [
        KeepDone::Seconds { seconds: 5 },
        KeepDone::Seconds { seconds: 15 },
        KeepDone::EndOfDay,
    ] {
        d.set_keep_done(choice).unwrap();
        assert_eq!(d.keep_done().unwrap(), choice);
    }
}

#[test]
fn r68_until_the_end_of_the_day_keeps_what_was_completed_today_until_midnight() {
    let d = device();
    // The stamps of a device never go back, so the earlier day comes first.
    d.set_now_for_tests("2026-10-04T23:50");
    d.set_keep_done(KeepDone::EndOfDay).unwrap();
    add(&d, "а");
    let yesterday = add(&d, "вчера");
    let b = add(&d, "б");
    d.complete_task(yesterday.id).unwrap();
    assert_eq!(view(&d, Scope::Inbox), ["а", "вчера", "б"], "kept on its own day");

    d.set_now_for_tests("2026-10-05T10:00");
    assert_eq!(view(&d, Scope::Inbox), ["а", "б"], "that day is over");
    assert_eq!(d.seconds_until_kept_leaves().unwrap(), None);
    d.complete_task(b.id).unwrap();
    assert_eq!(d.seconds_until_kept_leaves().unwrap(), Some(14 * 3600));

    d.set_now_for_tests("2026-10-05T23:59:59");
    let tasks = d.tasks(Scope::Inbox).unwrap();
    assert_eq!(titles(&tasks), ["а", "б"]);
    assert!(tasks[1].done.is_some(), "shown as completed");
    assert_eq!(d.counts().unwrap().inbox, 1, "not counted as open");
    assert_eq!(d.seconds_until_kept_leaves().unwrap(), Some(1));

    d.set_now_for_tests("2026-10-06T00:00");
    assert_eq!(view(&d, Scope::Inbox), ["а"], "gone at midnight");
    assert_eq!(view(&d, Scope::Completed), ["б", "вчера"]);
    assert_eq!(d.seconds_until_kept_leaves().unwrap(), None);
}

#[test]
fn r135_today_is_the_day_of_the_zone_the_call_is_made_in() {
    let d = device();
    // 22:30 UTC: 01:30 the next day three hours east, 12:30 the same day ten hours west.
    d.set_now_for_tests("2026-10-04T22:30");
    let (east, west) = (180, -600);
    for (title, due) in [("четвёртое", "2026-10-04"), ("пятое", "2026-10-05")] {
        d.create_task(NewTask {
            title: title.into(),
            due: Some(due.into()),
            ..NewTask::default()
        })
        .unwrap();
    }
    let today = |zone| {
        let mut titles = d.in_zone(zone, || view(&d, Scope::Today));
        titles.sort();
        titles
    };
    assert_eq!(today(east), ["пятое", "четвёртое"]);
    assert_eq!(today(west), ["четвёртое"]);
    assert_eq!(d.in_zone(east, || d.counts().unwrap().overdue), 1);
    assert_eq!(d.in_zone(west, || d.counts().unwrap().overdue), 0);

    let call = |zone| {
        d.in_zone(zone, || {
            d.quick_add("позвонить 9:00".into(), None, false, None).unwrap()
        })
    };
    assert_eq!(call(east).due.as_deref(), Some("2026-10-05T09:00"));
    assert_eq!(call(west).due.as_deref(), Some("2026-10-04T09:00"));

    d.set_keep_done(KeepDone::EndOfDay).unwrap();
    let task = add(&d, "полить цветы");
    let done = d.in_zone(east, || d.complete_task(task.id.clone()).unwrap());
    assert_eq!(
        done.done.as_deref(),
        Some("2026-10-05T01:30"),
        "written on the clock of the zone"
    );
    assert_eq!(
        d.in_zone(east, || d.seconds_until_kept_leaves().unwrap()),
        Some(22 * 3600 + 30 * 60),
        "kept until midnight of that zone"
    );
    assert!(
        !view(&d, Scope::Today).contains(&"пятое".to_string()),
        "outside, the clock is the device's again"
    );
}

#[test]
fn s32_the_time_is_stored_so_that_a_version_that_knows_only_minutes_can_read_it() {
    let d = device();
    let stored = |keep| {
        d.set_keep_done(keep).unwrap();
        d.setting_for_tests("keep_done").unwrap()
    };
    assert_eq!(stored(KeepDone::Seconds { seconds: 0 }), serde_json::json!(0));
    assert_eq!(stored(KeepDone::Seconds { seconds: 900 }), serde_json::json!(15));
    assert_eq!(stored(KeepDone::Seconds { seconds: 5 }), serde_json::json!("5s"));
    assert_eq!(stored(KeepDone::Seconds { seconds: 15 }), serde_json::json!("15s"));
    assert_eq!(stored(KeepDone::EndOfDay), serde_json::json!("day"));

    // What such a version wrote, and what a later one may write.
    let read = |value| {
        d.set_setting_for_tests("keep_done", value);
        d.keep_done().unwrap()
    };
    assert_eq!(read(serde_json::json!(15)), KeepDone::Seconds { seconds: 900 });
    assert_eq!(read(serde_json::json!(100_000)), KeepDone::Seconds { seconds: 86_400 });
    assert_eq!(read(serde_json::json!("90s")), KeepDone::Seconds { seconds: 90 });
    assert_eq!(
        read(serde_json::json!("9999999s")),
        KeepDone::Seconds { seconds: 86_400 }
    );
    for unknown in [
        serde_json::json!("week"),
        serde_json::json!("s"),
        serde_json::json!(-1),
        serde_json::json!(null),
    ] {
        assert_eq!(
            read(unknown.clone()),
            KeepDone::Seconds { seconds: 5 },
            "{unknown}: the default"
        );
    }
}

#[test]
fn r68_where_completed_tasks_are_shown_a_kept_one_joins_them_later() {
    let d = device();
    let list = d.create_list("Дом".into()).unwrap();
    d.set_list_show_done(list.id.clone(), true).unwrap();
    let in_list = |title: &str| {
        d.create_task(NewTask {
            title: title.into(),
            list_id: Some(list.id.clone()),
            ..NewTask::default()
        })
        .unwrap()
    };
    in_list("а");
    let b = in_list("б");
    in_list("в");
    let project = add(&d, "проект");
    d.set_project(project.id.clone(), true).unwrap();
    add_sub(&d, &project, "раз");
    let two = add_sub(&d, &project, "два");
    add_sub(&d, &project, "три");
    let of_list = || view(&d, Scope::List { id: list.id.clone() });
    let of_project = || view(&d, Scope::Project { id: project.id.clone() });
    let open_and_all = |status| {
        titles(
            &d.preview_filter(FilterSpec {
                list_ids: vec![list.id.clone()],
                status,
                ..FilterSpec::default()
            })
            .unwrap(),
        )
        .into_iter()
        .map(str::to_string)
        .collect::<Vec<_>>()
    };

    d.complete_task(b.id).unwrap();
    d.complete_task(two.id).unwrap();
    assert_eq!(of_list(), ["а", "б", "в"]);
    assert_eq!(of_project(), ["раз", "два", "три"]);
    assert_eq!(open_and_all(FilterStatus::Open), ["а", "б", "в"]);
    assert_eq!(open_and_all(FilterStatus::All), ["а", "б", "в"]);

    d.set_now_for_tests("2026-10-05T10:06");
    assert_eq!(of_list(), ["а", "в", "б"]);
    assert_eq!(of_project(), ["раз", "три", "два"]);
    assert_eq!(open_and_all(FilterStatus::Open), ["а", "в"]);
    assert_eq!(open_and_all(FilterStatus::All), ["а", "в", "б"]);
}

#[test]
fn r68_a_repeating_task_moves_on_and_its_record_stays_in_place_for_the_time() {
    let d = device();
    due_today(&d, "а");
    let daily = due_today(&d, "каждый день");
    due_today(&d, "в");
    d.set_repeat(
        daily.id.clone(),
        Some(Repeat {
            freq: Freq::Daily,
            ..weekly()
        }),
    )
    .unwrap();
    d.add_tag(daily.id.clone(), "дом".into()).unwrap();
    let next = d.complete_task(daily.id.clone()).unwrap();
    assert!(next.done.is_none());
    assert_eq!(next.due.as_deref(), Some("2026-10-06"));

    d.set_now_for_tests("2026-10-05T10:00:04");
    let today = d.tasks(Scope::Today).unwrap();
    assert_eq!(
        titles(&today),
        ["а", "каждый день", "в"],
        "the record stands where the task stood"
    );
    assert!(today[1].is_log && today[1].done.is_some(), "shown as completed");
    assert_eq!(
        view(&d, Scope::Upcoming),
        ["каждый день"],
        "the task is at its next date at once"
    );
    let inbox = d.tasks(Scope::Inbox).unwrap();
    assert_eq!(
        titles(&inbox),
        ["а", "каждый день", "каждый день", "в"],
        "the task and its record"
    );
    assert_eq!(inbox.iter().filter(|t| t.is_log).count(), 1);
    assert_eq!(
        view(&d, Scope::Tag { name: "дом".into() }),
        ["каждый день", "каждый день"]
    );
    assert_eq!(d.counts().unwrap().today, 2, "the record is not counted as open");
    assert_eq!(d.seconds_until_kept_leaves().unwrap(), Some(2));

    d.set_now_for_tests("2026-10-05T10:00:06");
    assert_eq!(view(&d, Scope::Today), ["а", "в"]);
    assert_eq!(view(&d, Scope::Inbox), ["а", "каждый день", "в"]);
    assert_eq!(view(&d, Scope::Completed), ["каждый день"]);
    assert_eq!(d.seconds_until_kept_leaves().unwrap(), None);

    d.set_keep_done(KeepDone::Seconds { seconds: 0 }).unwrap();
    d.set_now_for_tests("2026-10-06T10:00");
    d.complete_task(daily.id).unwrap();
    assert_eq!(view(&d, Scope::Today), ["а", "в"], "at once: the record is not shown");
}

#[test]
fn r68_the_record_of_a_project_task_stands_in_the_project_only() {
    let d = device();
    let project = add(&d, "ремонт");
    d.set_project(project.id.clone(), true).unwrap();
    let t = d
        .create_task(NewTask {
            title: "проветрить".into(),
            parent_id: Some(project.id.clone()),
            due: Some("2026-10-05".into()),
            ..NewTask::default()
        })
        .unwrap();
    d.set_repeat(t.id.clone(), Some(weekly())).unwrap();
    d.complete_task(t.id).unwrap();
    assert_eq!(
        view(&d, Scope::Project { id: project.id.clone() }),
        ["проветрить", "проветрить"]
    );
    assert_eq!(
        view(&d, Scope::Today),
        ["проветрить"],
        "the record, as the task was there"
    );
    assert!(view(&d, Scope::Inbox).is_empty(), "not a top-level row of the list");
    let p = d.task(project.id.clone()).unwrap();
    assert_eq!(
        (p.subtasks_total, p.subtasks_done),
        (1, 0),
        "the record is not a task of the project"
    );
    d.set_now_for_tests("2026-10-05T10:00:06");
    assert_eq!(view(&d, Scope::Project { id: project.id }), ["проветрить"]);
}

#[test]
fn r68_a_far_clock_does_not_keep_a_task() {
    let d = device();
    // Completed where the clock is three hours ahead: `done` carries no time zone.
    let far = due_today(&d, "издалека");
    d.set_now_for_tests("2026-10-05T13:00");
    d.complete_task(far.id).unwrap();
    d.set_now_for_tests("2026-10-05T10:01");
    assert!(view(&d, Scope::Today).is_empty());
}

#[test]
fn r68_the_time_runs_to_the_second_and_the_store_tells_when_it_ends() {
    let d = device();
    d.set_keep_done(KeepDone::Seconds { seconds: 15 }).unwrap();
    assert_eq!(d.seconds_until_kept_leaves().unwrap(), None, "nothing is kept");
    due_today(&d, "а");
    let b = due_today(&d, "б");

    d.set_now_for_tests("2026-10-05T10:00:40");
    d.complete_task(b.id).unwrap();
    d.set_now_for_tests("2026-10-05T10:00:50");
    assert_eq!(view(&d, Scope::Today), ["а", "б"]);
    assert_eq!(d.seconds_until_kept_leaves().unwrap(), Some(6));

    d.set_now_for_tests("2026-10-05T10:00:54");
    assert_eq!(view(&d, Scope::Today), ["а", "б"], "a second before the time is over");
    d.set_now_for_tests("2026-10-05T10:00:56");
    assert_eq!(view(&d, Scope::Today), ["а"], "a second after");
    assert_eq!(d.seconds_until_kept_leaves().unwrap(), None);

    d.set_keep_done(KeepDone::Seconds { seconds: 0 }).unwrap();
    assert_eq!(
        d.seconds_until_kept_leaves().unwrap(),
        None,
        "at once: nothing to wait for"
    );
}

#[test]
fn r69_a_task_closed_as_wont_do_is_kept_then_logged_and_can_be_reopened() {
    let d = device();
    let a = add(&d, "а");
    let b = add(&d, "б");
    add(&d, "в");

    let closed = d.wont_do_task(b.id.clone()).unwrap();
    assert!(closed.wont && closed.done.is_some());
    d.set_now_for_tests("2026-10-05T10:00:04");
    let inbox = d.tasks(Scope::Inbox).unwrap();
    assert_eq!(titles(&inbox), ["а", "б", "в"], "kept where it was (R68)");
    assert!(inbox[1].wont);
    assert_eq!(d.counts().unwrap().inbox, 2, "not counted as open");

    d.complete_task(a.id.clone()).unwrap();
    d.set_now_for_tests("2026-10-05T10:30");
    assert_eq!(view(&d, Scope::Inbox), ["в"]);
    assert_eq!(view(&d, Scope::Completed), ["а", "б"], "both outcomes are in the log");
    assert_eq!(view(&d, Scope::WontDo), ["б"]);
    assert!(!d.task(a.id.clone()).unwrap().wont);

    // Closing a closed task either way changes nothing.
    assert!(d.complete_task(b.id.clone()).unwrap().wont);
    assert!(!d.wont_do_task(a.id).unwrap().wont);

    d.reopen_task(b.id.clone()).unwrap();
    let back = d.task(b.id.clone()).unwrap();
    assert!(back.done.is_none() && !back.wont);
    assert_eq!(view(&d, Scope::Inbox), ["б", "в"]);
    assert!(view(&d, Scope::WontDo).is_empty());

    // Completed after that, it is completed.
    assert!(!d.complete_task(b.id).unwrap().wont);
    assert!(view(&d, Scope::WontDo).is_empty());
}

#[test]
fn r69_open_subtasks_get_the_same_outcome_and_completed_ones_keep_theirs() {
    let d = device();
    let top = add(&d, "родитель");
    d.set_project(top.id.clone(), true).unwrap();
    let done = add_sub(&d, &top, "сделана");
    let open = add_sub(&d, &top, "открыта");
    let deep = add_sub(&d, &open, "глубже");
    d.complete_task(done.id.clone()).unwrap();

    d.wont_do_task(top.id.clone()).unwrap();
    assert!(d.task(open.id.clone()).unwrap().wont);
    assert!(d.task(deep.id).unwrap().wont);
    let kept = d.task(done.id).unwrap();
    assert!(kept.done.is_some() && !kept.wont);
    assert_eq!(view(&d, Scope::WontDo), ["родитель"], "subtasks go with their task");

    // Reopening the parent leaves the subtasks alone (R11).
    d.reopen_task(top.id).unwrap();
    assert!(d.task(open.id).unwrap().wont);
}

#[test]
fn r69_r11_undoing_a_close_reopens_the_subtasks_closed_with_it() {
    for wont in [true, false] {
        let d = device();
        let top = add(&d, "родитель");
        d.set_project(top.id.clone(), true).unwrap();
        let done = add_sub(&d, &top, "сделана");
        let open = add_sub(&d, &top, "открыта");
        let deep = add_sub(&d, &open, "глубже");
        d.complete_task(done.id.clone()).unwrap();
        d.set_now_for_tests("2026-10-05T10:00:04");

        if wont {
            d.wont_do_task(top.id.clone()).unwrap();
        } else {
            d.complete_task(top.id.clone()).unwrap();
        }
        d.set_now_for_tests("2026-10-05T10:00:06");
        d.undo_close_task(top.id.clone()).unwrap();

        for id in [&top.id, &open.id, &deep.id] {
            let back = d.task(id.clone()).unwrap();
            assert!(
                back.done.is_none() && !back.wont,
                "closed by the action, reopened by its undo"
            );
        }
        let kept = d.task(done.id).unwrap();
        assert!(
            kept.done.is_some() && !kept.wont,
            "completed before the action, left as it was"
        );

        // Nothing to take back on an open task.
        d.undo_close_task(top.id.clone()).unwrap();
        assert!(d.task(top.id).unwrap().done.is_none());
    }
}

#[test]
fn r69_undoing_the_close_of_a_subtask_leaves_its_parent_and_neighbours_alone() {
    let d = device();
    let top = add(&d, "родитель");
    let one = add_sub(&d, &top, "первая");
    let two = add_sub(&d, &top, "вторая");

    d.wont_do_task(one.id.clone()).unwrap();
    assert!(d.task(top.id.clone()).unwrap().done.is_none(), "the parent stays open");
    assert!(
        d.task(two.id.clone()).unwrap().done.is_none(),
        "the neighbour stays open"
    );

    d.undo_close_task(one.id.clone()).unwrap();
    assert!(d.task(one.id).unwrap().done.is_none());
    assert!(d.task(top.id).unwrap().done.is_none());
    assert!(d.task(two.id).unwrap().done.is_none());
}

#[test]
fn r69_r17_a_repeating_task_skips_the_occurrence() {
    let d = device();
    let t = add(&d, "зарядка");
    d.set_due(t.id.clone(), Some("2026-10-05".into())).unwrap();
    d.set_repeat(t.id.clone(), Some(weekly())).unwrap();
    let sub = add_sub(&d, &t, "разминка");
    d.wont_do_task(sub.id.clone()).unwrap();

    let after = d.wont_do_task(t.id.clone()).unwrap();
    assert!(after.done.is_none() && !after.wont, "the task stays open");
    assert_eq!(after.due.as_deref(), Some("2026-10-12"));
    let sub = d.task(sub.id).unwrap();
    assert!(
        sub.done.is_none() && !sub.wont,
        "subtasks are reopened for the next round"
    );

    let log = d.tasks(Scope::WontDo).unwrap();
    assert_eq!(titles(&log), ["зарядка"]);
    assert!(log[0].is_log && log[0].wont);
    assert_eq!(log[0].due.as_deref(), Some("2026-10-05"));
    assert_eq!(view(&d, Scope::Completed), ["зарядка"]);

    // The next occurrence is completed: its record is a completed one.
    d.complete_task(t.id).unwrap();
    assert_eq!(view(&d, Scope::Completed).len(), 2);
    assert_eq!(view(&d, Scope::WontDo).len(), 1);
}

#[test]
fn r69_r30_wont_do_tasks_leave_both_numbers_of_the_counter() {
    let d = device();
    let project = add(&d, "проект");
    d.set_project(project.id.clone(), true).unwrap();
    let one = add_sub(&d, &project, "раз");
    let two = add_sub(&d, &project, "два");
    let three = add_sub(&d, &project, "три");
    d.complete_task(one.id).unwrap();
    d.complete_task(two.id).unwrap();
    assert_eq!(d.projects().unwrap()[0].subtasks_total, 3);

    d.wont_do_task(three.id.clone()).unwrap();
    let p = &d.projects().unwrap()[0];
    assert_eq!((p.subtasks_done, p.subtasks_total), (2, 2));

    d.reopen_task(three.id).unwrap();
    let p = &d.projects().unwrap()[0];
    assert_eq!((p.subtasks_done, p.subtasks_total), (2, 3));
}

#[test]
fn r69_r33_a_filter_tells_the_two_outcomes_apart() {
    let d = device();
    add(&d, "открыта");
    let done = add(&d, "выполнена");
    let wont = add(&d, "не буду");
    d.complete_task(done.id).unwrap();
    d.wont_do_task(wont.id).unwrap();
    d.set_now_for_tests("2026-10-05T10:30");

    let show = |status: FilterStatus| {
        let mut got = view_of(&d.preview_filter(FilterSpec { status, ..spec() }).unwrap());
        got.sort();
        got
    };
    assert_eq!(show(FilterStatus::Open), ["открыта"]);
    assert_eq!(show(FilterStatus::Done), ["выполнена"]);
    assert_eq!(show(FilterStatus::Wont), ["не буду"]);
    assert_eq!(show(FilterStatus::All), ["выполнена", "не буду", "открыта"]);

    // The value is stored and read back under its own name.
    let saved = d
        .create_filter(
            "брошенное".into(),
            FilterSpec {
                status: FilterStatus::Wont,
                ..spec()
            },
        )
        .unwrap();
    assert_eq!(view(&d, Scope::Filter { id: saved.id }), ["не буду"]);
}

fn view_of(tasks: &[TaskItem]) -> Vec<String> {
    tasks.iter().map(|t| t.title.clone()).collect()
}

#[test]
fn r69_r46_clearing_completed_removes_both_outcomes() {
    let d = device();
    d.set_now_for_tests("2026-09-01T09:00");
    let done = add(&d, "выполнена");
    let wont = add(&d, "не буду");
    d.complete_task(done.id).unwrap();
    d.wont_do_task(wont.id).unwrap();
    d.set_now_for_tests(NOW);
    assert_eq!(view(&d, Scope::Completed).len(), 2);

    assert_eq!(d.clear_completed(Some("2026-10-01".into())).unwrap(), 2);
    assert!(view(&d, Scope::Completed).is_empty());
    assert!(view(&d, Scope::WontDo).is_empty());
}

/// Attaches a file with that name and content and returns the attachment as the views get it.
fn shown_attachment(d: &Device, name: &str, content: &[u8]) -> Attachment {
    let task = add(&d.store, name);
    let src = tempfile::tempdir().unwrap();
    let file = src.path().join(name);
    std::fs::write(&file, content).unwrap();
    d.add_attachment(task.id.clone(), file.to_string_lossy().into_owned(), None)
        .unwrap();
    d.attachments(task.id).unwrap().remove(0)
}

#[test]
fn r85_a_name_without_a_type_takes_the_type_of_the_content() {
    let d = device();
    let jpeg = shown_attachment(&d, "Image", &[0xFF, 0xD8, 0xFF, 0xE0, 0, 16, b'J', b'F', b'I', b'F']);
    assert_eq!((jpeg.name.as_str(), jpeg.mime.as_str()), ("Image.jpg", "image/jpeg"));
    let pdf = shown_attachment(&d, "Scan", b"%PDF-1.7 and the rest");
    assert_eq!((pdf.name.as_str(), pdf.mime.as_str()), ("Scan.pdf", "application/pdf"));
    let png = shown_attachment(&d, "shot", b"\x89PNG\r\n\x1a\n0000");
    assert_eq!((png.name.as_str(), png.mime.as_str()), ("shot.png", "image/png"));
}

#[test]
fn r85_a_name_that_tells_the_type_and_content_that_tells_none_are_left_alone() {
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
fn r85_the_registers_keep_the_name_the_file_came_with() {
    let d = device();
    let shown = shown_attachment(&d, "Image", &[0xFF, 0xD8, 0xFF, 0xE0]);
    assert_eq!(shown.name, "Image.jpg");
    // Without the content the name is what was stored: nothing was written back.
    std::fs::remove_file(shown.local_path.unwrap()).unwrap();
    let bare = d.attachments(shown.task_id).unwrap().remove(0);
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

fn attach_named(d: &Device, task: &TaskItem, name: &str, content: &[u8]) -> Attachment {
    let src = tempfile::tempdir().unwrap();
    let file = src.path().join("source");
    std::fs::write(&file, content).unwrap();
    d.add_attachment(task.id.clone(), file.to_string_lossy().into_owned(), Some(name.into()))
        .unwrap()
}

fn named(name: &str, content: &[u8]) -> (String, Vec<u8>) {
    (name.to_string(), content.to_vec())
}

#[test]
fn r91_every_attachment_is_saved_under_its_name() {
    let d = device();
    let t = add(&d, "с тремя файлами");
    attach_named(&d, &t, "a.txt", b"first");
    attach_named(&d, &t, "b.pdf", b"%PDF-1.7 second");
    attach_named(&d, &t, "c.png", b"third");
    let out = tempfile::tempdir().unwrap();

    let report = d
        .save_attachments(t.id.clone(), out.path().to_string_lossy().into_owned())
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
    assert!(d.attachments(t.id).unwrap().iter().all(|f| f.local_path.is_some()));
}

#[test]
fn r91_a_name_that_is_taken_gets_a_number_and_nothing_is_replaced() {
    let d = device();
    let t = add(&d, "с одинаковыми именами");
    attach_named(&d, &t, "a.txt", b"one");
    attach_named(&d, &t, "a.txt", b"two");
    attach_named(&d, &t, "notes", b"three");
    attach_named(&d, &t, ".env", b"four");
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
    let report = d.save_attachments(t.id.clone(), dir.clone()).unwrap();

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
    d.save_attachments(t.id, dir).unwrap();
    assert_eq!(folder_content(out.path()).len(), 12);
}

#[test]
fn r91_a_name_from_another_device_stays_inside_the_folder() {
    let d = device();
    let t = add(&d, "с чужим именем");
    attach_named(&d, &t, "../up/and:out.txt", b"escaped");
    attach_named(&d, &t, "plain.txt", b"plain");
    let parent = tempfile::tempdir().unwrap();
    let out = parent.path().join("chosen");
    std::fs::create_dir(&out).unwrap();

    let report = d.save_attachments(t.id, out.to_string_lossy().into_owned()).unwrap();

    assert!(report.failed.is_empty(), "{report:?}");
    assert_eq!(
        folder_content(&out),
        [named(".._up_and_out.txt", b"escaped"), named("plain.txt", b"plain")]
    );
    assert_eq!(std::fs::read_dir(parent.path()).unwrap().count(), 1);
}

#[test]
fn r91_a_file_that_cannot_be_written_is_named_and_the_rest_are_saved() {
    let d = device();
    let t = add(&d, "с потерянным файлом");
    let lost = attach_named(&d, &t, "lost.txt", b"lost");
    attach_named(&d, &t, "kept.txt", b"kept");
    // The content is gone from the device and there is no storage to take it from.
    std::fs::remove_file(lost.local_path.unwrap()).unwrap();
    let out = tempfile::tempdir().unwrap();

    let report = d
        .save_attachments(t.id.clone(), out.path().to_string_lossy().into_owned())
        .unwrap();

    assert_eq!(
        (report.saved, report.failed, report.fetched),
        (vec!["kept.txt".to_string()], vec!["lost.txt".to_string()], 0)
    );
    assert_eq!(folder_content(out.path()), [named("kept.txt", b"kept")]);

    // A folder that is not there: every file is named, nothing is thrown.
    let nowhere = out.path().join("missing").to_string_lossy().into_owned();
    let report = d.save_attachments(t.id, nowhere).unwrap();
    assert_eq!((report.saved.len(), report.failed.len()), (0, 2));
}

/// Attaches the content of `file` to a new task.
fn attach_file(d: &Device, content: &[u8]) -> std::result::Result<Attachment, AppError> {
    let task = add(d, "с файлом");
    let src = tempfile::tempdir().unwrap();
    let file = src.path().join("file.bin");
    std::fs::write(&file, content).unwrap();
    d.add_attachment(task.id, file.to_string_lossy().into_owned(), None)
}

#[test]
fn r140_a_file_above_the_limit_is_not_added() {
    let d = device();
    d.set_attachment_limits(AttachmentLimits {
        max_file: 10,
        max_task: 1000,
    })
    .unwrap();

    let error = attach_file(&d, b"0123456789A").unwrap_err();
    assert!(matches!(error, AppError::TooLarge { .. }), "{error}");

    // The task has no attachment and the content was not left behind.
    let task = d.tasks(Scope::Inbox).unwrap().remove(0);
    assert!(d.attachments(task.id).unwrap().is_empty());
    assert_eq!(std::fs::read_dir(d.dir().join("blobs")).unwrap().count(), 0);
}

#[test]
fn r140_the_task_limit_counts_the_live_attachments() {
    let d = device();
    d.set_attachment_limits(AttachmentLimits {
        max_file: 100,
        max_task: 10,
    })
    .unwrap();
    let task = add(&d, "с двумя файлами");
    let src = tempfile::tempdir().unwrap();
    let file = src.path().join("six.bin");
    std::fs::write(&file, b"123456").unwrap();
    let path = file.to_string_lossy().into_owned();
    let first = d.add_attachment(task.id.clone(), path.clone(), None).unwrap();

    let error = d.add_attachment(task.id.clone(), path.clone(), None).unwrap_err();
    assert!(matches!(error, AppError::TooLarge { .. }), "{error}");
    assert_eq!(d.attachments(task.id.clone()).unwrap().len(), 1);

    // A removed attachment no longer counts, and the same content goes in.
    d.remove_attachment(first.id).unwrap();
    let second = d.add_attachment(task.id.clone(), path, None).unwrap();
    assert_eq!(second.size, 6);
}

#[test]
fn r140_a_large_file_is_copied_in_pieces() {
    let d = device();
    d.set_attachment_limits(AttachmentLimits {
        max_file: 64 * 1024 * 1024,
        max_task: 128 * 1024 * 1024,
    })
    .unwrap();
    let task = add(&d, "с большим файлом");
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
        .add_attachment(task.id, file.to_string_lossy().into_owned(), None)
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
fn r140_a_failed_add_does_not_leave_content() {
    let d = device();
    let src = tempfile::tempdir().unwrap();
    let file = src.path().join("file.bin");
    std::fs::write(&file, b"content nobody kept").unwrap();

    let error = d
        .add_attachment("no-such-task".into(), file.to_string_lossy().into_owned(), None)
        .unwrap_err();
    assert!(matches!(error, AppError::NotFound { .. }), "{error}");
    assert_eq!(std::fs::read_dir(d.dir().join("blobs")).unwrap().count(), 0);
}

#[test]
fn r140_the_limits_belong_to_the_device_and_stay() {
    let d = device();
    let dir = d.dir().to_path_buf();
    d.set_attachment_limits(AttachmentLimits {
        max_file: 3,
        max_task: 4,
    })
    .unwrap();

    // A second connection to the same database, as a restart of the app would open it.
    let again = Store::open(dir.to_string_lossy().into_owned()).unwrap();
    assert_eq!(
        again.attachment_limits(),
        AttachmentLimits {
            max_file: 3,
            max_task: 4
        }
    );
}

#[cfg(unix)]
#[test]
fn r142_the_data_folder_is_for_its_owner_alone() {
    use std::os::unix::fs::PermissionsExt;
    let mode = |path: &std::path::Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
    let d = device();
    assert_eq!(mode(d.dir()), 0o700);
    assert_eq!(mode(&d.dir().join("blobs")), 0o700);
    assert_eq!(mode(&d.dir().join("lists.sqlite")), 0o600);

    let task = add(&d, "с файлом");
    let src = tempfile::tempdir().unwrap();
    let file = src.path().join("file.bin");
    std::fs::write(&file, b"content").unwrap();
    let added = d
        .add_attachment(task.id, file.to_string_lossy().into_owned(), None)
        .unwrap();
    assert_eq!(mode(std::path::Path::new(added.local_path.as_ref().unwrap())), 0o600);

    d.log(LogLevel::Error, "attachments".into(), "a reason for a line".into());
    assert_eq!(mode(&d.dir().join("log.txt")), 0o600);
}

/// A project with tasks, all in Inbox: (project, tasks by title).
fn project_with(d: &Device, dates: &[(&str, Option<&str>, Option<&str>)]) -> (TaskItem, Vec<TaskItem>) {
    let project = add(d, "Ремонт");
    d.set_project(project.id.clone(), true).unwrap();
    let tasks = dates
        .iter()
        .map(|(title, start, due)| {
            d.create_task(NewTask {
                title: (*title).into(),
                parent_id: Some(project.id.clone()),
                start: start.map(str::to_string),
                due: due.map(str::to_string),
                ..NewTask::default()
            })
            .unwrap()
        })
        .collect();
    (project, tasks)
}

fn task_named(d: &Device, scope: Scope, title: &str) -> TaskItem {
    d.tasks(scope).unwrap().into_iter().find(|t| t.title == title).unwrap()
}

#[test]
fn r125_a_task_without_a_due_date_takes_the_projects() {
    let d = device(); // today is 2026-10-05
    let (project, tasks) = project_with(&d, &[("а", None, None), ("б", None, Some("2026-10-20"))]);
    d.set_due(project.id.clone(), Some("2026-10-10".into())).unwrap();

    let upcoming = d.tasks(Scope::Upcoming).unwrap();
    assert_eq!(titles(&upcoming), ["а", "б"], "ordered by the effective due date");
    let a = &upcoming[0];
    assert_eq!(
        (a.due.as_deref(), a.effective_due.as_deref()),
        (None, Some("2026-10-10")),
        "the task keeps only its own date"
    );
    let b = &upcoming[1];
    assert_eq!(
        (b.due.as_deref(), b.effective_due.as_deref()),
        (Some("2026-10-20"), Some("2026-10-20")),
        "an own due date later than the project's is kept"
    );
    assert_eq!(d.counts().unwrap().upcoming, 2);

    // Moving the project moves its task at once, and the data of the task stays as it was.
    d.set_due(project.id.clone(), Some("2026-10-03".into())).unwrap();
    assert_eq!(view(&d, Scope::Today), ["а"]);
    let counts = d.counts().unwrap();
    assert_eq!((counts.today, counts.overdue, counts.upcoming), (1, 1, 1));
    assert_eq!(d.task(tasks[0].id.clone()).unwrap().due, None);
    assert_eq!(
        d.task(tasks[0].id.clone()).unwrap().effective_due.as_deref(),
        Some("2026-10-03"),
        "a single task is read with its effective dates too"
    );

    // A project without a due date gives none.
    d.set_due(project.id.clone(), None).unwrap();
    assert_eq!(view(&d, Scope::Upcoming), ["б"]);
    assert_eq!(view(&d, Scope::Today), Vec::<String>::new());
}

#[test]
fn r125_a_task_does_not_start_before_its_project() {
    let d = device(); // today is 2026-10-05
    let (project, _) = project_with(
        &d,
        &[
            ("свой срок", None, Some("2026-10-05")),
            ("своё начало", Some("2026-10-04"), None),
            ("позже проекта", Some("2026-10-12"), None),
            ("без дат", None, None),
        ],
    );
    assert_eq!(view(&d, Scope::Today), ["своё начало", "свой срок"]);

    d.set_start(project.id.clone(), Some("2026-10-08".into())).unwrap();
    assert_eq!(
        view(&d, Scope::Today),
        Vec::<String>::new(),
        "nothing starts before the project"
    );
    let upcoming = d.tasks(Scope::Upcoming).unwrap();
    assert_eq!(titles(&upcoming), ["свой срок", "своё начало", "позже проекта"]);
    let start = |title: &str| {
        let t = upcoming.iter().find(|t| t.title == title).unwrap();
        (t.start.clone(), t.effective_start.clone())
    };
    assert_eq!(start("свой срок"), (None, Some("2026-10-08".into())));
    assert_eq!(
        start("своё начало"),
        (Some("2026-10-04".into()), Some("2026-10-08".into()))
    );
    assert_eq!(
        start("позже проекта"),
        (Some("2026-10-12".into()), Some("2026-10-12".into()))
    );
    assert_eq!(d.counts().unwrap().today, 0);

    // The start of the project gives no dates to a task that has none.
    let none = d.tasks(Scope::Project { id: project.id.clone() }).unwrap();
    let none = none.iter().find(|t| t.title == "без дат").unwrap();
    assert_eq!(
        (none.effective_start.as_deref(), none.effective_due.as_deref()),
        (None, None)
    );

    // With a due date the project dates every task, and its start holds them back.
    d.set_due(project.id.clone(), Some("2026-10-09".into())).unwrap();
    let t = task_named(&d, Scope::Upcoming, "без дат");
    assert_eq!(
        (t.effective_start.as_deref(), t.effective_due.as_deref()),
        (Some("2026-10-08"), Some("2026-10-09"))
    );

    // A due date puts the task in Today by its day, as an own due date does (R21).
    d.set_now_for_tests("2026-10-08T09:00");
    assert_eq!(view(&d, Scope::Today), ["свой срок"]);
    d.set_now_for_tests("2026-10-09T09:00");
    assert_eq!(view(&d, Scope::Today), ["свой срок", "своё начало", "без дат"]);
}

#[test]
fn r125_filters_select_a_task_by_its_effective_dates() {
    let d = device(); // today is 2026-10-05
    let (project, _) = project_with(&d, &[("а", None, None)]);
    let by = |due: DueWindow| {
        titles(&d.preview_filter(FilterSpec { due, ..spec() }).unwrap())
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>()
    };
    assert_eq!(by(DueWindow::NoDate), ["а"]);

    d.set_due(project.id.clone(), Some("2026-10-03".into())).unwrap();
    assert_eq!(by(DueWindow::Overdue), ["а"]);
    assert_eq!(by(DueWindow::NoDate), Vec::<String>::new());
    assert_eq!(by(DueWindow::Next { days: 7 }), ["а"]);

    d.set_due(project.id.clone(), Some("2026-10-05".into())).unwrap();
    assert_eq!(by(DueWindow::Today), ["а"]);
    assert_eq!(by(DueWindow::Overdue), Vec::<String>::new());
}

#[test]
fn r125_reminders_fire_on_the_inherited_due_date() {
    let d = device(); // now is 2026-10-05T10:00
    let (project, tasks) = project_with(&d, &[("а", None, None)]);
    d.set_due(project.id.clone(), Some("2026-10-07T18:00".into())).unwrap();

    let plan = d
        .planned_notifications(NotifySettings {
            summary_at: Some("08:00".into()),
            ..notify()
        })
        .unwrap();
    let a = plan
        .iter()
        .find(|n| n.task_id.as_deref() == Some(tasks[0].id.as_str()))
        .expect("the task of the project is reminded of");
    assert_eq!(
        (a.kind, a.at.as_str(), a.due.as_deref()),
        (NotificationKind::Due, "2026-10-07T17:45", Some("2026-10-07T18:00"))
    );
    let summary: Vec<(String, u32)> = plan
        .iter()
        .filter(|n| n.kind == NotificationKind::Summary && n.at.starts_with("2026-10-07"))
        .map(|n| (n.at.clone(), n.count))
        .collect();
    assert_eq!(
        summary,
        [("2026-10-07T08:00".to_string(), 1)],
        "the summary counts the task, not the project"
    );
}

#[test]
fn r129_a_task_goes_into_a_project_at_its_end_and_out_of_it_into_a_list() {
    let d = device();
    let home = d.create_list("Дом".into()).unwrap();
    let project = d
        .create_task(NewTask {
            title: "Ремонт".into(),
            list_id: Some(home.id.clone()),
            ..NewTask::default()
        })
        .unwrap();
    d.set_project(project.id.clone(), true).unwrap();
    d.quick_add_under("р".into(), project.id.clone(), false, None).unwrap();
    let a = add(&d, "а");
    d.set_due(a.id.clone(), Some("2026-10-07".into())).unwrap();

    d.move_to_project(a.id.clone(), project.id.clone()).unwrap();
    assert_eq!(
        view(&d, Scope::Project { id: project.id.clone() }),
        ["р", "а"],
        "at the end"
    );
    let moved = d.task(a.id.clone()).unwrap();
    assert_eq!(
        (moved.parent_id.as_deref(), moved.list_id.as_str(), moved.due.as_deref()),
        (Some(project.id.as_str()), home.id.as_str(), Some("2026-10-07")),
        "a task of the project, in the project's list, with its fields"
    );
    // Again into the same project: nothing changes.
    d.move_to_project(a.id.clone(), project.id.clone()).unwrap();
    assert_eq!(view(&d, Scope::Project { id: project.id.clone() }), ["р", "а"]);

    // A list out of the same picker takes it out of the project, also the project's own list.
    d.move_to_list(a.id.clone(), home.id.clone()).unwrap();
    let out = d.task(a.id.clone()).unwrap();
    assert_eq!((out.parent_id, out.list_id), (None, home.id.clone()));
    assert_eq!(view(&d, Scope::Project { id: project.id.clone() }), ["р"]);

    // Refused: a project, a task with subtasks, a subtask, a task that is not a project as the target.
    let other = add(&d, "Отпуск");
    d.set_project(other.id.clone(), true).unwrap();
    assert!(d.move_to_project(other.id.clone(), project.id.clone()).is_err());
    let holder = add(&d, "с подзадачей");
    let line = add_sub(&d, &holder, "строка");
    assert!(d.move_to_project(holder.id.clone(), project.id.clone()).is_err());
    assert!(d.move_to_project(line.id.clone(), project.id.clone()).is_err());
    assert!(d.move_to_project(a.id.clone(), holder.id.clone()).is_err());
    assert_eq!(d.task(holder.id).unwrap().parent_id, None);
    assert_eq!(d.task(line.id).unwrap().title, "строка");
}

fn placed(store: &Store, title: &str, first: bool, after: Option<&TaskItem>) -> TaskItem {
    store
        .create_task(NewTask {
            title: title.into(),
            first,
            after: after.map(|t| t.id.clone()),
            ..NewTask::default()
        })
        .unwrap()
}

#[test]
fn r130_a_new_task_goes_first_or_right_under_the_task_it_is_placed_after() {
    let d = device();
    let a = add(&d, "а");
    add(&d, "б");
    add(&d, "в");
    placed(&d, "г", true, None);
    assert_eq!(view(&d, Scope::Inbox), ["г", "а", "б", "в"]);
    placed(&d, "д", false, Some(&a));
    assert_eq!(view(&d, Scope::Inbox), ["г", "а", "д", "б", "в"]);
    // Without either a task still goes last, as everything that does not ask for a place.
    add(&d, "е");
    assert_eq!(view(&d, Scope::Inbox), ["г", "а", "д", "б", "в", "е"]);
}

#[test]
fn r130_placed_under_a_subtask_the_new_task_goes_under_its_parent() {
    let d = device();
    let a = add(&d, "а");
    add(&d, "б");
    let sub = add_sub(&d, &a, "подзадача");
    placed(&d, "в", false, Some(&sub));
    assert_eq!(view(&d, Scope::Inbox), ["а", "в", "б"]);
    assert_eq!(titles(&d.subtasks(a.id).unwrap()), ["подзадача"]);
}

#[test]
fn r130_placed_under_a_task_of_another_list_the_new_task_goes_first() {
    let d = device();
    let home = d.create_list("дом".into()).unwrap();
    let other = d
        .create_task(NewTask {
            title: "чужая".into(),
            list_id: Some(home.id.clone()),
            ..NewTask::default()
        })
        .unwrap();
    add(&d, "а");
    add(&d, "б");
    placed(&d, "в", false, Some(&other));
    assert_eq!(view(&d, Scope::Inbox), ["в", "а", "б"]);
    assert_eq!(view(&d, Scope::List { id: home.id }), ["чужая"]);
}

#[test]
fn r130_a_task_of_a_project_goes_first_or_under_a_task_of_the_project() {
    let d = device();
    let project = d.create_project("проект".into(), None).unwrap();
    let p1 = d.quick_add_under("п1".into(), project.id.clone(), false, None).unwrap();
    d.quick_add_under("п2".into(), project.id.clone(), false, None).unwrap();
    d.quick_add_under("п0".into(), project.id.clone(), true, None).unwrap();
    d.quick_add_under("п3".into(), project.id.clone(), false, Some(p1.id))
        .unwrap();
    assert_eq!(view(&d, Scope::Project { id: project.id }), ["п0", "п1", "п3", "п2"]);
}

#[test]
fn r130_quick_add_places_the_task_too() {
    let d = device();
    let a = add(&d, "а");
    add(&d, "б");
    d.quick_add("в !!".into(), None, true, None).unwrap();
    d.quick_add("г".into(), None, false, Some(a.id)).unwrap();
    assert_eq!(view(&d, Scope::Inbox), ["в", "а", "г", "б"]);
}

#[test]
fn r130_another_sort_order_places_the_new_task_by_itself() {
    let d = device();
    let list = d.create_list("сроки".into()).unwrap();
    d.set_list_sort(list.id.clone(), SortMode::Due).unwrap();
    let new = |title: &str, due: &str, first: bool| {
        d.create_task(NewTask {
            title: title.into(),
            list_id: Some(list.id.clone()),
            due: Some(due.into()),
            first,
            ..NewTask::default()
        })
        .unwrap()
    };
    new("позже", "2026-10-20", false);
    new("раньше", "2026-10-10", false);
    new("посередине", "2026-10-15", true);
    assert_eq!(view(&d, Scope::List { id: list.id }), ["раньше", "посередине", "позже"]);
}

#[test]
fn r130_a_subtask_goes_last_whatever_it_asks_for() {
    let d = device();
    let a = add(&d, "а");
    add_sub(&d, &a, "w");
    d.quick_add_under("x".into(), a.id.clone(), true, None).unwrap();
    d.create_task(NewTask {
        title: "y".into(),
        parent_id: Some(a.id.clone()),
        first: true,
        ..NewTask::default()
    })
    .unwrap();
    assert_eq!(titles(&d.subtasks(a.id).unwrap()), ["w", "x", "y"]);
}

#[test]
fn r146_due_notifications_are_those_of_the_window() {
    let d = device(); // now is 2026-10-05T10:00
    let mk = |title: &str, due: Option<&str>, remind: Option<&str>| {
        let t = d
            .create_task(NewTask {
                title: title.into(),
                due: due.map(str::to_string),
                ..NewTask::default()
            })
            .unwrap();
        if let Some(r) = remind {
            d.set_remind(t.id, Some(r.into())).unwrap();
        }
    };
    mk("а", None, Some("2026-10-05T10:01"));
    mk("б", Some("2026-10-05T10:02"), None);
    mk("в", Some("2026-10-06"), None);
    mk("г", Some("2026-10-05T11:00"), None);
    let settings = NotifySettings {
        enabled: true,
        lead_minutes: vec![0, 15],
        all_day_at: Some("09:00".into()),
        summary_at: Some("08:00".into()),
    };
    let due = |now: &str, since: &str, s: NotifySettings| {
        d.set_now_for_tests(now);
        d.due_notifications(s, since, now)
            .unwrap()
            .into_iter()
            .map(|n| (n.key.split(':').next().unwrap().to_string(), n.title, n.at, n.count))
            .collect::<Vec<_>>()
    };
    let one = |title: &str, at: &str| vec![("task".to_string(), title.to_string(), at.to_string(), 0)];
    assert_eq!(
        due("2026-10-05T10:01:30", "2026-10-05T10:00:00", settings.clone()),
        one("а", "2026-10-05T10:01")
    );
    assert!(
        due("2026-10-05T10:01:30", "2026-10-05T10:01:30", settings.clone()).is_empty(),
        "a moment before the window is not sent again"
    );
    assert_eq!(
        due("2026-10-05T10:02:00", "2026-10-05T10:01:30", settings.clone()),
        one("б", "2026-10-05T10:02"),
        "the end of the window is in it; the lead of 15 minutes was before the window"
    );
    assert_eq!(
        due("2026-10-05T10:45:10", "2026-10-05T10:44:40", settings.clone()),
        one("г", "2026-10-05T10:45")
    );
    assert_eq!(
        due("2026-10-06T08:00:20", "2026-10-06T07:59:50", settings.clone()),
        vec![("summary".to_string(), String::new(), "2026-10-06T08:00".to_string(), 3)]
    );
    assert_eq!(
        due("2026-10-06T09:00:05", "2026-10-06T08:59:35", settings.clone()),
        one("в", "2026-10-06T09:00")
    );
    assert_eq!(
        due(
            "2026-10-06T00:00:10",
            "2026-10-05T23:59:40",
            NotifySettings {
                summary_at: Some("00:00".into()),
                ..settings.clone()
            }
        ),
        vec![("summary".to_string(), String::new(), "2026-10-06T00:00".to_string(), 3)],
        "a window across midnight takes the summary of the new day"
    );
    assert!(due(
        "2026-10-05T10:01:30",
        "2026-10-05T10:00:00",
        NotifySettings {
            enabled: false,
            ..settings.clone()
        }
    )
    .is_empty());
    assert!(due("2026-10-05T10:01:30", "yesterday", settings.clone()).is_empty());
    // The end of the window is the caller's, not the clock's.
    d.set_now_for_tests("2026-10-05T10:05");
    assert!(d
        .due_notifications(settings, "2026-10-05T10:00:00", "2026-10-05T10:00:59")
        .unwrap()
        .is_empty());
}
