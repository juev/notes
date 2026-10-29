//! Sync through CalDAV: docs/specs/caldav.md. The server is `common/dav.rs`;
//! "another client" is played by editing the objects in its folder directly.

mod common;
#[path = "common/dav.rs"]
mod dav;

use std::path::PathBuf;

use common::*;
use notes_core::*;
use tiny_http::Server;

struct Dav {
    url: String,
    root: tempfile::TempDir,
}

impl Dav {
    fn home(&self) -> PathBuf {
        self.root.path().join("cal")
    }

    /// Path of the object of a task that lives in the inbox calendar.
    fn object(&self, task_id: &str) -> PathBuf {
        self.home().join("lists-inbox").join(format!("{task_id}.ics"))
    }

    fn read(&self, task_id: &str) -> String {
        std::fs::read_to_string(self.object(task_id)).unwrap()
    }

    /// Rewrites an object the way another client would: unfolded lines in, same text out.
    fn edit(&self, task_id: &str, change: impl Fn(Vec<String>) -> Vec<String>) {
        let unfolded = self.read(task_id).replace("\r\n ", "");
        let lines = change(
            unfolded
                .split("\r\n")
                .filter(|l| !l.is_empty())
                .map(str::to_string)
                .collect(),
        );
        std::fs::write(self.object(task_id), lines.join("\r\n") + "\r\n").unwrap();
    }

    fn objects(&self) -> usize {
        std::fs::read_dir(self.home().join("lists-inbox")).map_or(0, |d| {
            d.flatten()
                .filter(|e| e.file_name().to_string_lossy().ends_with(".ics"))
                .count()
        })
    }

    /// Requests the server received while `run` was going.
    fn requests(&self, run: impl FnOnce()) -> Vec<String> {
        let log = self.root.path().join(".requests");
        std::fs::write(&log, "").unwrap();
        run();
        let seen = std::fs::read_to_string(&log).unwrap();
        std::fs::remove_file(&log).unwrap();
        seen.lines().map(str::to_string).collect()
    }
}

fn start() -> Dav {
    let server = Server::http("127.0.0.1:0").unwrap();
    let port = server.server_addr().to_ip().unwrap().port();
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().to_path_buf();
    std::fs::create_dir(dir.join("cal")).unwrap();
    std::thread::spawn(move || dav::serve(&server, &dir));
    Dav {
        url: format!("http://127.0.0.1:{port}/cal"),
        root,
    }
}

fn connect(d: &Device, dav: &Dav) {
    d.set_sync_config(SyncConfig::CalDav {
        url: dav.url.clone(),
        user: "user".into(),
    })
    .unwrap();
    d.set_sync_password(Some("secret".into()));
}

fn pair(dav: &Dav) -> (Device, Device) {
    let (a, b) = (device(), device());
    connect(&a, dav);
    connect(&b, dav);
    (a, b)
}

fn settle(a: &Device, b: &Device) {
    a.sync_now().unwrap();
    b.sync_now().unwrap();
    a.sync_now().unwrap();
}

fn daily() -> Repeat {
    Repeat {
        freq: Freq::Daily,
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
fn c2_c4_tasks_become_objects_other_clients_can_read() {
    let dav = start();
    let (a, b) = pair(&dav);
    let top = a
        .create_task(NewTask {
            title: "Купить молоко, хлеб".into(),
            due: Some("2026-10-06T18:30".into()),
            priority: Some(Priority::High),
            tags: vec!["дом".into()],
            ..NewTask::default()
        })
        .unwrap();
    a.set_repeat(top.id.clone(), Some(daily())).unwrap();
    a.set_remind(top.id.clone(), Some("2026-10-06T18:00".into())).unwrap();
    let sub = add_sub(&a, &top, "зайти в магазин");

    let up = a.sync_now().unwrap();
    assert!(up.pushed >= 2);
    let text = dav.read(&top.id).replace("\r\n ", "");
    for expected in [
        "BEGIN:VTODO",
        "SUMMARY:Купить молоко\\, хлеб",
        "DUE:20261006T183000",
        "PRIORITY:1",
        "CATEGORIES:дом",
        "RRULE:FREQ=DAILY",
        "X-LISTS-STATE:",
    ] {
        assert!(text.contains(expected), "{expected} in\n{text}");
    }
    assert!(dav
        .read(&sub.id)
        .contains(&format!("RELATED-TO;RELTYPE=PARENT:{}", top.id)));

    assert!(b.sync_now().unwrap().pulled > 0);
    let got = b.task(top.id.clone()).unwrap();
    assert_eq!(
        (got.title.as_str(), got.due.as_deref(), got.priority),
        ("Купить молоко, хлеб", Some("2026-10-06T18:30"), Priority::High)
    );
    assert_eq!(got.tags, ["дом"]);
    assert!(got.repeat.is_some());
    assert_eq!(
        got.remind.as_deref(),
        Some("2026-10-06T18:00"),
        "a field with no standard property arrives through the state"
    );
    assert_eq!(titles(&b.subtasks(top.id).unwrap()), ["зайти в магазин"]);
}

#[test]
fn c16_a_second_run_writes_nothing() {
    let dav = start();
    let (a, b) = pair(&dav);
    add(&a, "задача");
    a.create_list("Работа".into()).unwrap();
    settle(&a, &b);
    b.sync_now().unwrap();
    assert_eq!(a.sync_now().unwrap(), SyncReport::default());
    assert_eq!(b.sync_now().unwrap(), SyncReport::default());
}

#[test]
fn s2_edits_of_different_fields_on_two_devices_are_both_kept() {
    let dav = start();
    let (a, b) = pair(&dav);
    let t = add(&a, "исходное");
    settle(&a, &b);

    a.set_title(t.id.clone(), "новое название".into()).unwrap();
    b.set_due(t.id.clone(), Some("2026-10-09".into())).unwrap();
    // C13: B writes first; A's write is refused, merged and repeated.
    b.sync_now().unwrap();
    a.sync_now().unwrap();
    b.sync_now().unwrap();
    for d in [&a, &b] {
        let got = d.task(t.id.clone()).unwrap();
        assert_eq!(
            (got.title.as_str(), got.due.as_deref()),
            ("новое название", Some("2026-10-09"))
        );
    }
    assert_eq!(a.sync_now().unwrap(), SyncReport::default());
}

#[test]
fn c13_write_refused_by_the_server_is_merged_and_repeated_in_the_same_run() {
    let dav = start();
    let (a, _b) = pair(&dav);
    let t = add(&a, "задача");
    a.sync_now().unwrap();

    // Another client rewrites the object after A has read the calendar and
    // before A's own PUT arrives: the server must answer 412.
    let theirs = dav.read(&t.id).replace("SUMMARY:задача", "SUMMARY:Changed in between");
    std::fs::write(
        dav.root.path().join(".race"),
        format!("cal/lists-inbox/{}.ics\n{theirs}", t.id),
    )
    .unwrap();
    a.set_priority(t.id.clone(), Priority::Medium).unwrap();
    a.sync_now().unwrap();

    assert!(!dav.root.path().join(".race").exists(), "the concurrent write happened");
    let on_server = dav.read(&t.id).replace("\r\n ", "");
    assert!(
        on_server.contains("SUMMARY:Changed in between") && on_server.contains("PRIORITY:5"),
        "{on_server}"
    );
    let got = a.task(t.id).unwrap();
    assert_eq!(
        (got.title.as_str(), got.priority),
        ("Changed in between", Priority::Medium)
    );
    assert_eq!(a.sync_now().unwrap(), SyncReport::default());
}

#[test]
fn c7_c8_another_client_edits_standard_properties_and_adds_its_own() {
    let dav = start();
    let (a, b) = pair(&dav);
    let t = add(&a, "старое название");
    a.set_remind(t.id.clone(), Some("2026-10-06T09:00".into())).unwrap();
    a.add_tag(t.id.clone(), "дом".into()).unwrap();
    settle(&a, &b);

    dav.edit(&t.id, |lines| {
        let mut out: Vec<String> = lines
            .into_iter()
            .map(|l| {
                if l.starts_with("SUMMARY:") {
                    "SUMMARY:Renamed elsewhere".to_string()
                } else {
                    l
                }
            })
            .filter(|l| !l.starts_with("CATEGORIES"))
            .collect();
        let end = out.iter().position(|l| l == "END:VTODO").unwrap();
        out.insert(end, "X-OTHER-CLIENT:keep me".into());
        out.insert(end, "DUE;VALUE=DATE:20261012".into());
        out.insert(end, "PRIORITY:1".into());
        out.insert(end, "CATEGORIES:Office".into());
        out
    });

    assert!(a.sync_now().unwrap().pulled > 0);
    let got = a.task(t.id.clone()).unwrap();
    assert_eq!(got.title, "Renamed elsewhere");
    assert_eq!(got.due.as_deref(), Some("2026-10-12"));
    assert_eq!(got.priority, Priority::High);
    assert_eq!(got.tags, ["office"]);
    assert_eq!(got.remind.as_deref(), Some("2026-10-06T09:00"), "untouched fields stay");

    // A's write-back keeps the other client's property, and B converges.
    a.set_notes(t.id.clone(), "после чужой правки".into()).unwrap();
    a.sync_now().unwrap();
    assert!(dav.read(&t.id).contains("X-OTHER-CLIENT:keep me"));
    b.sync_now().unwrap();
    assert_eq!(b.task(t.id.clone()).unwrap().title, "Renamed elsewhere");
    assert_eq!(b.task(t.id).unwrap().tags, ["office"]);
    a.sync_now().unwrap();
    assert_eq!(a.sync_now().unwrap(), SyncReport::default());
    assert_eq!(b.sync_now().unwrap(), SyncReport::default());
}

#[test]
fn c29_what_another_client_gives_a_subtask_is_removed() {
    let dav = start();
    let (a, b) = pair(&dav);
    let top = add(&a, "задача");
    let first = add_sub(&a, &top, "первая");
    let second = add_sub(&a, &top, "вторая");
    settle(&a, &b);
    assert!(!dav.read(&first.id).contains("\nDUE"));

    // Another client gives the first subtask a due date and a priority and hangs the second one under it.
    dav.edit(&first.id, |mut lines| {
        let end = lines.iter().position(|l| l == "END:VTODO").unwrap();
        lines.insert(end, "PRIORITY:1".to_string());
        lines.insert(end, "DUE;VALUE=DATE:20261009".to_string());
        lines
    });
    let under = first.id.clone();
    dav.edit(&second.id, move |lines| {
        lines
            .into_iter()
            .map(|l| {
                if l.starts_with("RELATED-TO") {
                    format!("RELATED-TO;RELTYPE=PARENT:{under}")
                } else {
                    l
                }
            })
            .collect()
    });
    settle(&a, &b);

    for d in [&a, &b] {
        let got = d.task(first.id.clone()).unwrap();
        assert_eq!((got.due, got.priority), (None, Priority::None));
        assert_eq!(titles(&d.subtasks(top.id.clone()).unwrap()), ["первая", "вторая"]);
    }
    let object = dav.read(&first.id);
    assert!(!object.contains("\nDUE") && !object.contains("\nPRIORITY"), "{object}");
    assert!(dav
        .read(&second.id)
        .contains(&format!("RELATED-TO;RELTYPE=PARENT:{}", top.id)));
}

#[test]
fn c9_state_removed_by_another_client_is_restored_without_losing_fields() {
    let dav = start();
    let (a, b) = pair(&dav);
    let t = add(&a, "задача");
    a.set_remind(t.id.clone(), Some("2026-10-06T09:00".into())).unwrap();
    settle(&a, &b);

    dav.edit(&t.id, |lines| {
        lines
            .into_iter()
            .filter(|l| !l.starts_with("X-LISTS-STATE"))
            .map(|l| {
                if l.starts_with("SUMMARY:") {
                    "SUMMARY:Stripped and renamed".to_string()
                } else {
                    l
                }
            })
            .collect()
    });
    a.sync_now().unwrap();
    let got = a.task(t.id.clone()).unwrap();
    assert_eq!(got.title, "Stripped and renamed");
    assert_eq!(got.remind.as_deref(), Some("2026-10-06T09:00"));
    assert!(
        dav.read(&t.id).contains("X-LISTS-STATE:"),
        "the state is back on the server"
    );

    b.sync_now().unwrap();
    let got = b.task(t.id).unwrap();
    assert_eq!(
        (got.title.as_str(), got.remind.as_deref()),
        ("Stripped and renamed", Some("2026-10-06T09:00"))
    );
}

#[test]
fn c10_task_created_by_another_client_gets_the_same_identity_everywhere() {
    let dav = start();
    let (a, b) = pair(&dav);
    settle(&a, &b);
    std::fs::write(
        dav.home().join("lists-inbox").join("foreign.ics"),
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Other//EN\r\nBEGIN:VTODO\r\nUID:foreign-uid-1\r\nSUMMARY:From another app\r\nDUE;VALUE=DATE:20261007\r\nPRIORITY:5\r\nEND:VTODO\r\nEND:VCALENDAR\r\n",
    )
    .unwrap();
    settle(&a, &b);
    for d in [&a, &b] {
        let got = d.task("foreign-uid-1".into()).unwrap();
        assert_eq!(
            (got.title.as_str(), got.due.as_deref(), got.priority),
            ("From another app", Some("2026-10-07"), Priority::Medium)
        );
        assert_eq!(got.list_id, INBOX_ID);
        assert_eq!(view(d, Scope::Inbox), ["From another app"]);
    }
    // The object stays under the name the other client gave it: no duplicate appears.
    assert_eq!(dav.objects(), 1);
    b.sync_now().unwrap();
    assert_eq!(a.sync_now().unwrap(), SyncReport::default());
}

#[test]
fn c11_repeating_task_completed_elsewhere_moves_to_the_next_occurrence() {
    let dav = start();
    let (a, b) = pair(&dav);
    let t = add(&a, "зарядка");
    a.set_due(t.id.clone(), Some("2026-10-05".into())).unwrap();
    a.set_repeat(t.id.clone(), Some(daily())).unwrap();
    settle(&a, &b);

    dav.edit(&t.id, |lines| {
        lines
            .into_iter()
            .flat_map(|l| {
                if l.starts_with("STATUS:") {
                    vec!["STATUS:COMPLETED".to_string(), "COMPLETED:20261005T080000Z".to_string()]
                } else {
                    vec![l]
                }
            })
            .collect()
    });
    a.sync_now().unwrap();
    let got = a.task(t.id.clone()).unwrap();
    assert!(got.done.is_none(), "still open");
    assert_eq!(got.due.as_deref(), Some("2026-10-06"));
    assert_eq!(
        view(&a, Scope::Completed),
        ["зарядка"],
        "the finished occurrence is recorded"
    );
    assert!(dav.read(&t.id).contains("STATUS:NEEDS-ACTION"));

    b.sync_now().unwrap();
    assert_eq!(b.task(t.id).unwrap().due.as_deref(), Some("2026-10-06"));
}

#[test]
fn plain_task_completed_and_reopened_elsewhere() {
    let dav = start();
    let (a, _b) = pair(&dav);
    let t = add(&a, "разовая");
    a.sync_now().unwrap();
    dav.edit(&t.id, |lines| {
        lines
            .into_iter()
            .map(|l| {
                if l.starts_with("STATUS:") {
                    "STATUS:COMPLETED".to_string()
                } else {
                    l
                }
            })
            .collect()
    });
    a.sync_now().unwrap();
    assert!(a.task(t.id.clone()).unwrap().done.is_some());

    dav.edit(&t.id, |lines| {
        lines
            .into_iter()
            .filter(|l| !l.starts_with("COMPLETED:") && !l.starts_with("PERCENT-COMPLETE"))
            .map(|l| {
                if l.starts_with("STATUS:") {
                    "STATUS:NEEDS-ACTION".to_string()
                } else {
                    l
                }
            })
            .collect()
    });
    a.sync_now().unwrap();
    assert!(a.task(t.id).unwrap().done.is_none());
}

#[test]
fn c6_c14_trash_removes_the_object_and_a_vanished_object_goes_to_the_trash() {
    let dav = start();
    let (a, b) = pair(&dav);
    let t = add(&a, "удаляемая");
    let kept = add(&a, "остаётся");
    settle(&a, &b);
    assert_eq!(dav.objects(), 2);

    b.set_notes(t.id.clone(), "правка до удаления".into()).unwrap();
    a.delete_task(t.id.clone()).unwrap();
    a.sync_now().unwrap();
    assert_eq!(dav.objects(), 1, "the trashed task has no object");

    b.sync_now().unwrap();
    assert_eq!(view(&b, Scope::Inbox), ["остаётся"]);
    assert_eq!(view(&b, Scope::Trash), ["удаляемая"]);
    assert_eq!(dav.objects(), 1, "B does not put the object back");

    // Restoring brings the object back, with the edit B made before it vanished.
    b.restore_task(t.id.clone()).unwrap();
    b.sync_now().unwrap();
    a.sync_now().unwrap();
    assert_eq!(a.task(t.id.clone()).unwrap().notes, "правка до удаления");
    assert!(!a.task(t.id).unwrap().deleted);

    // Another client deletes an object outright.
    std::fs::remove_file(dav.object(&kept.id)).unwrap();
    settle(&a, &b);
    for d in [&a, &b] {
        assert!(d.task(kept.id.clone()).unwrap().deleted);
    }
}

#[test]
fn c6_cleared_completed_tasks_leave_the_server() {
    let dav = start();
    let (a, b) = pair(&dav);
    let done = add(&a, "сделанная");
    add_sub(&a, &done, "её часть");
    a.complete_task(done.id.clone()).unwrap();
    let rep = add(&a, "зарядка");
    a.set_due(rep.id.clone(), Some("2026-10-05".into())).unwrap();
    a.set_repeat(rep.id.clone(), Some(daily())).unwrap();
    a.complete_task(rep.id.clone()).unwrap();
    settle(&a, &b);
    assert_eq!(dav.objects(), 4, "two tasks, a subtask and the record of the repeat");

    assert_eq!(a.clear_completed(None).unwrap(), 2);
    a.sync_now().unwrap();
    assert_eq!(dav.objects(), 1, "only the open repeating task is left");

    // The server carries no word of a purge: for B the objects vanished (C14).
    b.sync_now().unwrap();
    assert!(view(&b, Scope::Completed).is_empty());
    assert_eq!(view(&b, Scope::Inbox), ["зарядка"]);
    assert_eq!(view(&b, Scope::Trash), ["её часть", "зарядка", "сделанная"]);
    a.sync_now().unwrap();
    assert_eq!(dav.objects(), 1, "nobody puts the objects back");
    assert!(view(&a, Scope::Trash).is_empty());
}

#[test]
fn c1_c15_lists_are_calendars_with_their_settings() {
    let dav = start();
    let (a, b) = pair(&dav);
    let list = a.create_list("Работа".into()).unwrap();
    a.set_list_color(list.id.clone(), "#FF9500".into()).unwrap();
    a.set_list_sort(list.id.clone(), SortMode::Due).unwrap();
    let t = a
        .create_task(NewTask {
            title: "отчёт".into(),
            list_id: Some(list.id.clone()),
            ..NewTask::default()
        })
        .unwrap();
    settle(&a, &b);

    let props = std::fs::read_to_string(dav.home().join(&list.id).join(".props")).unwrap();
    assert!(
        props.contains("name=Работа") && props.contains("color=#FF9500"),
        "{props}"
    );
    let got = b.list(list.id.clone()).unwrap();
    assert_eq!(
        (got.name.as_str(), got.color.as_str(), got.sort),
        ("Работа", "#FF9500", SortMode::Due)
    );
    assert_eq!(view(&b, Scope::List { id: list.id.clone() }), ["отчёт"]);

    // Another client renames the calendar.
    std::fs::write(
        dav.home().join(&list.id).join(".props"),
        props.replace("name=Работа", "name=Office"),
    )
    .unwrap();
    settle(&b, &a);
    assert_eq!(a.list(list.id.clone()).unwrap().name, "Office");
    assert_eq!(b.list(list.id.clone()).unwrap().name, "Office");

    // Moving a task to another list moves its object to that calendar.
    a.move_to_list(t.id.clone(), INBOX_ID.into()).unwrap();
    settle(&a, &b);
    assert!(dav.object(&t.id).exists() && !dav.home().join(&list.id).join(format!("{}.ics", t.id)).exists());
    assert_eq!(view(&b, Scope::Inbox), ["отчёт"]);

    // Deleting the list removes the calendar once it is empty, and the other device follows.
    a.delete_list(list.id.clone()).unwrap();
    a.sync_now().unwrap();
    assert!(!dav.home().join(&list.id).exists());
    b.sync_now().unwrap();
    assert!(b.list(list.id).is_err());
    assert_eq!(
        view(&b, Scope::Inbox),
        ["отчёт"],
        "tasks that had left the list are untouched"
    );
}

#[test]
fn c1_server_that_drops_custom_properties_still_syncs_names_and_tasks() {
    let dav = start();
    std::fs::write(dav.root.path().join(".no-custom-props"), "").unwrap();
    let (a, b) = pair(&dav);
    let list = a.create_list("Дом".into()).unwrap();
    a.set_list_sort(list.id.clone(), SortMode::Title).unwrap();
    a.create_task(NewTask {
        title: "полить цветы".into(),
        list_id: Some(list.id.clone()),
        ..NewTask::default()
    })
    .unwrap();
    settle(&a, &b);
    let got = b.list(list.id.clone()).unwrap();
    assert_eq!(got.name, "Дом");
    assert_eq!(
        got.sort,
        SortMode::Manual,
        "settings beyond name and colour stay on the device that set them"
    );
    assert_eq!(view(&b, Scope::List { id: list.id }), ["полить цветы"]);
    // The refused property is not sent again and again.
    b.sync_now().unwrap();
    assert_eq!(a.sync_now().unwrap(), SyncReport::default());
}

#[test]
fn calendar_made_by_another_client_becomes_the_same_list_on_every_device() {
    let dav = start();
    let (a, b) = pair(&dav);
    let dir = dav.home().join("personal");
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(dir.join(".props"), "calendar=1\nname=Personal\ncolor=#00FF00FF\n").unwrap();
    std::fs::write(
        dir.join("x.ics"),
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VTODO\r\nUID:x-1\r\nSUMMARY:Existing task\r\nEND:VTODO\r\nEND:VCALENDAR\r\n",
    )
    .unwrap();
    settle(&a, &b);
    let find = |d: &Device| {
        d.lists()
            .unwrap()
            .into_iter()
            .find(|l| l.name == "Personal")
            .expect("list")
    };
    let (la, lb) = (find(&a), find(&b));
    assert_eq!(la.id, lb.id);
    assert_eq!(la.color, "#00FF00");
    assert_eq!(view(&a, Scope::List { id: la.id.clone() }), ["Existing task"]);
    assert_eq!(view(&b, Scope::List { id: lb.id }), ["Existing task"]);
}

#[test]
fn c5_attachment_reaches_the_other_device() {
    let dav = start();
    let (a, b) = pair(&dav);
    let t = add(&a, "с файлом");
    let src = tempfile::tempdir().unwrap();
    let file = src.path().join("note.txt");
    std::fs::write(&file, "вложение").unwrap();
    let att = a
        .add_attachment(t.id.clone(), file.to_string_lossy().into_owned(), None)
        .unwrap();

    assert_eq!(a.sync_now().unwrap().blobs_uploaded, 1);
    assert_eq!(b.sync_now().unwrap().blobs_downloaded, 1);
    let got = b.attachments(t.id.clone()).unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].sha256, att.sha256);
    assert_eq!(
        std::fs::read_to_string(got[0].local_path.as_ref().unwrap()).unwrap(),
        "вложение"
    );

    // Removing it on B takes the content out of the object.
    b.remove_attachment(got[0].id.clone()).unwrap();
    b.sync_now().unwrap();
    assert!(!dav.read(&t.id).contains("ATTACH"));
    a.sync_now().unwrap();
    assert!(a.attachments(t.id).unwrap().is_empty());
}

#[test]
fn c30_content_deleted_for_good_leaves_the_device() {
    let dav = start();
    let (a, b) = pair(&dav);
    let (trashed, purged) = (add(&a, "в корзину"), add(&a, "навсегда"));
    let src = tempfile::tempdir().unwrap();
    let mut hashes = vec![];
    for (task, text) in [(&trashed, "в корзине"), (&purged, "очищено")] {
        let file = src.path().join("f.txt");
        std::fs::write(&file, text).unwrap();
        let att = a
            .add_attachment(task.id.clone(), file.to_string_lossy().into_owned(), None)
            .unwrap();
        hashes.push(att.sha256);
    }
    settle(&a, &b);
    let on = |d: &Device, h: &str| d.dir().join("blobs").join(h).exists();
    assert!(hashes.iter().all(|h| on(&b, h)));

    a.delete_task(purged.id.clone()).unwrap();
    a.empty_trash().unwrap();
    a.delete_task(trashed.id.clone()).unwrap();
    settle(&a, &b);
    assert!(on(&a, &hashes[0]), "a task in the trash keeps its content");
    assert!(!on(&a, &hashes[1]), "content of a task deleted for good is removed");
    // The trash is not shared over CalDAV: on B both tasks are in it, and both keep their content.
    assert_eq!(view(&b, Scope::Trash).len(), 2);
    assert!(hashes.iter().all(|h| on(&b, h)));
    b.empty_trash().unwrap();
    b.sync_now().unwrap();
    assert!(hashes.iter().all(|h| !on(&b, h)));
}

/// A task with `count` attachments of `size` bytes each, synced from A.
fn with_attachments(a: &Device, count: u8, size: usize) -> TaskItem {
    let t = add(a, "с файлами");
    let src = tempfile::tempdir().unwrap();
    for i in 0..count {
        let file = src.path().join(format!("f{i}.bin"));
        std::fs::write(&file, vec![b'a' + i; size]).unwrap();
        a.add_attachment(t.id.clone(), file.to_string_lossy().into_owned(), None)
            .unwrap();
    }
    t
}

#[test]
fn c5_attachments_that_fill_the_object_all_reach_the_other_device() {
    let dav = start();
    let (a, b) = pair(&dav);
    let t = with_attachments(&a, 4, 5 * 1024 * 1024);

    assert_eq!(a.sync_now().unwrap().blobs_uploaded, 4);
    assert_eq!(b.sync_now().unwrap().blobs_downloaded, 4);
    let got = b.attachments(t.id).unwrap();
    assert_eq!(got.iter().filter(|a| a.local_path.is_some()).count(), 4);
}

#[test]
fn c5_attachments_beyond_the_object_stay_on_the_device() {
    let dav = start();
    let (a, b) = pair(&dav);
    let t = with_attachments(&a, 7, 4_900_000);

    assert_eq!(a.sync_now().unwrap().blobs_uploaded, 4);
    assert!(dav.read(&t.id).len() < 32 * 1024 * 1024);
    assert_eq!(b.sync_now().unwrap().blobs_downloaded, 4);
    assert_eq!(view(&b, Scope::Inbox), ["с файлами"], "the task itself arrives");
    let got = b.attachments(t.id.clone()).unwrap();
    assert_eq!(got.len(), 7, "every record arrives");
    assert_eq!(got.iter().filter(|a| a.local_path.is_some()).count(), 4);
    assert!(a
        .attachments(t.id.clone())
        .unwrap()
        .iter()
        .all(|a| a.local_path.is_some()));

    // Both devices agree on what the object holds: neither rewrites it.
    let seen = dav.requests(|| {
        a.sync_now().unwrap();
        b.sync_now().unwrap();
    });
    assert!(!seen.iter().any(|r| r.starts_with("PUT ")), "{seen:?}");

    // An edit on the device without the rest keeps what the object has.
    b.set_title(t.id.clone(), "переименована".into()).unwrap();
    b.sync_now().unwrap();
    assert_eq!(dav.read(&t.id).matches("ATTACH;").count(), 4);
    a.sync_now().unwrap();
    assert_eq!(a.task(t.id.clone()).unwrap().title, "переименована");
    assert_eq!(a.attachments(t.id).unwrap().len(), 7);
}

/// An object as another client would write it, `size` bytes of notes long.
fn foreign_object(uid: &str, title: &str, size: usize) -> String {
    format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Other//Client//EN\r\nBEGIN:VTODO\r\nUID:{uid}\r\nSUMMARY:{title}\r\nDESCRIPTION:{}\r\nEND:VTODO\r\nEND:VCALENDAR\r\n",
        "x".repeat(size)
    )
}

const TOO_LARGE: usize = 33 * 1024 * 1024;

#[test]
fn c23_object_too_large_to_read_is_reported_and_not_fetched_again() {
    let dav = start();
    let (a, b) = pair(&dav);
    add(&a, "обычная");
    a.sync_now().unwrap();
    let big = dav.object("big");
    std::fs::write(&big, foreign_object("big", "большая", TOO_LARGE)).unwrap();

    let unread = |d: &Device| d.sync_status().unwrap().last_error.unwrap_or_default();
    b.sync_now().unwrap();
    assert_eq!(view(&b, Scope::Inbox), ["обычная"], "the rest of the calendar arrives");
    assert!(
        unread(&b).contains("/cal/lists-inbox/big.ics is larger than 32 MB"),
        "{}",
        unread(&b)
    );

    // The same version is not fetched again, and the status keeps saying so.
    let seen = dav.requests(|| {
        b.sync_now().unwrap();
    });
    assert!(
        !seen.iter().any(|r| r.starts_with("GET ") || r.starts_with("REPORT ")),
        "{seen:?}"
    );
    assert!(unread(&b).contains("big.ics"));

    // Local changes still go up and come down.
    add(&a, "новая");
    a.sync_now().unwrap();
    assert!(unread(&a).contains("big.ics"));
    let seen = dav.requests(|| {
        b.sync_now().unwrap();
    });
    assert!(!seen.iter().any(|r| r.contains("big.ics")), "{seen:?}");
    assert_eq!(view(&b, Scope::Inbox), ["обычная", "новая"]);

    // The other client makes the object readable: it becomes a task.
    std::fs::write(&big, foreign_object("big", "большая", 10)).unwrap();
    b.sync_now().unwrap();
    assert_eq!(view(&b, Scope::Inbox), ["обычная", "новая", "большая"]);
    assert_eq!(b.sync_status().unwrap().last_error, None);
}

#[test]
fn c23_known_object_that_grew_too_large_is_not_written_over() {
    let dav = start();
    let (a, _b) = pair(&dav);
    let t = add(&a, "задача");
    a.sync_now().unwrap();
    std::fs::write(dav.object(&t.id), foreign_object(&t.id, "чужая правка", TOO_LARGE)).unwrap();

    a.set_due(t.id.clone(), Some("2026-10-09".into())).unwrap();
    let seen = dav.requests(|| {
        a.sync_now().unwrap();
    });
    assert!(!seen.iter().any(|r| r.starts_with("PUT ")), "{seen:?}");
    assert!(a.sync_status().unwrap().last_error.is_some());
    assert!(dav.read(&t.id).len() > TOO_LARGE);
    assert_eq!(view(&a, Scope::Inbox), ["задача"], "the task stays where it was");

    // Once the object can be read, the two edits meet.
    std::fs::write(dav.object(&t.id), foreign_object(&t.id, "чужая правка", 10)).unwrap();
    a.sync_now().unwrap();
    assert_eq!(a.task(t.id.clone()).unwrap().title, "чужая правка");
    assert!(dav.read(&t.id).contains("X-LISTS-STATE"), "and the task goes up again");
    assert_eq!(a.sync_status().unwrap().last_error, None);
}

#[test]
fn wrong_password_and_bad_address_are_reported() {
    let dav = start();
    let a = device();
    connect(&a, &dav);
    a.set_sync_password(Some("wrong".into()));
    add(&a, "ждёт");
    let err = a.sync_now().unwrap_err().to_string();
    assert!(err.contains("401"), "{err}");
    assert!(a
        .set_sync_config(SyncConfig::CalDav {
            url: "example.org/dav".into(),
            user: String::new()
        })
        .is_err());
    a.set_sync_password(Some("secret".into()));
    assert!(a.sync_now().unwrap().pushed > 0);
}

// C26
#[test]
fn connection_test_finds_the_calendars_and_writes_nothing() {
    let dav = start();
    let check = |url: &str, password: &str| {
        check_sync_connection(
            SyncConfig::CalDav {
                url: url.into(),
                user: "user".into(),
            },
            password.into(),
        )
    };
    let seen = dav.requests(|| assert_eq!(check(&dav.url, "secret").unwrap(), ConnectionCheck::Ready));
    assert!(seen.iter().all(|r| r.starts_with("PROPFIND ")), "{seen:?}");
    assert_eq!(std::fs::read_dir(dav.home()).unwrap().count(), 0);

    let err = check(&dav.url, "wrong").unwrap_err().to_string();
    assert!(err.contains("401"), "{err}");
    let err = check(&format!("{}/missing", dav.url), "secret")
        .unwrap_err()
        .to_string();
    assert!(err.contains("nothing is found"), "{err}");

    // The address is the server itself; it names the collection of calendars.
    std::fs::write(dav.root.path().join(".home"), "/cal/").unwrap();
    let root = dav.url.trim_end_matches("/cal");
    assert_eq!(check(root, "secret").unwrap(), ConnectionCheck::Ready);
}

#[test]
fn switching_from_the_log_to_caldav_carries_everything_over() {
    let dav = start();
    let folder = tempfile::tempdir().unwrap();
    let (a, b) = (device(), device());
    a.set_sync_config(SyncConfig::Folder {
        path: folder.path().to_string_lossy().into_owned(),
    })
    .unwrap();
    let t = add(&a, "из журнала");
    add_sub(&a, &t, "подзадача");
    a.sync_now().unwrap();

    connect(&a, &dav);
    a.sync_now().unwrap();
    connect(&b, &dav);
    b.sync_now().unwrap();
    assert_eq!(view(&b, Scope::Inbox), ["из журнала"]);
    assert_eq!(titles(&b.subtasks(t.id).unwrap()), ["подзадача"]);
}

#[test]
fn projects_and_saved_filters_travel_through_caldav() {
    let dav = start();
    let (a, b) = pair(&dav);
    let p = add(&a, "проект");
    a.set_project(p.id.clone(), true).unwrap();
    a.quick_add_under("шаг".into(), p.id.clone(), false, None).unwrap();
    a.create_filter(
        "Неделя".into(),
        FilterSpec {
            due: DueWindow::Next { days: 7 },
            ..FilterSpec::default()
        },
    )
    .unwrap();
    settle(&a, &b);

    assert_eq!(titles(&b.projects().unwrap()), ["проект"]);
    assert_eq!(view(&b, Scope::Project { id: p.id }), ["шаг"]);
    let filters = b.filters().unwrap();
    assert_eq!((filters.len(), filters[0].name.as_str()), (1, "Неделя"));

    b.delete_filter(filters[0].id.clone()).unwrap();
    settle(&b, &a);
    assert!(a.filters().unwrap().is_empty());
    a.sync_now().unwrap();
    assert_eq!(b.sync_now().unwrap(), SyncReport::default());
}

#[test]
fn c17_tasks_without_changes_are_not_rendered_again() {
    let dav = start();
    let (a, b) = pair(&dav);
    let one = add(&a, "one");
    let two = add(&a, "two");
    settle(&a, &b);
    // How many objects one run builds.
    let renders = || {
        let before = a.caldav_renders_for_tests();
        a.sync_now().unwrap();
        a.caldav_renders_for_tests() - before
    };

    assert_eq!(renders(), 0, "nothing changed");

    a.set_title(one.id.clone(), "one, edited".into()).unwrap();
    assert_eq!(renders(), 1, "only the edited task");
    assert!(dav.read(&one.id).contains("SUMMARY:one\\, edited"));
    assert_eq!(renders(), 0);

    // Another client's edit is merged and compared once, then left alone.
    dav.edit(&two.id, |lines| {
        lines
            .into_iter()
            .map(|l| {
                if l.starts_with("SUMMARY:") {
                    "SUMMARY:two elsewhere".into()
                } else {
                    l
                }
            })
            .collect()
    });
    assert_eq!(renders(), 1);
    assert_eq!(a.task(two.id.clone()).unwrap().title, "two elsewhere");
    assert_eq!(renders(), 0);
    let seen = dav.requests(|| {
        a.sync_now().unwrap();
    });
    assert!(seen.iter().all(|r| r.starts_with("PROPFIND ")), "{seen:?}");
}

#[test]
fn c17_subtask_follows_its_parent_to_another_calendar() {
    let dav = start();
    let (a, b) = pair(&dav);
    let top = add(&a, "parent");
    let sub = add_sub(&a, &top, "child");
    let work = a.create_list("Работа".into()).unwrap();
    settle(&a, &b);
    assert!(dav.object(&sub.id).exists());

    // The child's own registers do not change when its parent moves.
    a.move_to_list(top.id.clone(), work.id.clone()).unwrap();
    a.sync_now().unwrap();
    assert!(!dav.object(&sub.id).exists());
    assert!(dav.home().join(&work.id).join(format!("{}.ics", sub.id)).exists());
    b.sync_now().unwrap();
    assert_eq!(view(&b, Scope::List { id: work.id }), ["parent"]);
}

#[test]
fn c18_the_calendar_home_is_looked_up_once() {
    let dav = start();
    std::fs::write(dav.root.path().join(".home"), "/cal/").unwrap();
    let a = device();
    a.set_sync_config(SyncConfig::CalDav {
        url: dav.url.trim_end_matches("/cal").to_string(),
        user: "user".into(),
    })
    .unwrap();
    a.set_sync_password(Some("secret".into()));
    let t = add(&a, "задача");

    let first = dav.requests(|| {
        a.sync_now().unwrap();
    });
    assert_eq!(first[0], "PROPFIND / depth=0", "the home is asked for");
    assert!(dav.object(&t.id).exists());
    let second = dav.requests(|| {
        a.sync_now().unwrap();
    });
    assert!(!second.iter().any(|r| r.ends_with("depth=0")), "{second:?}");

    // The collection moved: the remembered address fails and is found again.
    std::fs::rename(dav.home(), dav.root.path().join("moved")).unwrap();
    std::fs::write(dav.root.path().join(".home"), "/moved/").unwrap();
    let third = dav.requests(|| {
        a.sync_now().unwrap();
    });
    assert_eq!(third[..2], ["PROPFIND /cal/ depth=1", "PROPFIND / depth=0"]);
    assert_eq!(view(&a, Scope::Inbox), ["задача"]);
    a.set_title(t.id.clone(), "переименована".into()).unwrap();
    a.sync_now().unwrap();
    let moved = dav.root.path().join("moved/lists-inbox").join(format!("{}.ics", t.id));
    assert!(std::fs::read_to_string(moved)
        .unwrap()
        .contains("SUMMARY:переименована"));
}

/// A device that finds the calendar home through `.home`, so that it can move.
fn device_at_root(dav: &Dav) -> Device {
    let d = device();
    d.set_sync_config(SyncConfig::CalDav {
        url: dav.url.trim_end_matches("/cal").to_string(),
        user: "user".into(),
    })
    .unwrap();
    d.set_sync_password(Some("secret".into()));
    d
}

fn move_home(dav: &Dav) -> PathBuf {
    let moved = dav.root.path().join("moved");
    std::fs::rename(dav.home(), &moved).unwrap();
    std::fs::write(dav.root.path().join(".home"), "/moved/").unwrap();
    moved
}

#[test]
fn c24_calendar_that_moved_to_another_address_is_the_same_list() {
    let dav = start();
    std::fs::write(dav.root.path().join(".home"), "/cal/").unwrap();
    let (a, b) = (device_at_root(&dav), device_at_root(&dav));
    let work = a.create_list("Работа".into()).unwrap();
    a.set_list_sort(work.id.clone(), SortMode::Due).unwrap();
    let t = a
        .create_task(NewTask {
            title: "отчёт".into(),
            list_id: Some(work.id.clone()),
            ..NewTask::default()
        })
        .unwrap();
    settle(&a, &b);
    b.sync_now().unwrap();

    let moved = move_home(&dav);
    settle(&a, &b);
    b.sync_now().unwrap();

    for d in [&a, &b] {
        let got = d.list(work.id.clone()).expect("the list is still there");
        assert_eq!((got.name.as_str(), got.sort), ("Работа", SortMode::Due));
        assert_eq!(view(d, Scope::List { id: work.id.clone() }), ["отчёт"]);
        assert!(view(d, Scope::Inbox).is_empty());
    }
    assert!(moved.join(&work.id).join(format!("{}.ics", t.id)).exists());
    assert_eq!(a.sync_now().unwrap(), SyncReport::default(), "C16 holds after the move");
}

#[test]
fn c24_moved_calendar_of_another_client_keeps_its_tasks() {
    let dav = start();
    std::fs::write(dav.root.path().join(".home"), "/cal/").unwrap();
    let a = device_at_root(&dav);
    let dir = dav.home().join("personal");
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(dir.join(".props"), "calendar=1\nname=Personal\n").unwrap();
    std::fs::write(
        dir.join("x.ics"),
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VTODO\r\nUID:x-1\r\nSUMMARY:Existing task\r\nEND:VTODO\r\nEND:VCALENDAR\r\n",
    )
    .unwrap();
    a.sync_now().unwrap();
    a.sync_now().unwrap();

    let moved = move_home(&dav);
    a.sync_now().unwrap();
    a.sync_now().unwrap();

    let personal: Vec<TaskList> = a
        .lists()
        .unwrap()
        .into_iter()
        .filter(|l| l.name == "Personal")
        .collect();
    assert_eq!(personal.len(), 1, "{personal:?}");
    assert_eq!(
        view(
            &a,
            Scope::List {
                id: personal[0].id.clone()
            }
        ),
        ["Existing task"]
    );
    assert!(view(&a, Scope::Inbox).is_empty());
    assert!(moved.join("personal/x.ics").exists());
}

/// Two calendars with one task each, both devices in step.
fn two_calendars(dav: &Dav) -> (Device, Device, TaskItem, TaskItem, TaskList) {
    let (a, b) = pair(dav);
    let home = add(&a, "home");
    let work = a.create_list("Работа".into()).unwrap();
    let office = a
        .create_task(NewTask {
            title: "office".into(),
            list_id: Some(work.id.clone()),
            ..NewTask::default()
        })
        .unwrap();
    settle(&a, &b);
    // A run after one's own writes reads the calendars once more.
    a.sync_now().unwrap();
    b.sync_now().unwrap();
    (a, b, home, office, work)
}

fn rename_elsewhere(dav: &Dav, task_id: &str, title: &str) {
    dav.edit(task_id, |lines| {
        lines
            .into_iter()
            .map(|l| {
                if l.starts_with("SUMMARY:") {
                    format!("SUMMARY:{title}")
                } else {
                    l
                }
            })
            .collect()
    });
}

#[test]
fn c19_only_calendars_that_changed_are_read() {
    let dav = start();
    let (a, _b, home, office, work) = two_calendars(&dav);
    let idle = dav.requests(|| {
        assert_eq!(a.sync_now().unwrap(), SyncReport::default());
    });
    assert_eq!(idle, ["PROPFIND /cal/ depth=1"]);

    rename_elsewhere(&dav, &home.id, "home elsewhere");
    let seen = dav.requests(|| {
        a.sync_now().unwrap();
    });
    assert!(seen.iter().any(|r| r.contains(" /cal/lists-inbox/ ")), "{seen:?}");
    assert!(
        seen.contains(&format!("GET /cal/lists-inbox/{}.ics", home.id)),
        "{seen:?}"
    );
    assert!(!seen.iter().any(|r| r.contains(&work.id)), "{seen:?}");
    assert_eq!(a.task(home.id.clone()).unwrap().title, "home elsewhere");

    // An object that vanished from one calendar does not take the others' tasks with it.
    a.sync_now().unwrap();
    std::fs::remove_file(dav.object(&home.id)).unwrap();
    a.sync_now().unwrap();
    assert!(view(&a, Scope::Inbox).is_empty());
    assert_eq!(view(&a, Scope::Trash), ["home elsewhere"]);
    assert_eq!(view(&a, Scope::List { id: work.id }), ["office"]);
    assert!(!a.task(office.id).unwrap().deleted);
}

#[test]
fn c19_server_without_change_tags_is_listed_on_every_run() {
    let dav = start();
    std::fs::write(dav.root.path().join(".no-ctag"), "").unwrap();
    std::fs::write(dav.root.path().join(".no-sync"), "").unwrap();
    let (a, b, home, _office, work) = two_calendars(&dav);
    let mut idle = dav.requests(|| {
        assert_eq!(a.sync_now().unwrap(), SyncReport::default());
    });
    idle.sort();
    assert_eq!(
        idle,
        [
            "PROPFIND /cal/ depth=1".to_string(),
            format!("PROPFIND /cal/{}/ depth=1", work.id),
            "PROPFIND /cal/lists-inbox/ depth=1".to_string(),
        ]
    );
    rename_elsewhere(&dav, &home.id, "home elsewhere");
    a.sync_now().unwrap();
    b.sync_now().unwrap();
    assert_eq!(b.task(home.id).unwrap().title, "home elsewhere");
}

#[test]
fn c19_sync_token_stands_in_for_a_missing_change_tag() {
    let dav = start();
    std::fs::write(dav.root.path().join(".no-ctag"), "").unwrap();
    let (a, b, home, _office, _work) = two_calendars(&dav);
    let idle = dav.requests(|| {
        assert_eq!(a.sync_now().unwrap(), SyncReport::default());
    });
    assert_eq!(idle, ["PROPFIND /cal/ depth=1"]);
    rename_elsewhere(&dav, &home.id, "home elsewhere");
    a.sync_now().unwrap();
    b.sync_now().unwrap();
    assert_eq!(b.task(home.id).unwrap().title, "home elsewhere");
}

const FOREIGN_EVENT: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Other//Client//EN\r\nBEGIN:VEVENT\r\nUID:meeting\r\nSUMMARY:meeting\r\nDTSTART:20261007T100000Z\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

/// A list deleted on the device whose calendar keeps an event of another client.
fn deleted_list_with_an_event(dav: &Dav) -> (Device, TaskList, PathBuf) {
    let (a, _b, _home, _office, work) = two_calendars(dav);
    let event = dav.home().join(&work.id).join("meeting.ics");
    std::fs::write(&event, FOREIGN_EVENT).unwrap();
    a.delete_list(work.id.clone()).unwrap();
    a.sync_now().unwrap();
    // A run after one's own writes reads the calendars once more.
    a.sync_now().unwrap();
    assert!(event.exists(), "C15: a calendar that is not empty stays");
    (a, work, event)
}

#[test]
fn c15_list_deleted_on_one_device_is_deleted_on_the_other_while_the_calendar_stays() {
    let dav = start();
    let (a, b, _home, office, work) = two_calendars(&dav);
    let event = dav.home().join(&work.id).join("meeting.ics");
    std::fs::write(&event, FOREIGN_EVENT).unwrap();
    a.delete_list(work.id.clone()).unwrap();
    settle(&a, &b);
    b.sync_now().unwrap();
    assert!(event.exists(), "C15: a calendar that is not empty stays");
    assert!(a.list(work.id.clone()).is_err(), "deleted where it was deleted");
    assert!(
        b.task(office.id).unwrap().deleted,
        "the task of the list is in the trash on the other device"
    );
    assert!(
        b.list(work.id.clone()).is_err(),
        "the list is deleted on the other device"
    );
    let idle = dav.requests(|| {
        assert_eq!(b.sync_now().unwrap(), SyncReport::default());
    });
    assert_eq!(idle, ["PROPFIND /cal/ depth=1"]);

    // Whichever device runs after the event is gone removes the calendar.
    std::fs::remove_file(&event).unwrap();
    b.sync_now().unwrap();
    assert!(!dav.home().join(&work.id).exists());
    a.sync_now().unwrap();
    assert!(a.list(work.id.clone()).is_err() && b.list(work.id).is_err());
}

#[test]
fn c19_object_that_is_not_a_task_is_not_fetched_on_every_run() {
    let dav = start();
    std::fs::write(dav.root.path().join(".no-ctag"), "").unwrap();
    std::fs::write(dav.root.path().join(".no-sync"), "").unwrap();
    let (a, _b, _home, office, work) = two_calendars(&dav);
    let event = dav.home().join(&work.id).join("meeting.ics");
    std::fs::write(&event, FOREIGN_EVENT).unwrap();
    let other = dav.home().join(&work.id).join("lunch.ics");
    std::fs::write(&other, FOREIGN_EVENT.replace("meeting", "lunch")).unwrap();
    a.sync_now().unwrap();
    let idle = dav.requests(|| {
        assert_eq!(a.sync_now().unwrap(), SyncReport::default());
    });
    assert!(!idle.iter().any(|r| r.starts_with("GET ")), "{idle:?}");

    // The other client edits its event: it is read once more, and then left alone.
    std::fs::write(&event, FOREIGN_EVENT.replace("SUMMARY:meeting", "SUMMARY:moved")).unwrap();
    let seen = dav.requests(|| {
        a.sync_now().unwrap();
    });
    assert_eq!(seen.iter().filter(|r| r.starts_with("GET ")).count(), 1, "{seen:?}");
    let idle = dav.requests(|| {
        assert_eq!(a.sync_now().unwrap(), SyncReport::default());
    });
    assert!(!idle.iter().any(|r| r.starts_with("GET ")), "{idle:?}");
    assert_eq!(
        a.sync_status().unwrap().last_error,
        None,
        "an event is not a problem to report"
    );

    // An event that vanishes takes nothing with it.
    std::fs::remove_file(&other).unwrap();
    assert_eq!(a.sync_now().unwrap(), SyncReport::default());
    assert!(!a.task(office.id).unwrap().deleted);

    // A task written to the address of an event is taken as a task.
    std::fs::write(&event, foreign_object("from-elsewhere", "from elsewhere", 1)).unwrap();
    a.sync_now().unwrap();
    assert_eq!(
        view(&a, Scope::List { id: work.id.clone() }),
        ["office", "from elsewhere"]
    );
}

#[test]
fn c19_calendar_kept_for_a_deleted_list_is_not_read_again() {
    let dav = start();
    let (a, work, event) = deleted_list_with_an_event(&dav);
    let idle = dav.requests(|| {
        assert_eq!(a.sync_now().unwrap(), SyncReport::default());
    });
    assert_eq!(idle, ["PROPFIND /cal/ depth=1"]);

    // The other client removes its event: the calendar is empty and goes (C15).
    std::fs::remove_file(&event).unwrap();
    a.sync_now().unwrap();
    assert!(!dav.home().join(&work.id).exists());
    assert!(a.list(work.id).is_err());
}

#[test]
fn c19_calendar_kept_for_a_deleted_list_is_remembered_after_reconnecting() {
    let dav = start();
    let (a, work, event) = deleted_list_with_an_event(&dav);
    // Sync turned off and on again: the device remembers nothing of the server.
    a.set_sync_config(SyncConfig::Off).unwrap();
    connect(&a, &dav);
    a.sync_now().unwrap();
    a.sync_now().unwrap();
    let idle = dav.requests(|| {
        assert_eq!(a.sync_now().unwrap(), SyncReport::default());
    });
    assert_eq!(idle, ["PROPFIND /cal/ depth=1"]);
    assert!(event.exists() && a.list(work.id).is_err());
}

#[test]
fn c19_calendar_kept_for_a_deleted_list_on_a_server_without_change_tags() {
    let dav = start();
    std::fs::write(dav.root.path().join(".no-ctag"), "").unwrap();
    std::fs::write(dav.root.path().join(".no-sync"), "").unwrap();
    let (a, work, event) = deleted_list_with_an_event(&dav);
    let idle = dav.requests(|| {
        assert_eq!(a.sync_now().unwrap(), SyncReport::default());
    });
    let listings = idle
        .iter()
        .filter(|r| r.contains(&format!("PROPFIND /cal/{}/", work.id)))
        .count();
    assert_eq!(listings, 1, "{idle:?}");

    std::fs::remove_file(&event).unwrap();
    a.sync_now().unwrap();
    assert!(!dav.home().join(&work.id).exists());
}

#[test]
fn c20_changes_are_read_from_the_sync_report() {
    let dav = start();
    let (a, b, home, _office, _work) = two_calendars(&dav);
    let extra = add(&a, "extra");
    settle(&a, &b);
    a.sync_now().unwrap();

    rename_elsewhere(&dav, &home.id, "home elsewhere");
    std::fs::remove_file(dav.object(&extra.id)).unwrap();
    let seen = dav.requests(|| {
        a.sync_now().unwrap();
    });
    assert!(
        seen.contains(&"REPORT /cal/lists-inbox/ sync-collection".to_string()),
        "{seen:?}"
    );
    assert!(
        !seen.iter().any(|r| r.starts_with("PROPFIND /cal/lists-inbox/")),
        "{seen:?}"
    );
    assert_eq!(view(&a, Scope::Inbox), ["home elsewhere"]);
    assert_eq!(view(&a, Scope::Trash), ["extra"]);
}

#[test]
fn c20_token_the_server_forgot_falls_back_to_the_listing() {
    let dav = start();
    let (a, _b, home, _office, _work) = two_calendars(&dav);
    let inbox = dav.home().join("lists-inbox");
    for entry in std::fs::read_dir(&inbox).unwrap().flatten() {
        if entry.file_name().to_string_lossy().starts_with(".sync-") {
            std::fs::remove_file(entry.path()).unwrap();
        }
    }
    rename_elsewhere(&dav, &home.id, "home elsewhere");
    let seen = dav.requests(|| {
        a.sync_now().unwrap();
    });
    let at = |what: &str| seen.iter().position(|r| r == what);
    let refused = at("REPORT /cal/lists-inbox/ sync-collection").expect("the report is tried");
    let listed = at("PROPFIND /cal/lists-inbox/ depth=1").expect("then the listing");
    assert!(refused < listed, "{seen:?}");
    assert_eq!(a.task(home.id).unwrap().title, "home elsewhere");
}

/// A device that reads a calendar holding 25 tasks for the first time.
fn first_read(dav: &Dav) -> (Device, Vec<String>) {
    let a = device();
    connect(&a, dav);
    for n in 0..25 {
        add(&a, &format!("task {n}"));
    }
    a.sync_now().unwrap();
    let b = device();
    connect(&b, dav);
    let seen = dav.requests(|| {
        b.sync_now().unwrap();
    });
    (b, seen)
}

#[test]
fn c21_several_objects_are_read_in_one_request() {
    let dav = start();
    let (b, seen) = first_read(&dav);
    assert_eq!(
        seen.iter().filter(|r| r.ends_with(" calendar-multiget")).count(),
        2,
        "batches of 20: {seen:?}"
    );
    assert!(!seen.iter().any(|r| r.starts_with("GET ")), "{seen:?}");
    assert_eq!(view(&b, Scope::Inbox).len(), 25);
}

#[test]
fn c21_server_without_reports_is_read_object_by_object() {
    let dav = start();
    std::fs::write(dav.root.path().join(".no-report"), "").unwrap();
    let (b, seen) = first_read(&dav);
    assert_eq!(seen.iter().filter(|r| r.starts_with("GET ")).count(), 25, "{seen:?}");
    assert_eq!(
        seen.iter().filter(|r| r.starts_with("REPORT ")).count(),
        1,
        "asked once, not for every batch: {seen:?}"
    );
    assert_eq!(view(&b, Scope::Inbox).len(), 25);
}

fn real_server() -> (String, String, String) {
    (
        std::env::var("LISTS_CALDAV_URL").expect("LISTS_CALDAV_URL"),
        std::env::var("LISTS_CALDAV_USER").unwrap_or_default(),
        std::env::var("LISTS_CALDAV_PASSWORD").unwrap_or_default(),
    )
}

fn real_device() -> Device {
    let (url, user, password) = real_server();
    let d = device();
    d.set_sync_config(SyncConfig::CalDav { url, user }).unwrap();
    d.set_sync_password(Some(password));
    d
}

/// A request of another client to the real server, made with `curl`; the answer is the status.
/// The server's own log, split by requests for `mark-…`, shows what each run cost.
fn elsewhere(method: &str, path: &str, body: Option<&str>) -> String {
    let (url, user, password) = real_server();
    let mut curl = std::process::Command::new("curl");
    curl.args(["-s", "-o", "/dev/null", "-w", "%{http_code}", "-X", method])
        .args(["-u", &format!("{user}:{password}")])
        .args(["-H", "Content-Type: text/calendar; charset=utf-8"]);
    if let Some(body) = body {
        curl.args(["--data-binary", body]);
    }
    let out = curl
        .arg(format!("{}/{path}", url.trim_end_matches('/')))
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// C15, C19, C25 against a real server: a list with an event of another client
/// in its calendar is deleted on one device.
#[test]
#[ignore = "needs a CalDAV server"]
fn real_server_calendar_kept_for_a_deleted_list() {
    let (a, b) = (real_device(), real_device());
    let work = a.create_list("Lists test kept".into()).unwrap();
    let task = a
        .create_task(NewTask {
            title: "lists test kept".into(),
            list_id: Some(work.id.clone()),
            ..NewTask::default()
        })
        .unwrap();
    settle(&a, &b);
    let event = format!("{}/meeting.ics", work.id);
    let put = elsewhere("PUT", &event, Some(FOREIGN_EVENT));
    assert!(put.starts_with('2'), "the server took the event: {put}");
    a.sync_now().unwrap();
    a.sync_now().unwrap();
    elsewhere("GET", "mark-idle-with-event", None);
    assert_eq!(a.sync_now().unwrap(), SyncReport::default());
    elsewhere("GET", "mark-end", None);

    a.delete_list(work.id.clone()).unwrap();
    settle(&a, &b);
    b.sync_now().unwrap();
    a.sync_now().unwrap();
    assert!(a.list(work.id.clone()).is_err() && b.list(work.id.clone()).is_err());
    assert!(b.task(task.id).unwrap().deleted);
    assert_eq!(elsewhere("GET", &event, None), "200", "the calendar and the event stay");
    elsewhere("GET", "mark-idle-deleted-a", None);
    assert_eq!(a.sync_now().unwrap(), SyncReport::default());
    elsewhere("GET", "mark-idle-deleted-b", None);
    assert_eq!(b.sync_now().unwrap(), SyncReport::default());
    elsewhere("GET", "mark-end", None);

    assert!(elsewhere("DELETE", &event, None).starts_with('2'));
    a.sync_now().unwrap();
    assert_eq!(
        elsewhere("PROPFIND", &format!("{}/", work.id), None),
        "404",
        "the calendar is gone"
    );
    b.sync_now().unwrap();
    assert!(b.list(work.id).is_err());
}

/// The same exchange against a real server, which the test server only imitates:
/// `LISTS_CALDAV_URL=… LISTS_CALDAV_USER=… LISTS_CALDAV_PASSWORD=… cargo test --test caldav real_server -- --ignored`.
/// It leaves the calendars it made empty; the inbox calendar stays.
#[test]
#[ignore = "needs a CalDAV server"]
fn real_server_round_trip() {
    let (a, b) = (real_device(), real_device());
    let work = a.create_list("Lists test".into()).unwrap();
    let mut tasks: Vec<TaskItem> = (0..25).map(|n| add(&a, &format!("lists test {n}"))).collect();
    tasks.push(
        a.create_task(NewTask {
            title: "lists test in a list".into(),
            list_id: Some(work.id.clone()),
            due: Some("2026-10-09T18:30".into()),
            ..NewTask::default()
        })
        .unwrap(),
    );
    settle(&a, &b);
    let ours = |d: &Device, scope: Scope| -> Vec<String> {
        let mut titles: Vec<String> = view(d, scope)
            .into_iter()
            .filter(|t| t.starts_with("lists test"))
            .collect();
        titles.sort();
        titles
    };
    assert_eq!(ours(&b, Scope::Inbox).len(), 25, "first read");
    assert_eq!(ours(&b, Scope::List { id: work.id.clone() }), ["lists test in a list"]);
    assert_eq!(
        b.task(tasks[25].id.clone()).unwrap().due.as_deref(),
        Some("2026-10-09T18:30")
    );

    // An edit and a removal travel to the other device; an idle run changes nothing.
    b.sync_now().unwrap();
    a.set_title(tasks[0].id.clone(), "lists test renamed".into()).unwrap();
    a.delete_task(tasks[1].id.clone()).unwrap();
    a.sync_now().unwrap();
    b.sync_now().unwrap();
    assert_eq!(b.task(tasks[0].id.clone()).unwrap().title, "lists test renamed");
    assert_eq!(ours(&b, Scope::Inbox).len(), 24);
    a.sync_now().unwrap();
    b.sync_now().unwrap();
    assert_eq!(a.sync_now().unwrap(), SyncReport::default());
    assert_eq!(b.sync_now().unwrap(), SyncReport::default());

    for task in &tasks {
        if task.id != tasks[1].id {
            a.delete_task(task.id.clone()).unwrap();
        }
    }
    a.delete_list(work.id).unwrap();
    settle(&a, &b);
    assert!(ours(&b, Scope::Inbox).is_empty());
}

#[test]
fn c14_removal_seen_in_a_run_that_failed_later_is_not_forgotten() {
    // Calendars are read in no particular order; several rounds cover both.
    for _ in 0..8 {
        let dav = start();
        let (a, _b, home, office, work) = two_calendars(&dav);
        std::fs::remove_file(dav.object(&home.id)).unwrap();
        let other = dav.home().join(&work.id).join(format!("{}.ics", office.id));
        let text = std::fs::read_to_string(&other).unwrap();
        std::fs::write(&other, text.replace("SUMMARY:office", "SUMMARY:office elsewhere")).unwrap();

        // The run reads one calendar and breaks on the other.
        std::fs::write(dav.root.path().join(".fail"), format!("/cal/{}/", work.id)).unwrap();
        assert!(a.sync_now().is_err());
        std::fs::remove_file(dav.root.path().join(".fail")).unwrap();

        a.sync_now().unwrap();
        a.sync_now().unwrap();
        assert_eq!(
            view(&a, Scope::Trash),
            ["home"],
            "the removal is acted on once the server answers"
        );
        assert_eq!(a.task(office.id).unwrap().title, "office elsewhere");
    }
}

fn status(dav: &Dav, id: &str, to: &'static str, completed: Option<&'static str>) {
    dav.edit(id, move |lines| {
        lines
            .into_iter()
            .filter(|l| completed.is_none() || !l.starts_with("COMPLETED:"))
            .flat_map(|l| {
                if l.starts_with("STATUS:") {
                    let mut out = vec![format!("STATUS:{to}")];
                    out.extend(completed.filter(|c| !c.is_empty()).map(|c| format!("COMPLETED:{c}")));
                    out
                } else {
                    vec![l]
                }
            })
            .collect()
    });
}

#[test]
fn c27_wont_do_is_written_as_cancelled_and_reaches_the_other_device() {
    let dav = start();
    let (a, b) = pair(&dav);
    let t = add(&a, "брошенная");
    a.wont_do_task(t.id.clone()).unwrap();
    settle(&a, &b);

    let object = dav.read(&t.id);
    assert!(object.contains("STATUS:CANCELLED"), "{object}");
    assert!(
        object.contains("COMPLETED:"),
        "kept for versions that read no CANCELLED"
    );
    assert!(!object.contains("PERCENT-COMPLETE"));
    assert!(b.task(t.id.clone()).unwrap().wont);

    b.reopen_task(t.id.clone()).unwrap();
    settle(&b, &a);
    assert!(dav.read(&t.id).contains("STATUS:NEEDS-ACTION"));
    let got = a.task(t.id).unwrap();
    assert!(got.done.is_none() && !got.wont);
}

#[test]
fn c27_cancelled_by_another_client_closes_the_task_as_wont_do() {
    let dav = start();
    let (a, b) = pair(&dav);
    let plain = add(&a, "разовая");
    let rep = add(&a, "зарядка");
    a.set_due(rep.id.clone(), Some("2026-10-05".into())).unwrap();
    a.set_repeat(rep.id.clone(), Some(daily())).unwrap();
    settle(&a, &b);

    status(&dav, &plain.id, "CANCELLED", None);
    status(&dav, &rep.id, "CANCELLED", None);
    a.sync_now().unwrap();
    assert!(a.task(plain.id.clone()).unwrap().wont);
    let got = a.task(rep.id.clone()).unwrap();
    assert!(got.done.is_none(), "the repeating task moves on (C11)");
    assert_eq!(got.due.as_deref(), Some("2026-10-06"));
    let log = a.tasks(Scope::WontDo).unwrap();
    assert_eq!(
        log.iter().filter(|t| t.is_log).count(),
        1,
        "the skipped occurrence is recorded"
    );
    b.sync_now().unwrap();
    assert!(b.task(plain.id.clone()).unwrap().wont);

    // COMPLETED left as it was: written by a version that does not know the outcome.
    status(&dav, &plain.id, "COMPLETED", None);
    a.sync_now().unwrap();
    assert!(a.task(plain.id.clone()).unwrap().wont, "not an edit");

    // A new completion time: another client completed the task.
    status(&dav, &plain.id, "COMPLETED", Some("20261006T080000Z"));
    a.sync_now().unwrap();
    let got = a.task(plain.id.clone()).unwrap();
    assert!(got.done.is_some() && !got.wont);

    status(&dav, &plain.id, "CANCELLED", None);
    a.sync_now().unwrap();
    assert!(a.task(plain.id.clone()).unwrap().wont, "and cancelled it again");

    status(&dav, &plain.id, "NEEDS-ACTION", Some(""));
    a.sync_now().unwrap();
    let got = a.task(plain.id).unwrap();
    assert!(got.done.is_none() && !got.wont);
}

const FOREIGN_TASK: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Other//EN\r\nBEGIN:VTODO\r\nUID:foreign-uid-1\r\nSUMMARY:From another app\r\nEND:VTODO\r\nEND:VCALENDAR\r\n";

fn caldav(dav: &Dav) -> SyncConfig {
    SyncConfig::CalDav {
        url: dav.url.clone(),
        user: "user".into(),
    }
}

/// A server with a task of device A, a task and an event of another client; C holds a task of its own.
fn server_and_newcomer(dav: &Dav) -> (Device, Device, TaskItem) {
    let (a, c) = (device(), device());
    connect(&a, dav);
    let from_a = add(&a, "а");
    a.sync_now().unwrap();
    let inbox = dav.home().join("lists-inbox");
    std::fs::write(inbox.join("foreign.ics"), FOREIGN_TASK).unwrap();
    std::fs::write(inbox.join("meeting.ics"), FOREIGN_EVENT).unwrap();
    a.sync_now().unwrap();
    add(&c, "в");
    (a, c, from_a)
}

#[test]
fn c28_there_is_a_side_to_choose_only_when_the_server_holds_objects() {
    let dav = start();
    let c = device();
    add(&c, "в");
    let seen = dav.requests(|| {
        assert!(!c.sync_conflict(caldav(&dav), "secret".into()).unwrap());
    });
    assert!(seen.iter().all(|r| r.starts_with("PROPFIND")), "{seen:?}");
    assert!(
        std::fs::read_dir(dav.home()).unwrap().next().is_none(),
        "no calendar is made"
    );

    let a = device();
    connect(&a, &dav);
    add(&a, "а");
    a.sync_now().unwrap();
    let seen = dav.requests(|| {
        assert!(c.sync_conflict(caldav(&dav), "secret".into()).unwrap());
    });
    assert!(seen.iter().all(|r| r.starts_with("PROPFIND")), "{seen:?}");
    assert!(!device().sync_conflict(caldav(&dav), "secret".into()).unwrap());
    assert!(c.sync_conflict(caldav(&dav), "wrong".into()).is_err());
}

#[test]
fn c28_taking_the_server_drops_what_the_device_held() {
    let dav = start();
    let (a, c, _) = server_and_newcomer(&dav);
    connect(&c, &dav);
    c.replace_local_with_remote().unwrap();
    settle(&c, &a);
    for d in [&a, &c] {
        let mut got = view(d, Scope::All);
        got.sort();
        assert_eq!(got, ["From another app", "а"]);
        assert!(view(d, Scope::Trash).is_empty());
    }
    // The task of A, the task and the event of the other client: nothing of C reached the server.
    assert_eq!(dav.objects(), 3);
}

#[test]
fn c28_taking_the_server_keeps_the_settings_that_do_not_travel() {
    let dav = start();
    let (a, c, _) = server_and_newcomer(&dav);
    c.set_keep_done(KeepDone::EndOfDay).unwrap();
    c.join_storage(caldav(&dav), SyncSide::Storage).unwrap();
    c.set_sync_password(Some("secret".into()));
    settle(&c, &a);
    assert_eq!(c.keep_done().unwrap(), KeepDone::EndOfDay);
    assert!(!view(&c, Scope::All).contains(&"в".to_string()));
}

#[test]
fn c28_sending_the_device_clears_the_server_of_the_rest() {
    let dav = start();
    let (a, c, from_a) = server_and_newcomer(&dav);
    connect(&c, &dav);
    c.replace_remote_with_local().unwrap();
    c.sync_now().unwrap();
    a.sync_now().unwrap();
    for d in [&a, &c] {
        assert_eq!(view(d, Scope::All), ["в"]);
        let mut gone = view(d, Scope::Trash);
        gone.sort();
        assert_eq!(gone, ["From another app", "а"]);
    }
    let inbox = dav.home().join("lists-inbox");
    assert!(!dav.object(&from_a.id).exists());
    assert!(!inbox.join("foreign.ics").exists());
    assert!(inbox.join("meeting.ics").exists(), "an event is not ours to remove");
    assert_eq!(dav.objects(), 2);
    // Nothing waits any more.
    c.sync_now().unwrap();
    assert_eq!(c.sync_now().unwrap(), SyncReport::default());
}

#[test]
fn c28_sending_the_device_undoes_an_edit_of_another_client() {
    let dav = start();
    let (a, b) = pair(&dav);
    let t = add(&a, "один");
    settle(&a, &b);
    rename_elsewhere(&dav, &t.id, "Renamed elsewhere");

    b.replace_remote_with_local().unwrap();
    b.sync_now().unwrap();
    a.sync_now().unwrap();
    assert!(dav.read(&t.id).contains("SUMMARY:один"), "{}", dav.read(&t.id));
    for d in [&a, &b] {
        assert_eq!(d.task(t.id.clone()).unwrap().title, "один");
    }
}

#[test]
fn s51_a_category_that_does_not_fit_is_not_a_local_register() {
    let dav = start();
    let (a, b) = pair(&dav);
    let t = add(&a, "задача");
    settle(&a, &b);
    let fits = "a".repeat(196);
    let long = "b".repeat(197);

    // Another client puts one category that fits into the field and one that does not.
    dav.edit(&t.id, |lines| {
        let mut out: Vec<String> = lines.into_iter().filter(|l| !l.starts_with("CATEGORIES")).collect();
        let end = out.iter().position(|l| l == "END:VTODO").unwrap();
        out.insert(end, format!("CATEGORIES:{fits},{long}"));
        out
    });

    assert!(a.sync_now().unwrap().pulled > 0);
    assert_eq!(a.task(t.id.clone()).unwrap().tags, std::slice::from_ref(&fits));
    let conn = rusqlite::Connection::open(a.dir().join("notes.sqlite")).unwrap();
    let written: i64 = conn
        .query_row(
            "SELECT count(*) FROM fields WHERE field = ?1",
            [format!("tag:{long}")],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(written, 0, "the category never becomes a register");

    b.sync_now().unwrap();
    assert_eq!(b.task(t.id).unwrap().tags, [fits]);
}
