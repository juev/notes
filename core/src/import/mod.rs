//! Import of a decrypted backup of Standard Notes (docs/specs/import.md). Reading the backup and
//! converting its parts are plain functions; writing into the storage is in `store.rs`.

pub(crate) mod backup;
pub(crate) mod lexical;
pub(crate) mod predicate;
mod store;

pub use store::{IgnoredItems, ImportOptions, ImportReport};
