#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum AppError {
    #[error("not found: {what}")]
    NotFound { what: String },
    #[error("invalid: {msg}")]
    Invalid { msg: String },
    #[error("storage: {msg}")]
    Storage { msg: String },
    #[error("sync: {msg}")]
    Sync { msg: String },
    /// R140: the content is above what this device takes, or all attachments of the task are.
    #[error("too large: {msg}")]
    TooLarge { msg: String },
}

pub type Result<T> = std::result::Result<T, AppError>;

impl AppError {
    pub fn invalid(msg: impl Into<String>) -> Self {
        AppError::Invalid { msg: msg.into() }
    }
    pub fn sync(msg: impl Into<String>) -> Self {
        AppError::Sync { msg: msg.into() }
    }
    pub fn not_found(what: impl Into<String>) -> Self {
        AppError::NotFound { what: what.into() }
    }
    pub fn too_large(msg: impl Into<String>) -> Self {
        AppError::TooLarge { msg: msg.into() }
    }
}

impl From<rusqlite::Error> for AppError {
    fn from(e: rusqlite::Error) -> Self {
        AppError::Storage { msg: e.to_string() }
    }
}

impl From<std::io::Error> for AppError {
    fn from(e: std::io::Error) -> Self {
        AppError::Storage { msg: e.to_string() }
    }
}

impl From<serde_json::Error> for AppError {
    fn from(e: serde_json::Error) -> Self {
        AppError::Storage { msg: e.to_string() }
    }
}
