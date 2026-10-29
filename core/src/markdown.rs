//! Markdown in notes: docs/specs/product.md, R48–R52.
//!
//! The note stays the text that was typed. The apps get the ranges to style,
//! to hide and to replace, in UTF-16 units, which is how Swift's `NSString`,
//! Kotlin and JavaScript all index a string, so one note looks the same
//! everywhere.

use std::ops::Range;

use pulldown_cmark::{Alignment, CodeBlockKind, Event, HeadingLevel, LinkType, Options, Parser, Tag};

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum MarkdownKind {
    Heading {
        level: u8,
    },
    Strong,
    Emphasis,
    Strikethrough,
    Code,
    CodeBlock,
    Quote,
    /// Text that opens `url`; only `http`, `https` and `mailto` get here.
    Link {
        url: String,
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
    /// A line of a table as it is typed: monospaced. Outside the block that
    /// holds the cursor the table is drawn as a grid from `MarkdownLayout::tables`.
    TableRow,
    /// Markup characters: hidden unless the cursor is in their block.
    Markup,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct MarkdownSpan {
    pub start: u32,
    pub end: u32,
    pub kind: MarkdownKind,
    /// Index into `MarkdownLayout::blocks`.
    pub block: u32,
}

/// A paragraph, a heading, a list item without the items nested in it, a code
/// block or a table. Markup shows in the block that holds the cursor,
/// its end included.
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

/// A table to draw as a grid (R60) while the cursor is outside its block.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct MarkdownTable {
    pub start: u32,
    pub end: u32,
    /// Index into `MarkdownLayout::blocks`.
    pub block: u32,
    pub columns: Vec<MarkdownAlign>,
    pub rows: Vec<MarkdownTableRow>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, uniffi::Record)]
pub struct MarkdownLayout {
    /// Ordered by start; of two that start together the longer comes first.
    pub spans: Vec<MarkdownSpan>,
    /// Ordered, not overlapping.
    pub blocks: Vec<MarkdownBlock>,
    /// Ordered by start.
    pub tables: Vec<MarkdownTable>,
}

/// A replacement in the note: `start..end` becomes `text`, the cursor moves to `cursor`.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct MarkdownEdit {
    pub start: u32,
    pub end: u32,
    pub text: String,
    pub cursor: u32,
}

/// What to style, hide and replace in a note.
#[uniffi::export]
pub fn markdown_layout(text: String) -> MarkdownLayout {
    let (spans, blocks, tables) = layout(&text);
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
            })
            .collect(),
        blocks: blocks
            .into_iter()
            .map(|b| MarkdownBlock {
                start: units.at(b.start),
                end: units.at(b.end),
            })
            .collect(),
    }
}

/// What Return does at `cursor` beyond starting a line: continues a list or a
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
    let (spans, _, _) = layout(&text);
    let marked = spans.iter().any(|s| {
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
        cursor: cursor + moved,
    })
}

enum Marker {
    Bullet(char),
    Number(u64, char),
}

/// What a line starts with before its text.
struct Prefix {
    /// Indentation and `>` signs: what the next line repeats as it is.
    lead: usize,
    quoted: bool,
    marker: Option<Marker>,
    checkbox: bool,
    /// Where the text begins.
    end: usize,
}

impl Prefix {
    fn of(line: &str) -> Prefix {
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
struct Utf16(Vec<u32>);

impl Utf16 {
    fn of(text: &str) -> Utf16 {
        let mut map = vec![0; text.len() + 1];
        let mut units = 0;
        for (i, c) in text.char_indices() {
            map[i] = units;
            units += c.len_utf16() as u32;
        }
        map[text.len()] = units;
        Utf16(map)
    }

    fn at(&self, byte: usize) -> u32 {
        self.0[byte]
    }

    /// `None` for an offset past the end or inside a surrogate pair.
    fn byte(&self, units: u32) -> Option<usize> {
        if units == 0 {
            return Some(0);
        }
        self.0.iter().position(|&u| u == units)
    }
}

struct Span {
    range: Range<usize>,
    kind: MarkdownKind,
    block: usize,
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

/// The address of a link with its scheme in lower case, or nothing when the
/// scheme is not one a note may lead to. The rest stays as typed.
fn address(url: &str) -> Option<String> {
    ["http://", "https://", "mailto:"].iter().find_map(|scheme| {
        let head = url.get(..scheme.len())?;
        head.eq_ignore_ascii_case(scheme)
            .then(|| format!("{scheme}{}", &url[scheme.len()..]))
    })
}

fn trimmed(text: &str, range: &Range<usize>) -> Range<usize> {
    let slice = &text[range.clone()];
    let start = range.start + (slice.len() - slice.trim_start_matches('\n').len());
    let end = range.end - (slice.len() - slice.trim_end_matches(['\n', '\r', ' ', '\t']).len());
    start..end.max(start)
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

fn layout(text: &str) -> (Vec<Span>, Vec<Range<usize>>, Vec<Table>) {
    let mut tables: Vec<Table> = Vec::new();
    let mut rows: Vec<(bool, Vec<Range<usize>>)> = Vec::new();
    let mut cells: Vec<Range<usize>> = Vec::new();
    let mut found: Vec<(Range<usize>, MarkdownKind)> = Vec::new();
    let mut leaves: Vec<Range<usize>> = Vec::new();
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
                close(text, &mut frame, depth, &mut found, &mut leaves);
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
                found.push((rule, MarkdownKind::Rule));
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
                            found.push((range.start..range.start + ticks, MarkdownKind::Markup));
                            found.push((range.start + ticks..range.end - ticks, MarkdownKind::Code));
                            found.push((range.end - ticks..range.end, MarkdownKind::Markup));
                        }
                    }
                    Event::TaskListMarker(checked) => {
                        found.push((range.clone(), MarkdownKind::Checkbox { checked }));
                        if let Some(item) = stack.iter_mut().rev().find(|f| matches!(f.tag, Tag::Item)) {
                            item.task = true;
                        }
                    }
                    Event::Text(_) => {
                        let plain = !stack
                            .iter()
                            .any(|f| matches!(f.tag, Tag::Link { .. } | Tag::Image { .. } | Tag::CodeBlock(_)));
                        if plain {
                            bare_links(text, &range, &mut found);
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
    let (spans, blocks) = arrange(text, found, leaves);
    (spans, blocks, tables)
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

fn bare_links(text: &str, range: &Range<usize>, found: &mut Vec<(Range<usize>, MarkdownKind)>) {
    let slice = &text[range.clone()];
    // A keyboard starts a sentence with a capital: `Https://` is an address too.
    // Lower case of ASCII alone keeps every byte offset.
    let lower = slice.to_ascii_lowercase();
    let mut from = 0;
    while let Some(i) = ["https://", "http://"]
        .iter()
        .filter_map(|scheme| lower[from..].find(scheme))
        .min()
    {
        let start = from + i;
        let boundary = slice[..start].chars().next_back().is_none_or(|c| !c.is_alphanumeric());
        let mut end = start + slice[start..].find(char::is_whitespace).unwrap_or(slice.len() - start);
        while slice[start..end].ends_with(['.', ',', ';', ':', '!', '?', ')', '"', '\'', '>']) {
            end -= 1;
        }
        let url = address(&slice[start..end])
            .filter(|url| boundary && url.split_once("://").is_some_and(|(_, rest)| !rest.is_empty()));
        if let Some(url) = url {
            found.push((range.start + start..range.start + end, MarkdownKind::Link { url }));
        }
        from = end.max(start + 1);
    }
}

fn close(
    text: &str,
    frame: &mut Frame,
    depth: usize,
    found: &mut Vec<(Range<usize>, MarkdownKind)>,
    leaves: &mut Vec<Range<usize>>,
) {
    let range = frame.range.clone();
    let inside = match (frame.children.first(), frame.children.last()) {
        (Some(first), Some(last)) => Some(first.start..last.end),
        _ => None,
    };
    let escapes = |found: &mut Vec<(Range<usize>, MarkdownKind)>| {
        for gap in gaps(&range, &frame.children) {
            if &text[gap.clone()] == "\\" {
                found.push((gap, MarkdownKind::Markup));
            }
        }
    };
    let styled = |kind: MarkdownKind, found: &mut Vec<(Range<usize>, MarkdownKind)>| {
        let Some(inside) = inside.clone() else { return };
        found.push((inside, kind));
        found.extend(
            gaps(&range, &frame.children)
                .into_iter()
                .map(|gap| (gap, MarkdownKind::Markup)),
        );
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
            // of CommonMark. Here it is a list item being started: the line above
            // must not turn into a heading for the one keystroke before its text.
            if let Some(last) = frame.children.last() {
                let under = &text[last.end.min(heading.end)..heading.end];
                if under.trim() == "-" {
                    let at = last.end + under.find('-').unwrap_or(0);
                    found.push((at..at + 1, MarkdownKind::ListMarker { ordered: false }));
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
            found.push((heading, MarkdownKind::Heading { level }));
            for gap in gaps(&range, &frame.children) {
                let slice = &text[gap.clone()];
                let start = gap.start + (slice.len() - slice.trim_start_matches(['\n', '\r']).len());
                let end = gap.end - (slice.len() - slice.trim_end_matches(['\n', '\r']).len());
                if start < end && !text[start..end].trim().is_empty() {
                    found.push((start..end, MarkdownKind::Markup));
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
            match (address(&url), inside.is_some()) {
                (Some(url), true) => styled(MarkdownKind::Link { url }, found),
                // Nothing to show but the address: it stays visible.
                (Some(url), false) => found.push((range.clone(), MarkdownKind::Link { url })),
                (None, true) => found.extend(
                    gaps(&range, &frame.children)
                        .into_iter()
                        .map(|gap| (gap, MarkdownKind::Markup)),
                ),
                (None, false) => {}
            }
        }
        Tag::CodeBlock(kind) => {
            let block = trimmed(text, &range);
            leaves.push(block.clone());
            found.push((block.clone(), MarkdownKind::CodeBlock));
            if matches!(kind, CodeBlockKind::Fenced(_)) {
                let rows = lines(text, &block);
                if let Some(first) = rows.first() {
                    found.push((first.clone(), MarkdownKind::Markup));
                }
                if let Some(last) = rows.last().filter(|_| rows.len() > 1) {
                    let fence = text[last.clone()].trim_start_matches([' ', '>']).trim();
                    if fence.len() >= 3 && (fence.bytes().all(|b| b == b'`') || fence.bytes().all(|b| b == b'~')) {
                        found.push((last.clone(), MarkdownKind::Markup));
                    }
                }
            }
        }
        Tag::BlockQuote(_) => {
            let quote = trimmed(text, &range);
            found.push((quote.clone(), MarkdownKind::Quote));
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
                    found.push((line.start + i..line.start + i + 1 + space, MarkdownKind::QuoteMarker));
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
            if frame.task {
                // The checkbox stands for the item; its bullet only shows while editing.
                let space = usize::from(bytes.get(len) == Some(&b' '));
                found.push((range.start..range.start + len + space, MarkdownKind::Markup));
            } else {
                found.push((range.start..range.start + len, MarkdownKind::ListMarker { ordered }));
            }
        }
        Tag::Table(_) => {
            // One block: the cursor anywhere in the table shows all of its source.
            let table = trimmed(text, &range);
            leaves.push(table.clone());
            for line in lines(text, &table) {
                found.push((line, MarkdownKind::TableRow));
            }
        }
        Tag::HtmlBlock => leaves.push(trimmed(text, &range)),
        _ => {}
    }
}

/// Turns what was found into ordered spans, each in its block.
fn arrange(
    text: &str,
    mut found: Vec<(Range<usize>, MarkdownKind)>,
    leaves: Vec<Range<usize>>,
) -> (Vec<Span>, Vec<Range<usize>>) {
    let line_of = |at: usize| {
        let start = text[..at].rfind('\n').map_or(0, |i| i + 1);
        let end = text[at..].find('\n').map_or(text.len(), |i| at + i);
        start..end
    };
    found.retain(|(range, _)| !range.is_empty());
    found.sort_by(|a, b| a.0.start.cmp(&b.0.start).then(b.0.end.cmp(&a.0.end)));

    // A block starts with its line, so that the marker of a list item or the
    // sign of a quote belongs to the text that follows it.
    let mut blocks: Vec<Range<usize>> = leaves
        .into_iter()
        .filter(|l| !l.is_empty())
        .map(|l| line_of(l.start).start..l.end)
        .collect();
    let holds = |blocks: &[Range<usize>], at: usize| blocks.iter().any(|b| b.start <= at && at < b.end);
    // What stands alone, the marker of an empty item, is a block of its line.
    for (range, _) in &found {
        if !holds(&blocks, range.start) {
            blocks.push(line_of(range.start));
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
        .filter_map(|(range, kind)| {
            let block = disjoint
                .iter()
                .position(|b| b.start <= range.start && range.start < b.end)?;
            Some(Span { range, kind, block })
        })
        .collect();
    (spans, disjoint)
}
