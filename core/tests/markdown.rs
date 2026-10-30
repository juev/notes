//! Markdown in notes as R48–R52 of Lists; the dialect of Notes is in docs/specs/editor.md.

use notes_core::MarkdownKind::*;
use notes_core::*;

fn cut(text: &str, start: u32, end: u32) -> String {
    let units: Vec<u16> = text.encode_utf16().collect();
    String::from_utf16(&units[start as usize..end as usize]).unwrap()
}

/// Every span as its kind and the text it covers.
fn spans(text: &str) -> Vec<(MarkdownKind, String)> {
    markdown_layout(text.into())
        .spans
        .into_iter()
        .map(|s| (s.kind, cut(text, s.start, s.end)))
        .collect()
}

/// The texts of the spans of one kind.
fn of(text: &str, kind: MarkdownKind) -> Vec<String> {
    spans(text)
        .into_iter()
        .filter(|(k, _)| *k == kind)
        .map(|(_, s)| s)
        .collect()
}

fn blocks(text: &str) -> Vec<String> {
    markdown_layout(text.into())
        .blocks
        .iter()
        .map(|b| cut(text, b.start, b.end))
        .collect()
}

fn link(url: &str) -> MarkdownKind {
    Link { url: url.into() }
}

/// The note after Return at `|`, with `|` where the cursor ends up; `None` when Return only starts a line.
fn enter(marked: &str) -> Option<String> {
    let at = marked.find('|').unwrap();
    let text = marked.replace('|', "");
    let cursor = text[..at].encode_utf16().count() as u32;
    let edit = markdown_newline(text.clone(), cursor)?;
    let mut units: Vec<u16> = text.encode_utf16().collect();
    units.splice(edit.start as usize..edit.end as usize, edit.text.encode_utf16());
    units.insert(edit.cursor as usize, '|' as u16);
    Some(String::from_utf16(&units).unwrap())
}

#[test]
fn r48_scenario_from_the_specification() {
    let text = "# План\n\n**важно** и *срочно*, см. [сайт](https://example.org)";
    assert_eq!(of(text, Heading { level: 1 }), ["# План"]);
    assert_eq!(of(text, Strong), ["важно"]);
    assert_eq!(of(text, Emphasis), ["срочно"]);
    assert_eq!(of(text, link("https://example.org")), ["сайт"]);
    assert_eq!(
        of(text, Markup),
        ["# ", "**", "**", "*", "*", "[", "](https://example.org)"]
    );
}

#[test]
fn r48_offsets_count_utf16_units() {
    let layout = markdown_layout("😀 **да**".into());
    let strong = layout.spans.iter().find(|s| s.kind == Strong).unwrap();
    assert_eq!((strong.start, strong.end), (5, 7));
}

#[test]
fn r48_headings_of_both_forms() {
    assert_eq!(of("### Три ###", Heading { level: 3 }), ["### Три ###"]);
    assert_eq!(of("### Три ###", Markup), ["### ", " ###"]);
    let text = "Заголовок\n===\n\nВторой\n---";
    assert_eq!(of(text, Heading { level: 1 }), ["Заголовок\n==="]);
    assert_eq!(of(text, Heading { level: 2 }), ["Второй\n---"]);
    assert_eq!(of(text, Markup), ["===", "---"]);
}

#[test]
fn r48_a_list_item_being_started_does_not_make_a_heading() {
    // By the letter of CommonMark a lone `-` underlines the line above it.
    for text in ["текст\n-", "текст\n- ", "- хлеб\n  - "] {
        assert!(
            !spans(text).iter().any(|(k, _)| matches!(k, Heading { .. })),
            "{text:?}"
        );
        assert_eq!(
            of(text, ListMarker { ordered: false }).last().map(String::as_str),
            Some("-"),
            "{text:?}"
        );
    }
    assert_eq!(of("текст\n--", Heading { level: 2 }).len(), 1);
}

#[test]
fn r48_emphasis_nests_and_strikes() {
    let text = "***оба*** и ~~нет~~ и __так__ и _эдак_";
    assert_eq!(of(text, Strong), ["оба", "так"]);
    assert_eq!(of(text, Strikethrough), ["нет"]);
    assert_eq!(of(text, Emphasis).len(), 2);
    // Nothing of the markup is left visible.
    let visible: String = text.replace(['*', '~', '_'], "");
    let hidden: usize = of(text, Markup).iter().map(|m| m.chars().count()).sum();
    assert_eq!(text.chars().count() - hidden, visible.chars().count());
}

#[test]
fn r48_unclosed_markup_is_text() {
    assert_eq!(spans("**не закрыто и `тоже"), []);
    assert_eq!(spans("2 * 3 * 4"), []);
}

#[test]
fn r48_code_in_a_line_and_in_blocks() {
    assert_eq!(of("вызов `f(x)` и ``a`b``", Code), ["f(x)", "a`b"]);
    assert_eq!(of("вызов `f(x)` и ``a`b``", Markup), ["`", "`", "``", "``"]);
    // Markup inside code is not markup.
    assert_eq!(of("`**нет**`", Strong), [] as [&str; 0]);

    let fenced = "```rust\nlet a = *b*;\n```\nпосле";
    assert_eq!(of(fenced, CodeBlock), ["```rust\nlet a = *b*;\n```"]);
    assert_eq!(of(fenced, Markup), ["```rust", "```"]);
    assert_eq!(of(fenced, Emphasis), [] as [&str; 0]);

    // A fence left open runs to the end and has no closing line to hide.
    assert_eq!(of("```\nкод", Markup), ["```"]);
    assert_eq!(of("абзац\n\n    отступ\n", CodeBlock).len(), 1);
}

#[test]
fn r48_lists_with_nesting() {
    let text = "- один\n- два\n  1. вложенный\n  2) другой\n* три\n+ четыре";
    assert_eq!(of(text, ListMarker { ordered: false }), ["-", "-", "*", "+"]);
    assert_eq!(of(text, ListMarker { ordered: true }), ["1.", "2)"]);
    assert_eq!(
        blocks(text),
        ["- один", "- два", "  1. вложенный", "  2) другой", "* три", "+ четыре"]
    );
}

#[test]
fn r48_a_block_is_what_markup_shows_in_together() {
    let text = "**один**\n\n*два*\nи ещё\n\n- пункт\n\n  второй абзац";
    assert_eq!(blocks(text), ["**один**", "*два*\nи ещё", "- пункт", "  второй абзац"]);
    let layout = markdown_layout(text.into());
    let block_of = |kind: MarkdownKind| layout.spans.iter().find(|s| s.kind == kind).unwrap().block;
    assert_eq!(block_of(Strong), 0);
    assert_eq!(block_of(Emphasis), 1);
    assert_eq!(block_of(ListMarker { ordered: false }), 2);
    for pair in layout.blocks.windows(2) {
        assert!(
            pair[0].end < pair[1].start,
            "blocks touch: a cursor between them would be in both"
        );
    }
}

#[test]
fn r48_task_lists() {
    let text = "- [ ] хлеб\n- [x] молоко\n1. [X] нумерованный";
    assert_eq!(of(text, Checkbox { checked: false }), ["[ ]"]);
    assert_eq!(of(text, Checkbox { checked: true }), ["[x]", "[X]"]);
    // The checkbox replaces the bullet.
    assert_eq!(of(text, ListMarker { ordered: false }), [] as [&str; 0]);
    assert_eq!(of(text, Markup), ["- ", "- ", "1. "]);
    // Brackets elsewhere are text.
    assert_eq!(of("не список [ ] и [x]", Checkbox { checked: false }), [] as [&str; 0]);
}

#[test]
fn r48_quotes() {
    let text = "> первая\n> вторая\nленивая\n\n> > вложенная\n> > ещё";
    assert_eq!(of(text, QuoteMarker), ["> ", "> ", "> ", "> ", "> ", "> "]);
    assert_eq!(of(text, Quote).len(), 3);
    assert_eq!(of("- > в списке\n  > дальше", QuoteMarker), ["> ", "> "]);
}

#[test]
fn r48_rule_and_table() {
    assert_eq!(of("до\n\n---\n\nпосле", Rule), ["---"]);
    let table = "| а | б |\n|---|---|\n| 1 | 2 |";
    assert_eq!(of(table, TableRow), ["| а | б |", "|---|---|", "| 1 | 2 |"]);
    assert_eq!(blocks(table), [table]);
}

/// The rows of the tables of a note: whether a header, and what each cell says.
fn grid(text: &str) -> Vec<(bool, Vec<String>)> {
    markdown_layout(text.into())
        .tables
        .iter()
        .flat_map(|t| t.rows.iter())
        .map(|r| (r.header, r.cells.iter().map(|c| cut(text, c.start, c.end)).collect()))
        .collect()
}

#[test]
fn r60_table_reports_cells_and_alignment() {
    let text =
        "до\n\nимя | 😀 длинная ячейка | c\n:-- | :-: | --:\n**ж** | два |\nодна\n| a | b | c | лишняя |\n\nпосле";
    let layout = markdown_layout(text.into());
    let [table] = &layout.tables[..] else {
        panic!("one table expected")
    };
    assert_eq!(
        table.columns,
        [MarkdownAlign::Left, MarkdownAlign::Center, MarkdownAlign::Right]
    );
    // The table is one block, so the cursor anywhere in it shows all of its source.
    let block = &layout.blocks[table.block as usize];
    assert_eq!((block.start, block.end), (table.start, table.end));
    assert!(cut(text, table.start, table.end).starts_with("имя"));
    assert!(cut(text, table.start, table.end).ends_with("лишняя |"));
    // The line of dashes is not a row; a short row is filled up, a long one cut.
    let row = |header: bool, cells: [&str; 3]| (header, cells.map(String::from).to_vec());
    assert_eq!(
        grid(text),
        [
            row(true, ["имя", "😀 длинная ячейка", "c"]),
            row(false, ["**ж**", "два", ""]),
            row(false, ["одна", "", ""]),
            row(false, ["a", "b", "c"]),
        ]
    );
    // A cell a short row is filled up with lies at the end of its line.
    let line_end = table.rows[2].cells[0].end;
    assert_eq!(cut(text, table.rows[2].cells[0].start, line_end), "одна");
    assert!(table.rows[2].cells[1..]
        .iter()
        .all(|c| (c.start, c.end) == (line_end, line_end)));
    // What styles the text of a cell lies inside the cell.
    let cell = &table.rows[1].cells[0];
    let strong = layout.spans.iter().find(|s| s.kind == Strong).unwrap();
    assert!(cell.start <= strong.start && strong.end <= cell.end);
}

#[test]
fn r60_tables_in_a_list_item_and_in_a_quote() {
    let text = "- пункт\n\n  | a | b |\n  |---|---|\n  | 1 |   |\n> | q | w |\n> |---|---|\n> | 1 | 2 |";
    let row = |header: bool, cells: [&str; 2]| (header, cells.map(String::from).to_vec());
    assert_eq!(
        grid(text),
        [
            row(true, ["a", "b"]),
            row(false, ["1", ""]),
            row(true, ["q", "w"]),
            row(false, ["1", "2"]),
        ]
    );
    assert_eq!(markdown_layout(text.into()).tables.len(), 2);
    assert!(markdown_layout("просто | текст".into()).tables.is_empty());
}

#[test]
fn r48_links_of_every_form() {
    let text = "[по сноске][1], <https://a.example/x> и голая https://b.example/y?z=1.\n\n[1]: https://c.example";
    assert_eq!(of(text, link("https://c.example")), ["по сноске"]);
    assert_eq!(of(text, link("https://a.example/x")), ["https://a.example/x"]);
    assert_eq!(of(text, link("https://b.example/y?z=1")), ["https://b.example/y?z=1"]);
    assert_eq!(
        of("<me@example.org>", link("mailto:me@example.org")),
        ["me@example.org"]
    );
    // An address inside a link or code is not linked twice.
    assert_eq!(
        spans("[https://a.example](https://b.example)")
            .iter()
            .filter(|(k, _)| matches!(k, Link { .. }))
            .count(),
        1
    );
    assert_eq!(
        spans("`https://a.example`")
            .iter()
            .filter(|(k, _)| matches!(k, Link { .. }))
            .count(),
        0
    );
    assert_eq!(spans("xhttps://a.example").len(), 0);
}

#[test]
fn r48_the_scheme_of_a_link_is_read_without_regard_to_case() {
    // The scenario of the specification: a keyboard starts a sentence with a capital.
    let text = "Https://example.org/A и [x](HTTPS://EXAMPLE.ORG)";
    assert_eq!(of(text, link("https://example.org/A")), ["Https://example.org/A"]);
    assert_eq!(of(text, link("https://EXAMPLE.ORG")), ["x"]);
    assert_eq!(
        of("x\nHTTP://b.example/Y.", link("http://b.example/Y")),
        ["HTTP://b.example/Y"]
    );
    assert_eq!(
        of("<Https://a.example/x>", link("https://a.example/x")),
        ["Https://a.example/x"]
    );
    assert_eq!(of("[x](MailTo:me@example.org)", link("mailto:me@example.org")), ["x"]);
    // Letters of more than one byte before the address do not move its range.
    assert_eq!(
        of("Ёж Https://a.example", link("https://a.example")),
        ["Https://a.example"]
    );
    // A scheme alone and a scheme inside a word are still text.
    assert_eq!(spans("Https://").len(), 0);
    assert_eq!(spans("xHttps://a.example").len(), 0);
}

#[test]
fn r48_an_image_is_a_link_with_its_text() {
    let text = "![схема](https://example.org/a.png)";
    assert_eq!(of(text, link("https://example.org/a.png")), ["схема"]);
    assert_eq!(of(text, Markup), ["![", "](https://example.org/a.png)"]);
    // Without a text the address is all there is to show.
    assert_eq!(of("![](https://example.org/a.png)", Markup), [] as [&str; 0]);
}

#[test]
fn r53_only_web_and_mail_addresses_are_links() {
    for text in [
        "[x](javascript:alert(1))",
        "[x](file:///etc/passwd)",
        "[x](data:text/html,hi)",
        "[x](/relative)",
        "<lists://add?title=x>",
    ] {
        assert!(!spans(text).iter().any(|(k, _)| matches!(k, Link { .. })), "{text}");
    }
    assert_eq!(of("[x](HTTPS://EXAMPLE.ORG)", link("https://EXAMPLE.ORG")), ["x"]);
}

#[test]
fn r48_html_is_text() {
    assert_eq!(spans("<img src=x onerror=alert(1)>"), []);
    assert_eq!(spans("текст <b>не жирный</b> дальше"), []);
}

#[test]
fn r48_escapes() {
    assert_eq!(
        spans(r"\*не курсив\*"),
        [(Markup, r"\".to_string()), (Markup, r"\".to_string())]
    );
    assert_eq!(of(r"- пункт \# и *так\**", Markup), [r"\", "*", r"\", "*"]);
    assert_eq!(of(r"# Заголовок \#", Markup), ["# ", r"\"]);
}

#[test]
fn r50_spans_stay_inside_the_text_and_on_character_boundaries() {
    let samples = [
        "",
        "\n\n",
        "простой текст",
        "# 😀\n- [ ] 👨‍👩‍👧 **семья**\n> цитата\r\nс CRLF\r\n\r\n```\nкод\n```\n",
        "- \n- [ ] \n> \n1. ",
        "**",
        "[](",
        "| a |\n|-|\n",
        "- a\n\n      код в пункте\n- > цитата\n  > ещё",
    ];
    for text in samples {
        let units: Vec<u16> = text.encode_utf16().collect();
        let layout = markdown_layout(text.into());
        for span in &layout.spans {
            assert!(
                span.start < span.end && span.end as usize <= units.len(),
                "{text:?}: {span:?}"
            );
            assert!(
                String::from_utf16(&units[span.start as usize..span.end as usize]).is_ok(),
                "{text:?}: {span:?}"
            );
            let block = &layout.blocks[span.block as usize];
            assert!(
                block.start <= span.start && span.start < block.end,
                "{text:?}: {span:?} outside {block:?}"
            );
        }
    }
}

#[test]
fn r52_return_continues_a_list() {
    assert_eq!(enter("- хлеб|").as_deref(), Some("- хлеб\n- |"));
    assert_eq!(enter("* хлеб|\n* молоко").as_deref(), Some("* хлеб\n* |\n* молоко"));
    assert_eq!(enter("9. девять|").as_deref(), Some("9. девять\n10. |"));
    assert_eq!(enter("1) раз|").as_deref(), Some("1) раз\n2) |"));
    assert_eq!(enter("  - [x] сделано|").as_deref(), Some("  - [x] сделано\n  - [ ] |"));
    assert_eq!(enter("- хлеб |и молоко").as_deref(), Some("- хлеб \n- |и молоко"));
    assert_eq!(enter("- 😀 да|").as_deref(), Some("- 😀 да\n- |"));
}

#[test]
fn r52_return_on_an_empty_item_takes_the_marker_off() {
    assert_eq!(enter("- хлеб\n- |").as_deref(), Some("- хлеб\n|"));
    assert_eq!(enter("- хлеб\n  - |").as_deref(), Some("- хлеб\n|"));
    assert_eq!(enter("- [ ] хлеб\n- [ ] |").as_deref(), Some("- [ ] хлеб\n|"));
    assert_eq!(enter("1. раз\n2. |\nдальше").as_deref(), Some("1. раз\n|\nдальше"));
}

#[test]
fn r52_return_continues_a_quote() {
    assert_eq!(enter("> цитата|").as_deref(), Some("> цитата\n> |"));
    assert_eq!(enter("> цитата\n> |").as_deref(), Some("> цитата\n|"));
    assert_eq!(enter("> - пункт|").as_deref(), Some("> - пункт\n> - |"));
    assert_eq!(enter("> - пункт\n> - |").as_deref(), Some("> - пункт\n> |"));
}

#[test]
fn r52_return_elsewhere_only_starts_a_line() {
    assert_eq!(enter("просто текст|"), None);
    assert_eq!(enter("# Заголовок|"), None);
    assert_eq!(enter("```\n- не список|\n```"), None);
    assert_eq!(enter("- - -|"), None);
    assert_eq!(enter("-не список|"), None);
    // Before the text of the item the marker is not carried over.
    assert_eq!(enter("|- хлеб"), None);
    assert_eq!(enter("-| хлеб"), None);
    assert_eq!(markdown_newline("- хлеб".into(), 99), None);
}
