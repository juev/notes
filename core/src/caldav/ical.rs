//! A small iCalendar reader and writer.
//!
//! It keeps everything it does not understand: a component is a list of
//! properties and nested components in their original order, so an object
//! written back still carries what another client put there.

#[derive(Debug, Clone, PartialEq)]
pub struct Prop {
    /// Upper-case name.
    pub name: String,
    /// Parameters as written, without the leading `;`.
    pub params: Vec<(String, String)>,
    /// The raw value: escaping is the business of whoever knows the value type.
    pub value: String,
}

impl Prop {
    pub fn new(name: &str, value: impl Into<String>) -> Self {
        Prop {
            name: name.to_string(),
            params: Vec::new(),
            value: value.into(),
        }
    }

    pub fn with(mut self, param: &str, value: &str) -> Self {
        self.params.push((param.to_string(), value.to_string()));
        self
    }

    pub fn param(&self, name: &str) -> Option<&str> {
        self.params
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Component {
    pub name: String,
    pub props: Vec<Prop>,
    pub subs: Vec<Component>,
}

impl Component {
    pub fn new(name: &str) -> Self {
        Component {
            name: name.to_string(),
            ..Component::default()
        }
    }

    pub fn get(&self, name: &str) -> Option<&Prop> {
        self.props.iter().find(|p| p.name == name)
    }

    pub fn all<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Prop> {
        self.props.iter().filter(move |p| p.name == name)
    }

    pub fn remove(&mut self, name: &str) {
        self.props.retain(|p| p.name != name);
    }

    /// Replaces every property of that name with one, or removes them when `value` is `None`.
    pub fn set(&mut self, prop: Option<Prop>, name: &str) {
        self.remove(name);
        if let Some(prop) = prop {
            self.props.push(prop);
        }
    }

    pub fn sub_mut(&mut self, name: &str) -> Option<&mut Component> {
        self.subs.iter_mut().find(|c| c.name == name)
    }

    pub fn sub(&self, name: &str) -> Option<&Component> {
        self.subs.iter().find(|c| c.name == name)
    }
}

/// Parses the first component in the text (normally `VCALENDAR`).
pub fn parse(text: &str) -> Option<Component> {
    // Unfold: a line starting with a space or a tab continues the previous one.
    let mut lines: Vec<String> = Vec::new();
    for raw in text.split('\n') {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        match line.strip_prefix(' ').or_else(|| line.strip_prefix('\t')) {
            Some(rest) if !lines.is_empty() => lines.last_mut()?.push_str(rest),
            _ if line.is_empty() => {}
            _ => lines.push(line.to_string()),
        }
    }
    let mut stack: Vec<Component> = Vec::new();
    for line in &lines {
        let prop = parse_line(line)?;
        match prop.name.as_str() {
            "BEGIN" => stack.push(Component::new(&prop.value.to_ascii_uppercase())),
            "END" => {
                let done = stack.pop()?;
                match stack.last_mut() {
                    Some(parent) => parent.subs.push(done),
                    None => return Some(done),
                }
            }
            _ => stack.last_mut()?.props.push(prop),
        }
    }
    None
}

fn parse_line(line: &str) -> Option<Prop> {
    // The name and parameters end at the first colon outside double quotes.
    let mut quoted = false;
    let mut colon = None;
    for (i, c) in line.char_indices() {
        match c {
            '"' => quoted = !quoted,
            ':' if !quoted => {
                colon = Some(i);
                break;
            }
            _ => {}
        }
    }
    let colon = colon?;
    let (head, value) = (&line[..colon], &line[colon + 1..]);
    let mut parts = split_outside_quotes(head, ';').into_iter();
    let name = parts.next()?.to_ascii_uppercase();
    let params = parts
        .filter_map(|p| {
            let (n, v) = p.split_once('=')?;
            Some((n.to_ascii_uppercase(), v.trim_matches('"').to_string()))
        })
        .collect();
    Some(Prop {
        name,
        params,
        value: value.to_string(),
    })
}

fn split_outside_quotes(s: &str, sep: char) -> Vec<&str> {
    let mut out = Vec::new();
    let (mut quoted, mut start) = (false, 0);
    for (i, c) in s.char_indices() {
        if c == '"' {
            quoted = !quoted;
        } else if c == sep && !quoted {
            out.push(&s[start..i]);
            start = i + 1;
        }
    }
    out.push(&s[start..]);
    out
}

pub fn serialize(component: &Component) -> String {
    let mut out = String::new();
    write_component(component, &mut out);
    out
}

fn write_component(c: &Component, out: &mut String) {
    fold(&format!("BEGIN:{}", c.name), out);
    for p in &c.props {
        let mut line = p.name.clone();
        for (name, value) in &p.params {
            let needs_quotes = value.contains([':', ';', ',']);
            line.push_str(&if needs_quotes {
                format!(";{name}=\"{value}\"")
            } else {
                format!(";{name}={value}")
            });
        }
        line.push(':');
        line.push_str(&p.value);
        fold(&line, out);
    }
    for sub in &c.subs {
        write_component(sub, out);
    }
    fold(&format!("END:{}", c.name), out);
}

/// Lines are at most 75 octets; longer ones continue on the next line after a space.
fn fold(line: &str, out: &mut String) {
    let mut width = 0;
    for c in line.chars() {
        let len = c.len_utf8();
        if width + len > 75 {
            out.push_str("\r\n ");
            width = 1;
        }
        out.push(c);
        width += len;
    }
    out.push_str("\r\n");
}

/// Escapes a TEXT value.
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            ';' => out.push_str("\\;"),
            ',' => out.push_str("\\,"),
            '\n' => out.push_str("\\n"),
            '\r' => {}
            c => out.push(c),
        }
    }
    out
}

pub fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') | Some('N') => out.push('\n'),
            Some(other) => out.push(other),
            None => {}
        }
    }
    out
}

/// Splits a TEXT list (`CATEGORIES`) on unescaped commas.
pub fn split_list(value: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                current.push('\\');
                if let Some(next) = chars.next() {
                    current.push(next);
                }
            }
            ',' => out.push(unescape(&std::mem::take(&mut current))),
            c => current.push(c),
        }
    }
    out.push(unescape(&current));
    out.into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Other//Client//EN\r\nBEGIN:VTODO\r\nUID:abc\r\nSUMMARY:Купить молоко\\, хлеб\r\nDESCRIPTION:line one\\nline two that is long enough to be folded by a client t\r\n hat folds at seventy-five octets\r\nDUE;VALUE=DATE:20261005\r\nX-OTHER;X-P=\"a:b\":keep me\r\nBEGIN:VALARM\r\nACTION:DISPLAY\r\nTRIGGER:-PT15M\r\nEND:VALARM\r\nEND:VTODO\r\nEND:VCALENDAR\r\n";

    #[test]
    fn parses_unfolds_and_keeps_unknown_parts() {
        let cal = parse(SAMPLE).unwrap();
        assert_eq!(cal.name, "VCALENDAR");
        let todo = cal.sub("VTODO").unwrap();
        assert_eq!(unescape(&todo.get("SUMMARY").unwrap().value), "Купить молоко, хлеб");
        assert!(
            unescape(&todo.get("DESCRIPTION").unwrap().value).ends_with("a client that folds at seventy-five octets")
        );
        assert_eq!(todo.get("DUE").unwrap().param("value"), Some("DATE"));
        assert_eq!(todo.get("X-OTHER").unwrap().param("X-P"), Some("a:b"));
        assert_eq!(todo.sub("VALARM").unwrap().get("TRIGGER").unwrap().value, "-PT15M");
    }

    #[test]
    fn round_trip_is_lossless_and_folded() {
        let cal = parse(SAMPLE).unwrap();
        let text = serialize(&cal);
        assert!(text.lines().all(|l| l.len() <= 75), "every line fits 75 octets");
        assert_eq!(parse(&text).unwrap(), cal);
    }

    #[test]
    fn folding_never_splits_a_character() {
        let mut todo = Component::new("VTODO");
        todo.props.push(Prop::new("SUMMARY", "я".repeat(200)));
        let text = serialize(&todo);
        assert!(text.split("\r\n").all(|l| l.len() <= 75));
        assert_eq!(parse(&text).unwrap().get("SUMMARY").unwrap().value, "я".repeat(200));
    }

    #[test]
    fn text_escaping() {
        let raw = "a;b,c\\d\ne";
        assert_eq!(escape(raw), "a\\;b\\,c\\\\d\\ne");
        assert_eq!(unescape(&escape(raw)), raw);
        assert_eq!(split_list("дом,работа\\, офис, x"), vec!["дом", "работа, офис", "x"]);
    }

    #[test]
    fn garbage_and_truncated_input_are_rejected() {
        assert!(parse("not a calendar").is_none());
        assert!(parse("BEGIN:VCALENDAR\r\nBEGIN:VTODO\r\nUID:x\r\n").is_none());
        assert!(parse("").is_none());
    }
}
