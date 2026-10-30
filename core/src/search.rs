//! R47: search over the titles, the texts without markup, the tags and the names of attachments.
//! R50–R53: links between notes. A pass over the notes without an index: the text without markup
//! is kept in `notes.plain` by `materialize_note`.

use std::collections::HashMap;
use std::ops::Range;

use rusqlite::{params, Connection};
use serde_json::json;
use uuid::Uuid;

use crate::db::KIND_NOTE;
use crate::error::{AppError, Result};
use crate::markdown::{preview, MarkdownRange};
use crate::model::Note;
use crate::store::{get_note, get_unlocked_note, query_notes, Store};

/// A note the search found (R47).
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct SearchHit {
    pub note: Note,
    /// Where the title matches, in UTF-16 units of the title.
    pub title_matches: Vec<MarkdownRange>,
    /// A piece of the text without markup around the first match in it, or the preview when only
    /// the title, a tag or an attachment matched.
    pub snippet: String,
    /// Where the snippet matches, in UTF-16 units of the snippet.
    pub snippet_matches: Vec<MarkdownRange>,
}

/// What a link to a note leads to (R52).
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum NoteLinkState {
    Live,
    /// In the Trash: a tap offers to restore it.
    InTrash,
    /// Deleted for good, or never on this device: broken.
    Gone,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct NoteLinkTarget {
    pub id: String,
    pub state: NoteLinkState,
    /// The title the note has now; empty when it is gone.
    pub title: String,
}

/// Text as the search compares it: lower case, «ё» as «е» (R47), with the byte in the original
/// text that each byte of the folded one came from.
struct Folded {
    text: String,
    origin: Vec<usize>,
}

fn fold(text: &str) -> Folded {
    let mut folded = String::with_capacity(text.len());
    let mut origin = Vec::with_capacity(text.len() + 1);
    for (at, c) in text.char_indices() {
        for lower in c.to_lowercase() {
            let lower = if lower == 'ё' { 'е' } else { lower };
            let before = folded.len();
            folded.push(lower);
            origin.extend(std::iter::repeat_n(at, folded.len() - before));
        }
    }
    origin.push(text.len());
    Folded { text: folded, origin }
}

impl Folded {
    /// Every place `term` occurs, as byte ranges of the original text.
    fn find(&self, term: &str) -> Vec<Range<usize>> {
        if term.is_empty() {
            return Vec::new();
        }
        let mut out: Vec<Range<usize>> = Vec::new();
        let mut from = 0;
        while let Some(i) = self.text[from..].find(term) {
            let start = from + i;
            let end = start + term.len();
            let range = self.origin[start]..self.origin[end];
            if out.last().is_none_or(|last| last.end <= range.start) {
                out.push(range);
            }
            from = start + self.text[start..].chars().next().map_or(1, char::len_utf8);
        }
        out
    }
}

/// Byte ranges of `text` as UTF-16 ranges.
fn units(text: &str, ranges: &[Range<usize>]) -> Vec<MarkdownRange> {
    let count = |to: usize| text[..to].encode_utf16().count() as u32;
    ranges
        .iter()
        .map(|r| MarkdownRange {
            start: count(r.start),
            end: count(r.end),
        })
        .collect()
}

/// R47: a piece of the plain text around `at`: from a little before it, on character bounds.
fn snippet_around(plain: &str, at: usize) -> Range<usize> {
    const BEFORE: usize = 40;
    const LENGTH: usize = 200;
    let line_start = plain[..at].rfind('\n').map_or(0, |i| i + 1);
    let start = plain[line_start..at]
        .char_indices()
        .rev()
        .nth(BEFORE)
        .map_or(line_start, |(i, _)| line_start + i);
    let end = plain[start..]
        .char_indices()
        .nth(LENGTH)
        .map_or(plain.len(), |(i, _)| start + i);
    start..end
}

/// Weights of where a term matched (R47): the title above a tag, a tag above an attachment, an
/// attachment above the text.
const IN_TITLE: u32 = 8;
const IN_TAG: u32 = 4;
const IN_ATTACHMENT: u32 = 2;
const IN_TEXT: u32 = 1;

#[uniffi::export]
impl Store {
    /// R47: the notes that hold every word of `query`, each as any part of a word, in the title,
    /// the text, the tags or the names of the attachments, whatever the case, with «е» and «ё» as one
    /// letter. The best come first; archived notes and the Trash only when asked for.
    pub fn search(&self, query: String, include_archived: bool, include_trash: bool) -> Result<Vec<SearchHit>> {
        let terms: Vec<String> = fold(&query).text.split_whitespace().map(String::from).collect();
        if terms.is_empty() {
            return Ok(Vec::new());
        }
        let inner = self.lock();
        let conn = &inner.conn;
        let notes = query_notes(conn, "WHERE n.purged = 0", &[])?;
        let plain = texts(conn, "SELECT id, plain FROM notes WHERE purged = 0")?;
        let mut names: HashMap<String, Vec<String>> = HashMap::new();
        {
            let mut stmt = conn.prepare("SELECT note_id, name FROM attachments WHERE deleted = 0")?;
            let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
            for row in rows {
                let (note, name) = row?;
                names.entry(note).or_default().push(fold(&name).text);
            }
        }
        let mut hits: Vec<(u32, SearchHit)> = Vec::new();
        for note in notes {
            if note.deleted && !include_trash || note.archived && !include_archived && !note.deleted {
                continue;
            }
            let title = fold(&note.title);
            let text = plain.get(&note.id).map(String::as_str).unwrap_or_default();
            let body = fold(text);
            let tags: Vec<String> = note.tags.iter().map(|t| fold(t).text).collect();
            let files = names.get(&note.id).cloned().unwrap_or_default();
            let mut score = 0;
            let mut all = true;
            for term in &terms {
                let in_title = title.text.contains(term.as_str());
                let in_tag = tags.iter().any(|t| t.contains(term.as_str()));
                let in_file = files.iter().any(|f| f.contains(term.as_str()));
                let in_text = body.find(term).len() as u32;
                if !(in_title || in_tag || in_file || in_text > 0) {
                    all = false;
                    break;
                }
                score += u32::from(in_title) * IN_TITLE
                    + u32::from(in_tag) * IN_TAG
                    + u32::from(in_file) * IN_ATTACHMENT
                    + in_text.min(10) * IN_TEXT;
            }
            if !all {
                continue;
            }
            let title_ranges: Vec<Range<usize>> = merge(terms.iter().flat_map(|t| title.find(t)).collect());
            let first = terms
                .iter()
                .filter_map(|t| body.find(t).first().cloned())
                .min_by_key(|r| r.start);
            let (snippet, snippet_ranges) = match first {
                Some(first) => {
                    let piece = snippet_around(text, first.start);
                    let snippet = text[piece.clone()].replace('\n', " ");
                    let ranges = merge(
                        terms
                            .iter()
                            .flat_map(|t| body.find(t))
                            .filter(|r| piece.start <= r.start && r.end <= piece.end)
                            .map(|r| r.start - piece.start..r.end - piece.start)
                            .collect(),
                    );
                    (snippet, ranges)
                }
                None => {
                    let shown = preview(text);
                    let folded = fold(&shown);
                    let ranges = merge(terms.iter().flat_map(|t| folded.find(t)).collect());
                    (shown, ranges)
                }
            };
            hits.push((
                score,
                SearchHit {
                    title_matches: units(&note.title, &title_ranges),
                    snippet_matches: units(&snippet, &snippet_ranges),
                    snippet,
                    note,
                },
            ));
        }
        hits.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then(b.1.note.edited_ms.cmp(&a.1.note.edited_ms))
                .then(a.1.note.id.cmp(&b.1.note.id))
        });
        Ok(hits.into_iter().map(|(_, hit)| hit).collect())
    }

    /// R50: the notes whose title holds `query`, for the picker that `[[` opens: those whose title
    /// starts with it first, then the latest edited. Notes in the Trash are left out.
    pub fn notes_matching_title(&self, query: String, limit: u32) -> Result<Vec<Note>> {
        let wanted = fold(query.trim()).text;
        let inner = self.lock();
        let mut found: Vec<(bool, Note)> = query_notes(&inner.conn, "WHERE n.purged = 0 AND n.deleted = 0", &[])?
            .into_iter()
            .filter_map(|note| {
                let title = fold(&note.title).text;
                title.contains(&wanted).then(|| (title.starts_with(&wanted), note))
            })
            .collect();
        found.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.edited_ms.cmp(&a.1.edited_ms)));
        Ok(found.into_iter().take(limit as usize).map(|(_, n)| n).collect())
    }

    /// R51: the notes whose text links to this one, by title. Notes in the Trash are left out.
    pub fn linked_from(&self, id: String) -> Result<Vec<Note>> {
        let inner = self.lock();
        let mut notes = query_notes(
            &inner.conn,
            "WHERE n.purged = 0 AND n.deleted = 0 AND n.id != ?1
             AND n.id IN (SELECT note_id FROM note_links WHERE target = ?1)",
            &[&id],
        )?;
        notes.sort_by(|a, b| {
            a.title
                .to_lowercase()
                .cmp(&b.title.to_lowercase())
                .then(a.id.cmp(&b.id))
        });
        Ok(notes)
    }

    /// R52: what each link leads to.
    pub fn link_targets(&self, ids: Vec<String>) -> Result<Vec<NoteLinkTarget>> {
        let inner = self.lock();
        ids.into_iter()
            .map(|id| {
                let row: Option<(String, bool, bool)> = inner
                    .conn
                    .query_row("SELECT title, deleted, purged FROM notes WHERE id = ?1", [&id], |r| {
                        Ok((r.get(0)?, r.get(1)?, r.get(2)?))
                    })
                    .ok();
                Ok(match row {
                    Some((title, false, false)) => NoteLinkTarget {
                        id,
                        state: NoteLinkState::Live,
                        title,
                    },
                    Some((title, true, false)) => NoteLinkTarget {
                        id,
                        state: NoteLinkState::InTrash,
                        title,
                    },
                    _ => NoteLinkTarget {
                        id,
                        state: NoteLinkState::Gone,
                        title: String::new(),
                    },
                })
            })
            .collect()
    }

    /// R53: moves the text at `start..end` of the note into a new note in the same folder: its first
    /// line is the title, the rest the text. A link to the new note takes the place of the selection.
    /// One action of undo. R28: refused for a locked note.
    pub fn note_from_selection(&self, id: String, start: u32, end: u32) -> Result<Note> {
        let text = crate::body::text(&self.lock().conn, &id)?;
        let (from, to) = byte_range(&text, start.min(end), start.max(end))?;
        let selected = &text[from..to];
        let (first, rest) = selected.split_once('\n').unwrap_or((selected, ""));
        let title = title_of(first);
        if title.is_empty() && rest.trim().is_empty() {
            return Err(AppError::invalid("the selection is empty"));
        }
        let new_id = Uuid::now_v7().to_string();
        let link = format!("[{}](note:{new_id})", crate::editing::escape_text(&title));
        let start_units = text[..from].encode_utf16().count() as u32;
        let length = selected.encode_utf16().count() as u32;
        self.act(|w| {
            let source = get_unlocked_note(w.tx, &id)?;
            w.set(KIND_NOTE, &new_id, "title", json!(title))?;
            w.set(KIND_NOTE, &new_id, "created", json!(w.now_ms()))?;
            let folder: Option<String> =
                w.tx.query_row("SELECT folder_id FROM notes WHERE id = ?1", [&source.id], |r| r.get(0))?;
            if let Some(folder) = folder {
                w.set(KIND_NOTE, &new_id, "folder", json!(folder))?;
            }
            crate::body::create(w, &new_id, rest.trim_start_matches('\n'))?;
            crate::body::splice(w, &id, start_units, length, &link)?;
            w.flush()?;
            get_note(w.tx, &new_id)
        })
    }
}

/// The title a first line makes: without the markup of a heading, a list item or a quote.
fn title_of(line: &str) -> String {
    let (plain, _) = crate::markdown::plain_and_links(line);
    plain.lines().next().unwrap_or_default().trim().to_string()
}

fn byte_range(text: &str, start: u32, end: u32) -> Result<(usize, usize)> {
    let mut units = 0u32;
    let (mut from, mut to) = (None, None);
    for (at, c) in text.char_indices().chain(std::iter::once((text.len(), ' '))) {
        if units == start {
            from = Some(at);
        }
        if units == end {
            to = Some(at);
            break;
        }
        units += c.len_utf16() as u32;
    }
    match (from, to) {
        (Some(from), Some(to)) => Ok((from, to)),
        _ => Err(AppError::invalid(format!(
            "the range {start}..{end} is not on the characters of the text"
        ))),
    }
}

fn texts(conn: &Connection, sql: &str) -> Result<HashMap<String, String>> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map(params![], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

fn merge(mut ranges: Vec<Range<usize>>) -> Vec<Range<usize>> {
    ranges.sort_by_key(|r| (r.start, r.end));
    let mut out: Vec<Range<usize>> = Vec::new();
    for range in ranges {
        match out.last_mut() {
            Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
            _ => out.push(range),
        }
    }
    out
}
