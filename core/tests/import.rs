//! Import from other task managers: docs/specs/import.md.

mod common;

use std::io::Write;

use common::*;
use notes_core::*;

/// A backup shaped like the ones 2Do 4.19 for macOS writes: a zip with
/// `2do.db` and the attachment files. Only the columns the importer reads.
fn twodo_backup(dir: &std::path::Path) -> String {
    let db = dir.join("2do.db");
    let _ = std::fs::remove_file(&db);
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute_batch(
        "CREATE TABLE calendars (uid TEXT, title TEXT, colorhex TEXT, isinboxcal INTEGER, isdeleted INTEGER, listtype INTEGER, smartsearch TEXT, displayorder INTEGER);
         CREATE TABLE tasks (primid INTEGER PRIMARY KEY, uid TEXT, calendaruid TEXT, title TEXT, notes TEXT, duedate DOUBLE, duetime DOUBLE, startdate DOUBLE,
            priority INTEGER, iscompleted INTEGER, completeddate DOUBLE, tags TEXT, parent TEXT, tasktype INTEGER, repeattype INTEGER, url TEXT, isdeleted INTEGER, displayorder INTEGER,
            repeatvalue INTEGER DEFAULT 0, recurrence INTEGER DEFAULT 1, recurrenceendtype INTEGER DEFAULT 0, recurrenceendrepeats INTEGER DEFAULT 0, recurrenceenddate DOUBLE DEFAULT 0);
         CREATE TABLE taskattachments (taskuid TEXT, displayname TEXT, fileextension TEXT, relativepath TEXT, isdeleted INTEGER, orderindex INTEGER);
         INSERT INTO calendars VALUES ('all', 'All', '#000000', 0, 0, 1, '', 0), ('inbox', 'Inbox', '#5E7C92', 1, 0, 6, '', 1),
            ('home', 'Дом', '#379417', 0, 0, 0, '', 2), ('smart', 'Smart', '#111111', 0, 0, 0, 'type:overdue', 3), ('gone', 'Удалён', '', 0, 1, 0, '', 4);
         INSERT INTO tasks (uid, calendaruid, title, notes, duedate, duetime, startdate, priority, iscompleted, completeddate, tags, parent, tasktype, repeattype, url, isdeleted, displayorder) VALUES
            ('t1', 'home', 'Полить цветы', 'раз в три дня', 1790510400, 999999, 6406192800, 10, 0, 0, 'дом_~|$$@$$|~_0_~|$$@$$|~__~|$$@$$|~_1_~|$$@$$|~_abc_~|$$@$$|~__~|$$@$$|~__~|$$@$$|~_0', '', 0, 258, '', 0, 1),
            ('t2', 'inbox', 'Ремонт', '', 1790510400, 930, 1790467200, 1, 0, 0, '', '', 2, 0, 'https://example.org', 0, 2),
            ('t3', 'inbox', 'Купить краску', '', 6406192800, 999999, 6406192800, 9, 1, 1790596633, '', 't2', 0, 0, '', 0, 3),
            ('t4', 'home', 'Удалена', '', 6406192800, 999999, 6406192800, 10, 0, 0, '', '', 0, 0, '', 1, 4),
            ('t5', 'home', 'Особый повтор', '', 1790510400, 999999, 6406192800, 10, 0, 0, '', '', 0, 513, '', 0, 5),
            ('t6', 'home', 'Последняя пятница', '', 1790510400, 999999, 6406192800, 5, 0, 0, '', '', 1, 264, '', 0, 6),
            ('t7', 'home', 'Пн, ср, пт', '', 1790510400, 999999, 6406192800, 10, 0, 0, '', '', 0, 84, '', 0, 7);
         UPDATE tasks SET repeatvalue = 6 WHERE uid = 't6';
         UPDATE tasks SET repeatvalue = 1, recurrenceendtype = 2, recurrenceendrepeats = 4 WHERE uid = 't7';
         UPDATE tasks SET repeatvalue = 3 WHERE uid = 't1';
         UPDATE tasks SET repeattype = 258, repeatvalue = 1, recurrence = 2, recurrenceendtype = 1, recurrenceenddate = 1792497600 WHERE uid = 't2';
         INSERT INTO taskattachments VALUES ('t2', 'План.jpg', 'jpg', 'aa/bb/payload.jpg', 0, 0), ('t2', 'evil', 'txt', '../../2do.db', 0, 1),
            ('t1', 'Image', 'jpg', 'cc/dd/payload.jpg', 0, 0);",
    )
    .unwrap();
    drop(conn);
    let path = dir.join("backup.2dodb");
    let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
    let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    zip.start_file("2do.db", options).unwrap();
    zip.write_all(&std::fs::read(&db).unwrap()).unwrap();
    zip.start_file("2DoBackupPayload/Attachments/aa/bb/payload.jpg", options)
        .unwrap();
    zip.write_all(b"jpeg bytes").unwrap();
    zip.start_file("2DoBackupPayload/Attachments/cc/dd/payload.jpg", options)
        .unwrap();
    zip.write_all(b"photo bytes").unwrap();
    zip.finish().unwrap();
    path.to_string_lossy().into_owned()
}

#[test]
fn twodo_backup_brings_lists_tasks_subtasks_tags_and_attachments() {
    let d = device();
    let dir = tempfile::tempdir().unwrap();
    let report = d.import_file(twodo_backup(dir.path())).unwrap();
    assert_eq!(
        (report.source.as_str(), report.lists, report.tasks, report.attachments),
        ("2Do", 1, 6, 2)
    );
    assert!(
        report
            .notes
            .iter()
            .any(|n| n.starts_with("4 repeat rules were converted")),
        "{:?}",
        report.notes
    );
    assert!(
        report
            .notes
            .iter()
            .any(|n| n.starts_with("1 repeating tasks use a rule")),
        "{:?}",
        report.notes
    );

    let lists = d.lists().unwrap();
    assert_eq!(
        lists.iter().map(|l| l.name.as_str()).collect::<Vec<_>>(),
        ["", "Дом"],
        "focus, smart and deleted lists are not lists"
    );
    assert_eq!(lists[1].color, "#379417");

    let home = d
        .tasks(Scope::List {
            id: lists[1].id.clone(),
        })
        .unwrap();
    assert_eq!(
        titles(&home),
        ["Полить цветы", "Особый повтор", "Последняя пятница", "Пн, ср, пт"]
    );
    let last_friday = home[2].repeat.clone().unwrap();
    assert_eq!(
        (last_friday.freq, last_friday.nth, last_friday.nth_weekday),
        (Freq::Monthly, Some(-1), Some(5))
    );
    assert_eq!(home[2].priority, Priority::Medium);
    assert!(!home[2].is_project, "a checklist is a plain task");
    let three_days = home[3].repeat.clone().unwrap();
    assert_eq!(
        (three_days.freq, three_days.weekdays.clone(), three_days.count),
        (Freq::Weekly, vec![1, 3, 5], Some(4))
    );
    assert_eq!(
        (home[0].due.as_deref(), home[0].notes.as_str()),
        (Some("2026-09-27"), "раз в три дня")
    );
    assert_eq!(home[0].tags, ["дом"]);
    let every_three_months = home[0].repeat.clone().expect("258 with value 3 is every three months");
    assert_eq!(
        (
            every_three_months.freq,
            every_three_months.interval,
            every_three_months.from_done
        ),
        (Freq::Monthly, 3, false)
    );
    assert_eq!(
        every_three_months.monthday,
        Some(27),
        "the day of the month comes from the due date"
    );
    assert_eq!(
        home[1].tags,
        ["2do-repeat"],
        "a rule that is not understood is flagged, not guessed"
    );
    assert!(home[1].repeat.is_none());

    // A project is not a row of the Inbox (R120), it is in the sidebar.
    assert!(d.tasks(Scope::Inbox).unwrap().is_empty());
    let projects = d.projects().unwrap();
    assert_eq!(titles(&projects), ["Ремонт"]);
    let project = &projects[0];
    assert!(project.is_project);
    assert_eq!(project.list_id, INBOX_ID);
    assert_eq!(project.priority, Priority::High);
    assert_eq!(
        (project.due.as_deref(), project.start.as_deref()),
        (Some("2026-09-27T09:30"), Some("2026-09-27"))
    );
    assert!(project.notes.contains("https://example.org"));
    let monthly = project.repeat.clone().unwrap();
    assert_eq!(
        (
            monthly.freq,
            monthly.interval,
            monthly.from_done,
            monthly.until.as_deref()
        ),
        (Freq::Monthly, 1, true, Some("2026-10-20"))
    );
    let subs = d.subtasks(project.id.clone()).unwrap();
    assert_eq!(titles(&subs), ["Купить краску"]);
    assert!(subs[0].done.is_some() && subs[0].priority == Priority::Low);
    let files = d.attachments(project.id.clone()).unwrap();
    assert_eq!(files.len(), 1, "a path that leaves the backup is ignored");
    assert_eq!(files[0].name, "План.jpg");
    assert_eq!(
        std::fs::read(files[0].local_path.as_ref().unwrap()).unwrap(),
        b"jpeg bytes"
    );

    // Importing the same backup again changes nothing: no duplicates.
    d.import_file(twodo_backup(dir.path())).unwrap();
    assert_eq!(d.projects().unwrap().len(), 1);
    assert!(d.tasks(Scope::Inbox).unwrap().is_empty());
    assert_eq!(d.lists().unwrap().len(), 2);
    assert_eq!(d.attachments(project.id.clone()).unwrap().len(), 1);
}

#[test]
fn todoist_csv_template() {
    let d = device();
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("Work.csv");
    std::fs::write(
        &file,
        "\u{feff}TYPE,CONTENT,DESCRIPTION,PRIORITY,INDENT,AUTHOR,RESPONSIBLE,DATE,DATE_LANG,TIMEZONE\n\
         section,Planning,,,,,,,,\n\
         task,Write the report @quarterly,\"Numbers, then text\",1,1,me (1),,2026-10-09,en,UTC\n\
         note,First comment,,,,,,,,\n\
         task,Collect numbers,,4,2,me (1),,tomorrow,en,UTC\n\
         task,Weekly review,,2,1,me (1),,every monday,en,UTC\n",
    )
    .unwrap();
    let report = d.import_file(file.to_string_lossy().into_owned()).unwrap();
    assert_eq!((report.source.as_str(), report.lists, report.tasks), ("Todoist", 1, 3));

    let list = d.lists().unwrap().into_iter().find(|l| l.name == "Work").unwrap();
    let tasks = d.tasks(Scope::List { id: list.id }).unwrap();
    assert_eq!(titles(&tasks), ["Write the report", "Weekly review"]);
    let first = &tasks[0];
    assert_eq!(first.priority, Priority::High, "1 is the highest in the CSV");
    assert_eq!(first.due.as_deref(), Some("2026-10-09"));
    assert_eq!(first.tags, ["quarterly"]);
    assert_eq!(first.notes, "Numbers, then text\nFirst comment");
    assert_eq!(titles(&d.subtasks(first.id.clone()).unwrap()), ["Collect numbers"]);
    assert!(
        tasks[1].notes.contains("Todoist date: every monday"),
        "a repeat in words is kept as text"
    );
    assert_eq!(tasks[1].priority, Priority::Medium);
}

#[test]
fn i9_a_subtask_keeps_its_title_and_one_level() {
    let d = device();
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("Home.csv");
    std::fs::write(
        &file,
        "TYPE,CONTENT,DESCRIPTION,PRIORITY,INDENT,AUTHOR,RESPONSIBLE,DATE,DATE_LANG,TIMEZONE\n\
         task,Repair,,4,1,me (1),,,en,UTC\n\
         task,Kitchen @room,About the kitchen,1,2,me (1),,2026-10-09,en,UTC\n\
         task,Tiles,,1,3,me (1),,2026-10-10,en,UTC\n\
         task,Hall,,4,2,me (1),,,en,UTC\n",
    )
    .unwrap();
    d.import_file(file.to_string_lossy().into_owned()).unwrap();

    let list = d.lists().unwrap().into_iter().find(|l| l.name == "Home").unwrap();
    let tasks = d.tasks(Scope::List { id: list.id }).unwrap();
    assert_eq!(titles(&tasks), ["Repair"]);
    let lines = d.subtasks(tasks[0].id.clone()).unwrap();
    assert_eq!(
        titles(&lines),
        ["Kitchen", "Tiles", "Hall"],
        "the deeper one stands after its former parent"
    );
    for line in &lines {
        assert_eq!(
            (line.due.as_deref(), line.priority, line.notes.as_str()),
            (None, Priority::None, "")
        );
        assert!(line.tags.is_empty());
    }
    assert!(d.tasks(Scope::Upcoming).unwrap().is_empty());
}

/// Every task the device has, open or closed, top level or not.
fn every_task(d: &Store) -> Vec<TaskItem> {
    let mut out = Vec::new();
    for scope in [Scope::All, Scope::Completed, Scope::WontDo] {
        for task in d.tasks(scope).unwrap() {
            if !out.iter().any(|t: &TaskItem| t.id == task.id) {
                out.extend(d.subtasks(task.id.clone()).unwrap());
                out.push(task);
            }
        }
    }
    out
}

/// I12 on rows of real TickTick backups, with their text replaced.
#[test]
fn i12_ticktick_backup_brings_lists_dates_reminders_checklists_and_subtasks() {
    let d = device();
    let file = format!("{}/tests/data/ticktick-backup.csv", env!("CARGO_MANIFEST_DIR"));
    let report = d.in_zone(0, || d.import_file(file.clone())).unwrap();
    assert_eq!(
        (report.source.as_str(), report.lists, report.tasks),
        ("TickTick", 7, 22)
    );
    assert_eq!(
        report.notes,
        [
            "1 reminder was beyond the first of a task not imported.",
            "1 note was imported as tasks.",
            "3 lists were kanban: the columns were not imported.",
            "A TickTick backup has no list colours, attachments or comments.",
        ]
    );

    let mut names: Vec<String> = d.lists().unwrap().into_iter().map(|l| l.name).collect();
    names.sort();
    for name in ["Folder 1: List 1", "Folder 2: List 2", "Folder 4: List 7", "List 6"] {
        assert!(names.contains(&name.to_string()), "{name} in {names:?}");
    }
    let all = every_task(&d);
    let task = |title: &str| {
        all.iter()
            .find(|t| t.title == title)
            .unwrap_or_else(|| panic!("{title}"))
    };

    // Inbox without a folder is the inbox; a range keeps both ends and the reminder counts from the start.
    let range = task("Task 12");
    assert_eq!(range.list_id, "inbox");
    assert_eq!(
        (range.start.as_deref(), range.due.as_deref(), range.remind.as_deref()),
        (
            Some("2026-06-07T04:30"),
            Some("2026-06-07T06:30"),
            Some("2026-06-07T04:00")
        )
    );
    assert_eq!(range.notes, "Note 12 line 1");
    // A day without a time, written as midnight in Perth, and its reminder two hours before.
    let perth = task("Task 2");
    assert_eq!(
        (perth.start.as_deref(), perth.due.as_deref()),
        (None, Some("2024-08-06"))
    );
    assert_eq!(perth.remind.as_deref(), Some("2024-08-05T22:00"));
    assert_eq!(perth.done.as_deref(), Some("2024-08-13T05:10"));
    assert!(!perth.wont);
    assert_eq!(
        task("Task 1").remind.as_deref(),
        Some("2024-07-18T05:00"),
        "the first of two reminders"
    );

    let wont = task("Task 3");
    assert!(wont.wont && wont.done.is_some());
    assert_eq!(wont.priority, Priority::High);
    assert_eq!(task("Task 6").priority, Priority::High);
    assert_eq!(task("Task 7").priority, Priority::Medium);
    assert_eq!(task("Task 8").priority, Priority::None);
    assert!(task("Task 8").done.is_none());

    let notes = task("Task 9");
    assert_eq!(notes.notes, "Note 9 line 1\nNote 9 line 2");
    assert_eq!(notes.tags, ["tag9a", "tag9b"]);
    assert_eq!(notes.due.as_deref(), Some("2024-06-02"));

    let weekly = task("Task 10").repeat.clone().unwrap();
    assert_eq!((weekly.freq, weekly.weekdays), (Freq::Weekly, vec![1, 3, 5]));
    let monthly = task("Task 11");
    assert_eq!(monthly.repeat.as_ref().unwrap().freq, Freq::Monthly);
    assert_eq!(monthly.due.as_deref(), Some("2024-04-15"));

    // Checklist items become subtasks; an item without text is dropped.
    let checklist = task("Task 4");
    assert_eq!(
        titles(&d.subtasks(checklist.id.clone()).unwrap()),
        ["Item 4.1", "Item 4.2", "Item 4.3", "Item 4.4", "Item 4.5", "Item 4.6"]
    );
    assert!(d.subtasks(task("Task 5").id.clone()).unwrap().is_empty());
    // Subtasks by parentId, in the order TickTick shows them.
    assert_eq!(
        titles(&d.subtasks(task("Task 14").id.clone()).unwrap()),
        ["Task 15", "Task 16"]
    );

    let again = d.in_zone(0, || d.import_file(file)).unwrap();
    assert_eq!((again.lists, again.tasks), (7, 22));
    assert_eq!(every_task(&d).len(), all.len(), "a second import adds nothing");
}

#[test]
fn i12_ticktick_checked_items_unknown_repeats_and_the_older_header() {
    let d = device();
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("ticktick-backup.csv");
    std::fs::write(
        &file,
        "\u{feff}\"Date: 2024-05-18+0000\"\n\"Version: 7.1\"\n\"Status: \n0 Normal\n1 Completed\n2 Archived\"\n\
         \"Folder Name\",\"List Name\",\"Title\",\"Kind\",\"Tags\",\"Content\",\"Is Check list\",\"Start Date\",\"Due Date\",\"Reminder\",\"Repeat\",\"Priority\",\"Status\",\"Created Time\",\"Completed Time\",\"Order\",\"Timezone\",\"Is All Day\",\"Is Floating\",\"Column Name\",\"Column Order\",\"View Mode\",\"taskId\",\"parentId\"\n\
         \"\",\"Home\",\"Pack\",\"CHECKLIST\",\"\",\"Before leaving\n▪Passport\n▫Tickets\n\",\"Y\",\"\",\"\",\"\",\"FREQ=HOURLY;INTERVAL=2\",\"1\",\"0\",\"2024-04-22T16:15:35+0000\",\"\",\"0\",\"UTC\",\"false\",\"false\",,,\"list\",\"1\",\"\"\n",
    )
    .unwrap();
    let report = d.import_file(file.to_string_lossy().into_owned()).unwrap();
    assert_eq!((report.source.as_str(), report.lists, report.tasks), ("TickTick", 1, 3));
    assert!(report.notes.contains(
        &"1 repeat rule was not expressible here; the rule text was added to the notes of the task.".to_string()
    ));
    let list = d.lists().unwrap().into_iter().find(|l| l.name == "Home").unwrap();
    let pack = &d.tasks(Scope::List { id: list.id }).unwrap()[0];
    assert_eq!(pack.priority, Priority::Low);
    assert_eq!(pack.notes, "Before leaving\nTickTick repeat: FREQ=HOURLY;INTERVAL=2");
    assert!(pack.repeat.is_none());
    let items = d.subtasks(pack.id.clone()).unwrap();
    assert_eq!(titles(&items), ["Passport", "Tickets"]);
    assert!(items[0].done.is_some() && items[1].done.is_none());
}

/// I13 on rows of a real Things 3.24.1 database, with their text replaced.
#[test]
fn i13_things_database_brings_areas_projects_dates_checklists_and_repeats() {
    let d = device();
    let dir = tempfile::tempdir().unwrap();
    // The way Things keeps it: a folder with `main.sqlite` in it.
    let folder = dir.path().join("Things Database.thingsdatabase");
    std::fs::create_dir(&folder).unwrap();
    let source = format!("{}/tests/data/things.sqlite", env!("CARGO_MANIFEST_DIR"));
    std::fs::copy(&source, folder.join("main.sqlite")).unwrap();
    // A task under a heading of a project in the Trash is in the Trash as well.
    let conn = rusqlite::Connection::open(folder.join("main.sqlite")).unwrap();
    conn.execute_batch(
        "INSERT INTO TMTask (uuid, type, status, trashed, title, start, \"index\", creationDate) VALUES
            ('gone-project', 1, 0, 1, 'Gone project', 1, 0, 0), ('gone-heading', 2, 0, 0, 'Gone heading', 1, 0, 0);
         UPDATE TMTask SET project = 'gone-project' WHERE uuid = 'gone-heading';
         INSERT INTO TMTask (uuid, type, status, trashed, title, start, heading, \"index\", creationDate)
            VALUES ('gone-task', 0, 0, 0, 'Gone with its project', 1, 'gone-heading', 0, 0);",
    )
    .unwrap();
    drop(conn);
    let before = std::fs::read(folder.join("main.sqlite")).unwrap();
    let path = folder.to_string_lossy().into_owned();
    let report = d.in_zone(0, || d.import_file(path.clone())).unwrap();
    assert_eq!((report.source.as_str(), report.lists, report.tasks), ("Things", 3, 26));
    assert_eq!(
        report.notes,
        [
            "2 tasks were in Someday: they got the tag \"someday\".",
            "1 heading was not imported; their tasks stay in the project.",
            "3 tasks were in the Trash and not imported.",
            "1 repeating task was paused and not imported.",
        ]
    );
    assert_eq!(
        std::fs::read(folder.join("main.sqlite")).unwrap(),
        before,
        "the database is only read"
    );

    let lists = d.lists().unwrap();
    let area = |name: &str| lists.iter().find(|l| l.name == name).unwrap().id.clone();
    let all = every_task(&d);
    let task = |title: &str| {
        all.iter()
            .find(|t| t.title == title)
            .unwrap_or_else(|| panic!("{title}"))
    };

    // The open task of the active repeating task carries the rule; the done ones it made stay done.
    let repeating = task("Task 26");
    assert_eq!(repeating.list_id, "inbox");
    assert_eq!(repeating.start.as_deref(), Some("2026-09-25"));
    let rule = repeating.repeat.clone().unwrap();
    assert_eq!((rule.freq, rule.interval, rule.monthday), (Freq::Monthly, 1, Some(25)));
    assert_eq!(task("Task 25").done.as_deref(), Some("2026-09-11T13:14"));
    assert!(task("Task 25").repeat.is_none());

    // A project of an area keeps its tasks, those under a heading included.
    let project = task("Project 13");
    assert!(project.is_project);
    assert_eq!(project.list_id, area("Area 1"));
    assert_eq!(
        titles(&d.subtasks(project.id.clone()).unwrap()),
        ["Task 12", "Task 11", "Task 10"]
    );
    let under_heading = task("Task 4");
    assert_eq!(under_heading.parent_id.as_deref(), Some(task("Project 3").id.as_str()));
    assert!(under_heading.wont, "cancelled in Things");

    // Dates: when as the start, the deadline as the due date, the reminder on the start day.
    let dated = task("Task 6");
    assert_eq!(
        (dated.start.as_deref(), dated.due.as_deref()),
        (Some("2022-11-13"), Some("2022-11-15"))
    );
    assert_eq!(task("Task 14").remind.as_deref(), Some("2025-09-04T10:30"));
    assert_eq!(task("Task 14").list_id, area("Area 2"));
    assert_eq!(task("Task 17").tags, ["someday"]);
    assert_eq!(task("Task 17").notes, "Note 17\nsecond line");
    assert_eq!(task("Task 27").tags, ["tag-1"]);

    // A checklist in the order of Things, with what was checked.
    let items = d.subtasks(task("Task 15").id.clone()).unwrap();
    assert_eq!(titles(&items), ["Item 1", "Item 2", "Item 3"]);
    assert_eq!(
        items.iter().map(|i| i.done.is_some()).collect::<Vec<_>>(),
        [false, true, true]
    );
    // Neither the trashed task, the heading nor the paused template came over.
    for gone in [
        "Task 22",
        "Heading 5",
        "Task 1",
        "Gone project",
        "Gone with its project",
    ] {
        assert!(all.iter().all(|t| t.title != gone), "{gone}");
    }

    let again = d.in_zone(0, || d.import_file(path)).unwrap();
    assert_eq!(again.tasks, 26);
    assert_eq!(every_task(&d).len(), all.len(), "a second import adds nothing");
}

#[test]
fn i13_an_sqlite_file_of_another_app_is_refused() {
    let d = device();
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("other.sqlite");
    rusqlite::Connection::open(&file)
        .unwrap()
        .execute_batch("CREATE TABLE notes (id INTEGER)")
        .unwrap();
    let error = d.import_file(file.to_string_lossy().into_owned()).unwrap_err();
    assert!(error.to_string().contains("not one of Things 3"), "{error}");
    assert!(d.tasks(Scope::All).unwrap().is_empty());
}

/// I14 on rows of a real OmniFocus 2 export, with their text replaced.
#[test]
fn i14_omnifocus_export_brings_projects_actions_groups_dates_and_completion() {
    let d = device();
    let file = format!("{}/tests/data/OmniFocus.csv", env!("CARGO_MANIFEST_DIR"));
    let report = d.in_zone(0, || d.import_file(file.clone())).unwrap();
    assert_eq!(
        (report.source.as_str(), report.lists, report.tasks),
        ("OmniFocus", 1, 64)
    );
    assert_eq!(
        report.notes,
        [
            "4 tasks had an estimated duration, which is not imported.",
            "An OmniFocus export has no repeat rules, folders or attachments.",
        ]
    );
    let list = d.lists().unwrap().into_iter().find(|l| l.name == "OmniFocus").unwrap();
    let projects = d.projects().unwrap();
    assert_eq!(
        titles(&projects),
        ["Project 1", "Project 4", "Project 6", "Project 10", "Project 33"]
    );
    assert!(projects.iter().all(|p| p.list_id == list.id));
    let in_project = |title: &str| {
        let project = projects.iter().find(|p| p.title == title).unwrap();
        d.tasks(Scope::Project { id: project.id.clone() }).unwrap()
    };

    // Actions are tasks of the project; an action group keeps its actions as subtasks.
    let first = in_project("Project 1");
    assert_eq!(titles(&first), ["Action 1.1", "Action 1.2", "Action 1.3", "Action 1.4"]);
    assert_eq!(first[1].notes, "Note 1.2 line 1");
    assert_eq!(
        titles(&d.subtasks(first[2].id.clone()).unwrap()),
        [
            "Action 1.3.1",
            "Action 1.3.2",
            "Action 1.3.3",
            "Action 1.3.4",
            "Action 1.3.5"
        ]
    );
    // Deeper levels come up to one (I9), right after their former parent.
    let group = &in_project("Project 10")[0];
    assert_eq!(group.notes, "Note 10.1 line 1\nNote 10.1 line 2");
    assert_eq!(
        titles(&d.subtasks(group.id.clone()).unwrap()),
        [
            "Action 10.1.1",
            "Action 10.1.1.1",
            "Action 10.1.1.2",
            "Action 10.1.1.3",
            "Action 10.1.1.3.1",
            "Action 10.1.1.3.2"
        ]
    );

    // Dates and completion in UTC, as the test zone reads them.
    let done = in_project("Project 4")
        .into_iter()
        .find(|t| t.title == "Action 4.1")
        .unwrap();
    assert_eq!(done.due.as_deref(), Some("2018-11-20T12:00"));
    assert_eq!(done.done.as_deref(), Some("2018-11-20T09:25"));
    let finished = d.tasks(Scope::Completed).unwrap();
    let project = finished.iter().find(|t| t.title == "Project 28").unwrap();
    assert!(project.is_project && !project.wont);
    assert_eq!(project.done.as_deref(), Some("2018-04-02T07:33"));

    let again = d.in_zone(0, || d.import_file(file)).unwrap();
    assert_eq!(again.tasks, 64);
    assert_eq!(
        d.projects().unwrap().len() + d.tasks(Scope::Completed).unwrap().len(),
        projects.len() + finished.len()
    );
    assert_eq!(in_project("Project 1").len(), 4, "a second import adds nothing");
    assert_eq!(d.subtasks(first[2].id.clone()).unwrap().len(), 5);
}

/// I14 for the columns of OmniFocus 3 and 4 as two independent descriptions give them.
#[test]
fn i14_omnifocus_3_and_4_columns_inbox_dropped_and_flagged() {
    let d = device();
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("Work.csv");
    std::fs::write(
        &file,
        "Task ID,Type,Name,Status,Project,Context,Start Date,Planned Date,Due Date,Completion Date,Duration,Flagged,Notes,Tags\r\n\
         1,Inbox,Call back,,,Phone,,,,,,1,,\"Phone, Home\"\r\n\
         2,Project,Garden,dropped,,,,,,,,0,,\r\n\
         2.1,Action,Buy seeds,,Garden,,2026-09-07 07:00:00 +0000,2026-09-08 07:00:00 +0000,,,15m,0,,Errands\r\n",
    )
    .unwrap();
    let report = d
        .in_zone(0, || d.import_file(file.to_string_lossy().into_owned()))
        .unwrap();
    assert_eq!(
        (report.source.as_str(), report.lists, report.tasks),
        ("OmniFocus", 1, 3)
    );
    assert!(report
        .notes
        .contains(&"1 task had a planned date, which is not imported.".to_string()));
    let inbox = d.tasks(Scope::Inbox).unwrap();
    assert_eq!(titles(&inbox), ["Call back"]);
    assert_eq!(inbox[0].priority, Priority::High, "flagged");
    assert_eq!(inbox[0].tags, ["home", "phone"], "the Tags column wins over Context");
    let dropped = d.tasks(Scope::WontDo).unwrap();
    assert_eq!(titles(&dropped), ["Garden"]);
    let seeds = &d
        .tasks(Scope::Project {
            id: dropped[0].id.clone(),
        })
        .unwrap()[0];
    assert_eq!(
        (seeds.title.as_str(), seeds.start.as_deref()),
        ("Buy seeds", Some("2026-09-07T07:00"))
    );
    assert_eq!(seeds.tags, ["errands"]);
}

#[test]
fn trello_board_json() {
    let d = device();
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("board.json");
    std::fs::write(
        &file,
        r#"{"name":"Launch","lists":[{"id":"l1","name":"To do","closed":false,"pos":1},{"id":"l2","name":"Old","closed":true,"pos":2}],
            "labels":[{"id":"lb1","name":"urgent","color":"red"},{"id":"lb2","name":"","color":"green"}],
            "cards":[{"id":"c1","name":"Ship it","desc":"All of it","closed":false,"idList":"l1","idLabels":["lb1","lb2"],"due":"2026-10-09T09:00:00.000Z","dueComplete":false},
                     {"id":"c2","name":"Archived","desc":"","closed":true,"idList":"l1","idLabels":[]},
                     {"id":"c3","name":"In old list","desc":"","closed":false,"idList":"l2","idLabels":[]}],
            "checklists":[{"id":"k1","idCard":"c1","name":"Steps","checkItems":[{"id":"i2","name":"Second","state":"incomplete","pos":2},{"id":"i1","name":"First","state":"complete","pos":1}]}]}"#,
    )
    .unwrap();
    let report = d.import_file(file.to_string_lossy().into_owned()).unwrap();
    assert_eq!((report.source.as_str(), report.lists, report.tasks), ("Trello", 1, 3));
    assert!(report.notes.iter().any(|n| n.starts_with("2 archived cards")));

    let list = d
        .lists()
        .unwrap()
        .into_iter()
        .find(|l| l.name == "Launch: To do")
        .unwrap();
    let cards = d.tasks(Scope::List { id: list.id }).unwrap();
    assert_eq!(titles(&cards), ["Ship it"]);
    assert_eq!(cards[0].notes, "All of it");
    assert_eq!(cards[0].tags, ["green", "urgent"]);
    assert!(cards[0].due.as_deref().unwrap().starts_with("2026-10-09T"));
    let items = d.subtasks(cards[0].id.clone()).unwrap();
    assert_eq!(titles(&items), ["First", "Second"], "checklist order follows pos");
    assert!(items[0].done.is_some() && items[1].done.is_none());
}

#[test]
fn r121_tasks_without_a_list_go_to_the_default_list_while_inbox_is_hidden() {
    let d = device();
    let home = d.create_list("Дом".into()).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("todo.json");
    let write = |id: &str, title: &str| {
        std::fs::write(
            &file,
            format!(
                r#"[{{"id":"L0","displayName":"Tasks","wellknownListName":"defaultList","tasks":[
                   {{"id":"{id}","title":"{title}","importance":"normal","status":"notStarted"}}]}}]"#
            ),
        )
        .unwrap();
        file.to_string_lossy().into_owned()
    };
    // R1: while Inbox is shown it takes them, whatever the default list for new tasks is.
    d.set_new_task_list(home.id.clone()).unwrap();
    d.import_file(write("a", "Pay rent")).unwrap();
    assert_eq!(view(&d, Scope::Inbox), ["Pay rent"]);

    d.hide_inbox(home.id.clone(), true).unwrap();
    d.import_file(write("b", "Call the bank")).unwrap();
    assert!(view(&d, Scope::Inbox).is_empty());
    let mut moved = view(&d, Scope::List { id: home.id });
    moved.sort();
    assert_eq!(moved, ["Call the bank", "Pay rent"]);
}

#[test]
fn microsoft_to_do_lists_json() {
    let d = device();
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("todo.json");
    std::fs::write(
        &file,
        r#"[{"id":"L0","displayName":"Tasks","wellknownListName":"defaultList","tasks":[
               {"id":"a","title":"Pay rent","importance":"high","status":"notStarted","body":{"content":"by card","contentType":"text"},
                "dueDateTime":{"dateTime":"2026-10-10T00:00:00.0000000","timeZone":"UTC"},"categories":["Money"],
                "recurrence":{"pattern":{"type":"absoluteMonthly","interval":1,"dayOfMonth":10}},
                "checklistItems":[{"id":"x","displayName":"Check the amount","isChecked":true}]}]},
            {"id":"L1","displayName":"Shopping","wellKnownListName":"none","tasks":[
               {"id":"b","title":"Milk","importance":"normal","status":"completed","completedDateTime":{"dateTime":"2026-10-01T08:00:00.0000000","timeZone":"UTC"}},
               {"id":"c","title":"Odd repeat","importance":"low","status":"notStarted","recurrence":{"pattern":{"type":"relativeMonthly","interval":1}}}]}]"#,
    )
    .unwrap();
    let report = d.import_file(file.to_string_lossy().into_owned()).unwrap();
    assert_eq!(
        (report.source.as_str(), report.lists, report.tasks),
        ("Microsoft To Do", 1, 4)
    );
    assert!(report
        .notes
        .iter()
        .any(|n| n.starts_with("1 tasks repeat on a relative pattern")));

    let rent = &d.tasks(Scope::Inbox).unwrap()[0];
    assert_eq!(
        (rent.title.as_str(), rent.priority, rent.due.as_deref()),
        ("Pay rent", Priority::High, Some("2026-10-10"))
    );
    assert_eq!(
        (rent.notes.as_str(), rent.tags.clone()),
        ("by card", vec!["money".to_string()])
    );
    let repeat = rent.repeat.clone().unwrap();
    assert_eq!((repeat.freq, repeat.monthday), (Freq::Monthly, Some(10)));
    assert!(d.subtasks(rent.id.clone()).unwrap()[0].done.is_some());

    let shopping = d.lists().unwrap().into_iter().find(|l| l.name == "Shopping").unwrap();
    assert_eq!(
        view(&d, Scope::List { id: shopping.id }),
        ["Odd repeat"],
        "the completed one is in Completed"
    );
    assert_eq!(view(&d, Scope::Completed), ["Milk"]);
}

#[test]
fn unknown_files_are_refused_with_a_reason() {
    let d = device();
    let dir = tempfile::tempdir().unwrap();
    let csv = dir.path().join("x.csv");
    std::fs::write(&csv, "a,b\n1,2\n").unwrap();
    assert!(d
        .import_file(csv.to_string_lossy().into_owned())
        .unwrap_err()
        .to_string()
        .contains("Todoist"));
    let json = dir.path().join("x.json");
    std::fs::write(&json, "{\"hello\":1}").unwrap();
    assert!(d.import_file(json.to_string_lossy().into_owned()).is_err());
    let zip = dir.path().join("x.2dodb");
    std::fs::write(&zip, b"PK not really").unwrap();
    assert!(d.import_file(zip.to_string_lossy().into_owned()).is_err());
    assert!(view(&d, Scope::Inbox).is_empty());
}

#[test]
fn i8_twodo_attachment_gets_the_extension_kept_beside_its_name() {
    let d = device();
    let dir = tempfile::tempdir().unwrap();
    d.import_file(twodo_backup(dir.path())).unwrap();
    let mut all = d.tasks(Scope::All).unwrap();
    all.extend(d.projects().unwrap());
    let files = |title: &str| {
        let task = all.iter().find(|t| t.title == title).unwrap();
        d.attachments(task.id.clone()).unwrap()
    };
    // 2Do names a photo "Image" and keeps "jpg" in a column of its own.
    let photo = files("Полить цветы");
    assert_eq!(
        (photo[0].name.as_str(), photo[0].mime.as_str()),
        ("Image.jpg", "image/jpeg")
    );
    // A name that already ends with the extension is left as it is.
    let plan = files("Ремонт");
    assert_eq!(plan[0].name, "План.jpg");
}

#[test]
fn i10_a_tag_that_does_not_fit_a_register_is_not_imported() {
    let d = device();
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("Long.csv");
    let fits = "a".repeat(196);
    let long = "b".repeat(197);
    std::fs::write(
        &file,
        format!(
            "TYPE,CONTENT,DESCRIPTION,PRIORITY,INDENT,AUTHOR,RESPONSIBLE,DATE,DATE_LANG,TIMEZONE\n\
             task,Keep @{fits} @{long},,4,1,me (1),,,en,UTC\n"
        ),
    )
    .unwrap();

    let report = d.import_file(file.to_string_lossy().into_owned()).unwrap();
    assert_eq!((report.source.as_str(), report.tasks), ("Todoist", 1));
    assert!(
        report
            .notes
            .iter()
            .any(|n| n == "1 tag was longer than 196 bytes and was not imported."),
        "{:?}",
        report.notes
    );
    let task = d.tasks(Scope::All).unwrap().remove(0);
    assert_eq!(task.tags, [fits], "the tag that fits is imported, the other one is not");
}

/// R155: the JSON export of `d` with ids replaced by names, in an order that does not depend on them.
fn exported(d: &Store, dir: &std::path::Path) -> serde_json::Value {
    let file = dir.join(format!("export-{}.json", uuid::Uuid::now_v7().simple()));
    d.export_json(file.to_string_lossy().into_owned(), None).unwrap();
    let mut v: serde_json::Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    let names: std::collections::HashMap<String, String> = v["lists"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| {
            (
                l["id"].as_str().unwrap().to_string(),
                l["name"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    let titles: std::collections::HashMap<String, String> = v["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| {
            (
                t["id"].as_str().unwrap().to_string(),
                t["title"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    let name = |id: &serde_json::Value| {
        id.as_str()
            .map(|i| names.get(i).cloned().unwrap_or_else(|| i.to_string()))
    };
    let mut lists: Vec<serde_json::Value> = v["lists"].as_array().unwrap().clone();
    for l in &mut lists {
        l["id"] = serde_json::json!(null);
    }
    let mut tasks: Vec<serde_json::Value> = v["tasks"].as_array().unwrap().clone();
    for t in &mut tasks {
        t["id"] = serde_json::json!(null);
        t["list"] = serde_json::json!(name(&t["list"]));
        t["parent"] = serde_json::json!(t["parent"].as_str().map(|p| titles[p].clone()));
    }
    let mut filters: Vec<serde_json::Value> = v["filters"].as_array().unwrap().clone();
    for f in &mut filters {
        f["id"] = serde_json::json!(null);
        let ids: Vec<serde_json::Value> = f["spec"]["list_ids"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|i| serde_json::json!(name(i)))
            .collect();
        f["spec"]["list_ids"] = serde_json::json!(ids);
    }
    let key = |x: &serde_json::Value| x.to_string();
    lists.sort_by_key(key);
    tasks.sort_by_key(key);
    filters.sort_by_key(key);
    v["lists"] = serde_json::json!(lists);
    v["tasks"] = serde_json::json!(tasks);
    v["filters"] = serde_json::json!(filters);
    v["exported"] = serde_json::json!(null);
    v
}

#[test]
fn r155_i11_an_export_imported_into_an_empty_device_gives_the_same_data_and_a_second_import_changes_nothing() {
    let a = device();
    let home = a.create_list("Дом".into()).unwrap();
    a.set_list_sort(home.id.clone(), SortMode::Due).unwrap();
    a.set_list_color(home.id.clone(), "#379417".into()).unwrap();
    let old = a.create_list("Архив".into()).unwrap();
    let x = a
        .create_task(NewTask {
            title: "а".into(),
            list_id: Some(home.id.clone()),
            notes: "строка\n\n- [ ] пункт".into(),
            due: Some("2026-10-07T18:30".into()),
            start: Some("2026-10-06".into()),
            ..NewTask::default()
        })
        .unwrap();
    a.set_remind(x.id.clone(), Some("2026-10-07T18:00".into())).unwrap();
    a.set_priority(x.id.clone(), Priority::High).unwrap();
    a.add_tag(x.id.clone(), "дом".into()).unwrap();
    a.add_tag(x.id.clone(), "срочно".into()).unwrap();
    let sub = add_sub(&a, &x, "а1");
    a.complete_task(sub.id).unwrap();
    add_sub(&a, &x, "а2");
    let wont = a
        .create_task(NewTask {
            title: "б".into(),
            list_id: Some(home.id.clone()),
            ..NewTask::default()
        })
        .unwrap();
    a.wont_do_task(wont.id).unwrap();
    let weekly = a
        .create_task(NewTask {
            title: "в".into(),
            list_id: Some(old.id.clone()),
            due: Some("2026-10-08".into()),
            ..NewTask::default()
        })
        .unwrap();
    a.set_repeat(
        weekly.id,
        Some(Repeat {
            freq: Freq::Weekly,
            interval: 2,
            weekdays: vec![1, 4],
            monthday: None,
            nth: None,
            nth_weekday: None,
            from_done: false,
            count: Some(5),
            until: None,
        }),
    )
    .unwrap();
    let project = a.create_project("Ремонт".into(), None).unwrap();
    a.create_task(NewTask {
        title: "краска".into(),
        parent_id: Some(project.id.clone()),
        ..NewTask::default()
    })
    .unwrap();
    a.set_list_archived(old.id, true).unwrap();
    a.create_filter(
        "Срочное".into(),
        FilterSpec {
            list_ids: vec![home.id, INBOX_ID.into()],
            min_priority: Priority::High,
            ..FilterSpec::default()
        },
    )
    .unwrap();

    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("Lists.json");
    a.export_json(file.to_string_lossy().into_owned(), None).unwrap();
    let raw: serde_json::Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    assert_eq!(
        (raw["format"].as_str(), raw["version"].as_u64()),
        (Some("lists"), Some(1))
    );

    let b = device();
    let report = b.import_file(file.to_string_lossy().into_owned()).unwrap();
    assert_eq!((report.source.as_str(), report.lists, report.tasks), ("Lists", 2, 7));
    assert!(
        report.notes.iter().any(|n| n.contains("1 saved filter")),
        "{:?}",
        report.notes
    );
    assert_eq!(exported(&b, dir.path()), exported(&a, dir.path()));

    b.import_file(file.to_string_lossy().into_owned()).unwrap();
    assert_eq!(
        exported(&b, dir.path()),
        exported(&a, dir.path()),
        "a second import changes nothing"
    );
    assert_eq!(b.lists().unwrap().len(), 3, "the inbox and two lists, not four");
}

#[test]
fn r155_one_list_and_a_newer_version_of_the_format() {
    let a = device();
    let home = a.create_list("Дом".into()).unwrap();
    a.create_task(NewTask {
        title: "дома".into(),
        list_id: Some(home.id.clone()),
        ..NewTask::default()
    })
    .unwrap();
    add(&a, "во входящих");
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("one.json");
    a.export_json(file.to_string_lossy().into_owned(), Some(home.id))
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    assert_eq!(v["lists"].as_array().unwrap().len(), 1);
    assert_eq!(v["tasks"].as_array().unwrap().len(), 1);
    assert_eq!(v["tasks"][0]["title"], "дома");
    assert!(a
        .export_json(file.to_string_lossy().into_owned(), Some("nope".into()))
        .is_err());

    std::fs::write(&file, r#"{"format":"lists","version":2,"lists":[],"tasks":[]}"#).unwrap();
    let refused = a
        .import_file(file.to_string_lossy().into_owned())
        .unwrap_err()
        .to_string();
    assert!(refused.contains("update the app"), "{refused}");
}

#[test]
fn r156_markdown_lays_out_lists_tasks_notes_and_subtasks() {
    let a = device();
    let home = a.create_list("Дом".into()).unwrap();
    let x = a
        .create_task(NewTask {
            title: "Купить краску".into(),
            list_id: Some(home.id.clone()),
            notes: "белую\nдве банки".into(),
            due: Some("2026-10-07T18:30".into()),
            ..NewTask::default()
        })
        .unwrap();
    a.set_priority(x.id.clone(), Priority::High).unwrap();
    a.add_tag(x.id.clone(), "дом".into()).unwrap();
    let sub = add_sub(&a, &x, "кисть");
    a.complete_task(sub.id).unwrap();
    let wont = a
        .create_task(NewTask {
            title: "Обои".into(),
            list_id: Some(home.id.clone()),
            ..NewTask::default()
        })
        .unwrap();
    a.wont_do_task(wont.id).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("Lists.md");
    a.export_markdown(file.to_string_lossy().into_owned(), None).unwrap();
    let text = std::fs::read_to_string(&file).unwrap();
    assert_eq!(
        text,
        "# Lists, 2026-10-05\n\n## Inbox\n\nNo tasks.\n\n## Дом\n\n\
         - [ ] Купить краску — due 2026-10-07 18:30 · high priority · #дом\n  белую\n  две банки\n  - [x] кисть\n\
         - [x] ~~Обои~~ — won't do\n"
    );
}
