use serde::{Deserialize, Serialize};

/// What a folder sorts its notes by (R16).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, uniffi::Enum)]
pub enum SortBy {
    /// The moment of the last edit.
    #[default]
    Edited,
    Created,
    Title,
}

/// How a folder orders its notes (R16); pinned notes come first whatever it says.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct FolderSort {
    pub by: SortBy,
    pub descending: bool,
}

impl Default for FolderSort {
    fn default() -> Self {
        FolderSort {
            by: SortBy::Edited,
            descending: true,
        }
    }
}

impl FolderSort {
    /// The stored form, `<by>:<asc|desc>`; what does not read gives the default (R16).
    pub fn parse(s: &str) -> Self {
        let Some((by, direction)) = s.split_once(':') else {
            return FolderSort::default();
        };
        let by = match by {
            "edited" => SortBy::Edited,
            "created" => SortBy::Created,
            "title" => SortBy::Title,
            _ => return FolderSort::default(),
        };
        match direction {
            "asc" => FolderSort { by, descending: false },
            "desc" => FolderSort { by, descending: true },
            _ => FolderSort::default(),
        }
    }
    pub fn as_string(self) -> String {
        let by = match self.by {
            SortBy::Edited => "edited",
            SortBy::Created => "created",
            SortBy::Title => "title",
        };
        format!("{by}:{}", if self.descending { "desc" } else { "asc" })
    }
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct Folder {
    pub id: String,
    pub name: String,
    /// A symbol from the app's set or an emoji; empty for the default (R10).
    pub icon: String,
    /// The folder it is shown under; none for the root (S10).
    pub parent_id: Option<String>,
    /// Daily mode (R17).
    pub daily: bool,
    pub sort: FolderSort,
    pub show_preview: bool,
    pub show_date: bool,
    /// Notes in it and in its subfolders that are neither deleted nor archived (R12).
    pub note_count: u32,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct Note {
    pub id: String,
    pub title: String,
    /// The folder the note is shown in; none for the root, which a deleted or unknown folder also gives (S10).
    pub folder_id: Option<String>,
    pub tags: Vec<String>,
    pub pinned: bool,
    pub starred: bool,
    pub archived: bool,
    pub locked: bool,
    /// In the Trash (S6).
    pub deleted: bool,
    /// Milliseconds since the epoch.
    pub created_ms: u64,
    /// The greatest stamp among the registers of the note, in milliseconds since the epoch.
    pub edited_ms: u64,
    /// When the note went to the Trash, in milliseconds since the epoch (R46).
    pub deleted_ms: Option<u64>,
    /// Attachments that are not deleted.
    pub attachments: u32,
}

#[derive(Debug, Clone, Default, uniffi::Record)]
pub struct NewNote {
    pub title: String,
    #[uniffi(default = None)]
    pub folder_id: Option<String>,
    #[uniffi(default = [])]
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, uniffi::Enum)]
pub enum Scope {
    /// Notes that are neither archived nor in the Trash (R44).
    All,
    Starred,
    Archived,
    Trash,
    /// Notes without tags.
    Untagged,
    /// Notes outside any folder (R13).
    NoFolder,
    /// The notes of the folder itself.
    Folder {
        id: String,
    },
    Tag {
        name: String,
    },
    /// A saved filter.
    Filter {
        id: String,
    },
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct TagCount {
    pub name: String,
    pub count: u32,
}

/// What the system views of the sidebar hold (R44).
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct Counts {
    pub all: u32,
    pub starred: u32,
    pub archived: u32,
    pub trash: u32,
    pub untagged: u32,
    pub no_folder: u32,
}

/// Which part of a time a condition of a filter lets through (R45). A date is `YYYY-MM-DD`
/// on the clock of the device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Enum)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum DateTest {
    /// Before the day starts.
    Before { date: String },
    /// After the day is over.
    After { date: String },
    /// Within the last `days` days up to now.
    Within { days: u32 },
}

/// One condition of a saved filter (R45).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Enum)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum Condition {
    InFolder {
        folder_id: String,
        subfolders: bool,
    },
    HasTag {
        name: String,
    },
    /// The note has no tags at all.
    NoTags,
    NoneOfTags {
        names: Vec<String>,
    },
    Pinned,
    Starred,
    /// Asks for archived notes, which a filter leaves out otherwise (R27).
    Archived,
    HasAttachments,
    TitleIs {
        text: String,
    },
    TitleContains {
        text: String,
    },
    Created {
        when: DateTest,
    },
    Edited {
        when: DateTest,
    },
}

/// Whether a note has to meet every condition of a filter or any of them (R45).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, uniffi::Enum)]
#[serde(rename_all = "snake_case")]
pub enum FilterJoin {
    #[default]
    All,
    Any,
}

/// What a saved filter selects (R45).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, uniffi::Record)]
pub struct FilterQuery {
    #[serde(default)]
    pub join: FilterJoin,
    #[serde(default)]
    pub conditions: Vec<Condition>,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct SavedFilter {
    pub id: String,
    pub name: String,
    pub icon: String,
    pub query: FilterQuery,
    /// Notes it selects now.
    pub count: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, uniffi::Enum)]
pub enum TextAlign {
    #[default]
    Left,
    Justify,
}

/// The settings every device shares (S12). The font size and the column width belong to the device.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct AppSettings {
    /// A whole number of days from 1 to 3650 (S8).
    pub trash_days: u32,
    pub font_body: String,
    pub font_mono: String,
    pub line_height: f64,
    pub paragraph_spacing: f64,
    pub align: TextAlign,
    pub hyphenate: bool,
    pub smart_typography: bool,
    pub spellcheck: bool,
    /// `auto` or a BCP 47 tag.
    pub spell_language: String,
    /// `root`, `last` or the identifier of a folder (R2).
    pub new_note_folder: String,
}

pub const DEFAULT_TRASH_DAYS: u32 = 30;
pub const MAX_TRASH_DAYS: u32 = 3650;

impl Default for AppSettings {
    fn default() -> Self {
        AppSettings {
            trash_days: DEFAULT_TRASH_DAYS,
            font_body: "system".into(),
            font_mono: "system".into(),
            line_height: 1.5,
            paragraph_spacing: 0.5,
            align: TextAlign::Left,
            hyphenate: false,
            smart_typography: true,
            spellcheck: true,
            spell_language: "auto".into(),
            new_note_folder: "root".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct Attachment {
    pub id: String,
    pub note_id: String,
    pub name: String,
    pub mime: String,
    pub size: u64,
    pub sha256: String,
    /// Absolute path of the content if it is present on this device.
    pub local_path: Option<String>,
}

/// R36: how much content one attachment and all attachments of one note may take on this device.
/// Kept by the device, not synced: a phone and a server have different room.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct AttachmentLimits {
    /// The largest single file that can be added or downloaded.
    pub max_file: u64,
    /// The most that all live attachments of one note may weigh together.
    pub max_note: u64,
}

/// R36: the limits of a device that has not chosen its own.
pub const DEFAULT_MAX_FILE: u64 = 512 * 1024 * 1024;
pub const DEFAULT_MAX_NOTE: u64 = 2 * 1024 * 1024 * 1024;

impl Default for AttachmentLimits {
    fn default() -> Self {
        AttachmentLimits {
            max_file: DEFAULT_MAX_FILE,
            max_note: DEFAULT_MAX_NOTE,
        }
    }
}

/// What saving every attachment of a note into a folder did.
#[derive(Debug, Clone, PartialEq, Default, uniffi::Record)]
pub struct AttachmentsSaved {
    /// The names the files were written under, in the order of the attachments.
    pub saved: Vec<String>,
    /// The names of the attachments that could not be downloaded or written.
    pub failed: Vec<String>,
    /// How many were downloaded for this (R35).
    pub fetched: u32,
}

/// What a test of sync settings found when the storage can be reached (S46).
#[derive(Debug, Clone, Copy, PartialEq, uniffi::Enum)]
pub enum ConnectionCheck {
    /// The storage answers at the address.
    Ready,
    /// The address does not exist yet, the level above it does: the first sync creates it.
    WillCreate,
}

#[derive(Debug, Clone, PartialEq, uniffi::Enum)]
pub enum SyncConfig {
    Off,
    Folder {
        path: String,
    },
    /// The password is not part of the configuration: it is never written to
    /// the database. The app keeps it in the system keychain or keystore and
    /// hands it over with `Store::set_sync_password` after opening the store.
    WebDav {
        url: String,
        user: String,
    },
}

#[derive(Debug, Clone, PartialEq, Default, uniffi::Record)]
pub struct SyncReport {
    /// Field changes received that actually changed local state.
    pub pulled: u32,
    /// Field changes uploaded.
    pub pushed: u32,
    pub blobs_uploaded: u32,
    pub blobs_downloaded: u32,
}

/// Which side is kept when a device joins a storage and both hold data (S33).
#[derive(Debug, Clone, Copy, PartialEq, uniffi::Enum)]
pub enum SyncSide {
    /// Both: the usual merge (S16).
    Merge,
    /// The storage: the device drops its own data (S36).
    Storage,
    /// The device: its data goes over the storage's (S37).
    Device,
}

/// Result of one pass over attachment content (S24).
#[derive(Debug, Clone, PartialEq, Default, uniffi::Record)]
pub struct AttachmentReport {
    pub uploaded: u32,
    pub downloaded: u32,
    /// Attachments whose content is still to be moved after this pass.
    pub waiting: u32,
}

#[derive(Debug, Clone, PartialEq, Default, uniffi::Record)]
pub struct SyncStatus {
    pub configured: bool,
    /// Local changes not uploaded yet.
    pub pending: u32,
    /// Attachments whose content is not on this device or not known to be in the storage (S24).
    pub attachments_waiting: u32,
    /// `YYYY-MM-DDTHH:MM` of the last successful run.
    pub last_ok: Option<String>,
    pub last_error: Option<String>,
    /// S28: the storage is encrypted with a passphrase, as far as this device knows.
    pub encrypted: bool,
}
