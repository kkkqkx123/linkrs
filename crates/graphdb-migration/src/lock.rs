use std::fs::{File, OpenOptions};
use std::path::Path;

use fs4::fs_std::FileExt;
use graphdb_core::StorageError;

use crate::generator::MigrationError;

pub struct MigrationFileLock {
    _file: File,
    path: std::path::PathBuf,
}

impl MigrationFileLock {
    pub fn try_acquire(path: &Path) -> Result<Self, MigrationError> {
        let file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)
            .map_err(|e| {
                MigrationError::Storage(Box::new(StorageError::from(e)))
            })?;

        file.try_lock_exclusive().map_err(|e| {
            MigrationError::Plan(format!(
                "migration in progress (lock file: {}): {}",
                path.display(),
                e
            ))
        })?;

        Ok(Self {
            _file: file,
            path: path.to_path_buf(),
        })
    }
}

impl Drop for MigrationFileLock {
    fn drop(&mut self) {
        let _ = self._file.unlock();
        let _ = std::fs::remove_file(&self.path);
    }
}

impl std::fmt::Debug for MigrationFileLock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MigrationFileLock")
            .field("path", &self.path)
            .finish()
    }
}
