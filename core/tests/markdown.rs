//! The Markdown of a note: E1–E16, E22, E30, E31 and E51 in docs/specs/editor.md.

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
fn e10_styles_hides_and_links() {
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
fn e10_offsets_count_utf16_units() {
    let layout = markdown_layout("😀 **да**".into());
    let strong = layout.spans.iter().find(|s| s.kind == Strong).unwrap();
    assert_eq!((strong.start, strong.end), (5, 7));
}

#[test]
fn e2_headings_of_both_forms() {
    assert_eq!(of("### Три ###", Heading { level: 3 }), ["### Три ###"]);
    assert_eq!(of("### Три ###", Markup), ["### ", " ###"]);
    let text = "Заголовок\n===\n\nВторой\n---";
    assert_eq!(of(text, Heading { level: 1 }), ["Заголовок\n==="]);
    assert_eq!(of(text, Heading { level: 2 }), ["Второй\n---"]);
    assert_eq!(of(text, Markup), ["===", "---"]);
}

#[test]
fn e5_a_list_item_being_started_does_not_make_a_heading() {
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
fn e1_emphasis_nests_and_strikes() {
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
fn e1_unclosed_markup_is_text() {
    assert_eq!(spans("**не закрыто и `тоже"), []);
    assert_eq!(spans("2 * 3 * 4"), []);
}

#[test]
fn e16_code_in_a_line_and_in_blocks() {
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
fn e12_lists_with_nesting() {
    let text = "- один\n- два\n  1. вложенный\n  2) другой\n* три\n+ четыре";
    assert_eq!(of(text, ListMarker { ordered: false }), ["-", "-", "*", "+"]);
    assert_eq!(of(text, ListMarker { ordered: true }), ["1.", "2)"]);
    assert_eq!(
        blocks(text),
        ["- один", "- два", "  1. вложенный", "  2) другой", "* три", "+ четыре"]
    );
}

#[test]
fn e10_a_block_is_a_paragraph_an_item_or_a_table() {
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
fn e12_task_lists() {
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
fn e12_quotes() {
    let text = "> первая\n> вторая\nленивая\n\n> > вложенная\n> > ещё";
    assert_eq!(of(text, QuoteMarker), ["> ", "> ", "> ", "> ", "> ", "> "]);
    assert_eq!(of(text, Quote).len(), 3);
    assert_eq!(of("- > в списке\n  > дальше", QuoteMarker), ["> ", "> "]);
}

#[test]
fn e12_rule_and_table() {
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
fn e15_table_reports_cells_and_alignment() {
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
fn e15_tables_in_a_list_item_and_in_a_quote() {
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
fn e13_links_of_every_form() {
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
fn e4_the_scheme_of_a_link_is_read_without_regard_to_case() {
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
fn e13_an_image_is_a_link_with_its_text() {
    let text = "![схема](https://example.org/a.png)";
    assert_eq!(of(text, link("https://example.org/a.png")), ["схема"]);
    assert_eq!(of(text, Markup), ["![", "](https://example.org/a.png)"]);
    // Without a text the address is all there is to show.
    assert_eq!(of("![](https://example.org/a.png)", Markup), [] as [&str; 0]);
}

#[test]
fn e4_other_schemes_do_not_open() {
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
fn e4_html_is_text() {
    assert_eq!(spans("<img src=x onerror=alert(1)>"), []);
    assert_eq!(spans("текст <b>не жирный</b> дальше"), []);
}

#[test]
fn e1_escapes() {
    assert_eq!(
        spans(r"\*не курсив\*"),
        [(Markup, r"\".to_string()), (Markup, r"\".to_string())]
    );
    assert_eq!(of(r"- пункт \# и *так\**", Markup), [r"\", "*", r"\", "*"]);
    assert_eq!(of(r"# Заголовок \#", Markup), ["# ", r"\"]);
}

#[test]
fn e10_spans_stay_inside_the_text_and_on_character_boundaries() {
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
        "==😀== и ===не=== ==",
        "![фото](attachment:a1) [x](note:n1) www.пример.рф me@пример.рф a@b.co",
        "| **a** | ==b== |\n|---|---|\n| `c` | [d](note:x) |",
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
            assert!(
                span.reveal_start <= span.start && span.end <= span.reveal_end,
                "{text:?}: {span:?} is not revealed where it is"
            );
        }
    }
}

#[test]
fn e22_return_continues_a_list() {
    assert_eq!(enter("- хлеб|").as_deref(), Some("- хлеб\n- |"));
    assert_eq!(enter("* хлеб|\n* молоко").as_deref(), Some("* хлеб\n* |\n* молоко"));
    assert_eq!(enter("9. девять|").as_deref(), Some("9. девять\n10. |"));
    assert_eq!(enter("1) раз|").as_deref(), Some("1) раз\n2) |"));
    assert_eq!(enter("  - [x] сделано|").as_deref(), Some("  - [x] сделано\n  - [ ] |"));
    assert_eq!(enter("- хлеб |и молоко").as_deref(), Some("- хлеб \n- |и молоко"));
    assert_eq!(enter("- 😀 да|").as_deref(), Some("- 😀 да\n- |"));
}

#[test]
fn e22_return_on_an_empty_item_takes_the_marker_off() {
    assert_eq!(enter("- хлеб\n- |").as_deref(), Some("- хлеб\n|"));
    assert_eq!(enter("- хлеб\n  - |").as_deref(), Some("- хлеб\n|"));
    assert_eq!(enter("- [ ] хлеб\n- [ ] |").as_deref(), Some("- [ ] хлеб\n|"));
    assert_eq!(enter("1. раз\n2. |\nдальше").as_deref(), Some("1. раз\n|\nдальше"));
}

#[test]
fn e22_return_continues_a_quote() {
    assert_eq!(enter("> цитата|").as_deref(), Some("> цитата\n> |"));
    assert_eq!(enter("> цитата\n> |").as_deref(), Some("> цитата\n|"));
    assert_eq!(enter("> - пункт|").as_deref(), Some("> - пункт\n> - |"));
    assert_eq!(enter("> - пункт\n> - |").as_deref(), Some("> - пункт\n> |"));
}

#[test]
fn e22_return_elsewhere_only_starts_a_line() {
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

/// The texts of the markup spans that show with the cursor at `‸`.
fn shown(marked: &str) -> Vec<String> {
    let at = marked.find('‸').unwrap();
    let text = marked.replace('‸', "");
    let cursor = text[..at].encode_utf16().count() as u32;
    markdown_layout(text.clone())
        .spans
        .into_iter()
        .filter(|s| s.kind == Markup && s.reveal_start <= cursor && cursor <= s.reveal_end)
        .map(|s| cut(&text, s.start, s.end))
        .collect()
}

fn quiet(text: &str) -> Vec<String> {
    markdown_layout(text.into())
        .no_spellcheck
        .iter()
        .map(|r| cut(text, r.start, r.end))
        .collect()
}

#[test]
fn e1_highlight() {
    let text = "до ==важное== и ==ещё **одно**== после";
    assert_eq!(of(text, Highlight), ["важное", "ещё **одно**"]);
    assert_eq!(of(text, Markup), ["==", "==", "==", "**", "**", "=="]);
    // Spaces inside the markers, a run of three and an escape keep the signs as text.
    for text in [
        "a == b == c",
        "===не===",
        r"\==нет==",
        "== нет==",
        "==нет ==",
        "a==",
        "====",
    ] {
        assert_eq!(of(text, Highlight), [] as [&str; 0], "{text:?}");
    }
    // In code and in an address the signs are text.
    assert_eq!(of("`==a==` и\n\n```\n==b==\n```", Highlight), [] as [&str; 0]);
    assert_eq!(of("https://a.example/?q==x==", Highlight), [] as [&str; 0]);
    // A highlight does not run from one paragraph into the next.
    assert_eq!(of("==a\n\nb==", Highlight), [] as [&str; 0]);
    assert_eq!(of("# ==заголовок==", Highlight), ["заголовок"]);
}

#[test]
fn e1_addresses_typed_without_markup() {
    let text = "см. www.example.org/путь, Www.Example.org. и почту me.too+x@mail.example.org.";
    assert_eq!(of(text, link("http://www.example.org/путь")), ["www.example.org/путь"]);
    assert_eq!(of(text, link("http://Www.Example.org")), ["Www.Example.org"]);
    assert_eq!(
        of(text, link("mailto:me.too+x@mail.example.org")),
        ["me.too+x@mail.example.org"]
    );
    for text in [
        "xwww.example.org",
        "www.",
        "www. пробел",
        "a@b",
        "@example.org",
        "a@.example.org",
    ] {
        assert!(!spans(text).iter().any(|(k, _)| matches!(k, Link { .. })), "{text:?}");
    }
    // An address with a user in it is one link, not a link and a mail address.
    assert_eq!(
        spans("https://user@example.org/x")
            .iter()
            .filter(|(k, _)| matches!(k, Link { .. }))
            .count(),
        1
    );
}

#[test]
fn e2_six_levels() {
    let text = "# 1\n## 2\n### 3\n#### 4\n##### 5\n###### 6\n####### 7";
    let levels: Vec<u8> = spans(text)
        .into_iter()
        .filter_map(|(k, _)| match k {
            Heading { level } => Some(level),
            _ => None,
        })
        .collect();
    assert_eq!(levels, [1, 2, 3, 4, 5, 6]);
}

#[test]
fn e4_links_to_notes_and_attachments_are_recognised() {
    assert_eq!(of("[план](NOTE:n-1)", NoteLink { id: "n-1".into() }), ["план"]);
    assert_eq!(
        of(
            "[a.pdf](Attachment:a-1)",
            AttachmentLink {
                id: "a-1".into(),
                inline: false
            }
        ),
        ["[a.pdf](Attachment:a-1)"]
    );
    // A scheme with no identifier leads nowhere.
    for text in ["[x](note:)", "[x](attachment:)", "[x](notes:1)"] {
        assert!(
            !spans(text)
                .iter()
                .any(|(k, _)| matches!(k, NoteLink { .. } | AttachmentLink { .. })),
            "{text:?}"
        );
    }
}

#[test]
fn e11_inline_markup_shows_with_the_cursor_in_or_next_to_the_span() {
    // The scenario of the specification.
    assert_eq!(shown("a‸ **b** c"), [] as [&str; 0]);
    assert_eq!(shown("a **b‸** c"), ["**", "**"]);
    assert_eq!(shown("a **b**‸ c"), ["**", "**"]);
    assert_eq!(shown("a ‸**b** c"), ["**", "**"]);
    assert_eq!(shown("a **b** ‸c"), [] as [&str; 0]);
    // Only the span with the cursor shows, not the others of the paragraph.
    assert_eq!(shown("*a‸* и ~~b~~ и ==c== и `d`"), ["*", "*"]);
    assert_eq!(shown("*a* и ~~b‸~~ и ==c== и `d`"), ["~~", "~~"]);
    assert_eq!(shown("*a* и ~~b~~ и ==‸c== и `d`"), ["==", "=="]);
    assert_eq!(shown("*a* и ~~b~~ и ==c== и `d‸`"), ["`", "`"]);
    // An escape shows with the character it escapes.
    assert_eq!(shown(r"a \*‸ b"), [r"\"]);
    assert_eq!(shown(r"a‸ \* b"), [] as [&str; 0]);
}

#[test]
fn e12_signs_of_headings_and_quotes_show_on_their_line() {
    assert_eq!(shown("# Заголо‸вок ##\n\nтекст"), ["# ", " ##"]);
    assert_eq!(shown("# Заголовок ##\n\nтек‸ст"), [] as [&str; 0]);
    assert_eq!(shown("Заголовок\n==‸="), ["==="]);
    let quote = "> первая\n> вторая";
    let signs: Vec<String> = markdown_layout(quote.into())
        .spans
        .into_iter()
        .filter(|s| s.kind == QuoteMarker && s.reveal_start <= 8 && 8 <= s.reveal_end)
        .map(|s| cut(quote, s.start, s.end))
        .collect();
    assert_eq!(signs, ["> "], "only the sign of the line with the cursor");
    // The bullet of a task item shows on its line.
    assert_eq!(shown("- [ ] хлеб‸\n- [ ] молоко"), ["- "]);
}

#[test]
fn e13_the_address_of_a_link_shows_with_the_cursor_in_the_link() {
    assert_eq!(shown("см. [сайт](https://example.org) ‸и"), [] as [&str; 0]);
    assert_eq!(
        shown("см. [са‸йт](https://example.org) и"),
        ["[", "](https://example.org)"]
    );
    assert_eq!(shown("см. [заметку](note:n‸1) и"), ["[", "](note:n1)"]);
}

#[test]
fn e14_an_attachment_is_its_source_while_the_cursor_is_on_its_line() {
    let text = "до\n![фото.jpg](attachment:a1) и текст\nпосле";
    let layout = markdown_layout(text.into());
    let span = layout
        .spans
        .iter()
        .find(|s| matches!(s.kind, AttachmentLink { .. }))
        .unwrap();
    assert_eq!(cut(text, span.start, span.end), "![фото.jpg](attachment:a1)");
    assert_eq!(
        cut(text, span.reveal_start, span.reveal_end),
        "![фото.jpg](attachment:a1) и текст"
    );
}

#[test]
fn e15_the_markup_of_a_cell_shows_with_the_rest_of_the_table() {
    assert_eq!(shown("| **a** | b |\n|---|---|\n| 1 | 2‸ |"), ["**", "**"]);
    assert_eq!(shown("| **a** | b |\n|---|---|\n| 1 | 2 |\n\nпосле‸"), [] as [&str; 0]);
    let text = "| a |\n|---|\n| b |";
    for span in markdown_layout(text.into()).spans {
        assert_eq!(
            (span.reveal_start, span.reveal_end),
            (0, text.encode_utf16().count() as u32)
        );
    }
}

#[test]
fn e16_fences_show_while_the_cursor_is_in_the_block() {
    assert_eq!(shown("```rust\nlet‸ a;\n```\n\nпосле"), ["```rust", "```"]);
    assert_eq!(shown("```rust\nlet a;\n```\n\nпос‸ле"), [] as [&str; 0]);
}

#[test]
fn e30_an_attachment_is_inline_when_written_as_an_image() {
    let text = "![фото.jpg](attachment:a1)\n\n[отчёт.docx](attachment:a2)";
    assert_eq!(
        of(
            text,
            AttachmentLink {
                id: "a1".into(),
                inline: true
            }
        ),
        ["![фото.jpg](attachment:a1)"]
    );
    assert_eq!(
        of(
            text,
            AttachmentLink {
                id: "a2".into(),
                inline: false
            }
        ),
        ["[отчёт.docx](attachment:a2)"]
    );
    // The link is replaced whole: no markup of its own to hide.
    assert_eq!(of(text, Markup), [] as [&str; 0]);
}

#[test]
fn e31_a_link_to_a_note_shows_its_text() {
    let text = "см. [План на год](note:0192-ab)";
    assert_eq!(of(text, NoteLink { id: "0192-ab".into() }), ["План на год"]);
    assert_eq!(of(text, Markup), ["[", "](note:0192-ab)"]);
}

#[test]
fn e51_spelling_is_not_checked_in_code_addresses_and_attachments() {
    let text =
        "слово `кот` и [ссылка](https://example.org/путь) и https://a.example\n\n![ф](attachment:a1)\n\n```\nкод\n```";
    assert_eq!(
        quiet(text),
        [
            "кот",
            "[",
            "](https://example.org/путь)",
            "https://a.example",
            "![ф](attachment:a1)",
            "```\nкод\n```"
        ]
    );
    // The text of a link is words and is checked; an autolink is all address.
    assert_eq!(quiet("<https://a.example>"), ["<https://a.example>"]);
    assert_eq!(quiet("просто текст"), [] as [&str; 0]);
}
