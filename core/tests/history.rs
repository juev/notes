//! The history of versions of a text: R40–R43 in docs/specs/product.md, and S71 in
//! docs/specs/sync.md for the names of devices.

mod common;

use common::*;
use notes_core::sync::remote::DirRemote;
use notes_core::*;

fn at(d: &Device, time: &str) {
    d.set_now_for_tests(&format!("2026-10-05T{time}"));
}

fn append(d: &Device, note: &Note, more: &str) {
    let end = d.get_text(note.id.clone()).unwrap().encode_utf16().count() as u32;
    d.splice_utf16(note.id.clone(), end, 0, more.into()).unwrap();
}

fn clock(version: &Version) -> String {
    chrono::DateTime::from_timestamp_millis(version.time_ms as i64)
        .unwrap()
        .format("%H:%M")
        .to_string()
}

fn cut(text: &str, range: &MarkdownRange) -> String {
    let units: Vec<u16> = text.encode_utf16().collect();
    String::from_utf16(&units[range.start as usize..range.end as usize]).unwrap()
}

#[test]
fn r40_r42_the_scenario_of_the_specification() {
    let a = device();
    let b = device();
    let storage = tempfile::tempdir().unwrap();
    let remote = DirRemote::new(storage.path());
    at(&a, "10:00");
    let note = a
        .create_note(NewNote {
            title: "История".into(),
            text: "первая".into(),
            ..NewNote::default()
        })
        .unwrap();
    at(&a, "10:20");
    append(&a, &note, " вторая");
    // Edits every minute from 11:00 to 13:30.
    for minute in 0..=150 {
        at(&a, &format!("{:02}:{:02}", 11 + minute / 60, minute % 60));
        append(&a, &note, ".");
    }
    at(&a, "14:00");
    let versions = a.versions(note.id.clone()).unwrap();
    let times: Vec<String> = versions.iter().rev().map(clock).collect();
    assert_eq!(times, ["10:00", "10:20", "11:59", "12:59", "13:30"]);
    let v1000 = versions.last().unwrap().clone();
    let v1020 = versions[versions.len() - 2].clone();
    assert_eq!((v1000.added, v1000.removed), (6, 0), "the first text");
    assert_eq!((v1020.added, v1020.removed), (7, 0), "\" вторая\"");
    assert!(v1000.this_device);

    // R41: the version against the current text.
    let diff = a.version_diff(note.id.clone(), v1020.id.clone()).unwrap();
    assert_eq!(diff.text, "первая вторая");
    assert!(
        diff.changed.is_empty(),
        "the version has nothing the current text lacks"
    );
    assert_eq!(diff.removed_at, [13], "the dots came after it");

    // R42: restoring 10:00 gives its text, and 10:20 is still in the history.
    a.restore_version(note.id.clone(), v1000.id.clone()).unwrap();
    assert_eq!(a.get_text(note.id.clone()).unwrap(), "первая");
    at(&a, "14:10");
    let after: Vec<String> = a
        .versions(note.id.clone())
        .unwrap()
        .iter()
        .map(|v| v.id.clone())
        .collect();
    assert!(after.contains(&v1020.id) && after.contains(&v1000.id));
    assert_eq!(after.len(), 6, "the restore is a version of its own");

    // On another device the text is the restored one, and the versions are the same.
    a.sync_with(&remote).unwrap();
    b.sync_with(&remote).unwrap();
    at(&b, "14:10");
    assert_eq!(b.get_text(note.id.clone()).unwrap(), "первая");
    assert_eq!(b.versions(note.id.clone()).unwrap(), {
        let mut on_a = a.versions(note.id.clone()).unwrap();
        for v in &mut on_a {
            v.this_device = false;
        }
        on_a
    });
}

#[test]
fn r40_the_edits_of_the_last_minutes_are_not_a_version_yet() {
    let d = device();
    at(&d, "10:00");
    let note = d
        .create_note(NewNote {
            title: "x".into(),
            text: "a".into(),
            ..NewNote::default()
        })
        .unwrap();
    at(&d, "10:03");
    assert!(d.versions(note.id.clone()).unwrap().is_empty());
    at(&d, "10:05");
    assert_eq!(d.versions(note.id).unwrap().len(), 1);
}

#[test]
fn r41_what_differs_is_marked_both_ways() {
    let d = device();
    at(&d, "10:00");
    let note = d
        .create_note(NewNote {
            title: "x".into(),
            text: "один два три".into(),
            ..NewNote::default()
        })
        .unwrap();
    at(&d, "11:00");
    d.set_text(note.id.clone(), "один 😀 три четыре".into()).unwrap();
    at(&d, "12:00");
    let first = d.versions(note.id.clone()).unwrap().pop().unwrap();
    let diff = d.version_diff(note.id.clone(), first.id).unwrap();
    assert_eq!(diff.text, "один два три");
    let changed: Vec<String> = diff.changed.iter().map(|r| cut(&diff.text, r)).collect();
    assert_eq!(changed.concat().trim(), "два");
    assert!(diff
        .removed_at
        .iter()
        .all(|at| *at as usize <= diff.text.encode_utf16().count()));
    assert!(!diff.removed_at.is_empty());
    assert!(d.version_diff(note.id, "not-a-version".into()).is_err());
}

#[test]
fn r42_a_restore_is_one_step_of_undo_and_refused_for_a_locked_note() {
    let d = device();
    at(&d, "10:00");
    let note = d
        .create_note(NewNote {
            title: "x".into(),
            text: "старый".into(),
            ..NewNote::default()
        })
        .unwrap();
    at(&d, "11:00");
    d.set_text(note.id.clone(), "новый".into()).unwrap();
    at(&d, "12:00");
    let old = d.versions(note.id.clone()).unwrap().pop().unwrap();
    d.restore_version(note.id.clone(), old.id.clone()).unwrap();
    assert_eq!(d.get_text(note.id.clone()).unwrap(), "старый");
    d.undo().unwrap();
    assert_eq!(d.get_text(note.id.clone()).unwrap(), "новый");
    d.set_locked(note.id.clone(), true).unwrap();
    assert!(d.restore_version(note.id.clone(), old.id).is_err());
    assert_eq!(d.get_text(note.id).unwrap(), "новый");
}

#[test]
fn r43_the_title_and_the_flags_make_no_versions() {
    let d = device();
    at(&d, "10:00");
    let note = add(&d, "Заголовок");
    at(&d, "11:00");
    d.set_title(note.id.clone(), "Другой".into()).unwrap();
    d.set_pinned(note.id.clone(), true).unwrap();
    at(&d, "12:00");
    assert_eq!(
        d.versions(note.id).unwrap().len(),
        1,
        "the empty text the note was made with"
    );
}

#[test]
fn s71_a_version_names_the_device_of_its_last_edit() {
    let a = device();
    let b = device();
    let storage = tempfile::tempdir().unwrap();
    let remote = DirRemote::new(storage.path());
    a.set_device_name("Mac".into()).unwrap();
    b.set_device_name("Pixel".into()).unwrap();
    at(&a, "10:00");
    let note = add(&a, "Общая");
    a.sync_with(&remote).unwrap();
    b.sync_with(&remote).unwrap();
    at(&b, "11:00");
    append(&b, &note, "с телефона");
    b.sync_with(&remote).unwrap();
    a.sync_with(&remote).unwrap();
    at(&a, "12:00");
    let versions = a.versions(note.id).unwrap();
    let named: Vec<(String, bool)> = versions.iter().map(|v| (v.device.clone(), v.this_device)).collect();
    assert_eq!(named, [("Pixel".to_string(), false), ("Mac".to_string(), true)]);
}
