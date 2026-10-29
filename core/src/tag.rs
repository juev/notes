//! Tags travel as registers of their own: the field is `tag:<name>`, the value
//! `true` or `false` (S5 in docs/specs/sync.md).
//!
//! The name shares the 200-byte limit of a register's key with the prefix, so a
//! tag the app accepts is one every device and a restored backup accept. An
//! over-long name is refused where it is typed, not dropped later by a peer.

use crate::db::key_ok;
use crate::error::{AppError, Result};

pub const PREFIX: &str = "tag:";

/// The longest name whose field still fits the key of a register.
pub const MAX_NAME_LEN: usize = crate::db::MAX_KEY_LEN - PREFIX.len();

/// The field of a tag, or nothing when it could not travel: no device would
/// accept it. The key limit is the only rule here: a name that is already
/// stored travels even if another version accepted what this one would not.
pub fn field(name: &str) -> Option<String> {
    if name.is_empty() {
        return None;
    }
    let field = format!("{PREFIX}{name}");
    key_ok(&field).then_some(field)
}

/// Whether a name read from a register can be shown: the same rule as for writing one.
pub fn name_ok(name: &str) -> bool {
    field(name).is_some()
}

/// What tells two tags apart: names that differ in case are one tag (R21).
pub fn key(name: &str) -> String {
    name.to_lowercase()
}

/// The field for a name that is copied or read, or the error when no register can carry it.
pub fn require_field(name: &str) -> Result<String> {
    field(name).ok_or_else(|| {
        if name.is_empty() {
            AppError::invalid("a tag is empty")
        } else {
            AppError::invalid(format!("a tag is longer than {MAX_NAME_LEN} bytes"))
        }
    })
}

/// A name the user just typed (R21): one or more words, without `,` and `#`, that fits a register.
/// Leading and trailing spaces go, and so do doubled ones inside.
pub fn clean(typed: &str) -> Result<String> {
    if typed.contains([',', '#']) {
        return Err(AppError::invalid("a tag has no commas and no #"));
    }
    let name = typed.split_whitespace().collect::<Vec<_>>().join(" ");
    require_field(&name)?;
    Ok(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn s5_the_prefix_counts_towards_the_key() {
        let latin = "a".repeat(MAX_NAME_LEN);
        let cyrillic = "я".repeat(MAX_NAME_LEN / 2);
        assert_eq!(field(&latin), Some(format!("tag:{latin}")));
        assert_eq!(field(&cyrillic), Some(format!("tag:{cyrillic}")));
        assert_eq!(field(&"a".repeat(MAX_NAME_LEN + 1)), None);
        assert_eq!(field(&"я".repeat(MAX_NAME_LEN / 2 + 1)), None);
        assert_eq!(field(""), None);
        assert_eq!(field("two words"), Some("tag:two words".into()));
    }

    #[test]
    fn r21_a_typed_name_is_words_without_comma_and_hash() {
        assert_eq!(clean("  Work   plans ").unwrap(), "Work plans");
        assert_eq!(clean("work").unwrap(), "work");
        assert!(clean("").is_err());
        assert!(clean("   ").is_err());
        assert!(clean("a,b").is_err());
        assert!(clean("#work").is_err());
        assert!(clean("two #words").is_err());
        assert!(clean(&"a".repeat(MAX_NAME_LEN + 1)).is_err());
        assert!(clean(&"a".repeat(MAX_NAME_LEN)).is_ok());
    }

    #[test]
    fn r21_names_that_differ_in_case_are_one_tag() {
        assert_eq!(key("Work"), key("work"));
        assert_eq!(key("Работа"), key("работа"));
        assert_ne!(key("work"), key("works"));
    }
}
