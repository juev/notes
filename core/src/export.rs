//! Export to JSON and Markdown: R155 and R156 in docs/specs/product.md. The JSON
//! format is described in docs/json-format.md and read back by `import.rs` (I11).

use std::io::Write;

use serde_json::{json, Value};

use crate::error::{AppError, Result};
use crate::model::{Freq, Priority, Repeat, TaskItem, INBOX_ID};
use crate::store::{query_tasks, Store};
use crate::LogLevel;

/// What `format` says in a file of ours; `version` changes when a reader would misread a file.
pub(crate) const FORMAT: &str = "lists";
pub(crate) const VERSION: u64 = 1;

pub(crate) fn priority_name(p: Priority) -> &'static str {
    match p {
        Priority::None => "none",
        Priority::Low => "low",
        Priority::Medium => "medium",
        Priority::High => "high",
    }
}

/// Parents before their subtasks, each in its place in the list; the records of finished
/// repeats are tasks done at that moment. The trash is left out.
fn exported_tasks(store: &Store, list: Option<&str>) -> Result<Vec<TaskItem>> {
    let inner = store.lock();
    let order = "ORDER BY t.parent_id IS NOT NULL, t.pos, t.id";
    match list {
        Some(list) => query_tasks(
            &inner.conn,
            &format!("WHERE t.eff_deleted = 0 AND t.list_id = ?1 {order}"),
            &[list],
        ),
        None => query_tasks(&inner.conn, &format!("WHERE t.eff_deleted = 0 {order}"), &[]),
    }
}

fn write_file(path: &str, data: &[u8]) -> Result<()> {
    let target = std::path::Path::new(path);
    let tmp = target.with_extension(format!("{}.part", uuid::Uuid::now_v7().simple()));
    let written = (|| -> Result<()> {
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(data)?;
        file.sync_all()?;
        Ok(std::fs::rename(&tmp, target)?)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written
}

impl Store {
    /// R155: the export as a JSON value: everything, or the list `list`.
    pub(crate) fn export_value(&self, list: Option<&str>) -> Result<Value> {
        let lists: Vec<Value> = self
            .lists()?
            .into_iter()
            .filter(|l| list.is_none_or(|id| l.id == id))
            .map(|l| {
                json!({
                    "id": l.id, "name": l.name, "color": l.color, "icon": l.icon, "archived": l.archived,
                    "sort": l.sort.as_str(), "showDone": l.show_done,
                    "defaultPriority": priority_name(l.default_priority), "defaultDueToday": l.default_due_today,
                })
            })
            .collect();
        if let Some(id) = list {
            if lists.is_empty() {
                return Err(AppError::not_found(format!("list {id}")));
            }
        }
        let mut tasks = Vec::new();
        for t in exported_tasks(self, list)? {
            let attachments: Vec<Value> = if t.attachments > 0 {
                self.attachments(t.id.clone())?
                    .into_iter()
                    .map(|a| json!({ "name": a.name, "mime": a.mime, "size": a.size, "sha256": a.sha256 }))
                    .collect()
            } else {
                vec![]
            };
            tasks.push(json!({
                "id": t.id, "list": t.list_id, "parent": t.parent_id, "title": t.title, "notes": t.notes,
                "start": t.start, "due": t.due, "remind": t.remind, "priority": priority_name(t.priority),
                "tags": t.tags, "repeat": t.repeat, "done": t.done, "wontDo": t.wont,
                "project": t.is_project, "attachments": attachments,
            }));
        }
        let filters: Vec<Value> = if list.is_none() {
            self.filters()?
                .into_iter()
                .map(|f| Ok(json!({ "id": f.id, "name": f.name, "spec": serde_json::to_value(&f.spec)? })))
                .collect::<Result<_>>()?
        } else {
            vec![]
        };
        let exported = self.lock().now().format("%Y-%m-%dT%H:%M").to_string();
        Ok(json!({
            "format": FORMAT, "version": VERSION, "exported": exported,
            "lists": lists, "tasks": tasks, "filters": filters,
        }))
    }

    /// R156: the export as Markdown, meant for reading.
    pub(crate) fn export_markdown_text(&self, list: Option<&str>) -> Result<String> {
        let lists = self.lists()?;
        let tasks = exported_tasks(self, list)?;
        let exported = self.lock().now().format("%Y-%m-%d").to_string();
        let mut out = format!("# Lists, {exported}\n");
        for l in lists.iter().filter(|l| list.is_none_or(|id| l.id == id)) {
            let name = if l.id == INBOX_ID { "Inbox" } else { l.name.as_str() };
            out.push_str(&format!("\n## {}\n\n", one_line(name)));
            let top: Vec<&TaskItem> = tasks
                .iter()
                .filter(|t| t.list_id == l.id && t.parent_id.is_none())
                .collect();
            if top.is_empty() {
                out.push_str("No tasks.\n");
            }
            for t in top {
                markdown_task(&mut out, t, &tasks, 0);
            }
        }
        Ok(out)
    }
}

/// A name or a title as one line of Markdown.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn repeat_text(r: &Repeat) -> String {
    let unit = match r.freq {
        Freq::Daily => "day",
        Freq::Weekly => "week",
        Freq::Monthly => "month",
        Freq::Yearly => "year",
    };
    if r.interval <= 1 {
        format!("every {unit}")
    } else {
        format!("every {} {unit}s", r.interval)
    }
}

fn markdown_task(out: &mut String, t: &TaskItem, all: &[TaskItem], depth: usize) {
    let indent = "  ".repeat(depth);
    let mark = if t.done.is_some() { "x" } else { " " };
    let title = one_line(&t.title);
    let title = if t.wont { format!("~~{title}~~") } else { title };
    let mut line = format!("{indent}- [{mark}] {title}");
    let mut facts = Vec::new();
    if t.is_project {
        facts.push("project".to_string());
    }
    if t.wont {
        facts.push("won't do".to_string());
    }
    if let Some(start) = &t.start {
        facts.push(format!("start {}", start.replace('T', " ")));
    }
    if let Some(due) = &t.due {
        facts.push(format!("due {}", due.replace('T', " ")));
    }
    if let Some(remind) = &t.remind {
        facts.push(format!("reminder {}", remind.replace('T', " ")));
    }
    if let Some(r) = &t.repeat {
        facts.push(repeat_text(r));
    }
    match t.priority {
        Priority::None => {}
        p => facts.push(format!("{} priority", priority_name(p))),
    }
    if !t.tags.is_empty() {
        facts.push(t.tags.iter().map(|g| format!("#{g}")).collect::<Vec<_>>().join(" "));
    }
    if t.attachments > 0 {
        facts.push(format!(
            "{} attachment{}",
            t.attachments,
            if t.attachments == 1 { "" } else { "s" }
        ));
    }
    if !facts.is_empty() {
        line.push_str(" — ");
        line.push_str(&facts.join(" · "));
    }
    out.push_str(&line);
    out.push('\n');
    // The notes stand under the task, indented as its content.
    for note in t.notes.lines() {
        if note.trim().is_empty() {
            out.push('\n');
        } else {
            out.push_str(&format!("{indent}  {note}\n"));
        }
    }
    for sub in all.iter().filter(|s| s.parent_id.as_deref() == Some(t.id.as_str())) {
        markdown_task(out, sub, all, depth + 1);
    }
}

#[uniffi::export]
impl Store {
    /// R155: writes the JSON export to `path`: everything, or the list `list_id`.
    pub fn export_json(&self, path: String, list_id: Option<String>) -> Result<()> {
        let result = self
            .export_value(list_id.as_deref())
            .and_then(|value| write_file(&path, &serde_json::to_vec_pretty(&value)?));
        self.note_result("export", &result, LogLevel::Info, |_| "JSON written".into());
        result
    }

    /// R156: writes the Markdown export to `path`: everything, or the list `list_id`.
    pub fn export_markdown(&self, path: String, list_id: Option<String>) -> Result<()> {
        let result = self
            .export_markdown_text(list_id.as_deref())
            .and_then(|text| write_file(&path, text.as_bytes()));
        self.note_result("export", &result, LogLevel::Info, |_| "Markdown written".into());
        result
    }
}
