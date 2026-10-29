//! The log of the app (R102): a text file in the data folder that the core
//! writes on macOS and Android alike. It holds what happened, never what the
//! tasks say: counts, outcomes and the text of errors.

use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::Ordering;

use crate::db;
use crate::error::Result;
use crate::store::Store;

const LEVEL_KEY: &str = "log_level";
const FILE: &str = "log.txt";
const PREVIOUS: &str = "log.1.txt";
/// Past this size the file becomes the previous one and a new file is started.
pub(crate) const MAX_BYTES: u64 = 1 << 20;

/// How much the log takes, each level with everything of those before it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, uniffi::Enum)]
pub enum LogLevel {
    Off,
    Error,
    Info,
    Debug,
}

impl LogLevel {
    fn name(self) -> &'static str {
        match self {
            LogLevel::Off => "off",
            LogLevel::Error => "error",
            LogLevel::Info => "info",
            LogLevel::Debug => "debug",
        }
    }

    /// A value that is not a level is the default: errors.
    pub(crate) fn from_name(name: Option<&str>) -> Self {
        match name {
            Some("off") => LogLevel::Off,
            Some("info") => LogLevel::Info,
            Some("debug") => LogLevel::Debug,
            _ => LogLevel::Error,
        }
    }

    pub(crate) fn from_code(code: u8) -> Self {
        match code {
            0 => LogLevel::Off,
            2 => LogLevel::Info,
            3 => LogLevel::Debug,
            _ => LogLevel::Error,
        }
    }

    pub(crate) fn code(self) -> u8 {
        match self {
            LogLevel::Off => 0,
            LogLevel::Error => 1,
            LogLevel::Info => 2,
            LogLevel::Debug => 3,
        }
    }
}

impl Store {
    fn log_file(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    /// Writes a line when the level takes it. The message is built only then.
    /// A log that cannot be written is not an error of the work it describes.
    pub(crate) fn note(&self, level: LogLevel, source: &str, message: impl FnOnce() -> String) {
        if level == LogLevel::Off || level > LogLevel::from_code(self.log_level.load(Ordering::Relaxed)) {
            return;
        }
        let label = match level {
            LogLevel::Error => "ERROR",
            LogLevel::Info => "INFO",
            _ => "DEBUG",
        };
        let message = message().replace(['\n', '\r'], " ");
        let line = format!(
            "{} {label} {source}: {message}\n",
            chrono::Local::now().format("%Y-%m-%dT%H:%M:%S")
        );
        let _writing = self.log_lock.lock().unwrap_or_else(|p| p.into_inner());
        let path = self.log_file(FILE);
        if std::fs::metadata(&path).is_ok_and(|m| m.len() > MAX_BYTES) {
            let _ = std::fs::rename(&path, self.log_file(PREVIOUS));
        }
        let mut options = std::fs::OpenOptions::new();
        options.create(true).append(true);
        if let Ok(mut file) = options.open(&path) {
            // R142: the log names failures, so it is for this user alone like the data.
            let _ = crate::store::private_file(&path);
            let _ = file.write_all(line.as_bytes());
        }
    }

    /// Notes how a piece of work ended: its error, or at `level` what `done` says of the result.
    pub(crate) fn note_result<T>(
        &self,
        source: &str,
        result: &Result<T>,
        level: LogLevel,
        done: impl FnOnce(&T) -> String,
    ) {
        match result {
            Ok(value) => self.note(level, source, || done(value)),
            Err(e) => self.note(LogLevel::Error, source, || e.to_string()),
        }
    }

    /// Reads the level kept on this device when the store is opened.
    pub(crate) fn load_log_level(&self) -> Result<()> {
        let saved = db::meta_get(&self.lock().conn, LEVEL_KEY)?;
        self.log_level
            .store(LogLevel::from_name(saved.as_deref()).code(), Ordering::Relaxed);
        Ok(())
    }
}

#[uniffi::export]
impl Store {
    /// R102: the level of the log on this device; errors until another is chosen.
    pub fn log_level(&self) -> LogLevel {
        LogLevel::from_code(self.log_level.load(Ordering::Relaxed))
    }

    /// Kept on this device and not synced.
    pub fn set_log_level(&self, level: LogLevel) -> Result<()> {
        db::meta_set(&self.lock().conn, LEVEL_KEY, level.name())?;
        // Said at the level that is higher of the two, so that the change is seen from either side.
        let before = self.log_level();
        if level > before {
            self.log_level.store(level.code(), Ordering::Relaxed);
        }
        self.note(LogLevel::Info, "log", || format!("level set to {}", level.name()));
        self.log_level.store(level.code(), Ordering::Relaxed);
        Ok(())
    }

    /// A line from the app around the core: its start, a failure of its own.
    /// The caller keeps the content of tasks, passwords and tokens out of it.
    pub fn log(&self, level: LogLevel, source: String, message: String) {
        self.note(level, &source, || message);
    }

    /// The files of the log that exist, the earlier one first; to hand them over.
    pub fn log_files(&self) -> Vec<String> {
        [PREVIOUS, FILE]
            .iter()
            .map(|name| self.log_file(name))
            .filter(|path| path.is_file())
            .map(|path| path.to_string_lossy().into_owned())
            .collect()
    }

    /// Removes both files of the log.
    pub fn clear_log(&self) -> Result<()> {
        let _writing = self.log_lock.lock().unwrap_or_else(|p| p.into_inner());
        for name in [FILE, PREVIOUS] {
            match std::fs::remove_file(self.log_file(name)) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
                _ => {}
            }
        }
        Ok(())
    }
}
