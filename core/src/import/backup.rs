//! I1, I11: the decrypted backup of Standard Notes as a zip, the folder it unpacks to, or the top
//! `.txt` file alone; its items as they are, with the fields the import reads.

use std::io::Read;
use std::path::Path;

use serde_json::Value;

use crate::error::{AppError, Result};

/// The top file of a backup, in the zip and in the folder.
pub(crate) const TOP_FILE: &str = "Standard Notes Backup and Import File.txt";

/// One item of the backup.
#[derive(Debug, Clone)]
pub(crate) struct Item {
    pub uuid: String,
    pub content_type: String,
    pub created_ms: u64,
    pub updated_ms: u64,
    pub deleted: bool,
    pub content: Value,
}

impl Item {
    pub fn str(&self, key: &str) -> &str {
        self.content.get(key).and_then(Value::as_str).unwrap_or_default()
    }

    /// A flag of the content: `true` or the string `"true"`.
    pub fn flag(&self, key: &str) -> bool {
        is_true(self.content.get(key))
    }

    /// The uuids of the items this one references, of the given content type.
    pub fn references(&self, content_type: &str) -> Vec<String> {
        self.content
            .get("references")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|r| r.get("content_type").and_then(Value::as_str) == Some(content_type))
            .filter_map(|r| r.get("uuid").and_then(Value::as_str).map(String::from))
            .collect()
    }
}

pub(crate) fn is_true(value: Option<&Value>) -> bool {
    match value {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => s == "true",
        _ => false,
    }
}

/// I1: reads the backup at `path`.
pub(crate) fn read(path: &Path) -> Result<Vec<Item>> {
    let top = if path.is_dir() {
        let file = [path.join(TOP_FILE)]
            .into_iter()
            .chain(
                std::fs::read_dir(path)?
                    .filter_map(|e| e.ok())
                    .map(|e| e.path().join(TOP_FILE)),
            )
            .find(|p| p.is_file())
            .ok_or_else(|| AppError::invalid(format!("no \"{TOP_FILE}\" in the folder")))?;
        std::fs::read(file)?
    } else {
        let bytes = std::fs::read(path)?;
        if bytes.starts_with(b"PK") {
            from_zip(&bytes)?
        } else {
            bytes
        }
    };
    parse(&top)
}

fn from_zip(bytes: &[u8]) -> Result<Vec<u8>> {
    let refused = |e: zip::result::ZipError| AppError::invalid(format!("not a backup of Standard Notes: {e}"));
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(refused)?;
    let name = zip
        .file_names()
        .filter_map(|n| n.ok().map(|n| n.to_string()))
        .filter(|n| n.rsplit('/').next() == Some(TOP_FILE))
        .min_by_key(String::len)
        .ok_or_else(|| AppError::invalid(format!("no \"{TOP_FILE}\" in the zip")))?;
    let mut out = Vec::new();
    zip.by_name(&name).map_err(refused)?.read_to_end(&mut out)?;
    Ok(out)
}

/// I1: the top file. An encrypted backup and another version are refused with what to do.
pub(crate) fn parse(bytes: &[u8]) -> Result<Vec<Item>> {
    let top: Value =
        serde_json::from_slice(bytes).map_err(|e| AppError::invalid(format!("not a backup of Standard Notes: {e}")))?;
    let items = top
        .get("items")
        .and_then(Value::as_array)
        .ok_or_else(|| AppError::invalid("not a backup of Standard Notes: no items"))?;
    let encrypted =
        top.get("keyParams").is_some() || items.iter().any(|i| i.get("content").is_some_and(Value::is_string));
    if encrypted {
        return Err(AppError::invalid(
            "the backup is encrypted: export a decrypted backup from Standard Notes and import that",
        ));
    }
    let version = top.get("version").and_then(Value::as_str).unwrap_or_default();
    if version != "004" {
        return Err(AppError::invalid(format!(
            "the backup has version \"{version}\"; only version 004 is read"
        )));
    }
    Ok(items.iter().filter_map(item).collect())
}

fn item(value: &Value) -> Option<Item> {
    let text = |key: &str| value.get(key).and_then(Value::as_str);
    let time = |stamp: &str, iso: &str| {
        // The timestamps of Standard Notes count microseconds.
        value
            .get(stamp)
            .and_then(Value::as_u64)
            .map(|us| us / 1000)
            .or_else(|| {
                text(iso)
                    .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
                    .and_then(|t| u64::try_from(t.timestamp_millis()).ok())
            })
            .unwrap_or(0)
    };
    Some(Item {
        uuid: text("uuid")?.to_string(),
        content_type: text("content_type")?.to_string(),
        created_ms: time("created_at_timestamp", "created_at"),
        updated_ms: time("updated_at_timestamp", "updated_at"),
        deleted: is_true(value.get("deleted")),
        content: value.get("content").cloned().unwrap_or(Value::Null),
    })
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    const TOP: &str = r#"{"version":"004","items":[
        {"uuid":"n1","content_type":"Note","created_at":"2025-01-02T03:04:05.000Z","updated_at":"2025-02-03T04:05:06.000Z",
         "created_at_timestamp":1735787045000000,"updated_at_timestamp":1738555506000000,"deleted":false,
         "content":{"title":"a","text":""}},
        {"uuid":"t1","content_type":"Tag","created_at":"2025-01-02T03:04:05.000Z","updated_at":"2025-01-02T03:04:05.000Z",
         "deleted":true,"content":{"title":"t","references":[{"uuid":"n1","content_type":"Note"}]}}
    ]}"#;

    #[test]
    fn i1_a_zip_a_folder_and_the_top_file_alone_read_the_same() {
        let dir = tempfile::tempdir().unwrap();
        let top = dir.path().join(TOP_FILE);
        std::fs::write(&top, TOP).unwrap();
        let folder = dir.path().join("unpacked");
        std::fs::create_dir_all(folder.join("Items")).unwrap();
        std::fs::write(folder.join(TOP_FILE), TOP).unwrap();
        let zip_path = dir.path().join("backup.zip");
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&zip_path).unwrap());
        zip.start_file(
            format!("Standard Notes Backup/{TOP_FILE}"),
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
        zip.write_all(TOP.as_bytes()).unwrap();
        zip.finish().unwrap();

        let summary = |items: Vec<Item>| {
            items
                .into_iter()
                .map(|i| (i.uuid, i.content_type, i.created_ms, i.updated_ms, i.deleted))
                .collect::<Vec<_>>()
        };
        let expected = vec![
            (
                "n1".to_string(),
                "Note".to_string(),
                1735787045000,
                1738555506000,
                false,
            ),
            ("t1".to_string(), "Tag".to_string(), 1735787045000, 1735787045000, true),
        ];
        assert_eq!(summary(read(&top).unwrap()), expected);
        assert_eq!(summary(read(&folder).unwrap()), expected);
        assert_eq!(
            summary(read(dir.path()).unwrap()),
            expected,
            "the folder that holds the unpacked one"
        );
        assert_eq!(summary(read(&zip_path).unwrap()), expected);
    }

    #[test]
    fn i1_an_encrypted_backup_and_another_version_are_refused() {
        let encrypted = r#"{"version":"004","keyParams":{"identifier":"x"},"items":[]}"#;
        let message = parse(encrypted.as_bytes()).unwrap_err().to_string();
        assert!(message.contains("export a decrypted backup"), "{message}");
        let sealed = r#"{"version":"004","items":[{"uuid":"n","content_type":"Note","content":"004:abc"}]}"#;
        assert!(parse(sealed.as_bytes()).unwrap_err().to_string().contains("decrypted"));
        let old = r#"{"version":"003","items":[]}"#;
        let message = parse(old.as_bytes()).unwrap_err().to_string();
        assert!(message.contains("\"003\""), "{message}");
        assert!(parse(b"not json").is_err());
    }
}
