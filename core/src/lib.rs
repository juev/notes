//! Shared core of Notes: storage of notes, folders, tags and attachments, merge of
//! concurrent edits, sync through a folder or WebDAV, encryption and backups. The apps
//! reach it through the UniFFI bindings generated from this crate.

uniffi::setup_scaffolding!();

mod backup;
mod body;
mod db;
mod error;
mod highlight;
mod hlc;
mod log;
mod markdown;
mod model;
mod order;
mod push;
mod store;
pub mod sync;
mod tag;
mod undo;

pub use backup::{Backup, BackupSettings};
pub use error::{AppError, Result};
pub use highlight::{code_highlight_acknowledgements, CodeClass};
pub use log::LogLevel;
pub use markdown::{
    markdown_layout, markdown_newline, MarkdownAlign, MarkdownBlock, MarkdownCell, MarkdownEdit, MarkdownKind,
    MarkdownLayout, MarkdownRange, MarkdownSpan, MarkdownTable, MarkdownTableRow,
};
pub use model::*;
pub use push::finish_pushes;
pub use store::Store;
pub use sync::check_sync_connection;
pub use sync::{StorageBackup, StorageBackupStatus};
