//! I2–I12: a backup of Standard Notes written into the storage as one action, with a report.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use rusqlite::Connection;
use serde_json::{json, Value};
use uuid::Uuid;

use super::backup::{self, Item};
use super::lexical::{self, FileLink};
use super::predicate;
use crate::db::{KIND_ATTACHMENT, KIND_FILTER, KIND_FOLDER, KIND_NOTE};
use crate::error::Result;
use crate::log::LogLevel;
use crate::store::{
    copy_hashing, folder_name, guess_mime, last_folder_pos, last_pos, private_file, spelling, Store, Writer,
};
use crate::{body, order, tag};

/// The choices of an import (I8, I9).
#[derive(Debug, Clone, Default, uniffi::Record)]
pub struct ImportOptions {
    /// I8: every folder and every note goes under one new folder "Standard Notes".
    #[uniffi(default = false)]
    pub under_one_folder: bool,
    /// I9: the files of no note go into one note "Files from Standard Notes".
    #[uniffi(default = false)]
    pub collect_unlinked_files: bool,
}

/// I11: how many items of a type were not imported.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct IgnoredItems {
    pub content_type: String,
    pub count: u32,
}

/// I12: what an import did. The lists name notes by their title.
#[derive(Debug, Clone, Default, PartialEq, Eq, uniffi::Record)]
pub struct ImportReport {
    pub notes: u32,
    pub folders: u32,
    pub tags: u32,
    pub filters: u32,
    pub attachments: u32,
    /// I3: items whose entity this device has already.
    pub already_imported: u32,
    /// I4: notes with neither a title nor a text.
    pub empty: u32,
    /// I4: notes of a type other than Super, Markdown and plain text, imported with their text as it is.
    pub other_types: Vec<String>,
    /// I6: protected notes, imported as ordinary ones.
    pub protected: Vec<String>,
    /// I7: notes that lost a format, a part or an image, with what.
    pub conversion: Vec<String>,
    /// I8: notes of several tags, with the folder chosen.
    pub several_tags: Vec<String>,
    /// I9: files not imported, with the note.
    pub files_not_imported: Vec<String>,
    /// I10: smart views not imported, by their title.
    pub filters_not_imported: Vec<String>,
    /// I11: items not imported, by type.
    pub ignored: Vec<IgnoredItems>,
    /// The report as text to copy.
    pub text: String,
}

const ROOT_FOLDER: &str = "Standard Notes";
const UNLINKED_NOTE: &str = "Files from Standard Notes";

/// I3: identifiers computed from the uuids of the backup, the same on every device.
fn namespace() -> Uuid {
    Uuid::new_v5(
        &Uuid::NAMESPACE_URL,
        b"https://evsyukov.org/ns/notes/import/standard-notes",
    )
}

fn id_for(kind: &str, key: &str) -> String {
    Uuid::new_v5(&namespace(), format!("{kind}:{key}").as_bytes()).to_string()
}

/// The actor of the first change of an imported text (I5 and `body::create_imported`).
fn seed(uuid: &str) -> [u8; 16] {
    *Uuid::new_v5(&namespace(), format!("actor:{uuid}").as_bytes()).as_bytes()
}

fn exists(conn: &Connection, kind: &str, id: &str) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM fields WHERE kind = ?1 AND id = ?2)",
        [kind, id],
        |r| r.get(0),
    )?)
}

/// The title a report gives a note: its title, its first line, or "Untitled" (R3).
fn shown_title(title: &str, text: &str) -> String {
    let title = title.trim();
    if !title.is_empty() {
        return title.to_string();
    }
    text.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map_or_else(|| "Untitled".to_string(), |l| l.chars().take(80).collect())
}

/// A flag of a note, in its content or in the data of the app (`appData`).
fn note_flag(item: &Item, key: &str) -> bool {
    item.flag(key)
        || backup::is_true(
            item.content
                .get("appData")
                .and_then(|a| a.get("org.standardnotes.sn"))
                .and_then(|a| a.get(key)),
        )
}

/// An icon of Standard Notes kept when it is an emoji; a name of its symbol set is not ours.
fn emoji(icon: &str) -> Option<String> {
    (!icon.is_empty() && !icon.is_ascii()).then(|| icon.to_string())
}

/// I9: a file of the backup and where its content was found.
struct File {
    uuid: String,
    name: String,
    mime: String,
    updated_ms: u64,
    notes: Vec<String>,
    /// The content in the store as (sha256, size), when one file matched.
    content: Option<(String, u64)>,
}

impl File {
    fn inline(&self) -> bool {
        let mime = if self.mime.is_empty() {
            guess_mime(&self.name)
        } else {
            &self.mime
        };
        ["image/", "video/", "audio/"].iter().any(|p| mime.starts_with(p)) || mime == "application/pdf"
    }
}

/// The files of a folder and its subfolders, by name and size.
fn index_files(dir: &Path, out: &mut HashMap<(String, u64), Vec<PathBuf>>) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let meta = entry.metadata()?;
        if meta.is_dir() {
            index_files(&path, out)?;
        } else if meta.is_file() {
            let name = entry.file_name().to_string_lossy().into_owned();
            out.entry((name, meta.len())).or_default().push(path);
        }
    }
    Ok(())
}

#[uniffi::export]
impl Store {
    /// R65, docs/specs/import.md: imports the decrypted backup of Standard Notes at `path` (a zip, its
    /// folder or the top `.txt` file). `files_dir` is a folder of the files downloaded from Standard
    /// Notes, matched by name and size (I9). The import is one action of undo (I2).
    pub fn import_standard_notes(
        &self,
        path: String,
        files_dir: Option<String>,
        options: ImportOptions,
    ) -> Result<ImportReport> {
        let result = self.import_backup(Path::new(&path), files_dir.as_deref().map(Path::new), &options);
        // R95: numbers only, never titles.
        match &result {
            Ok(r) => self.note(LogLevel::Info, "import", || {
                format!(
                    "Standard Notes: {} notes, {} folders, {} tags, {} filters, {} attachments; \
                     {} already imported, {} empty, {} files not imported",
                    r.notes,
                    r.folders,
                    r.tags,
                    r.filters,
                    r.attachments,
                    r.already_imported,
                    r.empty,
                    r.files_not_imported.len()
                )
            }),
            Err(e) => self.note(LogLevel::Error, "import", || format!("Standard Notes: {e}")),
        }
        result
    }
}

impl Store {
    fn import_backup(&self, path: &Path, files_dir: Option<&Path>, options: &ImportOptions) -> Result<ImportReport> {
        let items = backup::read(path)?;
        let live: Vec<&Item> = items.iter().filter(|i| !i.deleted).collect();
        let of = |kind: &str| {
            live.iter()
                .copied()
                .filter(|i| i.content_type == kind)
                .collect::<Vec<_>>()
        };
        let notes = of("Note");
        let tags = of("Tag");
        let views = of("SN|SmartTag");
        let file_items = of("SN|File");
        let mut report = ImportReport::default();

        // I11
        let mut ignored: BTreeMap<String, u32> = BTreeMap::new();
        for item in &live {
            if !matches!(item.content_type.as_str(), "Note" | "Tag" | "SN|SmartTag" | "SN|File") {
                *ignored.entry(item.content_type.clone()).or_default() += 1;
            }
        }
        report.ignored = ignored
            .into_iter()
            .map(|(content_type, count)| IgnoredItems { content_type, count })
            .collect();

        // I8: the tags of every note, and the number of notes of every tag.
        let note_uuids: HashSet<&str> = notes.iter().map(|n| n.uuid.as_str()).collect();
        let mut tags_of: HashMap<&str, Vec<&Item>> = HashMap::new();
        let mut count_of: HashMap<&str, usize> = HashMap::new();
        for tag in &tags {
            let mut seen = HashSet::new();
            for note in tag.references("Note") {
                if let Some(note) = note_uuids.get(note.as_str()) {
                    if seen.insert(*note) {
                        tags_of.entry(note).or_default().push(tag);
                        *count_of.entry(tag.uuid.as_str()).or_default() += 1;
                    }
                }
            }
        }

        // I9: the files and their content, copied into the store before the action.
        let mut found: HashMap<(String, u64), Vec<PathBuf>> = HashMap::new();
        if let Some(dir) = files_dir {
            index_files(dir, &mut found)?;
        }
        let max_file = self.attachment_limits().max_file;
        let mut copied: Vec<String> = Vec::new();
        let mut files: Vec<File> = Vec::new();
        for item in &file_items {
            let name = item.str("name").to_string();
            let size = item.content.get("decryptedSize").and_then(Value::as_u64).unwrap_or(0);
            let content = match found.get(&(name.clone(), size)).map(Vec::as_slice) {
                Some([only]) => self.copy_in(only, max_file, &mut copied).ok(),
                _ => None,
            };
            files.push(File {
                uuid: item.uuid.clone(),
                name,
                mime: item.str("mimeType").to_string(),
                updated_ms: item.updated_ms,
                notes: item.references("Note"),
                content,
            });
        }

        let written = self.act(|w| {
            write_all(
                w,
                &notes,
                &tags,
                &views,
                &files,
                &tags_of,
                &count_of,
                options,
                &mut report,
            )?;
            Ok(())
        });
        if let Err(e) = written {
            for sha256 in &copied {
                self.drop_blob_if_unused(sha256);
            }
            return Err(e);
        }
        report.text = report_text(&report);
        Ok(report)
    }

    /// Copies a file into the store, as an added attachment does. Returns (sha256, size); the
    /// hash of a new blob is pushed to `copied`, to be removed when the import fails.
    fn copy_in(&self, source: &Path, max_file: u64, copied: &mut Vec<String>) -> Result<(String, u64)> {
        let tmp = self
            .data_dir()
            .join("blobs")
            .join(format!(".import-{}.part", Uuid::now_v7().simple()));
        let (sha256, size) = copy_hashing(source, &tmp, max_file)?;
        let target = self.blob_path(&sha256);
        if target.exists() {
            let _ = std::fs::remove_file(&tmp);
        } else {
            std::fs::rename(&tmp, &target)?;
            private_file(&target)?;
            copied.push(sha256.clone());
        }
        Ok((sha256, size))
    }
}

#[allow(clippy::too_many_arguments)]
fn write_all(
    w: &mut Writer,
    notes: &[&Item],
    tags: &[&Item],
    views: &[&Item],
    files: &[File],
    tags_of: &HashMap<&str, Vec<&Item>>,
    count_of: &HashMap<&str, usize>,
    options: &ImportOptions,
    report: &mut ImportReport,
) -> Result<()> {
    // I8: "Standard Notes" above everything, when asked for.
    let root = if options.under_one_folder {
        let id = id_for("folder", "standard-notes");
        if !exists(w.tx, KIND_FOLDER, &id)? {
            new_folder(w, &id, ROOT_FOLDER, None, None)?;
            report.folders += 1;
        }
        Some(id)
    } else {
        None
    };

    // I8: a folder for every tag, in the order of their titles.
    let mut sorted: Vec<&Item> = tags.to_vec();
    sorted.sort_by_key(|t| (t.str("title").to_lowercase(), t.uuid.clone()));
    let mut folder_of_tag: HashMap<&str, String> = HashMap::new();
    let mut folder_by_title: HashMap<String, String> = HashMap::new();
    for tag in &sorted {
        let id = id_for("folder", &tag.uuid);
        if exists(w.tx, KIND_FOLDER, &id)? {
            report.already_imported += 1;
        } else {
            let name = folder_name(tag.str("title")).unwrap_or_else(|_| "Untitled".into());
            new_folder(w, &id, &name, root.as_deref(), emoji(tag.str("iconString")))?;
            report.folders += 1;
        }
        folder_of_tag.insert(tag.uuid.as_str(), id.clone());
        folder_by_title.entry(tag.str("title").to_string()).or_insert(id);
    }

    // I9: the attachment of a file in a note, when its content was found.
    let attachment_of = |file: &File, note: &str| id_for("attachment", &format!("{}:{note}", file.uuid));
    let mut tag_names: HashSet<String> = HashSet::new();
    for item in notes {
        let id = id_for("note", &item.uuid);
        if exists(w.tx, KIND_NOTE, &id)? {
            report.already_imported += 1;
            continue;
        }
        let title = item.str("title").to_string();
        let raw = item.str("text");
        let note_type = item.str("noteType");
        let mine: Vec<&File> = files.iter().filter(|f| f.notes.contains(&item.uuid)).collect();
        let link = |uuid: &str| match files.iter().find(|f| f.uuid == uuid) {
            Some(file) if file.content.is_some() && file.notes.contains(&item.uuid) => FileLink::Attached {
                id: attachment_of(file, &item.uuid),
                name: file.name.clone(),
                inline: file.inline(),
            },
            Some(file) => FileLink::Missing {
                name: file.name.clone(),
            },
            None => FileLink::Missing { name: uuid.to_string() },
        };
        let (text, lost) = if note_type == "super" {
            lexical::to_markdown(raw, &link)
        } else {
            (raw.to_string(), lexical::Lost::default())
        };
        if title.trim().is_empty() && text.trim().is_empty() {
            report.empty += 1;
            continue;
        }
        let shown = shown_title(&title, &text);
        if !matches!(note_type, "" | "super" | "markdown" | "plain-text") {
            report.other_types.push(shown.clone());
        }
        if !lost.is_empty() {
            let mut what = Vec::new();
            if lost.formats {
                what.push("underline, subscript or superscript dropped".to_string());
            }
            if !lost.nodes.is_empty() {
                what.push(format!(
                    "kept as text: {}",
                    lost.nodes.iter().cloned().collect::<Vec<_>>().join(", ")
                ));
            }
            for image in &lost.images {
                what.push(format!("image not imported: {image}"));
            }
            report.conversion.push(format!("{shown}: {}", what.join("; ")));
        }
        if note_flag(item, "protected") {
            report.protected.push(shown.clone());
        }

        // I8: the folder of the tag with the fewest notes, the others as tags.
        let mut own: Vec<&Item> = tags_of.get(item.uuid.as_str()).cloned().unwrap_or_default();
        own.sort_by_key(|t| {
            (
                count_of.get(t.uuid.as_str()).copied().unwrap_or(0),
                t.str("title").to_lowercase(),
                t.uuid.clone(),
            )
        });
        let folder = own
            .first()
            .and_then(|t| folder_of_tag.get(t.uuid.as_str()).cloned())
            .or_else(|| root.clone());
        if own.len() > 1 {
            report.several_tags.push(format!("{shown} → {}", own[0].str("title")));
        }

        // I5: the time of Standard Notes, not of the import.
        let at = item.updated_ms;
        w.set_at(KIND_NOTE, &id, "title", json!(title), at)?;
        w.set_at(KIND_NOTE, &id, "created", json!(item.created_ms), at)?;
        if let Some(folder) = &folder {
            w.set_at(KIND_NOTE, &id, "folder", json!(folder), at)?;
        }
        // I6
        for flag in ["pinned", "starred", "archived", "locked"] {
            if note_flag(item, flag) {
                w.set_at(KIND_NOTE, &id, flag, json!(true), at)?;
            }
        }
        if note_flag(item, "trashed") {
            // In the Trash from the moment of the import, for `trash_days` from it.
            let now = w.now_ms();
            w.set_at(KIND_NOTE, &id, "deleted", json!(now), at)?;
        }
        for tag in own.iter().skip(1) {
            let typed = tag.str("title").replace([',', '#'], " ");
            let Ok(name) = tag::clean(&typed) else { continue };
            let name = spelling(w.tx, &tag::key(&name))?.unwrap_or(name);
            w.set_at(KIND_NOTE, &id, &tag::require_field(&name)?, json!(true), at)?;
            tag_names.insert(tag::key(&name));
        }
        body::create_imported(w, &id, &text, seed(&item.uuid), at)?;

        // I9
        for file in &mine {
            match &file.content {
                Some(content) => {
                    attach(w, &attachment_of(file, &item.uuid), &id, file, content)?;
                    report.attachments += 1;
                }
                None => report.files_not_imported.push(format!("{} ({shown})", file.name)),
            }
        }
        w.flush()?;
        report.notes += 1;
    }

    // I9: the files of no note.
    let unlinked: Vec<&File> = files
        .iter()
        .filter(|f| f.notes.is_empty() && f.content.is_some())
        .collect();
    if options.collect_unlinked_files && !unlinked.is_empty() {
        let id = id_for("note", "files-from-standard-notes");
        if !exists(w.tx, KIND_NOTE, &id)? {
            let at = unlinked.iter().map(|f| f.updated_ms).max().unwrap_or(0);
            let mut lines = Vec::new();
            for file in &unlinked {
                let attachment = attachment_of(file, "unlinked");
                let content = file.content.as_ref().expect("filtered on content");
                attach(w, &attachment, &id, file, content)?;
                report.attachments += 1;
                let bang = if file.inline() { "!" } else { "" };
                lines.push(format!(
                    "{bang}[{}](attachment:{attachment})",
                    crate::editing::escape_text(&file.name)
                ));
            }
            w.set_at(KIND_NOTE, &id, "title", json!(UNLINKED_NOTE), at)?;
            w.set_at(KIND_NOTE, &id, "created", json!(at), at)?;
            if let Some(root) = &root {
                w.set_at(KIND_NOTE, &id, "folder", json!(root), at)?;
            }
            body::create_imported(w, &id, &lines.join("\n\n"), seed("files-from-standard-notes"), at)?;
            w.flush()?;
            report.notes += 1;
        }
    }

    // I10
    for view in views {
        let id = id_for("filter", &view.uuid);
        if exists(w.tx, KIND_FILTER, &id)? {
            report.already_imported += 1;
            continue;
        }
        let title = view.str("title").trim().to_string();
        let folder = |t: &str| folder_by_title.get(t).cloned();
        let query = view
            .content
            .get("predicate")
            .and_then(|p| predicate::to_query(p, &folder))
            .filter(|_| !title.is_empty());
        let Some(query) = query else {
            report.filters_not_imported.push(shown_title(&title, ""));
            continue;
        };
        let last = last_pos(w.tx, "SELECT max(pos) FROM filters WHERE deleted = 0", &[])?;
        w.set(KIND_FILTER, &id, "name", json!(title))?;
        w.set(
            KIND_FILTER,
            &id,
            "icon",
            json!(emoji(view.str("iconString")).unwrap_or_default()),
        )?;
        w.set(KIND_FILTER, &id, "query", serde_json::to_value(&query)?)?;
        w.set(KIND_FILTER, &id, "pos", json!(order::between(last.as_deref(), None)))?;
        w.flush()?;
        report.filters += 1;
    }
    report.tags = tag_names.len() as u32;
    Ok(())
}

fn new_folder(w: &mut Writer, id: &str, name: &str, parent: Option<&str>, icon: Option<String>) -> Result<()> {
    let pos = order::between(last_folder_pos(w.tx, parent)?.as_deref(), None);
    w.folder(id, "name", json!(name))?;
    w.folder(id, "pos", json!(pos))?;
    if let Some(parent) = parent {
        w.folder(id, "parent", json!(parent))?;
    }
    if let Some(icon) = icon {
        w.folder(id, "icon", json!(icon))?;
    }
    w.flush()
}

fn attach(w: &mut Writer, id: &str, note: &str, file: &File, content: &(String, u64)) -> Result<()> {
    let (sha256, size) = content;
    let mime = if file.mime.is_empty() {
        guess_mime(&file.name).to_string()
    } else {
        file.mime.clone()
    };
    let at = file.updated_ms;
    w.set_at(KIND_ATTACHMENT, id, "note", json!(note), at)?;
    w.set_at(KIND_ATTACHMENT, id, "name", json!(file.name), at)?;
    w.set_at(KIND_ATTACHMENT, id, "mime", json!(mime), at)?;
    w.set_at(KIND_ATTACHMENT, id, "size", json!(size), at)?;
    w.set_at(KIND_ATTACHMENT, id, "sha256", json!(sha256), at)
}

/// I12: the report as text, in the order of the requirements.
fn report_text(r: &ImportReport) -> String {
    let mut out = vec![
        format!(
            "Imported: {} notes, {} folders, {} tags, {} filters, {} attachments.",
            r.notes, r.folders, r.tags, r.filters, r.attachments
        ),
        format!("Skipped: {} already imported, {} empty.", r.already_imported, r.empty),
    ];
    let mut list = |head: &str, entries: &[String]| {
        if !entries.is_empty() {
            out.push(String::new());
            out.push(head.to_string());
            out.extend(entries.iter().map(|e| format!("- {e}")));
        }
    };
    list(
        "Notes of another type, imported with their text as it is:",
        &r.other_types,
    );
    list("Protected notes, imported as ordinary notes:", &r.protected);
    list("Notes that lost part of their formatting:", &r.conversion);
    list("Notes with several tags, and the folder chosen:", &r.several_tags);
    list("Files not imported:", &r.files_not_imported);
    list("Smart views not imported:", &r.filters_not_imported);
    let ignored: Vec<String> = r
        .ignored
        .iter()
        .map(|i| format!("{}: {}", i.content_type, i.count))
        .collect();
    list("Items not imported, by type:", &ignored);
    out.join("\n")
}
