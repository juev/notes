//! The edits an editor asks the core for (E21, E22, E23, E26 in docs/specs/editor.md). Each
//! returns one replacement for the platform to apply as ordinary input, or nothing where the
//! keystroke does what it does anywhere else. Positions are UTF-16 units, as everywhere.

use std::ops::Range;

use scraper::{ElementRef, Html, Node};

use crate::markdown::{address, layout, MarkdownEdit, MarkdownKind, Prefix, Utf16};

/// A format from the toolbar, the menu or a shortcut (E20, E21).
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum MarkdownFormat {
    Bold,
    Italic,
    Strikethrough,
    Code,
    Highlight,
    Link,
    Heading {
        level: u8,
    },
    /// Takes the line format off.
    Paragraph,
    BulletList,
    NumberedList,
    TaskList,
    Quote,
}

/// E21: applies `format` to the selection `start..end`, or to the word at the cursor when nothing
/// is selected. An inline format set there already is taken off; a line format replaces the one
/// the lines have, and taken off when every line has it already.
#[uniffi::export]
pub fn markdown_format(text: String, start: u32, end: u32, format: MarkdownFormat) -> Option<MarkdownEdit> {
    let units = Utf16::of(&text);
    let (from, to) = (units.byte(start.min(end))?, units.byte(start.max(end))?);
    let cursor = units.byte(end)?;
    let inline = |marker: &str, kind: MarkdownKind| toggle_inline(&text, from, to, cursor, marker, kind);
    let change = match format {
        MarkdownFormat::Bold => inline("**", MarkdownKind::Strong),
        MarkdownFormat::Italic => inline("*", MarkdownKind::Emphasis),
        MarkdownFormat::Strikethrough => inline("~~", MarkdownKind::Strikethrough),
        MarkdownFormat::Code => inline("`", MarkdownKind::Code),
        MarkdownFormat::Highlight => inline("==", MarkdownKind::Highlight),
        MarkdownFormat::Link => toggle_link(&text, from, to, cursor),
        line => toggle_lines(&text, from, to, cursor, line)?,
    };
    Some(change.edit(&text))
}

/// E22: Tab, or ⇧Tab with `outdent`, on the list items of the selection: an item goes under the
/// item before it at its level, or back to the level of its parent. `None` outside a list, where
/// Tab is a tab.
#[uniffi::export]
pub fn markdown_indent(text: String, start: u32, end: u32, outdent: bool) -> Option<MarkdownEdit> {
    let units = Utf16::of(&text);
    let (from, to) = (units.byte(start.min(end))?, units.byte(start.max(end))?);
    let cursor = units.byte(end)?;
    let region = lines_of(&text, from, to);
    let mut lines: Vec<String> = text.split('\n').map(String::from).collect();
    let first = text[..region.start].matches('\n').count();
    let count = text[region.clone()].matches('\n').count() + 1;
    let mut moved = false;
    let mut shift: isize = 0;
    for n in first..first + count {
        let Some(item) = item_of(&lines[n]) else { continue };
        let width = if outdent {
            if item.indent == 0 {
                continue;
            }
            match (0..n)
                .rev()
                .filter_map(|i| item_of(&lines[i]))
                .find(|p| p.indent < item.indent)
            {
                Some(parent) => parent.indent,
                None => 0,
            }
        } else {
            // Under the item before it at the same level; the first item of a level stays.
            match (0..n)
                .rev()
                .filter_map(|i| item_of(&lines[i]))
                .find(|p| p.indent <= item.indent)
            {
                Some(sibling) if sibling.indent == item.indent => sibling.content,
                _ => continue,
            }
        };
        let line = &lines[n];
        let rest = line.trim_start_matches([' ', '\t']);
        let new = format!("{}{rest}", " ".repeat(width));
        if n == first + count - 1 || from == to {
            shift = new.len() as isize - line.len() as isize;
        }
        lines[n] = new;
        moved = true;
    }
    if !moved {
        return None;
    }
    let replaced = lines[first..first + count].join("\n");
    let caret = if from == to {
        (cursor as isize + shift).max(region.start as isize) as usize
    } else {
        region.start + replaced.len()
    };
    let anchor = if from == to { caret } else { region.start };
    Some(
        Change {
            range: region,
            text: replaced,
            anchor,
            cursor: caret,
        }
        .edit(&text),
    )
}

/// E23: what pasting puts at `start..end`. Rich text, `html`, comes in as Markdown of the dialect;
/// a web address, `plain`, pasted over a selection makes a link with the selection as its text.
/// `None` when the plain text goes in as it is.
#[uniffi::export]
pub fn markdown_paste(
    text: String,
    start: u32,
    end: u32,
    plain: Option<String>,
    html: Option<String>,
) -> Option<MarkdownEdit> {
    let units = Utf16::of(&text);
    let (from, to) = (units.byte(start.min(end))?, units.byte(start.max(end))?);
    if let Some(markdown) = html.as_deref().map(markdown_from_html).filter(|m| !m.is_empty()) {
        let line_start = text[..from].rfind('\n').map_or(0, |i| i + 1);
        let line_end = text[to..].find('\n').map_or(text.len(), |i| to + i);
        let mut insert = markdown;
        // Blocks pasted into a line go on lines of their own.
        let block = insert.contains('\n');
        if block && !text[line_start..from].trim().is_empty() {
            insert.insert(0, '\n');
        }
        let caret = from + insert.len();
        if block && !text[to..line_end].trim().is_empty() {
            insert.push('\n');
        }
        return Some(
            Change {
                range: from..to,
                text: insert,
                anchor: caret,
                cursor: caret,
            }
            .edit(&text),
        );
    }
    let url = plain.as_deref().map(str::trim)?;
    if from == to || url.contains(char::is_whitespace) || address(url).is_none() {
        return None;
    }
    let link = format!("[{}]({})", &text[from..to], destination(url));
    let caret = from + link.len();
    Some(
        Change {
            range: from..to,
            text: link,
            anchor: caret,
            cursor: caret,
        }
        .edit(&text),
    )
}

/// E26: what typing replaces once the character before `cursor` is typed: straight double quotes
/// with «» in Russian text and “” in English, ` - ` with a non-breaking space and an em dash, three
/// dots with an ellipsis, `->` with an arrow. Nothing changes in code and in addresses.
#[uniffi::export]
pub fn markdown_typed(text: String, cursor: u32) -> Option<MarkdownEdit> {
    let units = Utf16::of(&text);
    let at = units.byte(cursor)?;
    let before = &text[..at];
    let typed = before.chars().next_back()?;
    let typed_at = at - typed.len_utf8();
    let quiet = layout(&text, false).no_spellcheck;
    let in_code = |at: usize| quiet.iter().any(|q| q.start <= at && at < q.end);
    if in_code(typed_at) {
        return None;
    }
    let replace = |range: Range<usize>, new: &str| {
        // What is replaced may begin in an address that the typed character ends.
        if in_code(range.start) {
            return None;
        }
        let caret = at - range.len() + new.len();
        Some(
            Change {
                range,
                text: new.to_string(),
                anchor: caret,
                cursor: caret,
            }
            .edit(&text),
        )
    };
    match typed {
        '"' => quotes(&text, typed_at, at),
        ' ' if before.ends_with(" - ") => {
            let line = &before[before.rfind('\n').map_or(0, |i| i + 1)..at - 3];
            if line.trim().is_empty() || line.ends_with(char::is_whitespace) {
                return None;
            }
            replace(at - 3..at, "\u{a0}— ")
        }
        '.' if before.ends_with("...") && !before.ends_with("....") => replace(at - 3..at, "…"),
        '>' if before.ends_with("->") && !before.ends_with("-->") => replace(at - 2..at, "→"),
        _ => None,
    }
}

/// A replacement in bytes of the text before it, with the selection after it in bytes of the text
/// after it.
struct Change {
    range: Range<usize>,
    text: String,
    anchor: usize,
    cursor: usize,
}

impl Change {
    fn edit(self, text: &str) -> MarkdownEdit {
        let units = Utf16::of(text);
        let after = format!("{}{}{}", &text[..self.range.start], self.text, &text[self.range.end..]);
        let new = Utf16::of(&after);
        MarkdownEdit {
            start: units.at(self.range.start),
            end: units.at(self.range.end),
            text: self.text,
            anchor: new.at(self.anchor),
            cursor: new.at(self.cursor),
        }
    }
}

/// The word around `at`: letters, digits and underscores.
fn word_at(text: &str, at: usize) -> Range<usize> {
    let part = |c: char| c.is_alphanumeric() || c == '_';
    let start = text[..at]
        .char_indices()
        .rev()
        .take_while(|(_, c)| part(*c))
        .last()
        .map_or(at, |(i, _)| i);
    let end = text[at..]
        .char_indices()
        .find(|(_, c)| !part(*c))
        .map_or(text.len(), |(i, _)| at + i);
    start..end
}

/// The selection without the spaces around it, or the word at the cursor when nothing is selected.
fn target(text: &str, from: usize, to: usize) -> Range<usize> {
    if from == to {
        return word_at(text, from);
    }
    let slice = &text[from..to];
    let start = from + (slice.len() - slice.trim_start().len());
    let end = to - (slice.len() - slice.trim_end().len());
    start..end.max(start)
}

fn toggle_inline(text: &str, from: usize, to: usize, cursor: usize, marker: &str, kind: MarkdownKind) -> Change {
    let parsed = layout(text, false);
    // Set already: the innermost span of the kind that holds the selection, with its markers.
    let set = parsed
        .spans
        .iter()
        .filter(|s| s.kind == kind && s.reveal.start <= from && to <= s.reveal.end)
        .min_by_key(|s| s.reveal.len());
    if let Some(span) = set {
        let whole = span.reveal.clone();
        let marks: Vec<Range<usize>> = parsed
            .spans
            .iter()
            .filter(|s| s.kind == MarkdownKind::Markup && s.reveal == whole)
            .map(|s| s.range.clone())
            .collect();
        let mut kept = String::new();
        let mut at = whole.start;
        for mark in &marks {
            kept.push_str(&text[at..mark.start]);
            at = mark.end;
        }
        kept.push_str(&text[at..whole.end]);
        let removed_before = |pos: usize| -> usize {
            marks
                .iter()
                .map(|m| m.end.min(pos).saturating_sub(m.start))
                .sum::<usize>()
        };
        let place = |pos: usize| pos - removed_before(pos);
        return Change {
            range: whole,
            text: kept,
            anchor: place(from),
            cursor: place(cursor),
        };
    }
    let word = target(text, from, to);
    let inner = &text[word.clone()];
    let wrapped = format!("{marker}{inner}{marker}");
    let (anchor, caret) = if from == to {
        let caret = cursor + marker.len();
        (caret, caret)
    } else {
        (word.start + marker.len(), word.end + marker.len())
    };
    Change {
        range: word,
        text: wrapped,
        anchor,
        cursor: caret,
    }
}

fn toggle_link(text: &str, from: usize, to: usize, cursor: usize) -> Change {
    let parsed = layout(text, false);
    let link = parsed
        .spans
        .iter()
        .filter(|s| {
            matches!(s.kind, MarkdownKind::Link { .. } | MarkdownKind::NoteLink { .. })
                && s.reveal.start <= from
                && to <= s.reveal.end
        })
        .min_by_key(|s| s.reveal.len());
    if let Some(span) = link.filter(|s| s.reveal != s.range) {
        let shown = text[span.range.clone()].to_string();
        let start = span.reveal.start;
        let caret = (cursor.saturating_sub(span.range.start - start)).clamp(start, start + shown.len());
        let (anchor, caret) = if from == to {
            (caret, caret)
        } else {
            (start, start + shown.len())
        };
        return Change {
            range: span.reveal.clone(),
            text: shown,
            anchor,
            cursor: caret,
        };
    }
    let word = target(text, from, to);
    let inner = &text[word.clone()];
    let linked = format!("[{inner}]()");
    // The cursor goes where the address is typed; with no text, where the text is.
    let caret = if inner.is_empty() {
        word.start + 1
    } else {
        word.start + linked.len() - 1
    };
    Change {
        range: word,
        text: linked,
        anchor: caret,
        cursor: caret,
    }
}

/// The lines that hold `from..to`; a selection that ends at the start of a line leaves that line out.
fn lines_of(text: &str, from: usize, to: usize) -> Range<usize> {
    let start = text[..from].rfind('\n').map_or(0, |i| i + 1);
    let to = if to > from && text[..to].ends_with('\n') {
        to - 1
    } else {
        to
    };
    let end = text[to..].find('\n').map_or(text.len(), |i| to + i);
    start..end.max(start)
}

/// The line format a line has, and the line split into indentation, format and text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LineFormat {
    Paragraph,
    Heading(u8),
    Bullet,
    Numbered,
    Task,
    Quote,
}

struct Line<'a> {
    indent: &'a str,
    format: LineFormat,
    text: &'a str,
}

fn split_line(line: &str) -> Line<'_> {
    let indent_len = line.len() - line.trim_start_matches([' ', '\t']).len();
    let (indent, rest) = line.split_at(indent_len);
    let hashes = rest.bytes().take_while(|b| *b == b'#').count();
    if (1..=6).contains(&hashes) && matches!(rest.as_bytes().get(hashes), Some(b' ') | None) {
        return Line {
            indent,
            format: LineFormat::Heading(hashes as u8),
            text: rest[hashes..].trim_start(),
        };
    }
    if let Some(quoted) = rest.strip_prefix('>') {
        return Line {
            indent,
            format: LineFormat::Quote,
            text: quoted.strip_prefix(' ').unwrap_or(quoted),
        };
    }
    let prefix = Prefix::of(rest);
    let format = match (&prefix.marker, prefix.checkbox) {
        (Some(_), true) => LineFormat::Task,
        (Some(crate::markdown::Marker::Bullet(_)), false) => LineFormat::Bullet,
        (Some(crate::markdown::Marker::Number(..)), false) => LineFormat::Numbered,
        (None, _) => LineFormat::Paragraph,
    };
    Line {
        indent,
        format,
        text: &rest[prefix.end..],
    }
}

fn toggle_lines(text: &str, from: usize, to: usize, cursor: usize, format: MarkdownFormat) -> Option<Change> {
    let wanted = match format {
        MarkdownFormat::Heading { level } => LineFormat::Heading(level.clamp(1, 6)),
        MarkdownFormat::Paragraph => LineFormat::Paragraph,
        MarkdownFormat::BulletList => LineFormat::Bullet,
        MarkdownFormat::NumberedList => LineFormat::Numbered,
        MarkdownFormat::TaskList => LineFormat::Task,
        MarkdownFormat::Quote => LineFormat::Quote,
        _ => return None,
    };
    let region = lines_of(text, from, to);
    let lines: Vec<&str> = text[region.clone()].split('\n').collect();
    let several = lines.len() > 1;
    let counted: Vec<Line> = lines.iter().map(|l| split_line(l)).collect();
    let all_set = counted
        .iter()
        .filter(|l| !several || !l.text.trim().is_empty())
        .all(|l| l.format == wanted);
    let target = if all_set { LineFormat::Paragraph } else { wanted };
    let mut number = 0;
    let mut out = Vec::new();
    for (raw, line) in lines.iter().zip(&counted) {
        if several && line.text.trim().is_empty() {
            out.push(raw.to_string());
            continue;
        }
        let prefix = match target {
            LineFormat::Paragraph => String::new(),
            LineFormat::Heading(level) => format!("{} ", "#".repeat(level as usize)),
            LineFormat::Bullet => "- ".into(),
            LineFormat::Numbered => {
                number += 1;
                format!("{number}. ")
            }
            LineFormat::Task => "- [ ] ".into(),
            LineFormat::Quote => "> ".into(),
        };
        out.push(format!("{}{prefix}{}", line.indent, line.text));
    }
    let replaced = out.join("\n");
    let (anchor, caret) = if from == to {
        // One line: the cursor keeps its place in the text of the line.
        let old_text_at = region.start + lines[0].len() - counted[0].text.len();
        let new_text_at = region.start + replaced.len() - counted[0].text.len();
        let caret = new_text_at + cursor.saturating_sub(old_text_at);
        (caret, caret)
    } else {
        (region.start, region.start + replaced.len())
    };
    Some(Change {
        range: region,
        text: replaced,
        anchor,
        cursor: caret,
    })
}

/// A list item as Tab sees it: the width of its indentation and where its text starts.
struct Item {
    indent: usize,
    content: usize,
}

fn item_of(line: &str) -> Option<Item> {
    let indent: usize = line
        .chars()
        .take_while(|c| matches!(c, ' ' | '\t'))
        .map(|c| if c == '\t' { 4 } else { 1 })
        .sum();
    let rest = line.trim_start_matches([' ', '\t']);
    let prefix = Prefix::of(rest);
    let marker = prefix.marker.as_ref()?;
    if prefix.quoted {
        return None;
    }
    let width = match marker {
        crate::markdown::Marker::Bullet(_) => 1,
        crate::markdown::Marker::Number(n, _) => n.to_string().len() + 1,
    };
    Some(Item {
        indent,
        content: indent + width + 1,
    })
}

// ---- rich text to Markdown (E23) ----

/// An address as the destination of a link: in angle brackets when it has spaces or brackets.
fn destination(url: &str) -> String {
    if url.contains([' ', '(', ')', '<', '>']) {
        format!("<{}>", url.replace('>', "%3E"))
    } else {
        url.to_string()
    }
}

/// Text with the characters of Markdown escaped, so that it reads back as the same text.
pub(crate) fn escape_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let chars: Vec<char> = text.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        let doubled_eq = c == '=' && (chars.get(i + 1) == Some(&'=') || i > 0 && chars[i - 1] == '=');
        if matches!(c, '\\' | '*' | '_' | '`' | '[' | ']' | '<' | '~') || doubled_eq {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// A paragraph whose lines must not start a block: `#`, `>`, a list marker or a rule.
pub(crate) fn escape_line_starts(paragraph: &str) -> String {
    paragraph
        .split('\n')
        .map(|line| {
            let lead = line.len() - line.trim_start().len();
            let (spaces, rest) = line.split_at(lead);
            let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
            if rest.starts_with(['#', '>', '-', '+']) {
                format!("{spaces}\\{rest}")
            } else if digits > 0 && matches!(rest.as_bytes().get(digits), Some(b'.' | b')')) {
                format!("{spaces}{}\\{}", &rest[..digits], &rest[digits..])
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// E23: rich text from the clipboard as Markdown of the dialect.
pub(crate) fn markdown_from_html(html: &str) -> String {
    let document = Html::parse_document(html);
    let blocks = blocks_of(document.root_element(), 0);
    let joined = blocks.join("\n\n");
    let mut out = String::new();
    let mut newlines = 0;
    for c in joined.trim().chars() {
        if c == '\n' {
            newlines += 1;
            if newlines > 2 {
                continue;
            }
        } else {
            newlines = 0;
        }
        out.push(c);
    }
    out
}

const SKIPPED: &[&str] = &[
    "head", "script", "style", "title", "meta", "noscript", "template", "button",
];
const BLOCKS: &[&str] = &[
    "html",
    "body",
    "p",
    "div",
    "section",
    "article",
    "header",
    "footer",
    "main",
    "nav",
    "aside",
    "figure",
    "figcaption",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "pre",
    "blockquote",
    "ul",
    "ol",
    "li",
    "table",
    "hr",
    "dl",
    "dt",
    "dd",
    "address",
    "details",
    "summary",
];

fn is_block(element: &ElementRef) -> bool {
    BLOCKS.contains(&element.value().name())
}

/// The blocks of the children of an element; runs of inline content make paragraphs.
fn blocks_of(element: ElementRef, depth: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut run = String::new();
    let flush = |run: &mut String, out: &mut Vec<String>| {
        let paragraph = run.trim();
        if !paragraph.is_empty() {
            out.push(escape_line_starts(&tidy(paragraph)));
        }
        run.clear();
    };
    for child in element.children() {
        match child.value() {
            Node::Text(t) => run.push_str(&escape_text(&collapse(t))),
            Node::Element(_) => {
                let Some(child) = ElementRef::wrap(child) else { continue };
                let name = child.value().name();
                if SKIPPED.contains(&name) {
                    continue;
                }
                if is_block(&child) {
                    flush(&mut run, &mut out);
                    out.extend(block(child, depth));
                } else {
                    run.push_str(&inline(child));
                }
            }
            _ => {}
        }
    }
    flush(&mut run, &mut out);
    out
}

/// Whitespace of HTML: any run is one space.
fn collapse(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut space = false;
    for c in text.chars() {
        if c.is_whitespace() && c != '\u{a0}' {
            if !space {
                out.push(' ');
            }
            space = true;
        } else {
            out.push(c);
            space = false;
        }
    }
    out
}

/// The spaces around hard line breaks dropped.
fn tidy(paragraph: &str) -> String {
    paragraph.split("\\\n").map(str::trim).collect::<Vec<_>>().join("\\\n")
}

fn block(element: ElementRef, depth: usize) -> Vec<String> {
    let name = element.value().name();
    match name {
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
            let level = (name.as_bytes()[1] - b'0') as usize;
            let text = one_line(&inline_children(element));
            if text.is_empty() {
                vec![]
            } else {
                vec![format!("{} {text}", "#".repeat(level))]
            }
        }
        "hr" => vec!["---".into()],
        "pre" => vec![fenced(element)],
        "blockquote" => {
            let inner = blocks_of(element, depth).join("\n\n");
            if inner.is_empty() {
                return vec![];
            }
            vec![inner
                .split('\n')
                .map(|l| {
                    if l.is_empty() {
                        ">".to_string()
                    } else {
                        format!("> {l}")
                    }
                })
                .collect::<Vec<_>>()
                .join("\n")]
        }
        "ul" | "ol" => {
            let list = list(element, name == "ol", depth);
            if list.is_empty() {
                vec![]
            } else {
                vec![list]
            }
        }
        "table" => table(element).into_iter().collect(),
        _ => blocks_of(element, depth),
    }
}

fn fenced(element: ElementRef) -> String {
    let code_class = element
        .children()
        .filter_map(ElementRef::wrap)
        .find(|c| c.value().name() == "code")
        .and_then(|c| c.value().attr("class").map(String::from));
    let classes = [element.value().attr("class").map(String::from), code_class];
    let language = classes
        .iter()
        .flatten()
        .flat_map(|c| c.split_whitespace())
        .find_map(|c| c.strip_prefix("language-").or_else(|| c.strip_prefix("lang-")))
        .unwrap_or_default()
        .to_string();
    let code: String = element.text().collect();
    let code = code.strip_suffix('\n').unwrap_or(&code);
    let longest = code.split(|c| c != '`').map(str::len).max().unwrap_or(0);
    let fence = "`".repeat(longest.max(2) + 1);
    format!("{fence}{language}\n{code}\n{fence}")
}

fn list(element: ElementRef, ordered: bool, depth: usize) -> String {
    let mut number: u64 = element.value().attr("start").and_then(|s| s.parse().ok()).unwrap_or(1);
    let mut items = Vec::new();
    for item in element.children().filter_map(ElementRef::wrap) {
        if item.value().name() != "li" {
            continue;
        }
        let checkbox = item
            .descendants()
            .filter_map(ElementRef::wrap)
            .find(|e| e.value().name() == "input" && e.value().attr("type") == Some("checkbox"));
        let marker = if ordered {
            let m = format!("{number}.");
            number += 1;
            m
        } else {
            "-".into()
        };
        let mut lead = format!("{marker} ");
        if let Some(checkbox) = checkbox {
            let checked = checkbox.value().attr("checked").is_some();
            lead.push_str(if checked { "[x] " } else { "[ ] " });
        }
        let pad = " ".repeat(marker.len() + 1);
        let content = blocks_of(item, depth + 1);
        let mut lines = Vec::new();
        for (i, part) in content.iter().enumerate() {
            for (j, line) in part.split('\n').enumerate() {
                if i == 0 && j == 0 {
                    lines.push(format!("{lead}{line}"));
                } else if line.is_empty() {
                    lines.push(String::new());
                } else {
                    lines.push(format!("{pad}{line}"));
                }
            }
        }
        if lines.is_empty() {
            lines.push(lead.trim_end().to_string());
        }
        items.push(lines.join("\n"));
    }
    items.join("\n")
}

fn table(element: ElementRef) -> Option<String> {
    let rows: Vec<Vec<String>> = element
        .descendants()
        .filter_map(ElementRef::wrap)
        .filter(|e| e.value().name() == "tr")
        .map(|row| {
            row.children()
                .filter_map(ElementRef::wrap)
                .filter(|c| matches!(c.value().name(), "td" | "th"))
                .map(|c| {
                    let parts = blocks_of(c, 0);
                    one_line(&parts.join(" ")).replace('|', "\\|")
                })
                .collect()
        })
        .filter(|r: &Vec<String>| !r.is_empty())
        .collect();
    let columns = rows.iter().map(Vec::len).max()?;
    let line = |cells: &[String]| {
        let mut cells = cells.to_vec();
        cells.resize(columns, String::new());
        format!("| {} |", cells.join(" | "))
    };
    let mut out = vec![line(&rows[0]), format!("|{}", " --- |".repeat(columns))];
    out.extend(rows[1..].iter().map(|r| line(r)));
    Some(out.join("\n"))
}

/// The text of a block as one line: hard breaks and line breaks become spaces.
fn one_line(text: &str) -> String {
    text.replace("\\\n", " ").replace('\n', " ").trim().to_string()
}

fn inline_children(element: ElementRef) -> String {
    let mut out = String::new();
    for child in element.children() {
        match child.value() {
            Node::Text(t) => out.push_str(&escape_text(&collapse(t))),
            Node::Element(_) => {
                if let Some(child) = ElementRef::wrap(child) {
                    if !SKIPPED.contains(&child.value().name()) {
                        out.push_str(&inline(child));
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// Markers around the text, outside its spaces: `** a **` would not be bold.
fn wrap(inner: &str, marker: &str) -> String {
    let core = inner.trim();
    if core.is_empty() {
        return inner.to_string();
    }
    let lead = &inner[..inner.len() - inner.trim_start().len()];
    let trail = &inner[inner.trim_end().len()..];
    format!("{lead}{marker}{core}{marker}{trail}")
}

fn inline(element: ElementRef) -> String {
    let name = element.value().name();
    let style = element
        .value()
        .attr("style")
        .unwrap_or_default()
        .replace(' ', "")
        .to_lowercase();
    match name {
        "br" => "\\\n".into(),
        "img" => {
            let src = element.value().attr("src").unwrap_or_default();
            let alt = escape_text(element.value().attr("alt").unwrap_or_default());
            match address(src) {
                Some(src) if !src.starts_with("mailto:") => format!("![{alt}]({})", destination(&src)),
                _ => alt,
            }
        }
        "code" | "kbd" | "samp" | "tt" => {
            let code: String = element.text().collect();
            let code = collapse(&code);
            if code.trim().is_empty() {
                return code;
            }
            let longest = code.split(|c| c != '`').map(str::len).max().unwrap_or(0);
            let ticks = "`".repeat(longest + 1);
            let pad = if code.starts_with('`') || code.ends_with('`') {
                " "
            } else {
                ""
            };
            format!("{ticks}{pad}{code}{pad}{ticks}")
        }
        "a" => {
            let inner = inline_children(element);
            let href = element.value().attr("href").unwrap_or_default();
            match address(href) {
                Some(url) if !inner.trim().is_empty() => {
                    let lead = &inner[..inner.len() - inner.trim_start().len()];
                    let trail = &inner[inner.trim_end().len()..];
                    format!("{lead}[{}]({}){trail}", inner.trim(), destination(&url))
                }
                _ => inner,
            }
        }
        _ => {
            let mut text = inline_children(element);
            // Google Docs wraps a whole document in <b style="font-weight:normal">.
            let normal = style.contains("font-weight:normal") || style.contains("font-weight:400");
            let bold = matches!(name, "strong" | "b") && !normal
                || style.contains("font-weight:bold")
                || ["600", "700", "800", "900"]
                    .iter()
                    .any(|w| style.contains(&format!("font-weight:{w}")));
            let italic = matches!(name, "em" | "i" | "cite") || style.contains("font-style:italic");
            let struck = matches!(name, "s" | "del" | "strike") || style.contains("line-through");
            if matches!(name, "mark") {
                text = wrap(&text, "==");
            }
            if struck {
                text = wrap(&text, "~~");
            }
            if italic {
                text = wrap(&text, "*");
            }
            if bold {
                text = wrap(&text, "**");
            }
            text
        }
    }
}

// ---- typography (E26) ----

fn is_cyrillic(c: char) -> bool {
    ('\u{0400}'..='\u{04FF}').contains(&c)
}

/// Whether the nearest letter before `at` in the paragraph is Russian; `None` without a letter.
fn russian_before(paragraph: &str) -> Option<bool> {
    paragraph.chars().rev().find(|c| c.is_alphabetic()).map(is_cyrillic)
}

/// A straight quote typed at `quote..at`: an opening one where a word cannot end before it, a
/// closing one otherwise. An opening quote takes the language of the letter before it; with no
/// letter it stays straight until the closing quote, which settles both by the letter before it.
fn quotes(text: &str, quote: usize, at: usize) -> Option<MarkdownEdit> {
    let paragraph_start = text[..quote].rfind("\n\n").map_or(0, |i| i + 2);
    let before = &text[paragraph_start..quote];
    let previous = before.chars().next_back();
    let opens = previous.is_none_or(|c| c.is_whitespace() || "([{«“„—–-".contains(c));
    let change = |range: Range<usize>, new: String| {
        let caret = at - range.len() + new.len();
        Change {
            range,
            text: new,
            anchor: caret,
            cursor: caret,
        }
        .edit(text)
    };
    if opens {
        let russian = russian_before(before)?;
        return Some(change(quote..at, if russian { "«" } else { "“" }.into()));
    }
    // The quote that this one closes: the last one in the paragraph left open.
    let mut open: Vec<(usize, char)> = Vec::new();
    for (i, c) in before.char_indices() {
        let previous = before[..i].chars().next_back();
        match c {
            '«' | '“' => open.push((paragraph_start + i, c)),
            '»' | '”' => {
                open.pop();
            }
            '"' if previous.is_none_or(|p| p.is_whitespace() || "([{«“„—–-".contains(p)) => {
                open.push((paragraph_start + i, c))
            }
            '"' => {
                open.pop();
            }
            _ => {}
        }
    }
    let russian = russian_before(before).unwrap_or(false);
    match open.pop() {
        Some((_, '«')) => Some(change(quote..at, "»".into())),
        Some((_, '“')) => Some(change(quote..at, "”".into())),
        Some((opener, _)) => {
            let (left, right) = if russian { ("«", "»") } else { ("“", "”") };
            let middle = &text[opener + 1..quote];
            Some(change(opener..at, format!("{left}{middle}{right}")))
        }
        None => Some(change(quote..at, if russian { "»" } else { "”" }.into())),
    }
}
