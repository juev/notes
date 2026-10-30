//! Sync over HTTP against the minimal WebDAV server from `common/dav.rs`.

mod common;
#[path = "common/dav.rs"]
mod dav;

use common::*;
use notes_core::*;
use tiny_http::Server;

struct Dav {
    url: String,
    root: tempfile::TempDir,
}

fn start() -> Dav {
    let server = Server::http("127.0.0.1:0").unwrap();
    let port = server.server_addr().to_ip().unwrap().port();
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().to_path_buf();
    // The account exists on the server; the collection the user points at does not yet.
    std::fs::create_dir(dir.join("dav")).unwrap();
    std::thread::spawn(move || dav::serve(&server, &dir));
    Dav {
        url: format!("http://127.0.0.1:{port}/dav/user"),
        root,
    }
}

impl Dav {
    /// Requests the server received while `run` was going.
    fn requests(&self, run: impl FnOnce()) -> Vec<String> {
        let log = self.root.path().join(".requests");
        std::fs::write(&log, "").unwrap();
        run();
        let seen = std::fs::read_to_string(&log).unwrap();
        std::fs::remove_file(&log).unwrap();
        seen.lines().map(str::to_string).collect()
    }

    fn storage(&self) -> std::path::PathBuf {
        self.root.path().join("dav/user/notes")
    }
}

fn connect(d: &Device, dav: &Dav, password: &str) {
    d.set_sync_config(SyncConfig::WebDav {
        url: dav.url.clone(),
        user: "user".into(),
    })
    .unwrap();
    d.set_sync_password(Some(password.into()));
}

#[test]
fn two_devices_converge_through_webdav() {
    let dav = start();
    let (a, b) = (device(), device());
    connect(&a, &dav, "secret");
    connect(&b, &dav, "secret");

    let t = add(&a, "через WebDAV");
    let src = tempfile::tempdir().unwrap();
    let file = src.path().join("note.txt");
    std::fs::write(&file, "вложение").unwrap();
    a.add_attachment(t.id.clone(), file.to_string_lossy().into_owned(), None)
        .unwrap();

    let up = a.sync_now().unwrap();
    assert!(up.pushed > 0);
    assert_eq!(a.sync_attachments().unwrap().uploaded, 1);
    // The client created the collections itself, starting from the missing base.
    assert!(dav.root.path().join("dav/user/notes/vault.json").is_file());

    let down = b.sync_now().unwrap();
    assert!(down.pulled > 0);
    assert_eq!(b.sync_attachments().unwrap().downloaded, 1);
    assert_eq!(oldest_first(&b), ["через WebDAV"]);
    let files = b.attachments(t.id.clone()).unwrap();
    assert_eq!(
        std::fs::read_to_string(files[0].local_path.as_ref().unwrap()).unwrap(),
        "вложение"
    );

    b.set_starred(t.id.clone(), true).unwrap();
    a.set_title(t.id.clone(), "переименовано".into()).unwrap();
    b.sync_now().unwrap();
    a.sync_now().unwrap();
    b.sync_now().unwrap();
    for d in [&a, &b] {
        let got = d.get_note(t.id.clone()).unwrap();
        assert_eq!((got.title.as_str(), got.starred), ("переименовано", true));
    }
    assert_eq!(a.sync_now().unwrap(), SyncReport::default());
}

#[test]
fn compaction_works_over_webdav() {
    let dav = start();
    let (a, b) = (device(), device());
    connect(&a, &dav, "secret");
    connect(&b, &dav, "secret");
    a.set_compact_after_for_tests(2);
    // The first run after connecting publishes a snapshot; these come on top of it.
    for i in 0..5 {
        add(&a, &format!("t{i}"));
        a.sync_now().unwrap();
    }
    let logs = std::fs::read_dir(dav.root.path().join("dav/user/notes/log"))
        .unwrap()
        .flatten()
        .filter(|f| f.file_name().to_string_lossy().ends_with(".jsonl"))
        .count();
    assert!(logs <= 2, "old log files were deleted over HTTP, {logs} left");
    b.sync_now().unwrap();
    assert_eq!(oldest_first(&b).len(), 5);
}

#[test]
fn wrong_password_is_reported_and_loses_nothing() {
    let dav = start();
    let a = device();
    connect(&a, &dav, "wrong");
    add(&a, "ждёт");
    let err = a.sync_now().unwrap_err().to_string();
    assert!(err.contains("401") && err.contains("password"), "{err}");
    assert!(a.sync_status().unwrap().pending > 0);

    connect(&a, &dav, "secret");
    assert!(a.sync_now().unwrap().pushed > 0);
}

fn check(url: &str, password: &str) -> Result<ConnectionCheck> {
    check_sync_connection(
        SyncConfig::WebDav {
            url: url.into(),
            user: "user".into(),
        },
        password.into(),
    )
}

// S46
#[test]
fn s46_connection_test_tells_a_ready_storage_from_one_the_first_sync_creates() {
    let dav = start();
    let seen = dav.requests(|| assert_eq!(check(&dav.url, "secret").unwrap(), ConnectionCheck::WillCreate));
    assert!(seen.iter().all(|r| r.starts_with("PROPFIND ")), "{seen:?}");
    assert!(!dav.root.path().join("dav/user").exists());

    std::fs::create_dir(dav.root.path().join("dav/user")).unwrap();
    let seen = dav.requests(|| assert_eq!(check(&dav.url, "secret").unwrap(), ConnectionCheck::Ready));
    assert_eq!(seen, ["PROPFIND /dav/user/ depth=0"]);
}

// S46
#[test]
fn s46_connection_test_names_what_is_wrong() {
    let dav = start();
    let err = check(&dav.url, "wrong").unwrap_err().to_string();
    assert!(err.contains("401") && err.contains("password"), "{err}");

    let err = check(&format!("{}/deeper/still", dav.url), "secret")
        .unwrap_err()
        .to_string();
    assert!(err.contains("nothing is found"), "{err}");

    // A server that knows nothing of WebDAV answers every method the same way.
    let plain = Server::http("127.0.0.1:0").unwrap();
    let port = plain.server_addr().to_ip().unwrap().port();
    std::thread::spawn(move || {
        for request in plain.incoming_requests() {
            request
                .respond(tiny_http::Response::from_string("<html></html>"))
                .unwrap();
        }
    });
    let err = check(&format!("http://127.0.0.1:{port}/files"), "secret")
        .unwrap_err()
        .to_string();
    assert!(err.contains("does not answer as WebDAV"), "{err}");

    // Nobody listens on a port that was just given back.
    let free = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let err = check(&format!("http://127.0.0.1:{free}/files"), "secret")
        .unwrap_err()
        .to_string();
    assert!(err.contains("PROPFIND"), "{err}");

    assert!(check("ftp://example.org", "secret").is_err());
}

// S46
#[test]
fn s46_connection_test_leaves_the_settings_of_the_device_alone() {
    let dav = start();
    let a = device();
    connect(&a, &dav, "secret");
    add(&a, "ждёт");
    let pending = a.sync_status().unwrap().pending;

    assert!(check(&dav.url, "wrong").is_err());
    assert_eq!(check(&dav.url, "secret").unwrap(), ConnectionCheck::WillCreate);

    assert_eq!(a.sync_status().unwrap().pending, pending);
    assert!(!dav.storage().exists());
    // The password the device holds is still the one it was given.
    assert!(a.sync_now().unwrap().pushed > 0);
}

#[test]
fn address_must_be_http() {
    let a = device();
    let bad = SyncConfig::WebDav {
        url: "ftp://example.org".into(),
        user: String::new(),
    };
    assert!(a.set_sync_config(bad).is_err());
    assert_eq!(a.sync_config().unwrap(), SyncConfig::Off);
}

#[test]
fn password_is_kept_out_of_the_database_and_asked_for_again_after_restart() {
    let dav = start();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().to_string_lossy().into_owned();
    {
        let store = Store::open(path.clone()).unwrap();
        store
            .set_sync_config(SyncConfig::WebDav {
                url: dav.url.clone(),
                user: "user".into(),
            })
            .unwrap();
        store.set_sync_password(Some("secret".into()));
        store
            .create_note(NewNote {
                title: "задача".into(),
                ..NewNote::default()
            })
            .unwrap();
        assert!(store.sync_now().unwrap().pushed > 0);
    }
    for file in std::fs::read_dir(dir.path())
        .unwrap()
        .flatten()
        .filter(|f| f.path().is_file())
    {
        let bytes = std::fs::read(file.path()).unwrap();
        assert!(
            !bytes.windows(6).any(|w| w == b"secret"),
            "password found in {:?}",
            file.file_name()
        );
    }

    // A new process knows where to sync but not the password.
    let store = Store::open(path).unwrap();
    assert!(matches!(store.sync_config().unwrap(), SyncConfig::WebDav { .. }));
    let err = store.sync_now().unwrap_err().to_string();
    assert!(err.contains("password is not available"), "{err}");
    store.set_sync_password(Some("secret".into()));
    store.sync_now().unwrap();
}

const LOG: &str = "PROPFIND /dav/user/notes/log/ depth=1";
const SNAP: &str = "PROPFIND /dav/user/notes/snap/ depth=1";

/// A and B in step; A compacts after every file it writes.
fn in_step(dav: &Dav) -> (Device, Device) {
    let (a, b) = (device(), device());
    connect(&a, dav, "secret");
    connect(&b, dav, "secret");
    add(&a, "t1");
    a.sync_now().unwrap();
    b.sync_now().unwrap();
    a.sync_now().unwrap();
    (a, b)
}

#[test]
fn s17_idle_run_is_one_request() {
    let dav = start();
    let (a, b) = in_step(&dav);
    for d in [&a, &b] {
        let seen = dav.requests(|| {
            assert_eq!(d.sync_now().unwrap(), SyncReport::default());
        });
        assert_eq!(seen, [LOG]);
    }
}

#[test]
fn s17_device_left_behind_by_compaction_catches_up_without_listing_snapshots() {
    let dav = start();
    let (a, b) = in_step(&dav);
    a.set_compact_after_for_tests(0);
    for title in ["t2", "t3"] {
        add(&a, title);
        a.sync_now().unwrap();
    }
    let seen = dav.requests(|| {
        b.sync_now().unwrap();
    });
    assert!(
        seen.iter().any(|r| r.starts_with("GET /dav/user/notes/snap/")),
        "{seen:?}"
    );
    assert!(!seen.contains(&SNAP.to_string()), "{seen:?}");
    assert_eq!(oldest_first(&b).len(), 3);
}

#[test]
fn s17_snapshot_written_without_a_mark_is_found_by_listing() {
    let dav = start();
    let (a, b) = in_step(&dav);
    a.set_compact_after_for_tests(0);
    add(&a, "t2");
    a.sync_now().unwrap();
    // What a version without marks leaves behind: nothing of A in the log.
    for entry in std::fs::read_dir(dav.storage().join("log")).unwrap().flatten() {
        if entry.file_name().to_string_lossy().starts_with(&a.device_id()) {
            std::fs::remove_file(entry.path()).unwrap();
        }
    }
    let seen = dav.requests(|| {
        b.sync_now().unwrap();
    });
    assert!(seen.contains(&SNAP.to_string()), "{seen:?}");
    assert_eq!(oldest_first(&b).len(), 2);
}

/// A WebDAV server that, like one written in Go, refuses a PUT into a missing
/// collection without reading the body and closes the connection (#250). Go
/// sends a 409 first, but the client is still writing and sees only the broken
/// connection; this server sends nothing, so the client sees the same whatever
/// the socket buffers hold. With `drop_puts` every PUT is closed that way.
struct EarlyRefusal {
    url: String,
    requests: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
}

fn early_refusal(folders: &[&str], drop_puts: bool) -> EarlyRefusal {
    use std::io::{BufRead, BufReader, Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let log = requests.clone();
    let mut folders: std::collections::HashSet<String> = folders.iter().map(|f| f.to_string()).collect();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let mut stream = stream.unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut first = String::new();
            reader.read_line(&mut first).unwrap();
            let mut length = 0;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line.trim().is_empty() {
                    break;
                }
                if let Some((name, value)) = line.split_once(':') {
                    if name.eq_ignore_ascii_case("content-length") {
                        length = value.trim().parse().unwrap();
                    }
                }
            }
            let mut parts = first.split_whitespace();
            let (method, path) = (parts.next().unwrap().to_string(), parts.next().unwrap().to_string());
            log.lock().unwrap().push(format!("{method} {path}"));
            let parent = format!("{}/", path.rsplit_once('/').unwrap().0);
            let refuse_early = method == "PUT" && (drop_puts || !folders.contains(&parent));
            if !refuse_early {
                reader.by_ref().take(length).read_to_end(&mut Vec::new()).unwrap();
            }
            let answer = match method.as_str() {
                "PUT" if refuse_early => None,
                "PUT" => Some("201 Created"),
                "MKCOL" if folders.contains(&path) => Some("405 Method Not Allowed"),
                "MKCOL" => {
                    folders.insert(path);
                    Some("201 Created")
                }
                "PROPFIND" if folders.contains(&path) => Some("207 Multi-Status"),
                _ => Some("404 Not Found"),
            };
            if let Some(answer) = answer {
                let _ = write!(
                    stream,
                    "HTTP/1.1 {answer}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                );
            }
        }
    });
    EarlyRefusal {
        url: format!("http://127.0.0.1:{port}/dav"),
        requests,
    }
}

impl EarlyRefusal {
    fn remote(&self) -> notes_core::sync::webdav::WebDavRemote {
        notes_core::sync::webdav::WebDavRemote::new(&self.url, "user", "secret").unwrap()
    }

    fn requests(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }
}

/// The size of the attachment from #250, well above what a Go server drains unread.
const BLOB: usize = 1_089_495;

#[test]
fn s18_put_creates_the_missing_folder_when_the_server_refuses_without_reading_the_body() {
    use notes_core::sync::remote::Remote;
    let server = early_refusal(&["/dav/", "/dav/blobs/"], false);
    server.remote().put("blobs/c2/blob", &vec![7; BLOB]).unwrap();
    assert_eq!(
        server.requests(),
        [
            "PUT /dav/blobs/c2/blob",
            "PROPFIND /dav/blobs/c2/",
            "MKCOL /dav/",
            "MKCOL /dav/blobs/",
            "MKCOL /dav/blobs/c2/",
            "PUT /dav/blobs/c2/blob",
        ]
    );
}

#[test]
fn s18_a_broken_connection_into_an_existing_folder_stays_an_error() {
    use notes_core::sync::remote::Remote;
    let server = early_refusal(&["/dav/", "/dav/blobs/", "/dav/blobs/c2/"], true);
    let error = server.remote().put("blobs/c2/blob", &vec![7; BLOB]).unwrap_err();
    assert!(error.to_string().contains("PUT"), "{error}");
    assert_eq!(server.requests(), ["PUT /dav/blobs/c2/blob", "PROPFIND /dav/blobs/c2/"]);
}

#[test]
fn s18_put_creates_the_missing_folder_after_a_409_without_asking_for_it() {
    use notes_core::sync::remote::Remote;
    let dav = start();
    let remote = notes_core::sync::webdav::WebDavRemote::new(&dav.url, "user", "secret").unwrap();
    let seen = dav.requests(|| remote.put("blobs/c2/blob", &vec![7; BLOB]).unwrap());
    assert_eq!(
        seen,
        [
            "PUT /dav/user/blobs/c2/blob",
            "MKCOL /dav/user/",
            "MKCOL /dav/user/blobs/",
            "MKCOL /dav/user/blobs/c2/",
            "PUT /dav/user/blobs/c2/blob",
        ]
    );
}

/// S28 over HTTP: the storage is encrypted from the settings, the other
/// device enters the passphrase, and nothing on the server reads in the clear.
#[test]
fn s28_s30_two_devices_converge_through_an_encrypted_webdav_storage() {
    let dav = start();
    let (a, b) = (device(), device());
    connect(&a, &dav, "secret");
    connect(&b, &dav, "secret");
    let t = add(&a, "зашифровано на сервере");
    let src = tempfile::tempdir().unwrap();
    let file = src.path().join("plan.bin");
    let bytes: Vec<u8> = (0..150_000u32).map(|i| (i % 253) as u8).collect();
    std::fs::write(&file, &bytes).unwrap();
    let attached = a
        .add_attachment(t.id.clone(), file.to_string_lossy().into_owned(), None)
        .unwrap();
    a.sync_now().unwrap();
    a.sync_attachments().unwrap();
    b.sync_now().unwrap();

    a.encrypt_storage("фраза-пароль".into()).unwrap();
    assert!(b.sync_now().unwrap_err().to_string().contains("enter its passphrase"));
    b.set_sync_passphrase(Some("фраза-пароль".into()));
    b.sync_now().unwrap();
    assert_eq!(b.sync_attachments().unwrap().downloaded, 1);
    let got = b.attachments(t.id.clone()).unwrap();
    assert_eq!(std::fs::read(got[0].local_path.as_ref().unwrap()).unwrap(), bytes);
    let u = add(&b, "ответ с B");
    b.sync_now().unwrap();
    a.sync_now().unwrap();
    assert_eq!(a.get_note(u.id.clone()).unwrap().title, "ответ с B");

    let mut stack = vec![dav.storage()];
    let mut seen = 0;
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            assert!(!name.contains(&attached.sha256), "{name}");
            let data = std::fs::read(&path).unwrap();
            // `vault.json` (here and the one that says the storage moved, S49 of Lists), the empty marks
            // and what the server keeps next to the files.
            if name == "vault.json" || name.ends_with(".snap") || name.starts_with('.') {
                continue;
            }
            assert!(data.starts_with(b"NSE1"), "{} is not sealed", path.display());
            seen += 1;
        }
    }
    assert!(seen >= 3, "logs, a snapshot and the content");
}

/// Every file the server holds under the storage, with its bytes, a snapshot unpacked.
fn server_files(dav: &Dav) -> Vec<(String, Vec<u8>)> {
    let mut found = Vec::new();
    let mut stack = vec![dav.storage()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let mut data = std::fs::read(&path).unwrap();
            // A snapshot is compressed; a sealed file is not.
            if data.starts_with(&[0x1f, 0x8b]) {
                let mut out = Vec::new();
                std::io::Read::read_to_end(&mut flate2::read::GzDecoder::new(&data[..]), &mut out).unwrap();
                data = out;
            }
            found.push((path.to_string_lossy().into_owned(), data));
        }
    }
    found
}

fn holds(data: &[u8], needle: &[u8]) -> bool {
    data.windows(needle.len()).any(|w| w == needle)
}

/// S3, S20, S28: the text of a note and an attachment of 200 000 bytes go through WebDAV, in the
/// clear and then sealed; the server never holds the title, the text or the hash once sealed.
#[test]
fn s3_s28_a_note_with_text_and_a_large_attachment_converges_through_webdav_clear_and_sealed() {
    let dav = start();
    let (a, b) = (device(), device());
    connect(&a, &dav, "secret");
    connect(&b, &dav, "secret");
    let title = "заголовок-для-сервера-5521";
    let secret = "текст заметки для сервера 7710";
    let t = a
        .create_note(NewNote {
            title: title.into(),
            text: secret.into(),
            ..NewNote::default()
        })
        .unwrap();
    let src = tempfile::tempdir().unwrap();
    let file = src.path().join("plan.bin");
    let bytes: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
    std::fs::write(&file, &bytes).unwrap();
    let attached = a
        .add_attachment(t.id.clone(), file.to_string_lossy().into_owned(), None)
        .unwrap();
    a.sync_now().unwrap();
    a.sync_attachments().unwrap();
    b.sync_now().unwrap();
    b.sync_attachments().unwrap();
    assert_eq!(b.get_text(t.id.clone()).unwrap(), secret);
    let got = b.attachments(t.id.clone()).unwrap();
    assert_eq!(std::fs::read(got[0].local_path.as_ref().unwrap()).unwrap(), bytes);

    // Edits at the two ends while apart merge through the server.
    a.splice_utf16(t.id.clone(), 0, 0, "A: ".into()).unwrap();
    let end = secret.encode_utf16().count() as u32;
    b.splice_utf16(t.id.clone(), end, 0, " :B".into()).unwrap();
    a.sync_now().unwrap();
    b.sync_now().unwrap();
    a.sync_now().unwrap();
    let merged = format!("A: {secret} :B");
    assert_eq!(a.get_text(t.id.clone()).unwrap(), merged);
    assert_eq!(b.get_text(t.id.clone()).unwrap(), merged);

    // In the clear the server holds the title in the registers.
    assert!(server_files(&dav).iter().any(|(_, d)| holds(d, title.as_bytes())));

    a.encrypt_storage("фраза-пароль".into()).unwrap();
    b.set_sync_passphrase(Some("фраза-пароль".into()));
    b.sync_now().unwrap();
    b.splice_utf16(t.id.clone(), 0, 0, "после шифрования ".into()).unwrap();
    b.sync_now().unwrap();
    a.sync_now().unwrap();
    assert_eq!(a.get_text(t.id.clone()).unwrap(), format!("после шифрования {merged}"));
    let c = device();
    connect(&c, &dav, "secret");
    c.set_sync_passphrase(Some("фраза-пароль".into()));
    c.sync_now().unwrap();
    assert_eq!(c.get_text(t.id.clone()).unwrap(), format!("после шифрования {merged}"));

    let files = server_files(&dav);
    assert!(files.len() >= 3);
    for (path, data) in &files {
        for secret in [title, "текст заметки для сервера", "после шифрования", &attached.sha256]
        {
            assert!(!path.contains(secret), "{path}");
            assert!(!holds(data, secret.as_bytes()), "{path} holds {secret}");
        }
        // Whatever holds the texts is sealed: `vault.json`, the empty marks and the server's own files apart.
        let name = path.rsplit('/').next().unwrap();
        if name == "vault.json" || name.ends_with(".snap") || name.starts_with('.') {
            continue;
        }
        assert!(data.starts_with(b"NSE1"), "{path} is not sealed");
    }
}
