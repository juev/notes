//! Highlighting of fenced code blocks (E3 in docs/specs/editor.md): syntect with its pure-Rust
//! regex engine and the syntaxes of bat as `two-face` builds them in. The core says what a token
//! is; the colour of each class is the platform's.

use std::ops::Range;
use std::sync::OnceLock;

use syntect::easy::ScopeRangeIterator;
use syntect::parsing::{ParseState, Scope, ScopeStack, SyntaxSet};
use syntect::util::LinesWithEndings;

/// What a token of code is, for its colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum CodeClass {
    Keyword,
    Operator,
    String,
    Number,
    /// Other literals: `true`, `null`, escapes, named constants.
    Constant,
    Comment,
    Type,
    Function,
    /// A tag of HTML or XML.
    Tag,
    /// An attribute of a tag.
    Attribute,
    /// A property of CSS, a key of YAML or TOML, a member.
    Property,
    /// A line added in a diff.
    Inserted,
    /// A line removed in a diff.
    Deleted,
    /// A heading of Markdown, or the header of a hunk in a diff.
    Heading,
    /// Other markup of a document language: emphasis, lists, links.
    Markup,
}

/// The scopes of TextMate grammars a class is read from. Within one scope the longer prefix is
/// listed first.
const CLASSES: &[(&str, CodeClass)] = &[
    ("comment", CodeClass::Comment),
    ("string", CodeClass::String),
    ("constant.numeric", CodeClass::Number),
    ("constant", CodeClass::Constant),
    ("keyword.operator", CodeClass::Operator),
    ("keyword", CodeClass::Keyword),
    ("storage", CodeClass::Keyword),
    ("entity.name.function", CodeClass::Function),
    ("support.function", CodeClass::Function),
    ("variable.function", CodeClass::Function),
    // YAML and TOML call a key a tag.
    ("entity.name.tag.yaml", CodeClass::Property),
    ("entity.name.tag.toml", CodeClass::Property),
    ("entity.name.tag", CodeClass::Tag),
    ("entity.other.attribute-name", CodeClass::Attribute),
    ("entity.name.section", CodeClass::Heading),
    ("entity.name", CodeClass::Type),
    ("support.type.property-name", CodeClass::Property),
    ("support.type", CodeClass::Type),
    ("support.class", CodeClass::Type),
    ("support.constant", CodeClass::Constant),
    ("variable.other.member", CodeClass::Property),
    ("meta.mapping.key", CodeClass::Property),
    ("markup.inserted", CodeClass::Inserted),
    ("markup.deleted", CodeClass::Deleted),
    ("markup.heading", CodeClass::Heading),
    ("meta.diff.range", CodeClass::Heading),
    ("markup", CodeClass::Markup),
];

fn syntaxes() -> &'static SyntaxSet {
    static SET: OnceLock<SyntaxSet> = OnceLock::new();
    SET.get_or_init(two_face::syntax::extra_newlines)
}

fn classes() -> &'static [(Scope, CodeClass)] {
    static TABLE: OnceLock<Vec<(Scope, CodeClass)>> = OnceLock::new();
    TABLE.get_or_init(|| {
        CLASSES
            .iter()
            .filter_map(|(name, class)| Some((Scope::new(name).ok()?, *class)))
            .collect()
    })
}

/// The class of the innermost scope that has one.
fn class_of(stack: &ScopeStack) -> Option<CodeClass> {
    stack.as_slice().iter().rev().find_map(|scope| {
        classes()
            .iter()
            .find(|(prefix, _)| prefix.is_prefix_of(*scope))
            .map(|(_, class)| *class)
    })
}

/// The tokens of `code` written in `language`, the word after the fence, as byte ranges of `code`
/// with their class, ordered and not overlapping. Nothing for `text`, `plain` and a language the
/// syntaxes do not know.
pub(crate) fn tokens(language: &str, code: &str) -> Vec<(Range<usize>, CodeClass)> {
    let token = language.to_lowercase();
    let token = match token.as_str() {
        "" | "text" | "plain" | "plaintext" | "txt" => return Vec::new(),
        "shell" => "bash",
        other => other,
    };
    let set = syntaxes();
    let Some(syntax) = set.find_syntax_by_token(token) else {
        return Vec::new();
    };
    let mut state = ParseState::new(syntax);
    let mut stack = ScopeStack::new();
    let mut out: Vec<(Range<usize>, CodeClass)> = Vec::new();
    let mut at = 0;
    for line in LinesWithEndings::from(code) {
        // A grammar that fails leaves the rest of the block plain.
        let Ok(ops) = state.parse_line(line, set) else {
            break;
        };
        for (range, op) in ScopeRangeIterator::new(&ops, line) {
            if stack.apply(op).is_err() {
                return out;
            }
            let Some(class) = class_of(&stack).filter(|_| !range.is_empty()) else {
                continue;
            };
            let range = at + range.start..at + range.end;
            match out.last_mut() {
                Some((last, same)) if *same == class && last.end == range.start => last.end = range.end,
                _ => out.push((range, class)),
            }
        }
        at += line.len();
    }
    // The line break that ends a comment or a line of a diff is not a token of its own.
    for (range, _) in &mut out {
        while range.end > range.start && matches!(code.as_bytes()[range.end - 1], b'\n' | b'\r') {
            range.end -= 1;
        }
    }
    out.retain(|(range, _)| !range.is_empty());
    out
}

/// The notices the licenses of the syntaxes ask to show with the app (E3), as plain text: the path
/// of each license in the assets of bat, then its text.
#[uniffi::export]
pub fn code_highlight_acknowledgements() -> String {
    two_face::acknowledgement::listing()
        .for_syntaxes()
        .iter()
        .map(|license| format!("{}\n\n{}\n", license.rel_path.display(), license.text.trim_end()))
        .collect::<Vec<_>>()
        .join("\n")
}
