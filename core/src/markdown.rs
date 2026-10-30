//! The Markdown of a note (E1–E16, E30, E31, E51 in docs/specs/editor.md); the scheme of parsing
//! and painting is that of R48–R52 of Lists.
//!
//! The note stays the text that was typed. The apps get the ranges to style,
//! to hide and to replace, in UTF-16 units, which is how Swift's `NSString`,
//! Kotlin and JavaScript all index a string, so one note looks the same
//! everywhere.

use std::collections::HashSet;
use std::ops::Range;

use pulldown_cmark::{Alignment, CodeBlockKind, Event, HeadingLevel, LinkType, Options, Parser, Tag};

use crate::highlight::{self, CodeClass};

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum MarkdownKind {
    Heading {
        level: u8,
    },
    Strong,
    Emphasis,
    Strikethrough,
    /// `==text==` (E1).
    Highlight,
    Code,
    CodeBlock,
    Quote,
    /// Text that opens `url`; only `http`, `https` and `mailto` get here (E4).
    Link {
        url: String,
    },
    /// The text of a link `note:<id>` to another note (E31).
    NoteLink {
        id: String,
    },
    /// A whole link `attachment:<id>` (E30), replaced by the attachment (E14): shown inside the
    /// note when written as an image, `![…](…)`, as a chip otherwise.
    AttachmentLink {
        id: String,
        inline: bool,
    },
    /// `-`, `*`, `+` or `1.` of a list item: shown as a bullet or as the number.
    ListMarker {
        ordered: bool,
    },
    /// `>` of a block quote with the space after it: shown as a bar.
    QuoteMarker,
    /// `[ ]` or `[x]` of a task list item: shown as a checkbox.
    Checkbox {
        checked: bool,
    },
    Rule,
    /// A token of the code in a fenced code block, by the language after the fence (E3).
    Token {
        class: CodeClass,
    },
    /// A line of a table as it is typed: monospaced. While the cursor is outside
    /// the table it is drawn as a grid from `MarkdownLayout::tables` (E15).
    TableRow,
    /// Markup characters: hidden while the cursor is outside the reveal range of the span.
    Markup,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct MarkdownSpan {
    pub start: u32,
    pub end: u32,
    pub kind: MarkdownKind,
    /// Index into `MarkdownLayout::blocks`.
    pub block: u32,
    /// The source of the span shows while the cursor is at `reveal_start..=reveal_end`, ends
    /// included: the markup of inline formatting while the cursor is in the formatted span or
    /// right next to it (E11), the `#` of a heading, the `>` of a quote and an attachment while it
    /// is on their line (E12, E14), the address of a link while it is in the link (E13), the
    /// fences of a code block while it is in the block (E16), a table while it is in the table (E15).
    pub reveal_start: u32,
    pub reveal_end: u32,
}

/// A paragraph, a heading, a list item without the items nested in it, a code
/// block or a table.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct MarkdownBlock {
    pub start: u32,
    pub end: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum MarkdownAlign {
    /// Not said: as the text runs.
    None,
    Left,
    Center,
    Right,
}

/// What a cell says, without the bars and the spaces around it. An empty cell
/// has `start == end`; spans that lie inside the range style its text.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct MarkdownCell {
    pub start: u32,
    pub end: u32,
}

/// A row of a table with one cell per column. The line of dashes is not a row.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct MarkdownTableRow {
    pub header: bool,
    pub cells: Vec<MarkdownCell>,
}

/// A table to draw as a grid (E15 in docs/specs/editor.md) while the cursor is outside its block.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct MarkdownTable {
    pub start: u32,
    pub end: u32,
    /// Index into `MarkdownLayout::blocks`.
    pub block: u32,
    pub columns: Vec<MarkdownAlign>,
    pub rows: Vec<MarkdownTableRow>,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct MarkdownRange {
    pub start: u32,
    pub end: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, uniffi::Record)]
pub struct MarkdownLayout {
    /// Ordered by start; of two that start together the longer comes first.
    pub spans: Vec<MarkdownSpan>,
    /// Ordered, not overlapping.
    pub blocks: Vec<MarkdownBlock>,
    /// Ordered by start.
    pub tables: Vec<MarkdownTable>,
    /// Where spelling is not checked (E51): code, addresses and attachment links. Ordered, not
    /// overlapping.
    pub no_spellcheck: Vec<MarkdownRange>,
}

/// A replacement in the note: `start..end` becomes `text`, then `anchor..cursor` is selected, in
/// the text after the replacement; nothing is selected when the two are equal.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct MarkdownEdit {
    pub start: u32,
    pub end: u32,
    pub text: String,
    pub anchor: u32,
    pub cursor: u32,
}

/// What to style, hide and replace in a note (E10).
#[uniffi::export]
pub fn markdown_layout(text: String) -> MarkdownLayout {
    let Parsed {
        spans,
        blocks,
        tables,
        no_spellcheck,
    } = layout(&text, true);
    let units = Utf16::of(&text);
    MarkdownLayout {
        tables: tables
            .into_iter()
            .filter_map(|t| {
                let block = blocks
                    .iter()
                    .position(|b| b.start <= t.range.start && t.range.start < b.end)?;
                Some(MarkdownTable {
                    start: units.at(t.range.start),
                    end: units.at(t.range.end),
                    block: block as u32,
                    columns: t.columns,
                    rows: t
                        .rows
                        .into_iter()
                        .map(|(header, cells)| MarkdownTableRow {
                            header,
                            cells: cells
                                .into_iter()
                                .map(|c| MarkdownCell {
                                    start: units.at(c.start),
                                    end: units.at(c.end),
                                })
                                .collect(),
                        })
                        .collect(),
                })
            })
            .collect(),
        spans: spans
            .into_iter()
            .map(|s| MarkdownSpan {
                start: units.at(s.range.start),
                end: units.at(s.range.end),
                kind: s.kind,
                block: s.block as u32,
                reveal_start: units.at(s.reveal.start),
                reveal_end: units.at(s.reveal.end),
            })
            .collect(),
        blocks: blocks
            .into_iter()
            .map(|b| MarkdownBlock {
                start: units.at(b.start),
                end: units.at(b.end),
            })
            .collect(),
        no_spellcheck: no_spellcheck
            .into_iter()
            .map(|r| MarkdownRange {
                start: units.at(r.start),
                end: units.at(r.end),
            })
            .collect(),
    }
}

/// What Return does at `cursor` beyond starting a line (E22): continues a list or a
/// block quote, or takes the marker off an item left empty. `None` where Return
/// only starts a line.
#[uniffi::export]
pub fn markdown_newline(text: String, cursor: u32) -> Option<MarkdownEdit> {
    let units = Utf16::of(&text);
    let at = units.byte(cursor)?;
    let line_start = text[..at].rfind('\n').map_or(0, |i| i + 1);
    let line_end = text[at..].find('\n').map_or(text.len(), |i| at + i);
    let line = &text[line_start..line_end];
    let prefix = Prefix::of(line);
    if !prefix.quoted && prefix.marker.is_none() || at - line_start < prefix.end {
        return None;
    }
    // The parser has the last word: `- - -` is a rule and a `-` in a code block is text.
    let marked = layout(&text, false).spans.iter().any(|s| {
        (line_start..line_start + prefix.end.max(1)).contains(&s.range.start)
            && matches!(
                s.kind,
                MarkdownKind::ListMarker { .. } | MarkdownKind::QuoteMarker | MarkdownKind::Checkbox { .. }
            )
    });
    if !marked {
        return None;
    }

    if line[prefix.end..].trim().is_empty() {
        let keep = match prefix.marker {
            Some(_) if prefix.quoted => prefix.lead,
            _ => 0,
        };
        let start = units.at(line_start + keep);
        return Some(MarkdownEdit {
            start,
            end: units.at(line_end),
            text: String::new(),
            anchor: start,
            cursor: start,
        });
    }
    let mut insert = format!("\n{}", &line[..prefix.lead]);
    match &prefix.marker {
        Some(Marker::Bullet(c)) => insert.push(*c),
        Some(Marker::Number(n, delimiter)) => insert.push_str(&format!("{}{delimiter}", n.saturating_add(1))),
        None => {}
    }
    if prefix.marker.is_some() {
        insert.push(' ');
    }
    if prefix.checkbox {
        insert.push_str("[ ] ");
    }
    let moved = insert.encode_utf16().count() as u32;
    Some(MarkdownEdit {
        start: cursor,
        end: cursor,
        text: insert,
        anchor: cursor + moved,
        cursor: cursor + moved,
    })
}

pub(crate) enum Marker {
    Bullet(char),
    Number(u64, char),
}

/// What a line starts with before its text.
pub(crate) struct Prefix {
    /// Indentation and `>` signs: what the next line repeats as it is.
    pub(crate) lead: usize,
    pub(crate) quoted: bool,
    pub(crate) marker: Option<Marker>,
    pub(crate) checkbox: bool,
    /// Where the text begins.
    pub(crate) end: usize,
}

impl Prefix {
    pub(crate) fn of(line: &str) -> Prefix {
        let bytes = line.as_bytes();
        let blank = |i: usize| matches!(bytes.get(i), Some(b' ' | b'\t'));
        let ends = |i: usize| i >= bytes.len() || blank(i);
        let mut i = 0;
        let mut quoted = false;
        loop {
            while blank(i) {
                i += 1;
            }
            if bytes.get(i) != Some(&b'>') {
                break;
            }
            quoted = true;
            i += 1;
        }
        // The quote sign keeps one space after it; the rest is indentation of what follows.
        let lead = i;
        let mut marker = None;
        if matches!(bytes.get(i), Some(b'-' | b'*' | b'+')) && ends(i + 1) {
            marker = Some(Marker::Bullet(bytes[i] as char));
            i += 1;
        } else {
            let digits = bytes[i..].iter().take_while(|b| b.is_ascii_digit()).count();
            let after = i + digits;
            if (1..=9).contains(&digits) && matches!(bytes.get(after), Some(b'.' | b')')) && ends(after + 1) {
                marker = line[i..after]
                    .parse()
                    .ok()
                    .map(|n| Marker::Number(n, bytes[after] as char));
                i = after + 1;
            }
        }
        let mut checkbox = false;
        if marker.is_some() {
            if blank(i) {
                i += 1;
            }
            if bytes.get(i) == Some(&b'[')
                && matches!(bytes.get(i + 1), Some(b' ' | b'x' | b'X'))
                && bytes.get(i + 2) == Some(&b']')
                && ends(i + 3)
            {
                checkbox = true;
                i += 3;
                if blank(i) {
                    i += 1;
                }
            }
        }
        Prefix {
            lead,
            quoted,
            marker,
            checkbox,
            end: i.min(bytes.len()),
        }
    }
}

/// Byte offsets to UTF-16 offsets and back.
pub(crate) struct Utf16(Vec<u32>);

impl Utf16 {
    pub(crate) fn of(text: &str) -> Utf16 {
        let mut map = vec![0; text.len() + 1];
        let mut units = 0;
        for (i, c) in text.char_indices() {
            map[i] = units;
            units += c.len_utf16() as u32;
        }
        map[text.len()] = units;
        Utf16(map)
    }

    pub(crate) fn at(&self, byte: usize) -> u32 {
        self.0[byte]
    }

    /// `None` for an offset past the end or inside a surrogate pair.
    pub(crate) fn byte(&self, units: u32) -> Option<usize> {
        if units == 0 {
            return Some(0);
        }
        self.0.iter().position(|&u| u == units)
    }
}

pub(crate) struct Span {
    pub(crate) range: Range<usize>,
    pub(crate) kind: MarkdownKind,
    pub(crate) block: usize,
    pub(crate) reveal: Range<usize>,
}

/// A span as the parse finds it; without a reveal range it reveals with its block.
struct Found {
    range: Range<usize>,
    kind: MarkdownKind,
    reveal: Option<Range<usize>>,
}

fn put(found: &mut Vec<Found>, range: Range<usize>, kind: MarkdownKind, reveal: Range<usize>) {
    found.push(Found {
        range,
        kind,
        reveal: Some(reveal),
    });
}

struct Frame<'a> {
    tag: Tag<'a>,
    range: Range<usize>,
    children: Vec<Range<usize>>,
    /// In a list item: its own text, up to the first block nested in it.
    inline: Option<Range<usize>>,
    task: bool,
}

fn is_inline(tag: &Tag) -> bool {
    matches!(
        tag,
        Tag::Emphasis
            | Tag::Strong
            | Tag::Strikethrough
            | Tag::Link { .. }
            | Tag::Image { .. }
            | Tag::Superscript
            | Tag::Subscript
    )
}

/// The address of a web or mail link with its scheme in lower case, or nothing when the
/// scheme is not one of those. The rest stays as typed.
pub(crate) fn address(url: &str) -> Option<String> {
    ["http://", "https://", "mailto:"].iter().find_map(|scheme| {
        let head = url.get(..scheme.len())?;
        head.eq_ignore_ascii_case(scheme)
            .then(|| format!("{scheme}{}", &url[scheme.len()..]))
    })
}

/// Where a link leads (E4): the web or mail, a note (E31) or an attachment (E30).
enum Target {
    Web(String),
    Note(String),
    Attachment(String),
}

fn target(url: &str) -> Option<Target> {
    let id = |scheme: &str| {
        let head = url.get(..scheme.len())?;
        let id = &url[scheme.len()..];
        (head.eq_ignore_ascii_case(scheme) && !id.is_empty()).then(|| id.to_string())
    };
    if let Some(id) = id("attachment:") {
        return Some(Target::Attachment(id));
    }
    if let Some(id) = id("note:") {
        return Some(Target::Note(id));
    }
    address(url).map(Target::Web)
}

fn trimmed(text: &str, range: &Range<usize>) -> Range<usize> {
    let slice = &text[range.clone()];
    let start = range.start + (slice.len() - slice.trim_start_matches('\n').len());
    let end = range.end - (slice.len() - slice.trim_end_matches(['\n', '\r', ' ', '\t']).len());
    start..end.max(start)
}

/// The lines that hold `range`, without the line break after the last of them.
fn line_of(text: &str, range: &Range<usize>) -> Range<usize> {
    let start = text[..range.start].rfind('\n').map_or(0, |i| i + 1);
    let end = text[range.end..].find('\n').map_or(text.len(), |i| range.end + i);
    start..end
}

/// The parts of `range` its children do not cover: the markup of an inline element.
fn gaps(range: &Range<usize>, children: &[Range<usize>]) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut at = range.start;
    for child in children {
        if child.start > at {
            out.push(at..child.start.min(range.end));
        }
        at = at.max(child.end);
    }
    if at < range.end {
        out.push(at..range.end);
    }
    out
}

fn lines(text: &str, range: &Range<usize>) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut start = range.start;
    for (i, _) in text[range.clone()].match_indices('\n') {
        out.push(start..range.start + i);
        start = range.start + i + 1;
    }
    if start < range.end {
        out.push(start..range.end);
    }
    out
}

/// A table as the parser saw it, in bytes: rows are (header, cells).
struct Table {
    range: Range<usize>,
    columns: Vec<MarkdownAlign>,
    rows: Vec<(bool, Vec<Range<usize>>)>,
}

/// The text of a cell: what the parser gives for it without the spaces around.
fn cell(text: &str, range: &Range<usize>) -> Range<usize> {
    let slice = &text[range.clone()];
    let start = range.start + (slice.len() - slice.trim_start().len());
    let end = range.end - (slice.len() - slice.trim_end().len());
    start..end.max(start)
}

/// A `==` that may open or close a highlight (E1), in the block that holds it.
struct Mark {
    block: usize,
    at: usize,
    opens: bool,
    closes: bool,
}

/// What the parse of a note gives, in bytes.
pub(crate) struct Parsed {
    pub(crate) spans: Vec<Span>,
    pub(crate) blocks: Vec<Range<usize>>,
    tables: Vec<Table>,
    pub(crate) no_spellcheck: Vec<Range<usize>>,
}

/// `highlight`: whether code blocks get the tokens of their language (E3), which only the layout
/// shows.
pub(crate) fn layout(text: &str, highlight: bool) -> Parsed {
    let mut tables: Vec<Table> = Vec::new();
    let mut rows: Vec<(bool, Vec<Range<usize>>)> = Vec::new();
    let mut cells: Vec<Range<usize>> = Vec::new();
    let mut found: Vec<Found> = Vec::new();
    let mut leaves: Vec<Range<usize>> = Vec::new();
    let mut quiet: Vec<Range<usize>> = Vec::new();
    let mut marks: Vec<Mark> = Vec::new();
    let mut stack: Vec<Frame> = Vec::new();
    let options = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;

    for (event, range) in Parser::new_ext(text, options).into_offset_iter() {
        match event {
            Event::Start(tag) => {
                if !is_inline(&tag) {
                    flush(stack.last_mut(), &mut leaves);
                }
                stack.push(Frame {
                    tag,
                    range,
                    children: Vec::new(),
                    inline: None,
                    task: false,
                });
            }
            Event::End(_) => {
                let Some(mut frame) = stack.pop() else { continue };
                let depth = stack.iter().filter(|f| matches!(f.tag, Tag::BlockQuote(_))).count();
                match &frame.tag {
                    Tag::TableCell => cells.push(cell(text, &frame.range)),
                    Tag::TableHead | Tag::TableRow => {
                        // A cell the row is filled up with lies where the line ends, not after it.
                        let end = trimmed(text, &frame.range).end;
                        let row = cells.drain(..).map(|c| c.start.min(end)..c.end.min(end)).collect();
                        rows.push((matches!(frame.tag, Tag::TableHead), row));
                    }
                    Tag::Table(columns) => tables.push(Table {
                        range: trimmed(text, &frame.range),
                        columns: columns
                            .iter()
                            .map(|a| match a {
                                Alignment::None => MarkdownAlign::None,
                                Alignment::Left => MarkdownAlign::Left,
                                Alignment::Center => MarkdownAlign::Center,
                                Alignment::Right => MarkdownAlign::Right,
                            })
                            .collect(),
                        rows: std::mem::take(&mut rows),
                    }),
                    _ => {}
                }
                close(text, &mut frame, depth, &mut found, &mut leaves, &mut quiet);
                if highlight {
                    code_tokens(text, &frame, &mut found);
                }
                if let Some(parent) = stack.last_mut() {
                    if is_inline(&frame.tag) {
                        extend(&mut parent.inline, &frame.range);
                    }
                    parent.children.push(frame.range);
                }
            }
            Event::Rule => {
                flush(stack.last_mut(), &mut leaves);
                let rule = trimmed(text, &range);
                leaves.push(rule.clone());
                put(&mut found, rule.clone(), MarkdownKind::Rule, rule);
                if let Some(parent) = stack.last_mut() {
                    parent.children.push(range);
                }
            }
            Event::Html(_) => {
                if let Some(parent) = stack.last_mut() {
                    parent.children.push(range);
                }
            }
            leaf => {
                match leaf {
                    Event::Code(_) => {
                        let ticks = text[range.clone()].bytes().take_while(|b| *b == b'`').count();
                        if ticks > 0 && range.len() >= ticks * 2 {
                            let inside = range.start + ticks..range.end - ticks;
                            put(
                                &mut found,
                                range.start..range.start + ticks,
                                MarkdownKind::Markup,
                                range.clone(),
                            );
                            put(&mut found, inside.clone(), MarkdownKind::Code, range.clone());
                            put(
                                &mut found,
                                range.end - ticks..range.end,
                                MarkdownKind::Markup,
                                range.clone(),
                            );
                            quiet.push(inside);
                        }
                    }
                    Event::TaskListMarker(checked) => {
                        let line = line_of(text, &range);
                        put(&mut found, range.clone(), MarkdownKind::Checkbox { checked }, line);
                        if let Some(item) = stack.iter_mut().rev().find(|f| matches!(f.tag, Tag::Item)) {
                            item.task = true;
                        }
                    }
                    Event::Text(_) => {
                        let code = stack.iter().any(|f| matches!(f.tag, Tag::CodeBlock(_)));
                        let linked = stack
                            .iter()
                            .any(|f| matches!(f.tag, Tag::Link { .. } | Tag::Image { .. }));
                        if !code && !linked {
                            bare_links(text, &range, &mut found, &mut quiet);
                        }
                        if !code {
                            let block = stack
                                .iter()
                                .rposition(|f| !is_inline(&f.tag))
                                .map_or(0, |i| stack[i].range.start);
                            highlight_marks(text, &range, block, &mut marks);
                        }
                    }
                    _ => {}
                }
                if let Some(parent) = stack.last_mut() {
                    extend(&mut parent.inline, &range);
                    parent.children.push(range);
                }
            }
        }
    }
    highlights(marks, &quiet, &mut found);
    // E15: while the cursor is in a table all of its source shows, the markup in its cells too.
    for f in &mut found {
        if let Some(table) = tables
            .iter()
            .find(|t| t.range.start <= f.range.start && f.range.end <= t.range.end)
        {
            f.reveal = Some(table.range.clone());
        }
    }
    let (spans, blocks) = arrange(text, found, leaves);
    Parsed {
        spans,
        blocks,
        tables,
        no_spellcheck: merged(quiet),
    }
}

/// The candidates for `==` among the characters of a text event: a run of exactly two `=` that no
/// backslash escapes. One opens when a character other than a space follows it, and closes when
/// one other than a space comes before it, like `~~` of GitHub.
fn highlight_marks(text: &str, range: &Range<usize>, block: usize, marks: &mut Vec<Mark>) {
    let bytes = text.as_bytes();
    let mut seen = HashSet::new();
    for (i, _) in text[range.clone()].match_indices('=') {
        let mut start = range.start + i;
        while start > 0 && bytes[start - 1] == b'=' {
            start -= 1;
        }
        let end = start + bytes[start..].iter().take_while(|b| **b == b'=').count();
        if end - start != 2 || !seen.insert(start) || (start > 0 && bytes[start - 1] == b'\\') {
            continue;
        }
        let after = text[end..].chars().next();
        let before = text[..start].chars().next_back();
        marks.push(Mark {
            block,
            at: start,
            opens: after.is_some_and(|c| !c.is_whitespace()),
            closes: before.is_some_and(|c| !c.is_whitespace()),
        });
    }
}

/// Pairs the candidates of each block from the left: a closing one ends the highlight the last
/// opening one started. `==` inside code and addresses is text.
fn highlights(mut marks: Vec<Mark>, quiet: &[Range<usize>], found: &mut Vec<Found>) {
    marks.retain(|m| !quiet.iter().any(|q| q.start <= m.at && m.at < q.end));
    marks.sort_by_key(|m| (m.block, m.at));
    let mut open: Option<&Mark> = None;
    for mark in &marks {
        match open {
            Some(first) if first.block == mark.block && mark.closes && mark.at > first.at + 2 => {
                let whole = first.at..mark.at + 2;
                put(found, first.at..first.at + 2, MarkdownKind::Markup, whole.clone());
                put(found, first.at + 2..mark.at, MarkdownKind::Highlight, whole.clone());
                put(found, mark.at..mark.at + 2, MarkdownKind::Markup, whole);
                open = None;
            }
            Some(first) if first.block == mark.block && !mark.opens => {}
            _ => open = mark.opens.then_some(mark),
        }
    }
}

/// Ranges in order, the overlapping and touching ones joined.
fn merged(mut ranges: Vec<Range<usize>>) -> Vec<Range<usize>> {
    ranges.retain(|r| !r.is_empty());
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

fn extend(extent: &mut Option<Range<usize>>, range: &Range<usize>) {
    *extent = Some(match extent.take() {
        Some(e) => e.start.min(range.start)..e.end.max(range.end),
        None => range.clone(),
    });
}

/// The text of a list item written without a paragraph of its own is a block.
fn flush(frame: Option<&mut Frame>, leaves: &mut Vec<Range<usize>>) {
    if let Some(frame) = frame {
        if matches!(frame.tag, Tag::Item) {
            leaves.extend(frame.inline.take());
        }
    }
}

/// Addresses typed without markup (E1, the autolinks of GitHub): `http://`, `https://` and
/// `www.` at the start of a word, and mail addresses.
fn bare_links(text: &str, range: &Range<usize>, found: &mut Vec<Found>, quiet: &mut Vec<Range<usize>>) {
    let slice = &text[range.clone()];
    // A keyboard starts a sentence with a capital: `Https://` is an address too.
    // Lower case of ASCII alone keeps every byte offset.
    let lower = slice.to_ascii_lowercase();
    let starts_word = |at: usize| slice[..at].chars().next_back().is_none_or(|c| !c.is_alphanumeric());
    let mut links: Vec<Range<usize>> = Vec::new();
    let mut from = 0;
    while let Some(i) = ["https://", "http://", "www."]
        .iter()
        .filter_map(|prefix| lower[from..].find(prefix))
        .min()
    {
        let start = from + i;
        let mut end = start + slice[start..].find(char::is_whitespace).unwrap_or(slice.len() - start);
        while slice[start..end].ends_with(['.', ',', ';', ':', '!', '?', ')', '"', '\'', '>']) {
            end -= 1;
        }
        let typed = &slice[start..end];
        let url = if lower[start..].starts_with("www.") {
            let host = typed.get(4..).unwrap_or_default();
            host.starts_with(|c: char| c.is_alphanumeric())
                .then(|| format!("http://{typed}"))
        } else {
            address(typed).filter(|url| url.split_once("://").is_some_and(|(_, rest)| !rest.is_empty()))
        };
        if let Some(url) = url.filter(|_| starts_word(start)) {
            links.push(start..end);
            found.push(Found {
                range: range.start + start..range.start + end,
                kind: MarkdownKind::Link { url },
                reveal: None,
            });
        }
        from = end.max(start + 1);
    }

    for (at, _) in slice.match_indices('@') {
        if links.iter().any(|l| l.start <= at && at < l.end) {
            continue;
        }
        let local = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '+' | '-' | '_');
        let host = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_');
        let start = slice[..at]
            .char_indices()
            .rev()
            .take_while(|(_, c)| local(*c))
            .last()
            .map_or(at, |(i, _)| i);
        let mut end = slice[at + 1..]
            .char_indices()
            .take_while(|(_, c)| host(*c))
            .last()
            .map_or(at + 1, |(i, c)| at + 1 + i + c.len_utf8());
        while end > at + 1 && slice[..end].ends_with('.') {
            end -= 1;
        }
        let domain = &slice[at + 1..end];
        let fits = start < at
            && starts_word(start)
            && domain.contains('.')
            && domain.ends_with(|c: char| c.is_ascii_alphanumeric())
            && !domain.starts_with('.');
        if fits {
            found.push(Found {
                range: range.start + start..range.start + end,
                kind: MarkdownKind::Link {
                    url: format!("mailto:{}", &slice[start..end]),
                },
                reveal: None,
            });
            links.push(start..end);
        }
    }
    quiet.extend(links.into_iter().map(|l| range.start + l.start..range.start + l.end));
}

/// E3: the tokens of a fenced code block. The parser gives its code as pieces, one per line
/// without the signs of the quotes and the indentation of the list items around it; the tokens of
/// the pieces joined are cut back into them.
fn code_tokens(text: &str, frame: &Frame, found: &mut Vec<Found>) {
    let Tag::CodeBlock(CodeBlockKind::Fenced(info)) = &frame.tag else {
        return;
    };
    let language = info.split_whitespace().next().unwrap_or_default();
    let mut code = String::new();
    let mut pieces: Vec<(usize, usize)> = Vec::new();
    for piece in &frame.children {
        pieces.push((code.len(), piece.start));
        code.push_str(&text[piece.clone()]);
    }
    let block = trimmed(text, &frame.range);
    for (range, class) in highlight::tokens(language, &code) {
        for (i, &(joined, source)) in pieces.iter().enumerate() {
            let next = pieces.get(i + 1).map_or(code.len(), |p| p.0);
            let start = range.start.max(joined);
            let end = range.end.min(next);
            if start < end {
                let piece = source + start - joined..source + end - joined;
                put(found, piece, MarkdownKind::Token { class }, block.clone());
            }
        }
    }
}

fn close(
    text: &str,
    frame: &mut Frame,
    depth: usize,
    found: &mut Vec<Found>,
    leaves: &mut Vec<Range<usize>>,
    quiet: &mut Vec<Range<usize>>,
) {
    let range = frame.range.clone();
    let inside = match (frame.children.first(), frame.children.last()) {
        (Some(first), Some(last)) => Some(first.start..last.end),
        _ => None,
    };
    let escapes = |found: &mut Vec<Found>| {
        for gap in gaps(&range, &frame.children) {
            if &text[gap.clone()] == "\\" {
                let escaped = text[gap.end..].chars().next().map_or(0, char::len_utf8);
                put(found, gap.clone(), MarkdownKind::Markup, gap.start..gap.end + escaped);
            }
        }
    };
    let styled = |kind: MarkdownKind, found: &mut Vec<Found>| {
        let Some(inside) = inside.clone() else { return };
        put(found, inside, kind, range.clone());
        for gap in gaps(&range, &frame.children) {
            put(found, gap, MarkdownKind::Markup, range.clone());
        }
    };
    match &frame.tag {
        Tag::Paragraph => {
            leaves.push(trimmed(text, &range));
            escapes(found);
        }
        Tag::Heading { level, .. } => {
            let heading = trimmed(text, &range);
            leaves.push(heading.clone());
            // A lone `-` under a line of text underlines a heading by the letter
            // of CommonMark. Here it is a list item being started (E5): the line above
            // must not turn into a heading for the one keystroke before its text.
            if let Some(last) = frame.children.last() {
                let under = &text[last.end.min(heading.end)..heading.end];
                if under.trim() == "-" {
                    let at = last.end + under.find('-').unwrap_or(0);
                    let line = line_of(text, &(at..at));
                    put(found, at..at + 1, MarkdownKind::ListMarker { ordered: false }, line);
                    return;
                }
            }
            let level = match level {
                HeadingLevel::H1 => 1,
                HeadingLevel::H2 => 2,
                HeadingLevel::H3 => 3,
                HeadingLevel::H4 => 4,
                HeadingLevel::H5 => 5,
                HeadingLevel::H6 => 6,
            };
            put(found, heading.clone(), MarkdownKind::Heading { level }, heading.clone());
            for gap in gaps(&range, &frame.children) {
                let slice = &text[gap.clone()];
                let start = gap.start + (slice.len() - slice.trim_start_matches(['\n', '\r']).len());
                let end = gap.end - (slice.len() - slice.trim_end_matches(['\n', '\r']).len());
                if start < end && !text[start..end].trim().is_empty() {
                    put(found, start..end, MarkdownKind::Markup, heading.clone());
                }
            }
        }
        Tag::Emphasis => styled(MarkdownKind::Emphasis, found),
        Tag::Strong => styled(MarkdownKind::Strong, found),
        Tag::Strikethrough => styled(MarkdownKind::Strikethrough, found),
        Tag::Link {
            dest_url, link_type, ..
        }
        | Tag::Image {
            dest_url, link_type, ..
        } => {
            let url = match link_type {
                LinkType::Email => format!("mailto:{dest_url}"),
                _ => dest_url.to_string(),
            };
            // An autolink shows nothing but its address; any other link hides it in its markup.
            if matches!(link_type, LinkType::Autolink | LinkType::Email) {
                quiet.push(range.clone());
            } else {
                quiet.extend(gaps(&range, &frame.children));
            }
            match (target(&url), inside.is_some()) {
                (Some(Target::Attachment(id)), _) => {
                    let inline = matches!(frame.tag, Tag::Image { .. });
                    let line = line_of(text, &range);
                    put(found, range.clone(), MarkdownKind::AttachmentLink { id, inline }, line);
                    quiet.push(range.clone());
                }
                (Some(Target::Web(url)), true) => styled(MarkdownKind::Link { url }, found),
                (Some(Target::Note(id)), true) => styled(MarkdownKind::NoteLink { id }, found),
                // Nothing to show but the address: it stays visible.
                (Some(Target::Web(url)), false) => put(found, range.clone(), MarkdownKind::Link { url }, range.clone()),
                (Some(Target::Note(id)), false) => {
                    put(found, range.clone(), MarkdownKind::NoteLink { id }, range.clone())
                }
                (None, true) => {
                    for gap in gaps(&range, &frame.children) {
                        put(found, gap, MarkdownKind::Markup, range.clone());
                    }
                }
                (None, false) => {}
            }
        }
        Tag::CodeBlock(kind) => {
            let block = trimmed(text, &range);
            leaves.push(block.clone());
            quiet.push(block.clone());
            put(found, block.clone(), MarkdownKind::CodeBlock, block.clone());
            if matches!(kind, CodeBlockKind::Fenced(_)) {
                let rows = lines(text, &block);
                if let Some(first) = rows.first() {
                    put(found, first.clone(), MarkdownKind::Markup, block.clone());
                }
                if let Some(last) = rows.last().filter(|_| rows.len() > 1) {
                    let fence = text[last.clone()].trim_start_matches([' ', '>']).trim();
                    if fence.len() >= 3 && (fence.bytes().all(|b| b == b'`') || fence.bytes().all(|b| b == b'~')) {
                        put(found, last.clone(), MarkdownKind::Markup, block.clone());
                    }
                }
            }
        }
        Tag::BlockQuote(_) => {
            let quote = trimmed(text, &range);
            put(found, quote.clone(), MarkdownKind::Quote, quote.clone());
            for (n, line) in lines(text, &quote).into_iter().enumerate() {
                let bytes = text[line.clone()].as_bytes();
                let mut i = 0;
                // Later lines repeat the signs of the quotes around this one first.
                let mut outer = if n == 0 { 0 } else { depth };
                loop {
                    while matches!(bytes.get(i), Some(b' ' | b'\t')) {
                        i += 1;
                    }
                    if outer == 0 || bytes.get(i) != Some(&b'>') {
                        break;
                    }
                    outer -= 1;
                    i += 1;
                }
                if outer == 0 && bytes.get(i) == Some(&b'>') {
                    let space = usize::from(bytes.get(i + 1) == Some(&b' '));
                    let sign = line.start + i..line.start + i + 1 + space;
                    put(found, sign, MarkdownKind::QuoteMarker, line.clone());
                }
            }
        }
        Tag::Item => {
            leaves.extend(frame.inline.take());
            escapes(found);
            let bytes = text[range.clone()].as_bytes();
            let digits = bytes.iter().take_while(|b| b.is_ascii_digit()).count();
            let (len, ordered) = match bytes.get(digits) {
                Some(b'.' | b')') if digits > 0 => (digits + 1, true),
                Some(b'-' | b'*' | b'+') if digits == 0 => (1, false),
                _ => return,
            };
            let line = line_of(text, &(range.start..range.start));
            if frame.task {
                // The checkbox stands for the item; its bullet only shows while editing.
                let space = usize::from(bytes.get(len) == Some(&b' '));
                put(
                    found,
                    range.start..range.start + len + space,
                    MarkdownKind::Markup,
                    line,
                );
            } else {
                put(
                    found,
                    range.start..range.start + len,
                    MarkdownKind::ListMarker { ordered },
                    line,
                );
            }
        }
        Tag::Table(_) => {
            // One block: the cursor anywhere in the table shows all of its source.
            let table = trimmed(text, &range);
            leaves.push(table.clone());
            for line in lines(text, &table) {
                put(found, line, MarkdownKind::TableRow, table.clone());
            }
        }
        Tag::HtmlBlock => leaves.push(trimmed(text, &range)),
        _ => {}
    }
}

/// Turns what was found into ordered spans, each in its block.
fn arrange(text: &str, mut found: Vec<Found>, leaves: Vec<Range<usize>>) -> (Vec<Span>, Vec<Range<usize>>) {
    let line_start = |at: usize| text[..at].rfind('\n').map_or(0, |i| i + 1);
    found.retain(|f| !f.range.is_empty());
    found.sort_by(|a, b| a.range.start.cmp(&b.range.start).then(b.range.end.cmp(&a.range.end)));

    // A block starts with its line, so that the marker of a list item or the
    // sign of a quote belongs to the text that follows it.
    let mut blocks: Vec<Range<usize>> = leaves
        .into_iter()
        .filter(|l| !l.is_empty())
        .map(|l| line_start(l.start)..l.end)
        .collect();
    let holds = |blocks: &[Range<usize>], at: usize| blocks.iter().any(|b| b.start <= at && at < b.end);
    // What stands alone, the marker of an empty item, is a block of its line.
    for f in &found {
        if !holds(&blocks, f.range.start) {
            blocks.push(line_of(text, &(f.range.start..f.range.start)));
        }
    }
    blocks.sort_by_key(|b| (b.start, b.end));
    let mut disjoint: Vec<Range<usize>> = Vec::new();
    for block in blocks {
        let start = disjoint.last().map_or(block.start, |last| block.start.max(last.end));
        if start < block.end {
            disjoint.push(start..block.end);
        }
    }

    let spans = found
        .into_iter()
        .filter_map(|f| {
            let block = disjoint
                .iter()
                .position(|b| b.start <= f.range.start && f.range.start < b.end)?;
            Some(Span {
                reveal: f.reveal.unwrap_or_else(|| disjoint[block].clone()),
                range: f.range,
                kind: f.kind,
                block,
            })
        })
        .collect();
    (spans, disjoint)
}
