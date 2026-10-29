//! What sync needs from storage: read, write, list and delete files by relative path.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::error::{AppError, Result};

pub trait Remote: Send + Sync {
    /// Tells one storage from another within a process.
    fn id(&self) -> String;
    /// File names directly inside `dir`. A missing directory is empty.
    fn list(&self, dir: &str) -> Result<Vec<String>>;
    fn get(&self, path: &str) -> Result<Option<Vec<u8>>>;
    /// Creates parent directories as needed and replaces an existing file.
    fn put(&self, path: &str, data: &[u8]) -> Result<()>;
    /// R140: writes the file at `local` as the content of `path`, without holding it whole in memory.
    fn put_file(&self, path: &str, local: &Path) -> Result<()> {
        self.put(path, &fs::read(local)?)
    }
    /// R140: copies the content at `path` into `out` in pieces. `false` when there is nothing there.
    /// `max` bounds the bytes taken; zero means no limit. A storage used whole reads the file and writes it.
    fn get_to(&self, path: &str, out: &mut dyn Write, max: u64) -> Result<bool> {
        let Some(data) = self.get(path)? else {
            return Ok(false);
        };
        if max != 0 && data.len() as u64 > max {
            return Err(AppError::too_large(format!("the content is more than {max} bytes")));
        }
        out.write_all(&data)?;
        Ok(true)
    }
    /// Deleting a missing file is not an error.
    fn delete(&self, path: &str) -> Result<()>;
    fn exists(&self, path: &str) -> Result<bool>;
    /// Folder names directly inside `dir`. A missing directory is empty.
    fn folders(&self, dir: &str) -> Result<Vec<String>> {
        Err(AppError::sync(format!("folders of {dir} cannot be listed here")))
    }
    /// Moves a file, replacing one at `to` and creating its parent directories.
    /// `false` when there is nothing at `from`: someone else moved it first.
    fn rename(&self, from: &str, to: &str) -> Result<bool> {
        copy_over(self, from, to)
    }
}

/// A move made of the operations every storage has: read, write, delete.
/// R140: the content goes through a temporary file, so a large attachment is not held in memory.
pub(crate) fn copy_over<R: Remote + ?Sized>(remote: &R, from: &str, to: &str) -> Result<bool> {
    let tmp = std::env::temp_dir().join(format!("lists-copy-{}.part", uuid::Uuid::now_v7().simple()));
    let found = {
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut out = options.open(&tmp)?;
        match remote.get_to(from, &mut out, 0) {
            Ok(found) => found,
            Err(e) => {
                drop(out);
                let _ = fs::remove_file(&tmp);
                return Err(e);
            }
        }
    };
    if !found {
        let _ = fs::remove_file(&tmp);
        return Ok(false);
    }
    let sent = remote.put_file(to, &tmp);
    let _ = fs::remove_file(&tmp);
    sent?;
    remote.delete(from)?;
    Ok(true)
}

/// A plain folder: a mounted share, a folder synced by another tool, or a test directory.
pub struct DirRemote {
    root: PathBuf,
}

impl DirRemote {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        DirRemote { root: root.into() }
    }

    fn resolve(&self, path: &str) -> Result<PathBuf> {
        if path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
        {
            return Err(AppError::sync(format!("bad path: {path}")));
        }
        Ok(self.root.join(path))
    }
}

impl Remote for DirRemote {
    fn id(&self) -> String {
        self.root.to_string_lossy().into_owned()
    }

    fn list(&self, dir: &str) -> Result<Vec<String>> {
        let entries = match fs::read_dir(self.resolve(dir)?) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
            Err(e) => return Err(e.into()),
        };
        let mut names = Vec::new();
        for entry in entries {
            let entry = entry?;
            if entry.file_type()?.is_file() {
                names.push(entry.file_name().to_string_lossy().into_owned());
            }
        }
        Ok(names)
    }

    fn get(&self, path: &str) -> Result<Option<Vec<u8>>> {
        match fs::read(self.resolve(path)?) {
            Ok(data) => Ok(Some(data)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    fn get_to(&self, path: &str, out: &mut dyn Write, max: u64) -> Result<bool> {
        let mut input = match fs::File::open(self.resolve(path)?) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(e.into()),
        };
        if max != 0 && input.metadata()?.len() > max {
            return Err(AppError::too_large(format!("the content is more than {max} bytes")));
        }
        let copied = std::io::copy(&mut input, out)?;
        if max != 0 && copied > max {
            return Err(AppError::too_large(format!("the content is more than {max} bytes")));
        }
        Ok(true)
    }

    fn put(&self, path: &str, data: &[u8]) -> Result<()> {
        let target = self.resolve(path)?;
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        // Readers never see a half-written file: write aside, then rename.
        let tmp = target.with_extension("part");
        fs::write(&tmp, data)?;
        fs::rename(&tmp, &target)?;
        Ok(())
    }

    fn put_file(&self, path: &str, local: &Path) -> Result<()> {
        let target = self.resolve(path)?;
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        let tmp = target.with_extension(format!("{}.part", uuid::Uuid::now_v7().simple()));
        fs::copy(local, &tmp)?;
        fs::rename(&tmp, &target)?;
        Ok(())
    }

    fn delete(&self, path: &str) -> Result<()> {
        match fs::remove_file(self.resolve(path)?) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    fn exists(&self, path: &str) -> Result<bool> {
        Ok(self.resolve(path)?.is_file())
    }

    fn folders(&self, dir: &str) -> Result<Vec<String>> {
        let entries = match fs::read_dir(self.resolve(dir)?) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
            Err(e) => return Err(e.into()),
        };
        let mut names = Vec::new();
        for entry in entries {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                names.push(entry.file_name().to_string_lossy().into_owned());
            }
        }
        Ok(names)
    }

    fn rename(&self, from: &str, to: &str) -> Result<bool> {
        let (source, target) = (self.resolve(from)?, self.resolve(to)?);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        match fs::rename(source, target) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e.into()),
        }
    }
}
