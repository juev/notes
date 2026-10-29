//! A minimal WebDAV server over a folder, for tests and for trying sync by
//! hand (`cargo run --example webdav -- <folder>`).
//!
//! It is stricter than most real servers where it matters to the client: a PUT
//! into a missing collection is refused (409), Basic auth is required, and
//! conditional requests are honoured. A file named `.race` stages a concurrent
//! write (see `handle`). When a file named `.requests` exists in the served
//! root, every authorised request is appended to it as `METHOD /path` (a
//! PROPFIND also carries its depth). `.fail` holds a path, and every request
//! under it is answered with 500. `.no-move` makes the server refuse MOVE with
//! 405, as a server without it would. `.move-403` makes it answer a MOVE into
//! a missing folder with 403 instead of 409, as SFTPGo does.

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

fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn hidden(name: &str) -> bool {
    name.starts_with('.')
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
        Method::NonStandard(ref m) if m.as_str() == "MKCOL" => {
            if path.is_dir() {
                405
            } else if !path.parent().is_some_and(Path::is_dir) {
                409
            } else {
                std::fs::create_dir(&path).unwrap();
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
                let collection = |href: &str| {
                    format!("<d:response><d:href>{href}/</d:href><d:propstat><d:prop><d:resourcetype><d:collection/></d:resourcetype></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>")
                };
                let mut xml = String::from(r#"<?xml version="1.0"?><d:multistatus xmlns:d="DAV:">"#);
                xml.push_str(&collection(&base));
                if depth != "0" {
                    let mut entries: Vec<_> = std::fs::read_dir(&path).unwrap().map(|e| e.unwrap()).collect();
                    entries.sort_by_key(|e| e.file_name());
                    for entry in entries {
                        let name = entry.file_name().to_string_lossy().into_owned();
                        if hidden(&name) {
                            continue;
                        }
                        if entry.path().is_dir() {
                            xml.push_str(&collection(&format!("{base}/{name}")));
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
        _ => 405,
    };
    request.respond(Response::empty(code)).unwrap();
}
