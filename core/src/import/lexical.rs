//! I7: the Lexical document of a Super note as Markdown of the dialect (docs/specs/editor.md),
//! node by node, by the table of docs/specs/import.md.

use std::collections::BTreeSet;

use serde_json::Value;

use crate::editing::{destination, escape_line_starts, escape_text, wrap};

/// A file of Standard Notes as the note links it (I9).
pub(crate) enum FileLink {
    /// Imported as the attachment `id`; `inline` for images, video, audio and PDF (E30).
    Attached { id: String, name: String, inline: bool },
    /// Not imported (no content found, or more than one match).
    Missing { name: String },
}

/// What the conversion of one note could not keep, for the report (I12).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Lost {
    /// Underline, subscript or superscript was dropped.
    pub formats: bool,
    /// Node types that were taken as their text.
    pub nodes: BTreeSet<String>,
    /// Images not imported, by their alt text or address.
    pub images: Vec<String>,
}

impl Lost {
    pub fn is_empty(&self) -> bool {
        !self.formats && self.nodes.is_empty() && self.images.is_empty()
    }
}

const BOLD: u64 = 1;
const ITALIC: u64 = 2;
const STRIKETHROUGH: u64 = 4;
const UNDERLINE: u64 = 8;
const CODE: u64 = 16;
const SUBSCRIPT: u64 = 32;
const SUPERSCRIPT: u64 = 64;
const HIGHLIGHT: u64 = 128;

struct Converter<'a> {
    files: &'a dyn Fn(&str) -> FileLink,
    lost: Lost,
}

/// I7: `text` of a Super note. An empty text is an empty note; a text that is not a Lexical
/// document is kept as it is.
pub(crate) fn to_markdown(text: &str, files: &dyn Fn(&str) -> FileLink) -> (String, Lost) {
    let mut converter = Converter {
        files,
        lost: Lost::default(),
    };
    if text.trim().is_empty() {
        return (String::new(), converter.lost);
    }
    let Ok(document) = serde_json::from_str::<Value>(text) else {
        return (text.to_string(), converter.lost);
    };
    let Some(root) = document.get("root") else {
        return (text.to_string(), converter.lost);
    };
    let blocks = converter.blocks(children(root));
    (blocks.join("\n\n"), converter.lost)
}

fn children(node: &Value) -> &[Value] {
    node.get("children")
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

fn kind(node: &Value) -> &str {
    node.get("type").and_then(Value::as_str).unwrap_or_default()
}

fn field<'v>(node: &'v Value, key: &str) -> &'v str {
    node.get(key).and_then(Value::as_str).unwrap_or_default()
}

/// The plain text of a node and its children.
fn text_of(node: &Value) -> String {
    match kind(node) {
        "linebreak" => "\n".into(),
        "tab" => "\t".into(),
        _ => {
            let own = field(node, "text").to_string();
            own + &children(node).iter().map(text_of).collect::<String>()
        }
    }
}

impl Converter<'_> {
    fn blocks(&mut self, nodes: &[Value]) -> Vec<String> {
        nodes
            .iter()
            .filter_map(|n| self.block(n))
            .filter(|b| !b.is_empty())
            .collect()
    }

    fn block(&mut self, node: &Value) -> Option<String> {
        Some(match kind(node) {
            // An empty paragraph between blocks is dropped by the filter of `blocks`.
            "paragraph" => escape_line_starts(&self.inline(children(node))),
            "heading" => {
                let level = field(node, "tag")
                    .strip_prefix('h')
                    .and_then(|n| n.parse::<usize>().ok())
                    .unwrap_or(1)
                    .clamp(1, 6);
                let text = self.inline(children(node)).replace("\\\n", " ");
                if text.trim().is_empty() {
                    return None;
                }
                format!("{} {}", "#".repeat(level), text.trim())
            }
            "list" => self.list(node, 0).join("\n"),
            "quote" => {
                let text = escape_line_starts(&self.inline(children(node)));
                text.split('\n')
                    .map(|l| {
                        if l.is_empty() {
                            ">".to_string()
                        } else {
                            format!("> {l}")
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            }
            "code" => {
                let code: String = children(node).iter().map(text_of).collect();
                let longest = code.split(|c| c != '`').map(str::len).max().unwrap_or(0);
                let fence = "`".repeat(longest.max(2) + 1);
                format!("{fence}{}\n{code}\n{fence}", field(node, "language"))
            }
            "table" => self.table(node),
            "horizontalrule" => "---".into(),
            "snfile" => self.file(field(node, "fileUuid")),
            "unencrypted-image" => self.image(node),
            _ => {
                if kind(node) != "root" {
                    self.lost.nodes.insert(kind(node).to_string());
                }
                let text = text_of(node);
                text.split("\n\n")
                    .map(|p| escape_line_starts(&escape_text(p.trim())))
                    .filter(|p| !p.is_empty())
                    .collect::<Vec<_>>()
                    .join("\n\n")
            }
        })
    }

    fn inline(&mut self, nodes: &[Value]) -> String {
        let mut out = String::new();
        for node in nodes {
            match kind(node) {
                "text" | "code-highlight" => out.push_str(&self.text(node)),
                "linebreak" => out.push_str("\\\n"),
                "tab" => out.push('\t'),
                "hashtag" => out.push_str(&escape_text(&text_of(node))),
                "link" => {
                    let inner = self.inline(children(node));
                    let url = field(node, "url");
                    if url.is_empty() || inner.trim().is_empty() {
                        out.push_str(&inner);
                    } else {
                        out.push_str(&format!("[{inner}]({})", destination(url)));
                    }
                }
                "autolink" => {
                    let text = text_of(node);
                    if node.get("isUnlinked").and_then(Value::as_bool).unwrap_or(false) {
                        out.push_str(&escape_text(&text));
                    } else {
                        out.push_str(&text);
                    }
                }
                "snfile" => out.push_str(&self.file(field(node, "fileUuid"))),
                "unencrypted-image" => out.push_str(&self.image(node)),
                other => {
                    self.lost.nodes.insert(other.to_string());
                    out.push_str(&escape_text(&text_of(node)));
                }
            }
        }
        out
    }

    /// A text node with the formats of its bit mask.
    fn text(&mut self, node: &Value) -> String {
        let text = field(node, "text");
        let format = node.get("format").and_then(Value::as_u64).unwrap_or(0);
        if format & (UNDERLINE | SUBSCRIPT | SUPERSCRIPT) != 0 {
            self.lost.formats = true;
        }
        let mut out = if format & CODE != 0 {
            let longest = text.split(|c| c != '`').map(str::len).max().unwrap_or(0);
            let ticks = "`".repeat(longest + 1);
            let pad = if text.starts_with('`') || text.ends_with('`') {
                " "
            } else {
                ""
            };
            if text.trim().is_empty() {
                text.to_string()
            } else {
                format!("{ticks}{pad}{text}{pad}{ticks}")
            }
        } else {
            escape_text(text)
        };
        for (bit, marker) in [(HIGHLIGHT, "=="), (STRIKETHROUGH, "~~"), (ITALIC, "*"), (BOLD, "**")] {
            if format & bit != 0 {
                out = wrap(&out, marker);
            }
        }
        out
    }

    /// The lines of a list at `indent`; a list item that holds a list nests it under the item
    /// before it.
    fn list(&mut self, node: &Value, indent: usize) -> Vec<String> {
        let list_type = field(node, "listType");
        let mut number = node.get("start").and_then(Value::as_u64).unwrap_or(1);
        let mut lines = Vec::new();
        let mut width = 2;
        for item in children(node) {
            let (nested, own): (Vec<&Value>, Vec<&Value>) = children(item).iter().partition(|c| kind(c) == "list");
            if !own.is_empty() || nested.is_empty() {
                let marker = match list_type {
                    "number" => {
                        let m = format!("{number}. ");
                        number += 1;
                        m
                    }
                    "check" => {
                        let checked = item.get("checked").and_then(Value::as_bool).unwrap_or(false);
                        if checked { "- [x] " } else { "- [ ] " }.to_string()
                    }
                    _ => "- ".to_string(),
                };
                width = if list_type == "check" { 2 } else { marker.len() };
                let own: Vec<Value> = own.into_iter().cloned().collect();
                let text = self.inline(&own);
                let pad = " ".repeat(indent + width);
                let mut parts = text.split('\n');
                lines.push(format!(
                    "{}{marker}{}",
                    " ".repeat(indent),
                    parts.next().unwrap_or_default()
                ));
                lines.extend(parts.map(|l| format!("{pad}{l}")));
            }
            for list in nested {
                lines.extend(self.list(list, indent + width));
            }
        }
        lines
    }

    fn table(&mut self, node: &Value) -> String {
        let rows: Vec<Vec<String>> = children(node)
            .iter()
            .map(|row| {
                children(row)
                    .iter()
                    .map(|cell| {
                        // Paragraphs of a cell are joined with a space.
                        let parts: Vec<String> = children(cell)
                            .iter()
                            .map(|p| self.inline(children(p)).replace("\\\n", " "))
                            .filter(|p| !p.trim().is_empty())
                            .collect();
                        parts.join(" ").replace('|', "\\|").trim().to_string()
                    })
                    .collect()
            })
            .filter(|r: &Vec<String>| !r.is_empty())
            .collect();
        let Some(columns) = rows.iter().map(Vec::len).max() else {
            return String::new();
        };
        let line = |cells: &[String]| {
            let mut cells = cells.to_vec();
            cells.resize(columns, String::new());
            format!("| {} |", cells.join(" | "))
        };
        let mut out = vec![line(&rows[0]), format!("|{}", " --- |".repeat(columns))];
        out.extend(rows[1..].iter().map(|r| line(r)));
        out.join("\n")
    }

    fn file(&mut self, uuid: &str) -> String {
        match (self.files)(uuid) {
            FileLink::Attached { id, name, inline } => {
                let bang = if inline { "!" } else { "" };
                format!("{bang}[{}](attachment:{id})", escape_text(&name))
            }
            FileLink::Missing { name } => escape_text(&format!("[File not imported: {name}]")),
        }
    }

    fn image(&mut self, node: &Value) -> String {
        let src = field(node, "src");
        let alt = field(node, "alt");
        let lower = src.to_ascii_lowercase();
        if lower.starts_with("http://") || lower.starts_with("https://") {
            return format!(
                "[{}]({})",
                escape_text(if alt.is_empty() { src } else { alt }),
                destination(src)
            );
        }
        let what = if alt.is_empty() { src } else { alt };
        self.lost.images.push(what.to_string());
        escape_text(&format!("[Image not imported: {what}]"))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::markdown::markdown_layout;
    use crate::MarkdownKind;

    fn text(t: &str, format: u64) -> Value {
        json!({"type": "text", "text": t, "format": format})
    }

    fn paragraph(children: Vec<Value>) -> Value {
        json!({"type": "paragraph", "children": children})
    }

    fn convert(blocks: Vec<Value>) -> (String, Lost) {
        let doc = json!({"root": {"type": "root", "children": blocks}}).to_string();
        let files = |uuid: &str| match uuid {
            "f-img" => FileLink::Attached {
                id: "a1".into(),
                name: "фото.jpg".into(),
                inline: true,
            },
            "f-doc" => FileLink::Attached {
                id: "a2".into(),
                name: "отчёт.docx".into(),
                inline: false,
            },
            _ => FileLink::Missing {
                name: "нет.pdf".into()
            },
        };
        to_markdown(&doc, &files)
    }

    #[test]
    fn i7_every_node_of_the_table() {
        let blocks = vec![
            json!({"type": "heading", "tag": "h2", "children": [text("План", 0)]}),
            paragraph(vec![
                text("жирный", 1),
                text(" ", 0),
                text("курсив", 2),
                text(" ", 0),
                text("оба", 3),
                text(" ", 0),
                text("код", 16),
                text(" ", 0),
                text("жирный код", 17),
                json!({"type": "linebreak"}),
                text("после\tразрыва", 0),
                json!({"type": "tab"}),
                json!({"type": "link", "url": "https://example.org", "children": [text("сайт", 0)]}),
                text(" ", 0),
                json!({"type": "autolink", "url": "https://a.example", "children": [text("https://a.example", 0)]}),
                text(" ", 0),
                json!({"type": "autolink", "isUnlinked": true, "children": [text("b_c.example", 0)]}),
                text(" ", 0),
                json!({"type": "hashtag", "text": "#метка"}),
            ]),
            paragraph(vec![]),
            json!({"type": "list", "listType": "bullet", "children": [
                {"type": "listitem", "children": [text("раз", 0)]},
                {"type": "listitem", "children": [
                    {"type": "list", "listType": "number", "children": [
                        {"type": "listitem", "children": [text("вложенный", 0)]},
                        {"type": "listitem", "children": [text("второй", 0)]}
                    ]}
                ]},
                {"type": "listitem", "children": [text("два", 0)]}
            ]}),
            json!({"type": "list", "listType": "check", "children": [
                {"type": "listitem", "checked": true, "children": [text("сделано", 0)]},
                {"type": "listitem", "checked": false, "children": [text("нет", 0)]}
            ]}),
            json!({"type": "quote", "children": [text("цитата", 0), {"type": "linebreak"}, text("дальше", 0)]}),
            json!({"type": "code", "language": "rust", "children": [
                {"type": "code-highlight", "text": "fn main() {"},
                {"type": "linebreak"},
                {"type": "code-highlight", "text": "    let s = \"**x**\";"},
                {"type": "linebreak"},
                {"type": "code-highlight", "text": "}"}
            ]}),
            json!({"type": "table", "children": [
                {"type": "tablerow", "children": [
                    {"type": "tablecell", "headerState": 1, "children": [paragraph(vec![text("Имя", 0)])]},
                    {"type": "tablecell", "headerState": 1, "children": [paragraph(vec![text("a|b", 0)])]}
                ]},
                {"type": "tablerow", "children": [
                    {"type": "tablecell", "headerState": 0, "children": [paragraph(vec![text("раз", 0)]), paragraph(vec![text("два", 0)])]},
                    {"type": "tablecell", "headerState": 0, "children": [paragraph(vec![text("3", 1)])]}
                ]}
            ]}),
            json!({"type": "horizontalrule"}),
            json!({"type": "snfile", "fileUuid": "f-img"}),
            json!({"type": "snfile", "fileUuid": "f-doc"}),
            json!({"type": "snfile", "fileUuid": "f-lost"}),
        ];
        let (markdown, lost) = convert(blocks);
        assert_eq!(
            markdown,
            "## План\n\n\
             **жирный** *курсив* ***оба*** `код` **`жирный код`**\\\n\
             после\tразрыва\t[сайт](https://example.org) https://a.example b\\_c.example #метка\n\n\
             - раз\n  1. вложенный\n  2. второй\n- два\n\n\
             - [x] сделано\n- [ ] нет\n\n\
             > цитата\\\n> дальше\n\n\
             ```rust\nfn main() {\n    let s = \"**x**\";\n}\n```\n\n\
             | Имя | a\\|b |\n| --- | --- |\n| раз два | **3** |\n\n\
             ---\n\n\
             ![фото.jpg](attachment:a1)\n\n\
             [отчёт.docx](attachment:a2)\n\n\
             \\[File not imported: нет.pdf\\]"
        );
        assert!(lost.is_empty(), "{lost:?}");
        // It parses back into the same structure.
        let kinds: Vec<MarkdownKind> = markdown_layout(markdown.clone())
            .spans
            .into_iter()
            .map(|s| s.kind)
            .collect();
        for kind in [
            MarkdownKind::Heading { level: 2 },
            MarkdownKind::Strong,
            MarkdownKind::Emphasis,
            MarkdownKind::Code,
            MarkdownKind::Link {
                url: "https://example.org".into(),
            },
            MarkdownKind::Link {
                url: "https://a.example".into(),
            },
            MarkdownKind::ListMarker { ordered: false },
            MarkdownKind::ListMarker { ordered: true },
            MarkdownKind::Checkbox { checked: true },
            MarkdownKind::Checkbox { checked: false },
            MarkdownKind::Quote,
            MarkdownKind::CodeBlock,
            MarkdownKind::TableRow,
            MarkdownKind::Rule,
            MarkdownKind::AttachmentLink {
                id: "a1".into(),
                inline: true,
            },
            MarkdownKind::AttachmentLink {
                id: "a2".into(),
                inline: false,
            },
        ] {
            assert!(kinds.contains(&kind), "{kind:?} in\n{markdown}");
        }
        assert!(
            !kinds
                .iter()
                .any(|k| matches!(k, MarkdownKind::Heading { .. } if *k != MarkdownKind::Heading { level: 2 })),
            "#метка is a word, not a heading"
        );
    }

    #[test]
    fn i7_characters_of_markdown_stay_literal() {
        let (markdown, _) = convert(vec![
            paragraph(vec![text("# не заголовок", 0)]),
            paragraph(vec![text(
                "- не список, 1. и *звёзды* _подчерки_ `тики` [скобки] ==",
                0,
            )]),
        ]);
        assert_eq!(
            markdown,
            "\\# не заголовок\n\n\\- не список, 1. и \\*звёзды\\* \\_подчерки\\_ \\`тики\\` \\[скобки\\] \\=\\="
        );
        let layout = markdown_layout(markdown);
        assert!(
            layout.spans.iter().all(|s| s.kind == MarkdownKind::Markup),
            "only the escapes are markup: {:?}",
            layout.spans
        );
    }

    #[test]
    fn i7_what_has_no_markdown_is_dropped_and_reported() {
        let (markdown, lost) = convert(vec![paragraph(vec![
            text("подчёркнуто", 8),
            text(" H", 0),
            text("2", 32),
            text("O x", 0),
            text("2", 64),
        ])]);
        assert_eq!(markdown, "подчёркнуто H2O x2");
        assert!(lost.formats);

        let (markdown, lost) = convert(vec![
            json!({"type": "unencrypted-image", "src": "../_resources/x.jpg", "alt": ""}),
            json!({"type": "unencrypted-image", "src": "https://example.org/y.png", "alt": "схема"}),
            json!({"type": "collapsible-container", "children": [paragraph(vec![text("внутри", 0)])]}),
        ]);
        assert_eq!(
            markdown,
            "\\[Image not imported: ../\\_resources/x.jpg\\]\n\n[схема](https://example.org/y.png)\n\nвнутри"
        );
        assert_eq!(lost.images, ["../_resources/x.jpg"]);
        assert_eq!(lost.nodes.into_iter().collect::<Vec<_>>(), ["collapsible-container"]);
    }

    #[test]
    fn i7_an_empty_text_and_a_text_that_is_not_lexical() {
        let none = |_: &str| FileLink::Missing { name: String::new() };
        assert_eq!(to_markdown("", &none).0, "");
        assert_eq!(to_markdown("просто текст", &none).0, "просто текст");
        assert_eq!(to_markdown("{\"a\": 1}", &none).0, "{\"a\": 1}");
    }
}
