//! Nudges between devices: S18–S23 and S27–S30 in docs/specs/sync.md, C22 in docs/specs/caldav.md.

mod common;
#[path = "common/dav.rs"]
mod dav;

use std::sync::{Arc, Mutex};

use common::*;
use notes_core::*;
use tiny_http::{Response, Server};

/// Stands in for a push service: remembers what was posted to each address.
struct Inbox {
    base: String,
    got: Arc<Mutex<Vec<String>>>,
}

impl Inbox {
    fn start() -> Inbox {
        let server = Server::http("127.0.0.1:0").unwrap();
        let base = format!("http://127.0.0.1:{}", server.server_addr().to_ip().unwrap().port());
        let got = Arc::new(Mutex::new(Vec::new()));
        let seen = got.clone();
        std::thread::spawn(move || {
            for mut request in server.incoming_requests() {
                let mut body = String::new();
                request.as_reader().read_to_string(&mut body).unwrap();
                seen.lock()
                    .unwrap()
                    .push(format!("{} {} {body}", request.method(), request.url()));
                request.respond(Response::empty(200)).unwrap();
            }
        });
        Inbox { base, got }
    }

    fn address(&self, name: &str) -> Option<String> {
        Some(format!("{}/{name}", self.base))
    }

    /// What arrived since the last look. Nudges leave after the run (S19): they are waited for first.
    fn take(&self) -> Vec<String> {
        finish_pushes();
        std::mem::take(&mut self.got.lock().unwrap())
    }
}

fn folder(storage: &tempfile::TempDir) -> SyncConfig {
    SyncConfig::Folder {
        path: storage.path().to_string_lossy().into_owned(),
    }
}

/// The scenario every kind of storage has to pass.
fn nudges_follow_uploads(a: &Device, b: &Device, inbox: &Inbox) {
    a.set_push_endpoint(inbox.address("a")).unwrap();
    b.set_push_endpoint(inbox.address("b")).unwrap();
    a.sync_now().unwrap();
    b.sync_now().unwrap();
    a.sync_now().unwrap();
    inbox.take();

    // S19: an upload nudges the others, not the sender.
    let task = add(a, "от A");
    a.sync_now().unwrap();
    assert_eq!(inbox.take(), ["POST /b sync"]);

    // S21: reading is not an upload, and neither is a run with nothing to do.
    b.sync_now().unwrap();
    a.sync_now().unwrap();
    b.sync_now().unwrap();
    assert_eq!(view(b, Scope::Inbox), ["от A"]);
    assert_eq!(inbox.take(), Vec::<String>::new());

    // S18: a device that stops listening is no longer nudged.
    b.set_push_endpoint(None).unwrap();
    b.sync_now().unwrap();
    a.set_title(task.id.clone(), "от A, изменено".into()).unwrap();
    a.sync_now().unwrap();
    assert_eq!(inbox.take(), Vec::<String>::new());

    // S20: an address nobody listens at does not get in the way.
    b.set_push_endpoint(Some("http://127.0.0.1:9/nobody".into())).unwrap();
    b.sync_now().unwrap();
    a.set_title(task.id.clone(), "от A, ещё раз".into()).unwrap();
    assert!(a.sync_now().unwrap().pushed > 0);
    b.sync_now().unwrap();
    assert_eq!(b.task(task.id).unwrap().title, "от A, ещё раз");
}

#[test]
fn s18_s21_through_a_folder() {
    let storage = tempfile::tempdir().unwrap();
    let inbox = Inbox::start();
    let (a, b) = (device(), device());
    a.set_sync_config(folder(&storage)).unwrap();
    b.set_sync_config(folder(&storage)).unwrap();
    nudges_follow_uploads(&a, &b, &inbox);
}

#[test]
fn s34_uploaded_content_nudges_the_others_once_more() {
    let storage = tempfile::tempdir().unwrap();
    let inbox = Inbox::start();
    let (a, b) = (device(), device());
    a.set_sync_config(folder(&storage)).unwrap();
    b.set_sync_config(folder(&storage)).unwrap();
    a.set_push_endpoint(inbox.address("a")).unwrap();
    b.set_push_endpoint(inbox.address("b")).unwrap();
    a.sync_now().unwrap();
    b.sync_now().unwrap();
    a.sync_now().unwrap();
    inbox.take();

    let task = add(&a, "с файлом");
    let src = tempfile::tempdir().unwrap();
    let file = src.path().join("a.txt");
    std::fs::write(&file, b"content").unwrap();
    a.add_attachment(task.id, file.to_string_lossy().into_owned(), None)
        .unwrap();
    a.sync_now().unwrap();
    assert_eq!(inbox.take(), ["POST /b sync"]);
    assert_eq!(a.sync_attachments().unwrap().uploaded, 1);
    assert_eq!(inbox.take(), ["POST /b sync"]);

    // Taking the content and a pass with nothing to move nudge nobody.
    b.sync_now().unwrap();
    assert_eq!(b.sync_attachments().unwrap().downloaded, 1);
    a.sync_attachments().unwrap();
    assert_eq!(inbox.take(), Vec::<String>::new());
}

#[test]
fn s18_record_in_the_storage_follows_the_address() {
    let storage = tempfile::tempdir().unwrap();
    let a = device();
    a.set_sync_config(folder(&storage)).unwrap();
    let record = storage
        .path()
        .join("lists/push")
        .join(format!("{}.json", a.device_id()));

    a.set_push_endpoint(Some("https://push.example.org/one".into()))
        .unwrap();
    a.sync_now().unwrap();
    assert!(std::fs::read_to_string(&record)
        .unwrap()
        .contains("https://push.example.org/one"));
    a.set_push_endpoint(Some("https://push.example.org/two".into()))
        .unwrap();
    a.sync_now().unwrap();
    assert!(std::fs::read_to_string(&record)
        .unwrap()
        .contains("https://push.example.org/two"));
    a.set_push_endpoint(None).unwrap();
    a.sync_now().unwrap();
    assert!(!record.exists());

    assert!(a.set_push_endpoint(Some("file:///etc/passwd".into())).is_err());
    assert_eq!(a.push_endpoint().unwrap(), None);
}

struct Dav {
    url: String,
    root: tempfile::TempDir,
}

fn caldav() -> Dav {
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

fn join(dav: &Dav) -> Device {
    let d = device();
    d.set_sync_config(SyncConfig::CalDav {
        url: dav.url.clone(),
        user: "user".into(),
    })
    .unwrap();
    d.set_sync_password(Some("secret".into()));
    d
}

#[test]
fn c22_through_caldav() {
    let dav = caldav();
    let inbox = Inbox::start();
    nudges_follow_uploads(&join(&dav), &join(&dav), &inbox);
}

#[test]
fn c22_server_that_drops_custom_properties_gets_no_nudges_and_no_repeated_writes() {
    let dav = caldav();
    std::fs::write(dav.root.path().join(".no-custom-props"), "").unwrap();
    let inbox = Inbox::start();
    let (a, b) = (join(&dav), join(&dav));
    a.set_push_endpoint(inbox.address("a")).unwrap();
    b.set_push_endpoint(inbox.address("b")).unwrap();
    for _ in 0..2 {
        a.sync_now().unwrap();
        b.sync_now().unwrap();
    }
    add(&a, "от A");
    a.sync_now().unwrap();
    assert_eq!(inbox.take(), Vec::<String>::new());

    let log = dav.root.path().join(".requests");
    std::fs::write(&log, "").unwrap();
    a.sync_now().unwrap();
    a.sync_now().unwrap();
    let seen = std::fs::read_to_string(&log).unwrap();
    assert!(!seen.contains("PROPPATCH"), "{seen}");
}

/// Stands in for an ntfy server: `POST /<topic>` reaches everyone who holds
/// `GET /<topic>/json` open. It can ask for an access token and can send
/// every request on to another server.
struct Ntfy {
    base: String,
    listeners: Arc<Mutex<Vec<(String, std::net::TcpStream)>>>,
    /// The token the server asks for; `None` lets everyone in.
    token: Arc<Mutex<Option<String>>>,
    /// Where requests are redirected to, when set.
    elsewhere: Arc<Mutex<Option<String>>>,
    /// Method, path and `Authorization` of every request, `-` when there was none.
    seen: Arc<Mutex<Vec<String>>>,
}

impl Ntfy {
    fn start() -> Ntfy {
        use std::io::{BufRead, BufReader, Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let listeners: Arc<Mutex<Vec<(String, std::net::TcpStream)>>> = Arc::default();
        let held = listeners.clone();
        let token: Arc<Mutex<Option<String>>> = Arc::default();
        let elsewhere: Arc<Mutex<Option<String>>> = Arc::default();
        let seen: Arc<Mutex<Vec<String>>> = Arc::default();
        let (asked, away, log) = (token.clone(), elsewhere.clone(), seen.clone());
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let held = held.clone();
                let (asked, away, log) = (asked.clone(), away.clone(), log.clone());
                std::thread::spawn(move || {
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let mut first = String::new();
                    reader.read_line(&mut first).unwrap();
                    let mut length = 0;
                    let mut authorization = "-".to_string();
                    let mut title = String::new();
                    loop {
                        let mut header = String::new();
                        reader.read_line(&mut header).unwrap();
                        if header.trim().is_empty() {
                            break;
                        }
                        if let Some(value) = header.to_ascii_lowercase().strip_prefix("content-length:") {
                            length = value.trim().parse().unwrap();
                        }
                        if header.to_ascii_lowercase().starts_with("authorization:") {
                            authorization = header["authorization:".len()..].trim().to_string();
                        }
                        if header.to_ascii_lowercase().starts_with("x-title:") {
                            title = header["x-title:".len()..].trim().to_string();
                        }
                    }
                    let mut body = vec![0; length];
                    reader.read_exact(&mut body).unwrap();
                    let mut stream = stream;
                    let mut parts = first.split_whitespace();
                    let (method, path) = (parts.next(), parts.next());
                    log.lock().unwrap().push(format!(
                        "{} {} {authorization}",
                        method.unwrap_or_default(),
                        path.unwrap_or_default()
                    ));
                    if let Some(target) = away.lock().unwrap().clone() {
                        let moved = format!(
                            "HTTP/1.1 307 Temporary Redirect\r\nLocation: {target}{}\r\nContent-Length: 0\r\n\r\n",
                            path.unwrap_or_default()
                        );
                        stream.write_all(moved.as_bytes()).unwrap();
                        return;
                    }
                    // Like ntfy: 401 to a token it does not know, 403 to a visitor without one.
                    if let Some(token) = asked.lock().unwrap().clone() {
                        if authorization != format!("Bearer {token}") {
                            let code = if authorization == "-" {
                                "403 Forbidden"
                            } else {
                                "401 Unauthorized"
                            };
                            stream
                                .write_all(format!("HTTP/1.1 {code}\r\nContent-Length: 0\r\n\r\n").as_bytes())
                                .unwrap();
                            return;
                        }
                    }
                    match (method, path) {
                        (Some("GET"), Some(path)) if path.ends_with("/json") => {
                            stream
                                .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/x-ndjson\r\nConnection: close\r\n\r\n{\"event\":\"open\"}\n")
                                .unwrap();
                            let topic = path.trim_end_matches("/json").to_string();
                            held.lock().unwrap().push((topic, stream));
                        }
                        (Some("POST"), Some(path)) => {
                            for (topic, listener) in held.lock().unwrap().iter_mut() {
                                if topic == path {
                                    // Like ntfy: the title of the message is the header it came with.
                                    let event = format!(
                                        "{{\"id\":\"x\",\"event\":\"message\",\"title\":\"{title}\",\"message\":\"sync\"}}\n"
                                    );
                                    let _ = listener.write_all(event.as_bytes());
                                }
                            }
                            stream
                                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
                                .unwrap();
                        }
                        _ => {}
                    }
                });
            }
        });
        Ntfy {
            base,
            listeners,
            token,
            elsewhere,
            seen,
        }
    }

    fn require(&self, token: &str) {
        *self.token.lock().unwrap() = Some(token.into());
    }

    fn redirect_to(&self, other: &Ntfy) {
        *self.elsewhere.lock().unwrap() = Some(other.base.clone());
    }

    /// What arrived since the last look. Nudges leave after the run (S19): they are waited for first.
    fn take(&self) -> Vec<String> {
        finish_pushes();
        std::mem::take(&mut self.seen.lock().unwrap())
    }

    /// Waits until `count` subscriptions are open.
    fn wait_for_listeners(&self, count: usize) {
        for _ in 0..500 {
            if self.listeners.lock().unwrap().len() >= count {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("nobody subscribed");
    }

    fn drop_listeners(&self) {
        for (_, stream) in self.listeners.lock().unwrap().drain(..) {
            let _ = stream.shutdown(std::net::Shutdown::Both);
        }
    }
}

/// Starts the wait on a thread of its own; the answer arrives on the channel.
fn waiting(d: &Device) -> std::sync::mpsc::Receiver<bool> {
    let (tx, rx) = std::sync::mpsc::channel();
    let store = d.store.clone();
    std::thread::spawn(move || {
        let _ = tx.send(store.wait_for_nudge());
    });
    rx
}

const SOON: std::time::Duration = std::time::Duration::from_secs(5);

#[test]
fn s22_an_edit_elsewhere_wakes_the_waiting_device() {
    let storage = tempfile::tempdir().unwrap();
    let ntfy = Ntfy::start();
    let (a, b) = (device(), device());
    for d in [&a, &b] {
        d.set_sync_config(folder(&storage)).unwrap();
        d.set_push_server(Some(format!("{}/", ntfy.base))).unwrap();
        d.sync_now().unwrap();
    }
    let topic = b.push_endpoint().unwrap().unwrap();
    assert!(
        topic.starts_with(&format!("{}/", ntfy.base)) && topic.len() == ntfy.base.len() + 33,
        "{topic}"
    );

    let woken = waiting(&b);
    ntfy.wait_for_listeners(1);
    add(&a, "от A");
    a.sync_now().unwrap();
    assert!(woken.recv_timeout(SOON).unwrap(), "the nudge arrives at once");
    b.sync_now().unwrap();
    assert_eq!(view(&b, Scope::Inbox), ["от A"]);

    // The topic is chosen once: turning the setting off and on keeps the address.
    b.set_push_server(None).unwrap();
    assert_eq!(b.push_endpoint().unwrap(), None);
    b.set_push_server(Some(ntfy.base.clone())).unwrap();
    assert_eq!(b.push_endpoint().unwrap().unwrap(), topic);
}

#[test]
fn s23_a_broken_subscription_counts_as_a_nudge_and_a_missing_server_as_none() {
    let ntfy = Ntfy::start();
    let b = device();
    b.set_push_retry_for_tests(50);
    assert!(!b.wait_for_nudge(), "no server is set");

    b.set_push_server(Some(ntfy.base.clone())).unwrap();
    let woken = waiting(&b);
    ntfy.wait_for_listeners(1);
    ntfy.drop_listeners();
    assert!(woken.recv_timeout(SOON).unwrap());

    b.set_push_server(Some("http://127.0.0.1:9".into())).unwrap();
    assert!(!b.wait_for_nudge(), "nobody answers there");
    assert!(b.set_push_server(Some("ntfy.sh".into())).is_err());
}

const TOKEN: &str = "tk_3gd7d2yftt4b8ixyfe9mnmro88o76";

/// Two devices on one storage, both listening through `ntfy`.
fn pair_on(ntfy: &Ntfy, storage: &tempfile::TempDir) -> (Device, Device) {
    let (a, b) = (device(), device());
    for d in [&a, &b] {
        d.set_push_retry_for_tests(50);
        d.set_sync_config(folder(storage)).unwrap();
        d.set_push_server(Some(ntfy.base.clone())).unwrap();
        d.sync_now().unwrap();
    }
    (a, b)
}

fn path_of(address: Option<String>) -> String {
    let address = address.unwrap();
    address[address.rfind('/').unwrap()..].to_string()
}

#[test]
fn s27_a_token_opens_a_server_that_requires_sign_in() {
    let storage = tempfile::tempdir().unwrap();
    let ntfy = Ntfy::start();
    ntfy.require(TOKEN);
    let (a, b) = pair_on(&ntfy, &storage);
    a.set_push_token(Some(format!(" {TOKEN}\n")));
    b.set_push_token(Some(TOKEN.into()));
    let topic = path_of(b.push_endpoint().unwrap());

    let woken = waiting(&b);
    ntfy.wait_for_listeners(1);
    add(&a, "от A");
    a.sync_now().unwrap();
    assert!(woken.recv_timeout(SOON).unwrap(), "the nudge arrives at once");
    assert_eq!(
        ntfy.take(),
        [
            format!("GET {topic}/json Bearer {TOKEN}"),
            format!("POST {topic} Bearer {TOKEN}")
        ]
    );
    assert!(!a.push_refused() && !b.push_refused());

    // The token is the app's to keep: neither the database nor the storage has it.
    for root in [a.dir(), b.dir(), storage.path()] {
        let mut left = vec![root.to_path_buf()];
        while let Some(dir) = left.pop() {
            for entry in std::fs::read_dir(dir).unwrap().flatten() {
                if entry.path().is_dir() {
                    left.push(entry.path());
                } else {
                    let data = std::fs::read(entry.path()).unwrap();
                    assert!(
                        !data.windows(TOKEN.len()).any(|w| w == TOKEN.as_bytes()),
                        "{:?}",
                        entry.path()
                    );
                }
            }
        }
    }
}

#[test]
fn s28_the_token_goes_to_the_own_server_only() {
    let storage = tempfile::tempdir().unwrap();
    let (home, other) = (Ntfy::start(), Ntfy::start());
    let (a, b, c) = (device(), device(), device());
    for d in [&a, &b, &c] {
        d.set_sync_config(folder(&storage)).unwrap();
    }
    a.set_push_server(Some(home.base.clone())).unwrap();
    a.set_push_token(Some(TOKEN.into()));
    // The same host on another port is another server.
    b.set_push_endpoint(Some(format!("{}/b", other.base))).unwrap();
    // An address that names the own server but leads elsewhere.
    let disguised = format!(
        "http://{}@{}/c",
        home.base.trim_start_matches("http://"),
        other.base.trim_start_matches("http://")
    );
    c.set_push_endpoint(Some(disguised)).unwrap();
    for d in [&b, &c, &a] {
        d.sync_now().unwrap();
    }
    add(&a, "от A");
    a.sync_now().unwrap();
    let mut got = other.take();
    got.sort();
    assert_eq!(got.len(), 2, "{got:?}");
    assert!(
        got[0].starts_with("POST /b -") && got[1].starts_with("POST /c "),
        "{got:?}"
    );
    assert!(!format!("{got:?}").contains(TOKEN), "{got:?}");
    assert_eq!(home.take(), Vec::<String>::new());

    // A redirect does not take the token along, neither from a nudge nor from a subscription.
    b.set_push_endpoint(Some(format!("{}/b", home.base))).unwrap();
    c.set_push_endpoint(None).unwrap();
    b.sync_now().unwrap();
    c.sync_now().unwrap();
    home.redirect_to(&other);
    add(&a, "ещё от A");
    a.sync_now().unwrap();
    a.set_push_retry_for_tests(50);
    let _ = waiting(&a).recv_timeout(SOON);
    let topic = path_of(a.push_endpoint().unwrap());
    let sent = home.take();
    assert!(sent.contains(&format!("POST /b Bearer {TOKEN}")), "{sent:?}");
    assert!(sent.contains(&format!("GET {topic}/json Bearer {TOKEN}")), "{sent:?}");
    let got = other.take();
    assert!(got.contains(&format!("GET {topic}/json -")), "{got:?}");
    assert!(!format!("{got:?}").contains(TOKEN), "{got:?}");
}

#[test]
fn s30_a_refusal_is_reported_until_the_server_lets_the_device_in() {
    let ntfy = Ntfy::start();
    ntfy.require(TOKEN);
    let b = device();
    b.set_push_retry_for_tests(50);
    b.set_push_server(Some(ntfy.base.clone())).unwrap();
    assert!(!b.push_refused(), "nothing was asked yet");

    assert!(!b.wait_for_nudge(), "no token");
    assert!(b.push_refused());
    b.set_push_token(Some("tk_wrong".into()));
    assert!(!b.push_refused(), "a changed setting has not been tried yet");
    assert!(!b.wait_for_nudge(), "a token the server does not know");
    assert!(b.push_refused());

    // The server starts to accept the token: the next attempt clears the report.
    ntfy.require("tk_wrong");
    let woken = waiting(&b);
    ntfy.wait_for_listeners(1);
    assert!(!b.push_refused());
    ntfy.drop_listeners();
    assert!(woken.recv_timeout(SOON).unwrap());

    // A server that is away says nothing about the token.
    b.set_push_server(Some("http://127.0.0.1:9".into())).unwrap();
    assert!(!b.wait_for_nudge());
    assert!(!b.push_refused());
}

#[test]
fn s29_a_device_that_only_sends_names_its_server() {
    let storage = tempfile::tempdir().unwrap();
    let ntfy = Ntfy::start();
    ntfy.require(TOKEN);
    let (phone, b) = (device(), device());
    for d in [&phone, &b] {
        d.set_sync_config(folder(&storage)).unwrap();
    }
    b.set_push_endpoint(Some(format!("{}/b", ntfy.base))).unwrap();
    b.sync_now().unwrap();
    phone.sync_now().unwrap();

    // Without a server of its own the device has nowhere to show the token.
    phone.set_push_token(Some(TOKEN.into()));
    add(&phone, "с телефона");
    phone.sync_now().unwrap();
    assert_eq!(ntfy.take(), ["POST /b -"]);
    assert!(!phone.push_refused());

    assert!(phone.set_push_send_server(Some("ntfy.example.org".into())).is_err());
    phone.set_push_send_server(Some(format!("{}/", ntfy.base))).unwrap();
    assert_eq!(phone.push_send_server().unwrap(), Some(ntfy.base.clone()));
    assert_eq!(phone.push_endpoint().unwrap(), None, "receiving is not touched");
    add(&phone, "ещё с телефона");
    phone.sync_now().unwrap();
    assert_eq!(ntfy.take(), [format!("POST /b Bearer {TOKEN}")]);
    assert!(!phone.push_refused());

    // S30: the refusal of a nudge is reported, and the sync run is not hurt by it.
    phone.set_push_token(Some("tk_wrong".into()));
    add(&phone, "и ещё");
    assert!(phone.sync_now().unwrap().pushed > 0);
    // S19: the nudge leaves after the run, and so does what its answer says.
    finish_pushes();
    assert!(phone.push_refused());
    phone.set_push_token(Some(TOKEN.into()));
    add(&phone, "последняя");
    phone.sync_now().unwrap();
    finish_pushes();
    assert!(!phone.push_refused());

    phone.set_push_send_server(None).unwrap();
    assert_eq!(phone.push_send_server().unwrap(), None);
}

/// A push service that takes a second to answer; remembers when each request came.
fn slow_inbox() -> (String, Arc<Mutex<Vec<std::time::Instant>>>) {
    let server = Arc::new(Server::http("127.0.0.1:0").unwrap());
    let base = format!("http://127.0.0.1:{}", server.server_addr().to_ip().unwrap().port());
    let came: Arc<Mutex<Vec<std::time::Instant>>> = Arc::default();
    let seen = came.clone();
    std::thread::spawn(move || {
        for request in server.incoming_requests() {
            seen.lock().unwrap().push(std::time::Instant::now());
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_secs(1));
                let _ = request.respond(Response::empty(200));
            });
        }
    });
    (base, came)
}

#[test]
fn s19_nudges_leave_together_and_the_run_does_not_wait_for_them() {
    let storage = tempfile::tempdir().unwrap();
    let (base, came) = slow_inbox();
    let (a, b, c) = (device(), device(), device());
    for d in [&a, &b, &c] {
        d.set_sync_config(folder(&storage)).unwrap();
    }
    b.set_push_endpoint(Some(format!("{base}/b"))).unwrap();
    c.set_push_endpoint(Some(format!("{base}/c"))).unwrap();
    b.sync_now().unwrap();
    c.sync_now().unwrap();

    add(&a, "от A");
    let started = std::time::Instant::now();
    assert!(a.sync_now().unwrap().pushed > 0);
    let run = started.elapsed();
    assert!(
        run < std::time::Duration::from_millis(900),
        "the run waited for an answer: {run:?}"
    );

    finish_pushes();
    let whole = started.elapsed();
    let came = came.lock().unwrap().clone();
    assert_eq!(came.len(), 2);
    for moment in &came {
        let after = moment.duration_since(started);
        assert!(
            after < std::time::Duration::from_millis(900),
            "a request waited for the other: {after:?}"
        );
    }
    assert!(
        whole >= std::time::Duration::from_secs(1),
        "the answers were not waited for: {whole:?}"
    );
    assert!(
        whole < std::time::Duration::from_millis(1900),
        "one after the other: {whole:?}"
    );
}

/// Two devices that take nudges through `ntfy`; returns them with the addresses they first made up.
fn sharers(ntfy: &Ntfy, config: impl Fn(&Device)) -> (Device, Device, String, String) {
    let (a, b) = (device(), device());
    for d in [&a, &b] {
        d.set_push_retry_for_tests(50);
        config(d);
        d.set_push_server(Some(ntfy.base.clone())).unwrap();
    }
    let (first, second) = (a.push_endpoint().unwrap().unwrap(), b.push_endpoint().unwrap().unwrap());
    assert_ne!(first, second, "each makes up a topic of its own");
    a.sync_now().unwrap();
    b.sync_now().unwrap();
    // Whichever of the two has to move does so on a run that reads the addresses.
    add(&a, "от A");
    a.sync_now().unwrap();
    b.sync_now().unwrap();
    finish_pushes();
    (a, b, first, second)
}

#[test]
fn s41_devices_on_one_server_come_to_one_topic() {
    let storage = tempfile::tempdir().unwrap();
    let ntfy = Ntfy::start();
    let (a, b, first, second) = sharers(&ntfy, |d| d.set_sync_config(folder(&storage)).unwrap());
    let shared = first.clone().min(second.clone());
    assert_eq!(a.push_endpoint().unwrap().unwrap(), shared);
    assert_eq!(b.push_endpoint().unwrap().unwrap(), shared);

    // One request reaches the topic; the other device wakes, the sender passes its own nudge by.
    let (at_a, at_b) = (waiting(&a), waiting(&b));
    ntfy.wait_for_listeners(2);
    ntfy.take();
    add(&a, "ещё от A");
    a.sync_now().unwrap();
    assert_eq!(ntfy.take(), [format!("POST {} -", path_of(Some(shared.clone())))]);
    assert!(at_b.recv_timeout(SOON).unwrap(), "the other device is woken");
    assert!(
        at_a.recv_timeout(std::time::Duration::from_millis(400)).is_err(),
        "the sender woke itself"
    );

    // The shared topic outlives the setting being turned off and on.
    b.set_push_server(None).unwrap();
    b.set_push_server(Some(ntfy.base.clone())).unwrap();
    assert_eq!(b.push_endpoint().unwrap().unwrap(), shared);
}

#[test]
fn s41_an_address_of_another_kind_keeps_to_itself() {
    let storage = tempfile::tempdir().unwrap();
    let ntfy = Ntfy::start();
    let (a, b, first, second) = sharers(&ntfy, |d| d.set_sync_config(folder(&storage)).unwrap());
    let shared = first.min(second);
    ntfy.take();

    // The way a phone is addressed: on the same server, and not a topic of this app.
    let phone = device();
    phone.set_sync_config(folder(&storage)).unwrap();
    let given = format!("{}/upAbCdEf123456?up=1", ntfy.base);
    phone.set_push_endpoint(Some(given.clone())).unwrap();
    phone.sync_now().unwrap();
    add(&phone, "с телефона");
    phone.sync_now().unwrap();
    assert_eq!(phone.push_endpoint().unwrap().unwrap(), given);
    assert_eq!(
        ntfy.take(),
        [format!("POST {} -", path_of(Some(shared.clone())))],
        "one request for both"
    );

    b.sync_now().unwrap();
    add(&b, "от B");
    b.sync_now().unwrap();
    let mut asked = ntfy.take();
    asked.sort();
    let mut expected = [
        format!("POST {} -", path_of(Some(shared.clone()))),
        "POST /upAbCdEf123456?up=1 -".to_string(),
    ];
    expected.sort();
    assert_eq!(asked, expected);
    assert_eq!(a.push_endpoint().unwrap().unwrap(), shared);
    assert_eq!(b.push_endpoint().unwrap().unwrap(), shared);
}

#[test]
fn s41_a_device_on_another_server_keeps_its_topic() {
    let storage = tempfile::tempdir().unwrap();
    let (here, there) = (Ntfy::start(), Ntfy::start());
    let (a, b) = (device(), device());
    a.set_sync_config(folder(&storage)).unwrap();
    b.set_sync_config(folder(&storage)).unwrap();
    a.set_push_server(Some(here.base.clone())).unwrap();
    b.set_push_server(Some(there.base.clone())).unwrap();
    let (first, second) = (a.push_endpoint().unwrap().unwrap(), b.push_endpoint().unwrap().unwrap());
    for d in [&a, &b, &a, &b] {
        add(d, "правка");
        d.sync_now().unwrap();
    }
    finish_pushes();
    assert_eq!(a.push_endpoint().unwrap().unwrap(), first);
    assert_eq!(b.push_endpoint().unwrap().unwrap(), second);
}

#[test]
fn s41_through_caldav() {
    let dav = caldav();
    let ntfy = Ntfy::start();
    let (a, b, first, second) = sharers(&ntfy, |d| {
        d.set_sync_config(SyncConfig::CalDav {
            url: dav.url.clone(),
            user: "user".into(),
        })
        .unwrap();
        d.set_sync_password(Some("secret".into()));
    });
    let shared = first.min(second);
    assert_eq!(a.push_endpoint().unwrap().unwrap(), shared);
    assert_eq!(b.push_endpoint().unwrap().unwrap(), shared);

    ntfy.take();
    add(&b, "от B");
    b.sync_now().unwrap();
    assert_eq!(ntfy.take(), [format!("POST {} -", path_of(Some(shared)))]);
}
