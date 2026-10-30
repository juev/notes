//! I10: the predicate of a smart view of Standard Notes as the conditions of a saved filter
//! (R45 in docs/specs/product.md), when every part of it has an equivalent.

use serde_json::Value;

use super::backup::is_true;
use crate::model::{Condition, FilterJoin, FilterQuery};

/// The query of a filter for `predicate`, or nothing when a keypath, an operator or the nesting
/// has no equivalent. `folder` gives the folder made from the tag of a title (I8).
pub(crate) fn to_query(predicate: &Value, folder: &dyn Fn(&str) -> Option<String>) -> Option<FilterQuery> {
    let operator = predicate.get("operator").and_then(Value::as_str)?;
    match operator {
        "and" | "or" => {
            let join = if operator == "and" {
                FilterJoin::All
            } else {
                FilterJoin::Any
            };
            let mut conditions = Vec::new();
            for part in predicate.get("value").and_then(Value::as_array)? {
                // A part joined the same way adds its conditions; another join cannot be said.
                if let Some(inner) = part
                    .get("operator")
                    .and_then(Value::as_str)
                    .filter(|o| matches!(*o, "and" | "or"))
                {
                    if inner != operator {
                        return None;
                    }
                    conditions.extend(to_query(part, folder)?.conditions);
                } else {
                    conditions.push(condition(part, folder)?);
                }
            }
            Some(FilterQuery { join, conditions })
        }
        _ => Some(FilterQuery {
            join: FilterJoin::All,
            conditions: vec![condition(predicate, folder)?],
        }),
    }
}

fn condition(predicate: &Value, folder: &dyn Fn(&str) -> Option<String>) -> Option<Condition> {
    let keypath = predicate.get("keypath").and_then(Value::as_str)?;
    let operator = predicate.get("operator").and_then(Value::as_str)?;
    let value = predicate.get("value");
    let text = || value.and_then(Value::as_str).map(String::from);
    match (keypath, operator) {
        ("tags", "includes") => Some(Condition::InFolder {
            folder_id: folder(&text()?)?,
            subfolders: false,
        }),
        ("title", "=") => Some(Condition::TitleIs { text: text()? }),
        ("title", "includes") => Some(Condition::TitleContains { text: text()? }),
        ("text", "includes") => Some(Condition::TextContains { text: text()? }),
        ("pinned", "=") if is_true(value) => Some(Condition::Pinned),
        ("starred", "=") if is_true(value) => Some(Condition::Starred),
        ("archived", "=") if is_true(value) => Some(Condition::Archived),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn query(predicate: Value) -> Option<FilterQuery> {
        let folder = |title: &str| (title == "work").then(|| "folder-work".to_string());
        to_query(&predicate, &folder)
    }

    #[test]
    fn i10_a_predicate_with_equivalents_becomes_a_filter() {
        // The scenario of the specification.
        let predicate = json!({"operator": "and", "value": [
            {"keypath": "tags", "operator": "includes", "value": "work"},
            {"keypath": "title", "operator": "=", "value": "x"}
        ]});
        assert_eq!(
            query(predicate),
            Some(FilterQuery {
                join: FilterJoin::All,
                conditions: vec![
                    Condition::InFolder {
                        folder_id: "folder-work".into(),
                        subfolders: false
                    },
                    Condition::TitleIs { text: "x".into() },
                ],
            })
        );
        let predicate = json!({"operator": "or", "value": [
            {"keypath": "title", "operator": "includes", "value": "a"},
            {"keypath": "text", "operator": "includes", "value": "b"},
            {"operator": "or", "value": [{"keypath": "pinned", "operator": "=", "value": true}]},
            {"keypath": "starred", "operator": "=", "value": "true"},
            {"keypath": "archived", "operator": "=", "value": true}
        ]});
        assert_eq!(
            query(predicate),
            Some(FilterQuery {
                join: FilterJoin::Any,
                conditions: vec![
                    Condition::TitleContains { text: "a".into() },
                    Condition::TextContains { text: "b".into() },
                    Condition::Pinned,
                    Condition::Starred,
                    Condition::Archived,
                ],
            })
        );
        assert_eq!(
            query(json!({"keypath": "pinned", "operator": "=", "value": true})),
            Some(FilterQuery {
                join: FilterJoin::All,
                conditions: vec![Condition::Pinned]
            })
        );
    }

    #[test]
    fn i10_a_predicate_without_an_equivalent_is_not_imported() {
        // The scenario of the specification.
        assert_eq!(
            query(json!({"keypath": "title", "operator": "matches", "value": "^a"})),
            None
        );
        // An unknown tag, a false flag, another keypath, and a join inside the other join.
        assert_eq!(
            query(json!({"keypath": "tags", "operator": "includes", "value": "other"})),
            None
        );
        assert_eq!(
            query(json!({"keypath": "pinned", "operator": "=", "value": false})),
            None
        );
        assert_eq!(
            query(json!({"keypath": "updated_at", "operator": ">", "value": "1.days.ago"})),
            None
        );
        let mixed = json!({"operator": "and", "value": [
            {"keypath": "title", "operator": "=", "value": "x"},
            {"operator": "or", "value": [{"keypath": "pinned", "operator": "=", "value": true}]}
        ]});
        assert_eq!(query(mixed), None);
    }
}
