//! Highlighting of code blocks: E3 in docs/specs/editor.md.

use notes_core::CodeClass::{
    Attribute, Comment, Constant, Deleted, Function, Heading, Inserted, Keyword, Markup, Number, Property, Tag, Type,
};
use notes_core::*;

fn cut(text: &str, start: u32, end: u32) -> String {
    let units: Vec<u16> = text.encode_utf16().collect();
    String::from_utf16(&units[start as usize..end as usize]).unwrap()
}

/// The tokens of a fenced block with `code` in `language`, as their class and text.
fn tokens(language: &str, code: &str) -> Vec<(CodeClass, String)> {
    let text = format!("```{language}\n{code}\n```");
    markdown_layout(text.clone())
        .spans
        .into_iter()
        .filter_map(|s| match s.kind {
            MarkdownKind::Token { class } => Some((class, cut(&text, s.start, s.end))),
            _ => None,
        })
        .collect()
}

fn has(language: &str, code: &str, expected: &[(CodeClass, &str)]) {
    let got = tokens(language, code);
    for (class, text) in expected {
        assert!(
            got.iter().any(|(c, t)| c == class && t == text),
            "{language}: no {class:?} {text:?} in {got:?}"
        );
    }
}

#[test]
fn e3_every_language_of_the_list_is_highlighted() {
    for shell in ["bash", "sh", "shell", "zsh"] {
        has(
            shell,
            "# c\necho \"hi\"",
            &[(Comment, "# c"), (CodeClass::String, "\"hi\"")],
        );
    }
    has(
        "javascript",
        "// c\nconst a = \"s\";",
        &[(Comment, "// c"), (Keyword, "const"), (CodeClass::String, "\"s\"")],
    );
    has(
        "typescript",
        "// c\nlet a: number = 1;",
        &[(Comment, "// c"), (Type, "number"), (Number, "1")],
    );
    has(
        "json",
        "{\"k\": \"v\", \"n\": 1}",
        &[(CodeClass::String, "\"v\""), (Number, "1")],
    );
    has(
        "yaml",
        "# c\nkey: \"v\"",
        &[(Comment, "# c"), (Property, "key"), (CodeClass::String, "\"v\"")],
    );
    has(
        "toml",
        "# c\nkey = \"v\"",
        &[(Comment, "# c"), (Property, "key"), (CodeClass::String, "\"v\"")],
    );
    has("markdown", "# Title\n**b**", &[(Heading, "# Title"), (Markup, "**b**")]);
    has(
        "python",
        "# c\ndef f(): return \"s\"",
        &[(Comment, "# c"), (Keyword, "def"), (Function, "f")],
    );
    has(
        "go",
        "// c\nfunc main() { s := \"x\" }",
        &[(Comment, "// c"), (Keyword, "func"), (CodeClass::String, "\"x\"")],
    );
    has(
        "rust",
        "// c\nfn main() { let s = \"x\"; }",
        &[(Comment, "// c"), (Keyword, "fn"), (Function, "main")],
    );
    has(
        "swift",
        "// c\nlet s = \"x\"",
        &[(Comment, "// c"), (Keyword, "let"), (CodeClass::String, "\"x\"")],
    );
    has(
        "kotlin",
        "// c\nval s = \"x\"",
        &[(Comment, "// c"), (Keyword, "val"), (CodeClass::String, "\"x\"")],
    );
    has(
        "sql",
        "-- c\nSELECT 'x' FROM t;",
        &[(Comment, "-- c"), (Keyword, "SELECT"), (CodeClass::String, "'x'")],
    );
    has(
        "html",
        "<!-- c -->\n<p class=\"a\">x</p>",
        &[(Comment, "<!-- c -->"), (Tag, "p"), (Attribute, "class")],
    );
    has(
        "css",
        "/* c */\na { color: red; }",
        &[(Comment, "/* c */"), (Property, "color"), (Constant, "red")],
    );
    has(
        "diff",
        "@@ -1 +1 @@\n-old\n+new",
        &[(Heading, "@@ -1 +1 @@"), (Deleted, "-old"), (Inserted, "+new")],
    );
    has("dockerfile", "# c\nFROM alpine", &[(Comment, "# c"), (Keyword, "FROM")]);
    // The language after the fence is read whatever its case, and the info after it is ignored.
    has("Rust title=x", "fn main() {}", &[(Keyword, "fn")]);
}

#[test]
fn e3_plain_text_and_an_unknown_language_are_not_highlighted() {
    for language in ["", "text", "plain", "no-such-language"] {
        assert_eq!(tokens(language, "fn main() { \"x\" }"), [], "{language:?}");
    }
    // Neither is an indented block, which has no language.
    let layout = markdown_layout("абзац\n\n    fn main() {}\n".into());
    assert!(!layout
        .spans
        .iter()
        .any(|s| matches!(s.kind, MarkdownKind::Token { .. })));
}

#[test]
fn e3_tokens_stay_in_the_code_of_a_block_inside_a_quote_and_a_list() {
    let text = "> ```rust\n> let a = 1; // c\n> ```\n\n- пункт\n\n  ```rust\n  fn f() {}\n  ```";
    let layout = markdown_layout(text.into());
    let got: Vec<(CodeClass, String)> = layout
        .spans
        .iter()
        .filter_map(|s| match s.kind {
            MarkdownKind::Token { class } => Some((class, cut(text, s.start, s.end))),
            _ => None,
        })
        .collect();
    assert!(got.contains(&(Keyword, "let".into())), "{got:?}");
    assert!(got.contains(&(Comment, "// c".into())), "{got:?}");
    assert!(got.contains(&(Keyword, "fn".into())), "{got:?}");
    // The sign of the quote and the indentation are not code.
    assert!(
        got.iter().all(|(_, t)| !t.contains('>') && !t.starts_with(' ')),
        "{got:?}"
    );
    // Offsets count UTF-16 units across emoji before the block.
    let emoji = "😀\n\n```rust\nfn f() {}\n```";
    let span = markdown_layout(emoji.into())
        .spans
        .into_iter()
        .find(|s| s.kind == MarkdownKind::Token { class: Keyword })
        .unwrap();
    assert_eq!(cut(emoji, span.start, span.end), "fn");
}

#[test]
fn e3_the_notices_of_the_syntaxes_are_given_to_the_app() {
    let text = code_highlight_acknowledgements();
    assert!(
        text.contains("TypeScript"),
        "the syntax of TypeScript is under Apache-2.0"
    );
    assert!(text.contains("Permission is hereby granted"), "the MIT text is there");
}
