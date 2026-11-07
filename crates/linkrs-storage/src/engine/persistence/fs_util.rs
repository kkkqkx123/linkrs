//! Shared filesystem durability helpers: recursive tree and single
//! directory fsync. Used by the checkpoint writer and the snapshot manager.

use std::path::Path;

use linkrs_core::fs::sync_dir;
use linkrs_core::StorageResult;

/// Recursively fsync every file under `root`, then each directory bottom-up.
pub(crate) fn sync_tree(root: &Path) -> StorageResult<()> {
    fn visit(directory: &Path) -> StorageResult<()> {
        for item in std::fs::read_dir(directory)? {
            let path = item?.path();
            if path.is_dir() {
                visit(&path)?;
            } else if path.is_file() {
                linkrs_core::fs::sync_file(path)?;
            }
        }
        sync_directory(directory)
    }

    visit(root)
}

/// fsync a single directory entry so rename/create operations are durable.
pub(crate) fn sync_directory(directory: &Path) -> StorageResult<()> {
    sync_dir(directory).map_err(|error| {
        linkrs_core::StorageError::io_error(format!(
            "Failed to sync directory {}: {}",
            directory.display(),
            error
        ))
    })?;
    Ok(())
}
