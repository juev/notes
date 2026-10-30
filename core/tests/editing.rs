//! The edits of the editor: E21, E22, E23 and E26 in docs/specs/editor.md.

use notes_core::*;

/// The note with the selection marked as `[` … `]` or the cursor as `|`: the text and the
/// selection in UTF-16 units.
fn marked(note: &str) -> (String, u32, u32) {
    let units = |s: &str| s.encode_utf16().count() as u32;
    if let Some(at) = note.find('|') {
        let text = note.replacen('|', "", 1);
        let cursor = units(&note[..at]);
        return (text, cursor, cursor);
    }
    let open = note.find('⟦').expect("a cursor or a selection");
    let close = note.find('⟧').unwrap();
    let text = note.replacen('⟦', "", 1).replacen('⟧', "", 1);
    let start = units(&note[..open]);
    let end = units(&note[..close]) - 1;
    (text, start, end)
}

/// The note after the edit, with the new selection or cursor marked the same way.
fn after(text: &str, edit: MarkdownEdit) -> String {
    let mut units: Vec<u16> = text.encode_utf16().collect();
    units.splice(edit.start as usize..edit.end as usize, edit.text.encode_utf16());
    if edit.anchor == edit.cursor {
        units.insert(edit.cursor as usize, '|' as u16);
    } else {
        let (a, b) = (edit.anchor.min(edit.cursor), edit.anchor.max(edit.cursor));
        units.insert(b as usize, '⟧' as u16);
        units.insert(a as usize, '⟦' as u16);
    }
    String::from_utf16(&units).unwrap()
}

fn format(note: &str, format: MarkdownFormat) -> Option<String> {
    let (text, start, end) = marked(note);
    markdown_format(text.clone(), start, end, format).map(|e| after(&text, e))
}

fn indent(note: &str, outdent: bool) -> Option<String> {
    let (text, start, end) = marked(note);
    markdown_indent(text.clone(), start, end, outdent).map(|e| after(&text, e))
}

fn paste(note: &str, plain: Option<&str>, html: Option<&str>) -> Option<String> {
    let (text, start, end) = marked(note);
    markdown_paste(
        text.clone(),
        start,
        end,
        plain.map(String::from),
        html.map(String::from),
    )
    .map(|e| after(&text, e))
}

fn typed(note: &str) -> Option<String> {
    let (text, cursor, _) = marked(note);
    markdown_typed(text.clone(), cursor).map(|e| after(&text, e))
}

use MarkdownFormat::*;

#[test]
fn e21_a_format_goes_on_the_word_at_the_cursor_and_comes_off_again() {
    // The scenario of the specification.
    assert_eq!(format("слово| дальше", Bold).as_deref(), Some("**слово|** дальше"));
    assert_eq!(format("**сло|во** дальше", Bold).as_deref(), Some("сло|во дальше"));
    assert_eq!(format("a ⟦два слова⟧ b", Italic).as_deref(), Some("a *⟦два слова⟧* b"));
    assert_eq!(format("a *⟦два слова⟧* b", Italic).as_deref(), Some("a ⟦два слова⟧ b"));
    assert_eq!(format("x ⟦код⟧", Code).as_deref(), Some("x `⟦код⟧`"));
    assert_eq!(format("x ⟦это⟧", Strikethrough).as_deref(), Some("x ~~⟦это⟧~~"));
    assert_eq!(format("x ⟦это⟧", Highlight).as_deref(), Some("x ==⟦это⟧=="));
    assert_eq!(format("x ==э|то==", Highlight).as_deref(), Some("x э|то"));
    // The spaces at the ends of a selection stay outside the markers.
    assert_eq!(format("a⟦ b ⟧c", Bold).as_deref(), Some("a **⟦b⟧** c"));
    // With no word at the cursor the markers go in with the cursor between them.
    assert_eq!(format("a | b", Bold).as_deref(), Some("a **|** b"));
    // Bold comes off a word inside a longer bold span as the whole span.
    assert_eq!(format("**один д|ва**", Bold).as_deref(), Some("один д|ва"));
    // Italic inside bold is a format of its own.
    assert_eq!(format("**жир|ный**", Italic).as_deref(), Some("***жир|ный***"));
}

#[test]
fn e21_a_link_wraps_the_selection_and_comes_off_as_its_text() {
    assert_eq!(format("см. ⟦сайт⟧", Link).as_deref(), Some("см. [сайт](|)"));
    assert_eq!(format("a | b", Link).as_deref(), Some("a [|]() b"));
    assert_eq!(
        format("см. [са|йт](https://example.org)", Link).as_deref(),
        Some("см. са|йт")
    );
}

#[test]
fn e21_a_line_format_replaces_the_one_the_line_has() {
    assert_eq!(
        format("Заго|ловок", Heading { level: 1 }).as_deref(),
        Some("# Заго|ловок")
    );
    assert_eq!(
        format("# Заго|ловок", Heading { level: 2 }).as_deref(),
        Some("## Заго|ловок")
    );
    assert_eq!(
        format("## Заго|ловок", Heading { level: 2 }).as_deref(),
        Some("Заго|ловок")
    );
    assert_eq!(format("## Заго|ловок", Paragraph).as_deref(), Some("Заго|ловок"));
    assert_eq!(format("- пу|нкт", NumberedList).as_deref(), Some("1. пу|нкт"));
    assert_eq!(format("1. пу|нкт", TaskList).as_deref(), Some("- [ ] пу|нкт"));
    assert_eq!(format("- [x] пу|нкт", TaskList).as_deref(), Some("пу|нкт"));
    assert_eq!(format("> ци|тата", BulletList).as_deref(), Some("- ци|тата"));
    assert_eq!(format("ци|тата", Quote).as_deref(), Some("> ци|тата"));
    // Every line of a selection; blank lines between stay blank, numbers run on.
    assert_eq!(
        format("⟦раз\n\nдва\nтри⟧", NumberedList).as_deref(),
        Some("⟦1. раз\n\n2. два\n3. три⟧")
    );
    assert_eq!(format("⟦- раз\n- два⟧", BulletList).as_deref(), Some("⟦раз\nдва⟧"));
    // Indentation stays.
    assert_eq!(format("- раз\n  дв|а", BulletList).as_deref(), Some("- раз\n  - дв|а"));
}

#[test]
fn e22_tab_moves_an_item_under_the_one_before_it() {
    assert_eq!(indent("- раз\n- дв|а", false).as_deref(), Some("- раз\n  - дв|а"));
    assert_eq!(indent("1. раз\n2. дв|а", false).as_deref(), Some("1. раз\n   2. дв|а"));
    assert_eq!(
        indent("- раз\n  - два\n  - тр|и", false).as_deref(),
        Some("- раз\n  - два\n    - тр|и")
    );
    // The first item of a level has nothing to go under.
    assert_eq!(indent("- раз|\n- два", false), None);
    assert_eq!(indent("- раз\n  - дв|а", false), None);
    // Outside a list Tab is a tab.
    assert_eq!(indent("прос|то", false), None);
}

#[test]
fn e22_shift_tab_moves_an_item_back_to_its_parent() {
    assert_eq!(indent("- раз\n  - дв|а", true).as_deref(), Some("- раз\n- дв|а"));
    assert_eq!(
        indent("- раз\n  - два\n    - тр|и", true).as_deref(),
        Some("- раз\n  - два\n  - тр|и")
    );
    assert_eq!(indent("- р|аз", true), None);
    // Several items at once.
    assert_eq!(
        indent("- раз\n⟦  - два\n  - три⟧", true).as_deref(),
        Some("- раз\n⟦- два\n- три⟧")
    );
}

#[test]
fn e23_a_table_from_a_browser_becomes_a_table_of_the_dialect() {
    // The scenario of the specification.
    let html = "<html><body><!--StartFragment--><table><thead><tr><th>Имя</th><th>Кол-во</th></tr></thead>\
                <tbody><tr><td>яблоки</td><td><b>3</b></td></tr><tr><td>a|b</td></tr></tbody></table><!--EndFragment--></body></html>";
    assert_eq!(
        paste("|", None, Some(html)).as_deref(),
        Some("| Имя | Кол-во |\n| --- | --- |\n| яблоки | **3** |\n| a\\|b |  ||")
    );
    // It reads back as a table with the same cells.
    let layout = markdown_layout("| Имя | Кол-во |\n| --- | --- |\n| яблоки | **3** |".into());
    assert_eq!(layout.tables.len(), 1);
}

#[test]
fn e23_a_url_over_a_selection_makes_a_link() {
    // The scenario of the specification.
    assert_eq!(
        paste("нажми ⟦here⟧", Some("https://example.org/x"), None).as_deref(),
        Some("нажми [here](https://example.org/x)|")
    );
    // Without a selection, or with text that is not an address, the text goes in as it is.
    assert_eq!(paste("нажми |", Some("https://example.org"), None), None);
    assert_eq!(paste("нажми ⟦here⟧", Some("просто текст"), None), None);
    assert_eq!(paste("нажми ⟦here⟧", Some("javascript:alert(1)"), None), None);
}

#[test]
fn e23_rich_text_keeps_the_kinds_of_text_of_the_dialect() {
    let html = r#"<h2>План</h2><p>Это <strong>важно</strong>, <em>срочно</em>, <s>отменено</s>, <mark>отмечено</mark>
        и <code>код</code>.<br>Новая строка с <a href="https://example.org/a b">ссылкой</a>.</p>
        <ul><li>раз</li><li><input type="checkbox" checked> сделано<ul><li>вложенный</li></ul></li></ul>
        <ol start="3"><li>три</li><li>четыре</li></ol>
        <blockquote><p>цитата</p></blockquote><hr>
        <pre><code class="language-rust">fn main() {
    println!("hi");
}</code></pre>
        <script>alert(1)</script><p>* не список и [не ссылка] и a==b</p>"#;
    let markdown = paste("|", None, Some(html)).unwrap().replace('|', "");
    assert_eq!(
        markdown,
        "## План\n\n\
         Это **важно**, *срочно*, ~~отменено~~, ==отмечено== и `код`.\\\n\
         Новая строка с [ссылкой](<https://example.org/a b>).\n\n\
         - раз\n- [x] сделано\n  - вложенный\n\n\
         3. три\n4. четыре\n\n\
         > цитата\n\n---\n\n\
         ```rust\nfn main() {\n    println!(\"hi\");\n}\n```\n\n\
         \\* не список и \\[не ссылка\\] и a\\=\\=b"
    );
    let layout = markdown_layout(markdown.clone());
    let kinds: Vec<MarkdownKind> = layout.spans.iter().map(|s| s.kind.clone()).collect();
    for kind in [
        MarkdownKind::Heading { level: 2 },
        MarkdownKind::Strong,
        MarkdownKind::Emphasis,
        MarkdownKind::Strikethrough,
        MarkdownKind::Highlight,
        MarkdownKind::Code,
        MarkdownKind::Checkbox { checked: true },
        MarkdownKind::Quote,
        MarkdownKind::Rule,
        MarkdownKind::CodeBlock,
    ] {
        assert!(kinds.contains(&kind), "{kind:?} in {markdown}");
    }
    // The escaped `*` starts no list: the bullets are those of "раз" and "вложенный".
    let bullets = kinds
        .iter()
        .filter(|k| **k == MarkdownKind::ListMarker { ordered: false })
        .count();
    assert_eq!(bullets, 2, "{markdown}");
}

#[test]
fn e23_google_docs_wraps_everything_in_a_bold_that_is_not() {
    let html = r#"<meta charset="utf-8"><b style="font-weight:normal;" id="docs-internal-guid-1"><p dir="ltr"><span style="font-weight:700;">жирный</span><span style="font-weight:400;"> и обычный</span><span style="font-style:italic;"> курсив</span></p></b>"#;
    assert_eq!(
        paste("|", None, Some(html)).as_deref(),
        Some("**жирный** и обычный *курсив*|")
    );
}

#[test]
fn e23_blocks_pasted_into_a_line_go_on_lines_of_their_own() {
    let html = "<ul><li>a</li><li>b</li></ul>";
    assert_eq!(
        paste("до | после", None, Some(html)).as_deref(),
        Some("до \n- a\n- b|\n после")
    );
    assert_eq!(
        paste("до <b>x</b>|", None, Some("<b>жирно</b>")).as_deref(),
        Some("до <b>x</b>**жирно**|")
    );
}

#[test]
fn e26_quotes_follow_the_language_of_the_letter_before() {
    // The scenarios of the specification.
    assert_eq!(typed("\"слово\"|").as_deref(), Some("«слово»|"));
    assert_eq!(typed("\"word\"|").as_deref(), Some("“word”|"));
    // An opening quote after a letter takes its language at once.
    assert_eq!(typed("сказал \"|").as_deref(), Some("сказал «|"));
    assert_eq!(typed("said \"|").as_deref(), Some("said “|"));
    assert_eq!(typed("сказал «слово\"|").as_deref(), Some("сказал «слово»|"));
    // A quote at the start with no letter before it waits for the closing one.
    assert_eq!(typed("\"|"), None);
}

#[test]
fn e26_dashes_ellipses_and_arrows() {
    // The scenarios of the specification.
    assert_eq!(typed("a - |").as_deref(), Some("a\u{a0}— |"));
    assert_eq!(typed("ну...|").as_deref(), Some("ну…|"));
    assert_eq!(typed("a ->|").as_deref(), Some("a →|"));
    // A list marker is not a dash, four dots are not an ellipsis, `-->` is not an arrow.
    assert_eq!(typed("- |"), None);
    assert_eq!(typed("  - |"), None);
    assert_eq!(typed("ну....|"), None);
    assert_eq!(typed("<!--|"), None);
    assert_eq!(typed("a -->|"), None);
}

#[test]
fn e26_nothing_changes_in_code_and_in_addresses() {
    // The scenario of the specification.
    assert_eq!(typed("`a \"b\"|`"), None);
    assert_eq!(typed("```\nx = \"y\"|\n```"), None);
    assert_eq!(typed("```\na -> b ...|\n```"), None);
    assert_eq!(typed("[x](https://example.org/a...|)"), None);
    assert_eq!(typed("https://example.org/a->|"), None);
}
