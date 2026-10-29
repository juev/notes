//! Shared core of Lists: storage, merge of concurrent edits, sync, recurrence
//! and quick-entry parsing. The apps reach it through the UniFFI bindings
//! generated from this crate.

uniffi::setup_scaffolding!();

mod backup;
mod caldav;
mod db;
mod error;
mod export;
mod hlc;
mod import;
mod log;
mod markdown;
mod model;
mod order;
mod push;
mod quickadd;
mod recur;
mod store;
mod subtasks;
pub mod sync;
mod tag;
mod undo;

pub use backup::{Backup, BackupSettings};
pub use error::{AppError, Result};
pub use import::ImportReport;
pub use log::LogLevel;
pub use markdown::{
    markdown_layout, markdown_newline, MarkdownAlign, MarkdownBlock, MarkdownCell, MarkdownEdit, MarkdownKind,
    MarkdownLayout, MarkdownSpan, MarkdownTable, MarkdownTableRow,
};
pub use model::*;
pub use push::finish_pushes;
pub use store::{days_between, shift_date, Store};
pub use sync::check_sync_connection;
pub use sync::{StorageBackup, StorageBackupStatus};
