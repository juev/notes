//! Tags travel as registers of their own: the field is `tag:<name>`, the value
//! `true` or `false` (S51 in docs/specs/sync.md).
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
/// stored travels even if it was typed with a space long ago.
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

/// The field for a name that is copied or read, or the error when no register can carry it.
pub fn require_field(name: &str) -> Result<String> {
    field(name).ok_or_else(|| {
        if name.is_empty() {
            AppError::invalid("a tag is one word")
        } else {
            AppError::invalid(format!("a tag is longer than {MAX_NAME_LEN} bytes"))
        }
    })
}

/// The field for a name the user just typed: one word that fits a register.
pub fn require(name: &str) -> Result<String> {
    if name.chars().any(char::is_whitespace) {
        return Err(AppError::invalid("a tag is one word"));
    }
    require_field(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn s51_the_prefix_counts_towards_the_key() {
        let latin = "a".repeat(MAX_NAME_LEN);
        let cyrillic = "я".repeat(MAX_NAME_LEN / 2);
        assert_eq!(field(&latin), Some(format!("tag:{latin}")));
        assert_eq!(field(&cyrillic), Some(format!("tag:{cyrillic}")));
        assert_eq!(field(&"a".repeat(MAX_NAME_LEN + 1)), None);
        assert_eq!(field(&"я".repeat(MAX_NAME_LEN / 2 + 1)), None);
        assert_eq!(field(""), None);
        assert_eq!(field("two words"), Some("tag:two words".into()));
        assert!(require("two words").is_err());
    }
}
