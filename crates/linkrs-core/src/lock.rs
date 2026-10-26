//! Advisory file-lock helpers shared by the embedded shell and the server.
//!
//! The database path is a *work directory* (containing `data/`, `wal/`,
//! `checkpoint/`, ...), so the lock file lives at `<work_dir>/LOCK` rather
//! than next to a single database file. `:memory:` databases skip locking.
//!
//! Locks are advisory (`fs4`): the kernel releases them automatically when
//! the owning process exits or panics, so there is no "stale lock" recovery
//! and no PID bookkeeping. The lock file itself is never deleted.

use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

use fs4::fs_std::FileExt;

/// Guard holding an advisory lock for the lifetime of the database handle.
pub struct DbFileLock {
    _file: File,
}

/// Compute the lock file path for a database path.
pub fn lock_path_for_db(db_path: &str) -> Option<PathBuf> {
    if db_path == ":memory:" {
        return None;
    }
    let path = Path::new(db_path);
    let dir = if path.is_file() {
        path.parent().unwrap_or(Path::new(".")).to_path_buf()
    } else {
        path.to_path_buf()
    };
    Some(dir.join("LOCK"))
}

/// Acquire an advisory lock for `db_path`.
///
/// `read_only == true` requests a shared (`SH`) lock, otherwise an exclusive
/// (`EX`) lock. Returns `Ok(None)` for `:memory:` databases.
pub fn acquire_db_lock(db_path: &str, read_only: bool) -> io::Result<Option<DbFileLock>> {
    let Some(lock_path) = lock_path_for_db(db_path) else {
        return Ok(None);
    };

    if !read_only {
        if let Some(parent) = lock_path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
    }

    let file = OpenOptions::new()
        .read(true)
        .write(!read_only)
        .create(!read_only)
        .truncate(false)
        .open(&lock_path)?;

    let acquired = if read_only {
        FileExt::try_lock_shared(&file)
    } else {
        FileExt::try_lock_exclusive(&file)
    };
    let locked = acquired.map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;

    if !locked {
        let hint = if read_only {
            "another process holds a write lock; stop the other process or retry later"
        } else {
            "use --readonly to open a shared read view, or stop the other process"
        };
        return Err(io::Error::new(
            io::ErrorKind::ResourceBusy,
            format!(
                "database at '{}' is in use by another process (lock file: {}). {}",
                db_path,
                lock_path.display(),
                hint
            ),
        ));
    }

    Ok(Some(DbFileLock { _file: file }))
}