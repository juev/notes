//! A minimal WebDAV and CalDAV server over a folder, for tests and for trying
//! sync by hand (`cargo run --example webdav -- <folder>`).
//!
//! It is stricter than most real servers where it matters to the client: a PUT
//! into a missing collection is refused (409), Basic auth is required, and
//! conditional requests are honoured. A calendar is a folder with a `.props`
//! file; a file named `.no-custom-props` in the served root makes the server
//! drop properties it does not know, as some real servers do. A file named
//! `.race` stages a concurrent write (see `handle`). When a file named
//! `.requests` exists in the served root, every authorised request is appended
//! to it as `METHOD /path` (a PROPFIND also carries its depth, a REPORT its
//! kind). `.home` holds the path the server names as the calendar home; `.fail`
//! holds a path, and every request under it is answered with 500.
//!
//! Calendars carry a change tag and a sync token, and answer the
//! `sync-collection` and `calendar-multiget` reports. `.no-ctag`, `.no-sync`
//! and `.no-report` in the served root turn off the tag, the token with its
//! report, and both reports. `.no-move` makes the server refuse MOVE with
//! 405, as a server without it would. `.move-403` makes it answer a MOVE into
//! a missing folder with 403 instead of 409, as SFTPGo does (#304).

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::path::Path;

use tiny_http::{Header, Method, Request, Response, Server};

pub const AUTH: &str = "Basic dXNlcjpzZWNyZXQ="; // user:secret

/// Serves `root` until the process exits.
pub fn serve(server: &Server, root: &Path) {
    for request in server.incoming_requests() {
        handle(request, root);
    }
}

pub fn etag(data: &[u8]) -> String {
    let mut hasher = DefaultHasher::new();
    data.hash(&mut hasher);
    format!("\"{:016x}\"", hasher.finish())
}

fn header(request: &Request, name: &'static str) -> Option<String> {
    request
        .headers()
        .iter()
        .find(|h| h.field.equiv(name))
        .map(|h| h.value.as_str().to_string())
}

fn with_etag(response: Response<std::io::Cursor<Vec<u8>>>, tag: &str) -> Response<std::io::Cursor<Vec<u8>>> {
    response.with_header(Header::from_bytes("ETag", tag).unwrap())
}

/// Text of the first element whose tag ends in `name>`, XML-unescaped.
fn element(xml: &str, name: &str) -> Option<String> {
    let open = xml.find(&format!("{name}>"))? + name.len() + 1;
    let close = xml[open..].find("</")? + open;
    Some(
        xml[open..close]
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&amp;", "&"),
    )
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn read_props(dir: &Path) -> Vec<(String, String)> {
    std::fs::read_to_string(dir.join(".props"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.split_once('=').map(|(k, v)| (k.to_string(), v.to_string())))
        .collect()
}

fn write_props(dir: &Path, root: &Path, body: &str) {
    let mut props = read_props(dir);
    let keep_custom = !root.join(".no-custom-props").exists();
    for (key, tag) in [
        ("name", "displayname"),
        ("color", "calendar-color"),
        ("state", ":state"),
        ("filters", ":filters"),
        ("push", ":push"),
    ] {
        if matches!(key, "state" | "filters" | "push") && !keep_custom {
            continue;
        }
        if let Some(value) = element(body, tag) {
            props.retain(|(k, _)| k != key);
            props.push((key.to_string(), value));
        }
    }
    let text: String = props.iter().map(|(k, v)| format!("{k}={v}\n")).collect();
    std::fs::write(dir.join(".props"), text).unwrap();
}

fn hidden(name: &str) -> bool {
    name.starts_with('.')
}

/// Names and ETags of the files in a collection, in name order.
fn members(dir: &Path) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap())
        .filter(|e| e.path().is_file() && !hidden(&e.file_name().to_string_lossy()))
        .map(|e| {
            (
                e.file_name().to_string_lossy().into_owned(),
                etag(&std::fs::read(e.path()).unwrap()),
            )
        })
        .collect();
    out.sort();
    out
}

/// Changes whenever a member is added, removed or rewritten.
fn ctag(dir: &Path) -> String {
    let mut hasher = DefaultHasher::new();
    members(dir).hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

const TOKEN: &str = "http://test.invalid/sync/";

/// The token for the calendar as it is now. What it stands for is kept in a
/// hidden file, so that a later report can tell what changed since.
fn sync_token(dir: &Path) -> String {
    let tag = ctag(dir);
    let listing: String = members(dir).iter().map(|(n, e)| format!("{n}\t{e}\n")).collect();
    std::fs::write(dir.join(format!(".sync-{tag}")), listing).unwrap();
    format!("{TOKEN}{tag}")
}

fn multistatus(inner: &str) -> String {
    format!(
        r#"<?xml version="1.0"?><d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">{inner}</d:multistatus>"#
    )
}

/// Answer to a `sync-collection` report; `None` when the token is not known.
fn sync_report(dir: &Path, base: &str, body: &str) -> Option<String> {
    let token = element(body, "sync-token")?;
    let then = std::fs::read_to_string(dir.join(format!(".sync-{}", token.strip_prefix(TOKEN)?))).ok()?;
    let then: Vec<(String, String)> = then
        .lines()
        .filter_map(|l| l.split_once('\t').map(|(n, e)| (n.to_string(), e.to_string())))
        .collect();
    let now = members(dir);
    let mut xml = String::new();
    for (name, tag) in now.iter().filter(|m| !then.contains(m)) {
        xml.push_str(&format!(
            "<d:response><d:href>{base}/{name}</d:href><d:propstat><d:prop><d:getetag>{}</d:getetag></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>",
            escape(tag)
        ));
    }
    for (name, _) in then.iter().filter(|(n, _)| !now.iter().any(|(m, _)| m == n)) {
        xml.push_str(&format!(
            "<d:response><d:href>{base}/{name}</d:href><d:status>HTTP/1.1 404 Not Found</d:status></d:response>"
        ));
    }
    xml.push_str(&format!("<d:sync-token>{}</d:sync-token>", sync_token(dir)));
    Some(multistatus(&xml))
}

/// Answer to a `calendar-multiget` report: the objects named in the request.
fn multiget_report(root: &Path, body: &str) -> String {
    let mut xml = String::new();
    for href in body.split("href>").skip(1).step_by(2) {
        let href = href.split('<').next().unwrap_or_default();
        match std::fs::read(root.join(href.trim_start_matches('/'))) {
            Ok(data) => xml.push_str(&format!(
                "<d:response><d:href>{href}</d:href><d:propstat><d:prop><d:getetag>{}</d:getetag><c:calendar-data>{}</c:calendar-data></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>",
                escape(&etag(&data)),
                escape(&String::from_utf8_lossy(&data))
            )),
            Err(_) => xml.push_str(&format!(
                "<d:response><d:href>{href}</d:href><d:status>HTTP/1.1 404 Not Found</d:status></d:response>"
            )),
        }
    }
    multistatus(&xml)
}

fn handle(mut request: Request, root: &Path) {
    if header(&request, "Authorization").as_deref() != Some(AUTH) {
        return request.respond(Response::empty(401)).unwrap();
    }
    let url = request.url().to_string();
    let relative = url.trim_start_matches('/').trim_end_matches('/');
    let path = root.join(relative);
    let method = request.method().clone();
    if let Ok(broken) = std::fs::read_to_string(root.join(".fail")) {
        if url.starts_with(broken.trim()) {
            return request.respond(Response::empty(500)).unwrap();
        }
    }
    if root.join(".requests").exists() {
        let detail = match (method.as_str(), header(&request, "Depth")) {
            ("PROPFIND", Some(depth)) => format!(" depth={depth}"),
            // The body is not read yet; the header tells the two reports apart.
            ("REPORT", Some(_)) => " sync-collection".to_string(),
            ("REPORT", None) => " calendar-multiget".to_string(),
            _ => String::new(),
        };
        let mut log = std::fs::OpenOptions::new()
            .append(true)
            .open(root.join(".requests"))
            .unwrap();
        writeln!(log, "{method} {url}{detail}").unwrap();
    }
    let if_match = header(&request, "If-Match");
    let if_none_match = header(&request, "If-None-Match");
    // Test hook for a concurrent writer: `.race` holds a path and new content.
    // The content lands just before the next PUT to that path is examined.
    if method == Method::Put {
        if let Ok(race) = std::fs::read_to_string(root.join(".race")) {
            if let Some((target, content)) = race.split_once('\n') {
                if target == relative {
                    std::fs::write(&path, content).unwrap();
                    std::fs::remove_file(root.join(".race")).unwrap();
                }
            }
        }
    }
    let current = std::fs::read(&path).ok().filter(|_| path.is_file());
    let precondition_failed = match (&if_match, &if_none_match, &current) {
        (Some(wanted), _, Some(data)) => *wanted != etag(data),
        (Some(_), _, None) => true,
        (None, Some(_), Some(_)) => true,
        _ => false,
    };
    let code = match method {
        Method::Get | Method::Head => match current {
            Some(data) => {
                let tag = etag(&data);
                let body = if method == Method::Head { vec![] } else { data };
                return request.respond(with_etag(Response::from_data(body), &tag)).unwrap();
            }
            None => 404,
        },
        Method::Put => {
            if !path.parent().is_some_and(Path::is_dir) {
                409
            } else if precondition_failed {
                412
            } else {
                let mut data = Vec::new();
                request.as_reader().read_to_end(&mut data).unwrap();
                let existed = path.exists();
                std::fs::write(&path, &data).unwrap();
                let response = Response::from_data(vec![]).with_status_code(if existed { 204 } else { 201 });
                return request.respond(with_etag(response, &etag(&data))).unwrap();
            }
        }
        Method::Delete => {
            if path.is_dir() {
                std::fs::remove_dir_all(&path).map_or(404, |_| 204)
            } else if current.is_none() {
                404
            } else if precondition_failed {
                412
            } else {
                std::fs::remove_file(&path).map_or(404, |_| 204)
            }
        }
        Method::NonStandard(ref m) if matches!(m.as_str(), "MKCOL" | "MKCALENDAR") => {
            if path.is_dir() {
                405
            } else if !path.parent().is_some_and(Path::is_dir) {
                409
            } else {
                std::fs::create_dir(&path).unwrap();
                if m.as_str() == "MKCALENDAR" {
                    let mut body = String::new();
                    request.as_reader().read_to_string(&mut body).unwrap();
                    std::fs::write(path.join(".props"), "calendar=1\n").unwrap();
                    write_props(&path, root, &body);
                }
                201
            }
        }
        Method::NonStandard(ref m) if m.as_str() == "MOVE" => {
            // The destination is an absolute address; only its path matters here.
            let destination = header(&request, "Destination").unwrap_or_default();
            let target = destination
                .split_once("://")
                .and_then(|(_, rest)| rest.split_once('/'))
                .map(|(_, path)| root.join(path.trim_end_matches('/')));
            match target {
                _ if root.join(".no-move").exists() => 405,
                None => 400,
                Some(_) if current.is_none() => 404,
                Some(target) if !target.parent().is_some_and(Path::is_dir) => {
                    if root.join(".move-403").exists() {
                        403
                    } else {
                        409
                    }
                }
                Some(target) => {
                    let existed = target.exists();
                    std::fs::rename(&path, &target).unwrap();
                    if existed {
                        204
                    } else {
                        201
                    }
                }
            }
        }
        Method::NonStandard(ref m) if m.as_str() == "PROPPATCH" => {
            if !path.is_dir() {
                404
            } else {
                let mut body = String::new();
                request.as_reader().read_to_string(&mut body).unwrap();
                write_props(&path, root, &body);
                207
            }
        }
        Method::NonStandard(ref m) if m.as_str() == "PROPFIND" => {
            if !path.is_dir() {
                404
            } else {
                let depth = header(&request, "Depth").unwrap_or_else(|| "1".into());
                let base = if relative.is_empty() {
                    String::new()
                } else {
                    format!("/{relative}")
                };
                let mut xml = String::from(
                    r#"<?xml version="1.0"?><d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav" xmlns:a="http://apple.com/ns/ical/" xmlns:cs="http://calendarserver.org/ns/" xmlns:l="https://evsyukov.org/ns/lists">"#,
                );
                let collection = |href: &str, dir: &Path| {
                    let props = read_props(dir);
                    let get = |key: &str| props.iter().find(|(k, _)| k == key).map(|(_, v)| escape(v));
                    let calendar = get("calendar").is_some();
                    let mut inner = format!(
                        "<d:resourcetype><d:collection/>{}</d:resourcetype>",
                        if calendar { "<c:calendar/>" } else { "" }
                    );
                    if calendar {
                        inner.push_str(r#"<c:supported-calendar-component-set><c:comp name="VTODO"/></c:supported-calendar-component-set>"#);
                        if !root.join(".no-ctag").exists() {
                            inner.push_str(&format!("<cs:getctag>{}</cs:getctag>", ctag(dir)));
                        }
                        if !root.join(".no-sync").exists() {
                            inner.push_str(&format!("<d:sync-token>{}</d:sync-token>", sync_token(dir)));
                        }
                    }
                    if let Ok(home) = std::fs::read_to_string(root.join(".home")) {
                        inner.push_str(&format!(
                            "<c:calendar-home-set><d:href>{}</d:href></c:calendar-home-set>",
                            home.trim()
                        ));
                    }
                    if let Some(name) = get("name") {
                        inner.push_str(&format!("<d:displayname>{name}</d:displayname>"));
                    }
                    if let Some(color) = get("color") {
                        inner.push_str(&format!("<a:calendar-color>{color}</a:calendar-color>"));
                    }
                    if let Some(state) = get("state") {
                        inner.push_str(&format!("<l:state>{state}</l:state>"));
                    }
                    if let Some(filters) = get("filters") {
                        inner.push_str(&format!("<l:filters>{filters}</l:filters>"));
                    }
                    if let Some(push) = get("push") {
                        inner.push_str(&format!("<l:push>{push}</l:push>"));
                    }
                    format!("<d:response><d:href>{href}/</d:href><d:propstat><d:prop>{inner}</d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>")
                };
                xml.push_str(&collection(&base, &path));
                if depth != "0" {
                    let mut entries: Vec<_> = std::fs::read_dir(&path).unwrap().map(|e| e.unwrap()).collect();
                    entries.sort_by_key(|e| e.file_name());
                    for entry in entries {
                        let name = entry.file_name().to_string_lossy().into_owned();
                        if hidden(&name) {
                            continue;
                        }
                        if entry.path().is_dir() {
                            xml.push_str(&collection(&format!("{base}/{name}"), &entry.path()));
                        } else {
                            let tag = escape(&etag(&std::fs::read(entry.path()).unwrap()));
                            xml.push_str(&format!(
                                "<d:response><d:href>{base}/{name}</d:href><d:propstat><d:prop><d:resourcetype/><d:getetag>{tag}</d:getetag></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>"
                            ));
                        }
                    }
                }
                xml.push_str("</d:multistatus>");
                let response = Response::from_string(xml)
                    .with_status_code(207)
                    .with_header(Header::from_bytes("Content-Type", "application/xml").unwrap());
                return request.respond(response).unwrap();
            }
        }
        Method::NonStandard(ref m) if m.as_str() == "REPORT" && !root.join(".no-report").exists() => {
            let mut body = String::new();
            request.as_reader().read_to_string(&mut body).unwrap();
            let answer = if !path.is_dir() {
                None
            } else if body.contains("calendar-multiget") {
                Some(multiget_report(root, &body))
            } else if body.contains("sync-collection") && !root.join(".no-sync").exists() {
                sync_report(&path, &format!("/{relative}"), &body)
            } else {
                None
            };
            match answer {
                Some(xml) => {
                    let response = Response::from_string(xml)
                        .with_status_code(207)
                        .with_header(Header::from_bytes("Content-Type", "application/xml").unwrap());
                    return request.respond(response).unwrap();
                }
                // RFC 6578 asks for 403 with a precondition element on a token that is not valid.
                None => 403,
            }
        }
        _ => 405,
    };
    request.respond(Response::empty(code)).unwrap();
}
