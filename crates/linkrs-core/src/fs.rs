//! Cross-platform filesystem durability helpers.
//!
//! Fsync is portable-hostile. On Windows `FlushFileBuffers` requires a handle
//! opened with write access, so `File::open(path).sync_all()` (read-only) fails
//! with `ERROR_ACCESS_DENIED` (os error 5) for both files and directories,
//! while the same call works on Unix. Directory handles additionally require
//! `FILE_FLAG_BACKUP_SEMANTICS`. `sync_file` and `sync_dir` open handles with
//! the access Windows requires and otherwise keep the Unix behavior.

use std::path::Path;

/// fsync a regular file so its contents become durable.
///
/// On Unix a read-only handle is sufficient for `fsync`. On Windows the handle
/// must be opened with write access, otherwise `FlushFileBuffers` returns
/// `ERROR_ACCESS_DENIED`.
pub fn sync_file(path: impl AsRef<Path>) -> std::io::Result<()> {
    let path = path.as_ref();
    #[cfg(windows)]
    {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)?;
        file.sync_all()
    }
    #[cfg(not(windows))]
    {
        std::fs::File::open(path)?.sync_all()
    }
}

/// fsync a directory so that file creates/renames within it become durable.
///
/// On Unix a read-only handle is sufficient for `fsync`. On Windows the handle
/// must be opened with write access plus `FILE_FLAG_BACKUP_SEMANTICS`,
/// otherwise `FlushFileBuffers` returns `ERROR_ACCESS_DENIED`.
pub fn sync_dir(path: impl AsRef<Path>) -> std::io::Result<()> {
    let path = path.as_ref();
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;

        const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;

        let file = std::fs::OpenOptions::new()
            .write(true)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
            .open(path)?;
        file.sync_all()
    }
    #[cfg(not(windows))]
    {
        std::fs::File::open(path)?.sync_all()
    }
}
