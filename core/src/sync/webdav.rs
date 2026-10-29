//! WebDAV as dumb file storage: GET, PUT, DELETE, HEAD, MKCOL, MOVE and a depth-1 PROPFIND.

use std::io::{Read, Write};
use std::path::Path;
use std::time::Duration;

use base64::Engine;

use super::remote::{copy_over, Remote};
use crate::error::{AppError, Result};
use crate::model::ConnectionCheck;

const PROPFIND_BODY: &str = r#"<?xml version="1.0" encoding="utf-8"?><d:propfind xmlns:d="DAV:"><d:prop><d:resourcetype/></d:prop></d:propfind>"#;

/// Upper bound for one downloaded file; attachments larger than this are not synced.
const MAX_BODY: u64 = 512 * 1024 * 1024;

pub struct WebDavRemote {
    agent: ureq::Agent,
    /// Collection URL ending in `/`.
    base: String,
    auth: Option<String>,
}

impl WebDavRemote {
    pub fn new(url: &str, user: &str, password: &str) -> Result<Self> {
        let url = url.trim();
        if !(url.starts_with("https://") || url.starts_with("http://")) {
            return Err(AppError::sync("the address must start with https:// or http://"));
        }
        let auth = (!user.is_empty()).then(|| {
            format!(
                "Basic {}",
                base64::engine::general_purpose::STANDARD.encode(format!("{user}:{password}"))
            )
        });
        Ok(WebDavRemote {
            agent: ureq::AgentBuilder::new()
                .timeout_connect(Duration::from_secs(15))
                .timeout(Duration::from_secs(120))
                .build(),
            base: format!("{}/", url.trim_end_matches('/')),
            auth,
        })
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base, path)
    }

    fn request(&self, method: &str, url: &str) -> ureq::Request {
        let request = self.agent.request(method, url);
        match &self.auth {
            Some(auth) => request.set("Authorization", auth),
            None => request,
        }
    }

    /// Asks whether sync could work here without writing anything (S26).
    pub fn check(&self) -> Result<ConnectionCheck> {
        if self.answers(&self.base)? {
            return Ok(ConnectionCheck::Ready);
        }
        // Sync creates the last collection of the address itself, but not the ones above it.
        let above = self.base.trim_end_matches('/').rsplit_once('/').map(|(up, _)| up);
        match above {
            Some(up) if !up.ends_with('/') && self.answers(&format!("{up}/"))? => Ok(ConnectionCheck::WillCreate),
            _ => Err(AppError::sync("nothing is found at this address (HTTP 404)")),
        }
    }

    /// Whether a collection exists at `url`; an error when the answer is not the one WebDAV gives.
    fn answers(&self, url: &str) -> Result<bool> {
        let result = self
            .request("PROPFIND", url)
            .set("Depth", "0")
            .set("Content-Type", "application/xml")
            .send_string(PROPFIND_BODY);
        match status(result, "PROPFIND")? {
            207 => Ok(true),
            404 => Ok(false),
            code @ (401 | 403) => Err(fail("PROPFIND", url, code)),
            code => Err(AppError::sync(format!(
                "the server does not answer as WebDAV (HTTP {code})"
            ))),
        }
    }

    /// Creates every collection on the way to `path`.
    fn make_parents(&self, path: &str) -> Result<()> {
        let mut dir = String::new();
        // The base collection itself may be missing too.
        status(self.request("MKCOL", &self.base).call(), "MKCOL")?;
        let parts: Vec<&str> = path.split('/').collect();
        for part in &parts[..parts.len().saturating_sub(1)] {
            dir.push_str(part);
            dir.push('/');
            // 405: already exists. Anything else that matters surfaces on the retried PUT.
            status(self.request("MKCOL", &self.url(&dir)).call(), "MKCOL")?;
        }
        Ok(())
    }

    /// Whether the server says the collection that would hold `path` is not there.
    fn folder_missing(&self, path: &str) -> bool {
        let folder = match path.rsplit_once('/') {
            Some((dir, _)) => self.url(&format!("{dir}/")),
            None => self.base.clone(),
        };
        matches!(self.answers(&folder), Ok(false))
    }

    /// R140: PUT with the file as the body. The length is declared, so the server gets no chunked body.
    fn send_file(&self, path: &str, size: u64, local: &Path) -> Result<u16> {
        let file = std::fs::File::open(local)?;
        let request = self
            .request("PUT", &self.url(path))
            .set("Content-Length", &size.to_string());
        status(request.send(file), "PUT")
    }
}

fn finish_put(path: &str, code: u16) -> Result<()> {
    if (200..300).contains(&code) {
        Ok(())
    } else {
        Err(fail("PUT", path, code))
    }
}

/// Collapses a response into its status code, turning transport failures into errors.
fn status(result: std::result::Result<ureq::Response, ureq::Error>, what: &str) -> Result<u16> {
    match result {
        Ok(response) => Ok(response.status()),
        Err(ureq::Error::Status(code, _)) => Ok(code),
        Err(e) => Err(AppError::sync(format!("{what}: {e}"))),
    }
}

fn fail(what: &str, path: &str, code: u16) -> AppError {
    let hint = match code {
        401 | 403 => " (check the user name and password)",
        507 => " (the storage is full)",
        _ => "",
    };
    AppError::sync(format!("{what} {path}: HTTP {code}{hint}"))
}

impl WebDavRemote {
    /// Names directly inside `dir`, each with whether it is a folder.
    fn entries(&self, dir: &str) -> Result<Vec<(String, bool)>> {
        let url = self.url(&format!("{dir}/"));
        let result = self
            .request("PROPFIND", &url)
            .set("Depth", "1")
            .set("Content-Type", "application/xml")
            .send_string(PROPFIND_BODY);
        let response = match result {
            Ok(r) => r,
            Err(ureq::Error::Status(404, _)) => return Ok(vec![]),
            Err(ureq::Error::Status(code, _)) => return Err(fail("PROPFIND", dir, code)),
            Err(e) => return Err(AppError::sync(format!("PROPFIND: {e}"))),
        };
        let body = response
            .into_string()
            .map_err(|e| AppError::sync(format!("PROPFIND: {e}")))?;
        parse_entries(&body, dir)
    }
}

impl Remote for WebDavRemote {
    fn id(&self) -> String {
        self.base.clone()
    }

    fn list(&self, dir: &str) -> Result<Vec<String>> {
        Ok(self
            .entries(dir)?
            .into_iter()
            .filter(|(_, folder)| !folder)
            .map(|(name, _)| name)
            .collect())
    }

    fn folders(&self, dir: &str) -> Result<Vec<String>> {
        Ok(self
            .entries(dir)?
            .into_iter()
            .filter(|(_, folder)| *folder)
            .map(|(name, _)| name)
            .collect())
    }

    /// S49: one MOVE, the content stays on the server. A server without MOVE gets a copy.
    fn rename(&self, from: &str, to: &str) -> Result<bool> {
        let send = || {
            let request = self
                .request("MOVE", &self.url(from))
                .set("Destination", &self.url(to))
                .set("Overwrite", "T");
            status(request.call(), "MOVE")
        };
        match send()? {
            200..=299 => return Ok(true),
            405 | 501 => return copy_over(self, from, to),
            // The source is gone, or the folder of the target is missing (S43).
            404 | 409 => {}
            // SFTPGo answers a MOVE into a missing folder with 403 (#304).
            403 if self.folder_missing(to) => {}
            code => return Err(fail("MOVE", from, code)),
        }
        if !self.exists(from)? {
            return Ok(false);
        }
        self.make_parents(to)?;
        match send()? {
            200..=299 => Ok(true),
            405 | 501 => copy_over(self, from, to),
            code => Err(fail("MOVE", from, code)),
        }
    }

    fn get(&self, path: &str) -> Result<Option<Vec<u8>>> {
        match self.request("GET", &self.url(path)).call() {
            Ok(response) => {
                let mut data = Vec::new();
                response
                    .into_reader()
                    .take(MAX_BODY)
                    .read_to_end(&mut data)
                    .map_err(|e| AppError::sync(format!("GET {path}: {e}")))?;
                Ok(Some(data))
            }
            Err(ureq::Error::Status(404, _)) => Ok(None),
            Err(ureq::Error::Status(code, _)) => Err(fail("GET", path, code)),
            Err(e) => Err(AppError::sync(format!("GET {path}: {e}"))),
        }
    }

    fn get_to(&self, path: &str, out: &mut dyn Write, max: u64) -> Result<bool> {
        match self.request("GET", &self.url(path)).call() {
            Ok(response) => {
                let cap = if max == 0 { u64::MAX } else { max.saturating_add(1) };
                let mut reader = response.into_reader().take(cap);
                let copied = std::io::copy(&mut reader, out).map_err(|e| AppError::sync(format!("GET {path}: {e}")))?;
                if max != 0 && copied > max {
                    return Err(AppError::too_large(format!("the content is more than {max} bytes")));
                }
                Ok(true)
            }
            Err(ureq::Error::Status(404, _)) => Ok(false),
            Err(ureq::Error::Status(code, _)) => Err(fail("GET", path, code)),
            Err(e) => Err(AppError::sync(format!("GET {path}: {e}"))),
        }
    }

    fn put(&self, path: &str, data: &[u8]) -> Result<()> {
        let send = || status(self.request("PUT", &self.url(path)).send_bytes(data), "PUT");
        match send() {
            Ok(code) if !matches!(code, 404 | 409) => return finish_put(path, code),
            Ok(_) => {}
            // A server may refuse before reading the body and close the connection (#250):
            // the client then sees a broken connection instead of the 409.
            Err(e) if !self.folder_missing(path) => return Err(e),
            Err(_) => {}
        }
        self.make_parents(path)?;
        finish_put(path, send()?)
    }

    /// R140: the file goes to the server from the disk, in the pieces ureq reads, not from memory.
    fn put_file(&self, path: &str, local: &Path) -> Result<()> {
        let size = std::fs::metadata(local)?.len();
        match self.send_file(path, size, local) {
            Ok(code) if !matches!(code, 404 | 409) => return finish_put(path, code),
            Ok(_) => {}
            Err(e) if !self.folder_missing(path) => return Err(e),
            Err(_) => {}
        }
        self.make_parents(path)?;
        finish_put(path, self.send_file(path, size, local)?)
    }

    fn delete(&self, path: &str) -> Result<()> {
        match status(self.request("DELETE", &self.url(path)).call(), "DELETE")? {
            200..=299 | 404 => Ok(()),
            code => Err(fail("DELETE", path, code)),
        }
    }

    fn exists(&self, path: &str) -> Result<bool> {
        match status(self.request("HEAD", &self.url(path)).call(), "HEAD")? {
            200..=299 => Ok(true),
            404 => Ok(false),
            code => Err(fail("HEAD", path, code)),
        }
    }
}

/// Names from a multistatus body, each with whether it is a folder. The listed
/// folder `dir` itself is skipped.
fn parse_entries(body: &str, dir: &str) -> Result<Vec<(String, bool)>> {
    let doc = roxmltree::Document::parse(body).map_err(|e| AppError::sync(format!("PROPFIND answer: {e}")))?;
    let listed = format!("/{}", dir.trim_matches('/'));
    let mut names = Vec::new();
    for response in doc.descendants().filter(|n| n.tag_name().name() == "response") {
        let href = response
            .descendants()
            .find(|n| n.tag_name().name() == "href")
            .and_then(|n| n.text())
            .unwrap_or("")
            .trim();
        let folder = href.ends_with('/') || response.descendants().any(|n| n.tag_name().name() == "collection");
        let path = percent_decode(href.trim_end_matches('/'));
        if path.is_empty() || path.ends_with(&listed) {
            continue;
        }
        if let Some(name) = path.rsplit('/').next().filter(|n| !n.is_empty()) {
            names.push((name.to_string(), folder));
        }
    }
    Ok(names)
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = (bytes[i] == b'%').then(|| s.get(i + 1..i + 3)).flatten();
        match hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
            Some(byte) => {
                out.push(byte);
                i += 3;
            }
            None => {
                out.push(bytes[i]);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listing_tells_files_from_folders_and_decodes_names() {
        let body = r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:">
  <d:response><d:href>/dav/lists/v1/log/</d:href>
    <d:propstat><d:prop><d:resourcetype><d:collection/></d:resourcetype></d:prop></d:propstat></d:response>
  <d:response><d:href>/dav/lists/v1/log/aaaaaaaaaaaa-0000000001.jsonl</d:href>
    <d:propstat><d:prop><d:resourcetype/></d:prop></d:propstat></d:response>
  <D:response xmlns:D="DAV:"><D:href>http://host/dav/lists/v1/log/a%20b.jsonl</D:href></D:response>
  <d:response><d:href>/dav/lists/v1/log/c2/</d:href></d:response>
</d:multistatus>"#;
        assert_eq!(
            parse_entries(body, "lists/v1/log").unwrap(),
            vec![
                ("aaaaaaaaaaaa-0000000001.jsonl".to_string(), false),
                ("a b.jsonl".to_string(), false),
                ("c2".to_string(), true),
            ]
        );
    }
}
