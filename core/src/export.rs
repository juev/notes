//! R60–R63: export to Markdown that Obsidian opens, to HTML with the typography of the app, and to
//! JSON with every field. Export reads and writes files only; the notes stay as they are (R63).

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::PathBuf;

use base64::Engine as _;
use pulldown_cmark::{CowStr, Event, LinkType, Options, Parser, Tag, TagEnd};
use serde_json::{json, Value};

use crate::editing::escape_text;
use crate::error::{AppError, Result};
use crate::markdown::{address, layout, MarkdownKind};
use crate::model::{AppSettings, Attachment, Folder, Note, TextAlign};
use crate::store::{live_folders, query_notes, Store};

/// What to export to Markdown (R60).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum ExportScope {
    Note {
        id: String,
    },
    /// The folder with its subfolders.
    Folder {
        id: String,
    },
    All,
}

/// What an export wrote.
#[derive(Debug, Clone, Default, PartialEq, Eq, uniffi::Record)]
pub struct ExportReport {
    pub notes: u32,
    pub attachments: u32,
    /// Attachments whose content has not reached this device, as `name (title of the note)`.
    pub missing: Vec<String>,
}

/// R60: a name a file can have on every platform, without the characters that Obsidian reads as
/// links or headings.
fn file_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_control() || "/\\:*?\"<>|#^[]".contains(c) {
                '-'
            } else {
                c
            }
        })
        .collect();
    let cleaned = cleaned.trim().trim_matches('.').trim().to_string();
    let cleaned: String = cleaned.chars().take(120).collect();
    if cleaned.is_empty() {
        "Untitled".into()
    } else {
        cleaned
    }
}

/// A name not taken yet in `taken` (compared without case): `name`, then `name 2`, `name 3`…
fn unique(taken: &mut HashSet<String>, name: &str, extension: &str) -> String {
    let mut n = 1;
    loop {
        let candidate = if n == 1 {
            format!("{name}{extension}")
        } else {
            format!("{name} {n}{extension}")
        };
        if taken.insert(candidate.to_lowercase()) {
            return candidate;
        }
        n += 1;
    }
}

fn iso(ms: u64) -> String {
    chrono::DateTime::from_timestamp_millis(ms as i64)
        .map(|t| t.format("%Y-%m-%dT%H:%M:%SZ").to_string())
        .unwrap_or_default()
}

/// R3: what a note is called where its title is empty.
fn shown_title(note: &Note) -> String {
    if !note.title.trim().is_empty() {
        return note.title.trim().to_string();
    }
    let first = note.preview.trim();
    if first.is_empty() {
        "Untitled".into()
    } else {
        first.chars().take(60).collect()
    }
}

/// A destination of a link that may hold spaces and brackets.
fn destination(path: &str) -> String {
    let encoded = path
        .replace('%', "%25")
        .replace(' ', "%20")
        .replace('(', "%28")
        .replace(')', "%29");
    encoded.replace('<', "%3C").replace('>', "%3E")
}

/// The links of a text replaced: an attachment of the note by `attachment`, a link to a note by
/// `note`. Each gets the id and the text of the link and gives the new markup, or `None` to keep
/// the text of the link alone.
fn rewrite_links(
    text: &str,
    attachment: &dyn Fn(&str, bool, &str) -> Option<String>,
    note: &dyn Fn(&str, &str) -> Option<String>,
) -> String {
    let parsed = layout(text, false);
    let mut changes: Vec<(Range<usize>, String)> = Vec::new();
    for span in &parsed.spans {
        match &span.kind {
            MarkdownKind::AttachmentLink { id, inline } => {
                let whole = &text[span.range.clone()];
                let shown = whole
                    .trim_start_matches('!')
                    .strip_prefix('[')
                    .and_then(|s| s.split_once("]("))
                    .map_or("", |(t, _)| t);
                let new = attachment(id, *inline, shown).unwrap_or_else(|| shown.to_string());
                changes.push((span.range.clone(), new));
            }
            MarkdownKind::NoteLink { id } => {
                let shown = &text[span.range.clone()];
                let new = note(id, shown).unwrap_or_else(|| shown.to_string());
                changes.push((span.reveal.clone(), new));
            }
            _ => {}
        }
    }
    changes.sort_by_key(|c| std::cmp::Reverse(c.0.start));
    let mut out = text.to_string();
    for (range, new) in changes {
        out.replace_range(range, &new);
    }
    out
}

/// R61: the CSS of the typography of the app (E40–E48 in docs/specs/editor.md).
fn css(settings: &AppSettings) -> String {
    let body = match settings.font_body.as_str() {
        "Literata" => "'Literata', Georgia, serif",
        "Inter" => "'Inter', system-ui, sans-serif",
        "iA Writer Quattro" => "'iA Writer Quattro', 'iA Writer Quattro S', serif",
        _ => "system-ui, -apple-system, 'Segoe UI', Roboto, sans-serif",
    };
    let mono = match settings.font_mono.as_str() {
        "JetBrains Mono" => "'JetBrains Mono', ui-monospace, monospace",
        _ => "ui-monospace, 'SF Mono', Menlo, Consolas, monospace",
    };
    let justified = settings.align == TextAlign::Justify;
    let align = if justified { "justify" } else { "left" };
    // E45: justified text turns hyphenation on.
    let hyphens = if justified || settings.hyphenate {
        "auto"
    } else {
        "manual"
    };
    let line = settings.line_height;
    let spacing = settings.paragraph_spacing * line;
    format!(
        "body {{ font-family: {body}; font-size: 16px; line-height: {line}; max-width: 72ch; margin: 2em auto; \
         padding: 0 1em; text-align: {align}; hyphens: {hyphens}; -webkit-hyphens: {hyphens}; }}\n\
         p, ul, ol, blockquote, table, pre {{ margin: 0 0 {spacing}em; }}\n\
         h1, h2, h3, h4, h5, h6 {{ margin: {heading}em 0 {spacing}em; line-height: 1.2; hyphens: manual; }}\n\
         h1 {{ font-size: 1.6em; }} h2 {{ font-size: 1.35em; }} h3, h4, h5, h6 {{ font-size: 1.15em; }}\n\
         code, pre {{ font-family: {mono}; font-size: 0.9em; hyphens: manual; }}\n\
         pre {{ padding: 0.75em 1em; background: #f4f4f4; overflow-x: auto; text-align: left; }}\n\
         blockquote {{ margin-left: 0; padding-left: 1em; border-left: 3px solid #ccc; }}\n\
         table {{ border-collapse: collapse; hyphens: manual; }} th, td {{ border: 1px solid #ccc; padding: 0.25em 0.5em; }}\n\
         img {{ max-width: 100%; }} mark {{ background: #fff3a3; }}\n\
         @page {{ margin: 2cm; }}\n",
        heading = 2.0 * spacing
    )
}

/// E46: the language of a paragraph by the letters of its text without markup and addresses:
/// Russian for Cyrillic, English otherwise.
fn language(text: &str) -> &'static str {
    let (text, _) = crate::markdown::plain_and_links(text);
    let cyrillic = text.chars().filter(|c| ('\u{0400}'..='\u{04FF}').contains(c)).count();
    let latin = text.chars().filter(|c| c.is_ascii_alphabetic()).count();
    if cyrillic > latin {
        "ru"
    } else {
        "en"
    }
}

/// R61: the HTML of a text of the dialect. Raw HTML of the note is text (E4); a link opens only
/// with a web or mail address, or with one of `allowed` that the export put there itself.
fn render(text: &str, allowed: &HashSet<String>) -> String {
    let parsed = layout(text, false);
    let highlights: Vec<Range<usize>> = parsed
        .spans
        .iter()
        .filter(|s| s.kind == MarkdownKind::Highlight)
        .map(|s| s.reveal.clone())
        .collect();
    let options = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let mut events: Vec<Event> = Vec::new();
    let mut dropped: Vec<bool> = Vec::new();
    // Images the export cannot show are left as their text: their ends are dropped too.
    let mut hidden_images = 0usize;
    for (event, range) in Parser::new_ext(text, options).into_offset_iter() {
        match event {
            Event::Html(t) | Event::InlineHtml(t) => events.push(Event::Text(t)),
            Event::Start(Tag::Paragraph) => {
                let lang = language(&text[range.clone()]);
                events.push(Event::Html(CowStr::from(format!("<p lang=\"{lang}\">"))));
            }
            Event::End(TagEnd::Paragraph) => events.push(Event::Html("</p>\n".into())),
            Event::Start(Tag::Link {
                link_type,
                dest_url,
                title,
                id,
            }) => {
                let url = match link_type {
                    LinkType::Email => format!("mailto:{dest_url}"),
                    _ => dest_url.to_string(),
                };
                let open = address(&url).is_some() || allowed.contains(&url);
                dropped.push(!open);
                if open {
                    events.push(Event::Start(Tag::Link {
                        link_type,
                        dest_url: CowStr::from(url),
                        title,
                        id,
                    }));
                }
            }
            Event::End(TagEnd::Link) => {
                if !dropped.pop().unwrap_or(true) {
                    events.push(Event::End(TagEnd::Link));
                }
            }
            Event::Start(Tag::Image { dest_url, .. })
                if !(address(&dest_url).is_some() || allowed.contains(dest_url.as_ref())) =>
            {
                hidden_images += 1;
            }
            Event::End(TagEnd::Image) if hidden_images > 0 => hidden_images -= 1,
            Event::Text(t) => {
                // The markers of a highlight are text to the parser: they become <mark>.
                let inside = text.get(range.clone()) == Some(t.as_ref());
                let marks: Vec<&Range<usize>> = if inside {
                    highlights
                        .iter()
                        .filter(|h| range.start < h.end && h.start < range.end)
                        .collect()
                } else {
                    Vec::new()
                };
                if marks.is_empty() {
                    events.push(Event::Text(t));
                    continue;
                }
                let mut at = range.start;
                let mut points: Vec<(usize, &str)> = Vec::new();
                for h in marks {
                    if range.contains(&h.start) {
                        points.push((h.start, "<mark>"));
                    }
                    if h.end > range.start && h.end <= range.end {
                        points.push((h.end, "</mark>"));
                    }
                }
                points.sort();
                for (point, tag) in points {
                    let (skip_from, skip_to) = if tag == "<mark>" {
                        (point, point + 2)
                    } else {
                        (point - 2, point)
                    };
                    if skip_from > at {
                        events.push(Event::Text(CowStr::from(text[at..skip_from].to_string())));
                    }
                    events.push(Event::Html(tag.into()));
                    at = skip_to.max(at);
                }
                if at < range.end {
                    events.push(Event::Text(CowStr::from(text[at..range.end].to_string())));
                }
            }
            other => events.push(other),
        }
    }
    let mut out = String::new();
    pulldown_cmark::html::push_html(&mut out, events.into_iter());
    out
}

fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Where the content of an attachment is on this device.
fn content_of(store: &Store, attachment: &Attachment) -> Option<PathBuf> {
    attachment
        .local_path
        .as_ref()
        .map(PathBuf::from)
        .filter(|p| p.exists())
        .or_else(|| {
            let path = store.blob_path(&attachment.sha256);
            path.exists().then_some(path)
        })
}

#[uniffi::export]
impl Store {
    /// R60: one note, a folder with its subfolders, or all notes as Markdown files with front matter
    /// in `dir`: folders become directories, attachments go into `_attachments/` next to their
    /// notes, and links between notes become `[[…]]` links that Obsidian follows. The Trash is not
    /// exported.
    pub fn export_markdown(&self, scope: ExportScope, dir: String) -> Result<ExportReport> {
        let dir = PathBuf::from(dir);
        let folders = live_folders(&self.lock().conn)?;
        let notes = query_notes(&self.lock().conn, "WHERE n.purged = 0 AND n.deleted = 0", &[])?;
        let by_id: HashMap<&str, &Folder> = folders.iter().map(|f| (f.id.as_str(), f)).collect();
        // The directory of every folder, relative to `dir`.
        let mut paths: HashMap<String, PathBuf> = HashMap::new();
        let mut taken: HashMap<PathBuf, HashSet<String>> = HashMap::new();
        let root_of_scope: Option<&str> = match &scope {
            ExportScope::Folder { id } => {
                if !by_id.contains_key(id.as_str()) {
                    return Err(AppError::not_found(format!("folder {id}")));
                }
                Some(id.as_str())
            }
            _ => None,
        };
        fn path_of(
            id: &str,
            by_id: &HashMap<&str, &Folder>,
            stop: Option<&str>,
            paths: &mut HashMap<String, PathBuf>,
            taken: &mut HashMap<PathBuf, HashSet<String>>,
            depth: usize,
        ) -> Option<PathBuf> {
            if let Some(known) = paths.get(id) {
                return Some(known.clone());
            }
            let folder = by_id.get(id)?;
            // The folder of the scope is the top directory; a folder outside it has none.
            let parent = if Some(id) == stop {
                PathBuf::new()
            } else {
                match &folder.parent_id {
                    Some(parent) if depth < 64 => path_of(parent, by_id, stop, paths, taken, depth + 1)?,
                    Some(_) => return None,
                    None if stop.is_some() => return None,
                    None => PathBuf::new(),
                }
            };
            let name = unique(taken.entry(parent.clone()).or_default(), &file_name(&folder.name), "");
            let path = parent.join(name);
            paths.insert(id.to_string(), path.clone());
            Some(path)
        }
        let mut chosen: Vec<(Note, PathBuf)> = Vec::new();
        for note in notes {
            let place = match (&scope, &note.folder_id) {
                (ExportScope::Note { id }, _) if *id != note.id => continue,
                (ExportScope::Note { .. }, _) => PathBuf::new(),
                (ExportScope::Folder { .. }, None) => continue,
                (_, None) => PathBuf::new(),
                (_, Some(folder)) => match path_of(folder, &by_id, root_of_scope, &mut paths, &mut taken, 0) {
                    Some(path) => path,
                    None if root_of_scope.is_none() => PathBuf::new(),
                    None => continue,
                },
            };
            chosen.push((note, place));
        }
        if let ExportScope::Note { id } = &scope {
            if chosen.is_empty() {
                return Err(AppError::not_found(format!("note {id}")));
            }
        }
        // The file of every note, so that links can name it.
        let mut files: HashMap<String, (PathBuf, String)> = HashMap::new();
        for (note, place) in &chosen {
            let stem = unique(
                taken.entry(place.clone()).or_default(),
                &file_name(&shown_title(note)),
                "",
            );
            files.insert(note.id.clone(), (place.clone(), stem));
        }
        let mut report = ExportReport::default();
        let mut attachment_names: HashMap<PathBuf, HashSet<String>> = HashMap::new();
        for (note, place) in &chosen {
            let (_, stem) = &files[&note.id];
            let folder = dir.join(place);
            std::fs::create_dir_all(&folder)?;
            let mut written: HashMap<String, String> = HashMap::new();
            for attachment in self.attachments(note.id.clone())? {
                let Some(source) = content_of(self, &attachment) else {
                    report
                        .missing
                        .push(format!("{} ({})", attachment.name, shown_title(note)));
                    continue;
                };
                let names = attachment_names.entry(folder.clone()).or_default();
                let (base, extension) = match attachment.name.rsplit_once('.') {
                    Some((b, e)) if !b.is_empty() && !e.is_empty() => (b.to_string(), format!(".{}", file_name(e))),
                    _ => (attachment.name.clone(), String::new()),
                };
                let name = unique(names, &file_name(&base), &extension);
                std::fs::create_dir_all(folder.join("_attachments"))?;
                std::fs::copy(&source, folder.join("_attachments").join(&name))?;
                written.insert(attachment.id.clone(), format!("_attachments/{name}"));
                report.attachments += 1;
            }
            let text = self.get_text(note.id.clone())?;
            let text = rewrite_links(
                &text,
                &|id, inline, shown| {
                    let path = written.get(id)?;
                    let bang = if inline { "!" } else { "" };
                    Some(format!("{bang}[{shown}]({})", destination(path)))
                },
                &|id, _| files.get(id).map(|(_, stem)| format!("[[{stem}]]")),
            );
            let mut front = vec![
                "---".to_string(),
                format!("title: {}", json!(note.title)),
                format!("created: {}", iso(note.created_ms)),
                format!("edited: {}", iso(note.edited_ms)),
            ];
            if !note.tags.is_empty() {
                front.push(format!("tags: {}", json!(note.tags)));
            }
            for (flag, set) in [
                ("pinned", note.pinned),
                ("starred", note.starred),
                ("archived", note.archived),
                ("locked", note.locked),
            ] {
                if set {
                    front.push(format!("{flag}: true"));
                }
            }
            front.push(format!("id: {}", note.id));
            front.push("---".into());
            let content = format!("{}\n\n{text}\n", front.join("\n"));
            std::fs::write(folder.join(format!("{stem}.md")), content)?;
            report.notes += 1;
        }
        Ok(report)
    }

    /// R61: a note as an HTML file in `dir` with the typography of the app, its attachments
    /// embedded in the file or written next to it into `<name>_files/`. Returns the path of the
    /// file. PDF is printed from it by the apps.
    pub fn export_html(&self, id: String, dir: String, embed_attachments: bool) -> Result<String> {
        let note = self.get_note(id.clone())?;
        let settings = self.settings()?;
        let dir = PathBuf::from(dir);
        std::fs::create_dir_all(&dir)?;
        let mut taken = HashSet::new();
        let stem = unique(&mut taken, &file_name(&shown_title(&note)), "");
        let files_dir = format!("{stem}_files");
        let mut urls: HashMap<String, (String, String)> = HashMap::new();
        let mut names = HashSet::new();
        for attachment in self.attachments(id.clone())? {
            let Some(source) = content_of(self, &attachment) else {
                continue;
            };
            let url = if embed_attachments {
                let bytes = std::fs::read(&source)?;
                format!(
                    "data:{};base64,{}",
                    attachment.mime,
                    base64::engine::general_purpose::STANDARD.encode(bytes)
                )
            } else {
                let name = unique(&mut names, &file_name(&attachment.name), "");
                std::fs::create_dir_all(dir.join(&files_dir))?;
                std::fs::copy(&source, dir.join(&files_dir).join(&name))?;
                destination(&format!("{files_dir}/{name}"))
            };
            urls.insert(attachment.id.clone(), (url, attachment.name.clone()));
        }
        let allowed: HashSet<String> = urls.values().map(|(u, _)| u.clone()).collect();
        let text = self.get_text(id)?;
        let text = rewrite_links(
            &text,
            &|id, inline, shown| {
                let (url, _) = urls.get(id)?;
                let bang = if inline { "!" } else { "" };
                Some(format!("{bang}[{shown}]({url})"))
            },
            &|_, shown| Some(escape_text(shown)),
        );
        let body = render(&text, &allowed);
        let title = escape_html(&shown_title(&note));
        let html = format!(
            "<!DOCTYPE html>\n<html lang=\"{lang}\">\n<head>\n<meta charset=\"utf-8\">\n\
             <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n<title>{title}</title>\n\
             <style>\n{css}</style>\n</head>\n<body>\n<h1 class=\"title\">{title}</h1>\n{body}</body>\n</html>\n",
            lang = language(&format!("{} {text}", note.title)),
            css = css(&settings),
        );
        let path = dir.join(format!("{stem}.html"));
        std::fs::write(&path, html)?;
        Ok(path.to_string_lossy().into_owned())
    }

    /// R62: everything as `notes.json` in `dir`, in the format of "Export format" in
    /// docs/specs/product.md, and with `with_attachments` the content of the attachments in
    /// `attachments/`, named by their SHA-256.
    pub fn export_json(&self, dir: String, with_attachments: bool) -> Result<ExportReport> {
        let dir = PathBuf::from(dir);
        std::fs::create_dir_all(&dir)?;
        let settings = self.settings()?;
        let (notes, folders) = {
            let inner = self.lock();
            let notes = query_notes(&inner.conn, "WHERE n.purged = 0 ORDER BY n.created, n.id", &[])?;
            let mut stmt = inner.conn.prepare(
                "SELECT f.id, f.deleted, (SELECT value FROM fields WHERE kind = 'folder' AND id = f.id AND field = 'parent')
                 FROM folders f ORDER BY f.pos, f.id",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, bool>(1)?,
                    r.get::<_, Option<String>>(2)?,
                ))
            })?;
            let folders: Vec<(String, bool, Option<String>)> = rows.collect::<rusqlite::Result<_>>()?;
            (notes, folders)
        };
        let live: HashMap<String, Folder> = live_folders(&self.lock().conn)?
            .into_iter()
            .map(|f| (f.id.clone(), f))
            .collect();
        let mut report = ExportReport::default();
        let mut written: HashSet<String> = HashSet::new();
        let mut note_values = Vec::new();
        for note in &notes {
            let stored_folder: Option<String> =
                self.lock()
                    .conn
                    .query_row("SELECT folder_id FROM notes WHERE id = ?1", [&note.id], |r| r.get(0))?;
            let mut attachments = Vec::new();
            for attachment in self.attachments(note.id.clone())? {
                let mut file = Value::Null;
                if with_attachments {
                    match content_of(self, &attachment) {
                        Some(source) => {
                            let name = format!("attachments/{}", attachment.sha256);
                            if written.insert(attachment.sha256.clone()) {
                                std::fs::create_dir_all(dir.join("attachments"))?;
                                std::fs::copy(&source, dir.join(&name))?;
                            }
                            file = json!(name);
                            report.attachments += 1;
                        }
                        None => report
                            .missing
                            .push(format!("{} ({})", attachment.name, shown_title(note))),
                    }
                }
                attachments.push(json!({
                    "id": attachment.id,
                    "name": attachment.name,
                    "mime": attachment.mime,
                    "size": attachment.size,
                    "sha256": attachment.sha256,
                    "file": file,
                }));
            }
            note_values.push(json!({
                "id": note.id,
                "title": note.title,
                "folder_id": stored_folder,
                "tags": note.tags,
                "pinned": note.pinned,
                "starred": note.starred,
                "archived": note.archived,
                "locked": note.locked,
                "created": iso(note.created_ms),
                "edited": iso(note.edited_ms),
                "trashed": note.deleted_ms.map(iso),
                "text": self.get_text(note.id.clone())?,
                "attachments": attachments,
            }));
            report.notes += 1;
        }
        let folder_values: Vec<Value> = folders
            .iter()
            .map(|(id, deleted, parent)| {
                let f = live.get(id);
                json!({
                    "id": id,
                    "name": f.map(|f| f.name.clone()),
                    "icon": f.map(|f| f.icon.clone()),
                    "parent_id": parent.as_deref().and_then(|p| serde_json::from_str::<String>(p).ok()),
                    "daily": f.map(|f| f.daily),
                    "sort": f.map(|f| f.sort.as_string()),
                    "show_preview": f.map(|f| f.show_preview),
                    "show_date": f.map(|f| f.show_date),
                    "trashed": deleted,
                })
            })
            .collect();
        let filters: Vec<Value> = self
            .filters()?
            .into_iter()
            .map(|f| json!({"id": f.id, "name": f.name, "icon": f.icon, "query": f.query}))
            .collect();
        let document = json!({
            "format": "notes-export",
            "version": 1,
            "exported": iso(self.lock().now_ms()),
            "settings": {
                "trash_days": settings.trash_days,
                "font_body": settings.font_body,
                "font_mono": settings.font_mono,
                "line_height": settings.line_height,
                "paragraph_spacing": settings.paragraph_spacing,
                "align": if settings.align == TextAlign::Justify { "justify" } else { "left" },
                "hyphenate": settings.hyphenate,
                "smart_typography": settings.smart_typography,
                "spellcheck": settings.spellcheck,
                "spell_language": settings.spell_language,
                "new_note_folder": settings.new_note_folder,
            },
            "folders": folder_values,
            "notes": note_values,
            "filters": filters,
        });
        std::fs::write(dir.join("notes.json"), serde_json::to_string_pretty(&document)?)?;
        Ok(report)
    }
}
