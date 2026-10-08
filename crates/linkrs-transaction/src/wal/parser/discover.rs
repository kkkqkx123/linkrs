//! Shared WAL directory discovery and file-header validation.

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use linkrs_core::wal::types::{
    WalError, WalFileHeader, WalRecoveryMode, WalResult, WAL_FILE_HEADER_SIZE,
};

/// One fully-read WAL file with its validated file header.
pub(super) struct WalFileData {
    pub(super) buffer: Vec<u8>,
    pub(super) file_header: WalFileHeader,
}

/// List, filter and sort WAL files in `wal_dir`, honoring `ErrorIfMissing`.
pub(super) fn discover_wal_files(
    wal_dir: &Path,
    recovery_mode: WalRecoveryMode,
) -> WalResult<Vec<PathBuf>> {
    if !wal_dir.exists() {
        if recovery_mode == WalRecoveryMode::ErrorIfMissing {
            return Err(WalError::FileNotFound(
                wal_dir.to_string_lossy().to_string(),
            ));
        }
        std::fs::create_dir_all(wal_dir).map_err(|e| WalError::IoError(e.to_string()))?;
        return Ok(Vec::new());
    }

    let mut wal_files: Vec<PathBuf> = std::fs::read_dir(wal_dir)
        .map_err(|e| WalError::IoError(e.to_string()))?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension().is_some_and(|ext| ext == "wal")
                || path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("thread_") && n.contains("_wal_"))
        })
        .collect();

    wal_files.sort();

    if wal_files.is_empty() && recovery_mode == WalRecoveryMode::ErrorIfMissing {
        return Err(WalError::FileNotFound("No WAL files found".to_string()));
    }

    Ok(wal_files)
}

/// Read the whole file and validate its header. `None` for empty files.
pub(super) fn read_wal_file(path: &Path, verify_checksum: bool) -> WalResult<Option<WalFileData>> {
    let metadata = std::fs::metadata(path).map_err(|e| WalError::IoError(e.to_string()))?;

    if metadata.len() == 0 {
        return Ok(None);
    }

    let mut file = File::open(path).map_err(|e| WalError::IoError(e.to_string()))?;

    let file_size = metadata.len() as usize;
    let mut buffer = Vec::with_capacity(file_size);
    file.read_to_end(&mut buffer)
        .map_err(|e| WalError::IoError(e.to_string()))?;

    if buffer.len() < WAL_FILE_HEADER_SIZE {
        return Err(WalError::InvalidFileHeader);
    }

    let file_header = WalFileHeader::from_bytes(&buffer[..WAL_FILE_HEADER_SIZE])
        .ok_or(WalError::InvalidFileHeader)?;

    if !file_header.is_valid() {
        return Err(WalError::InvalidFileHeader);
    }

    if let Some(header_checksum) = file_header.checksum_enabled() {
        if header_checksum != verify_checksum {
            log::warn!(
                "WAL file {:?} checksum config mismatch: header={}, parser verify={}",
                path,
                header_checksum,
                verify_checksum
            );
        }
    }

    Ok(Some(WalFileData {
        buffer,
        file_header,
    }))
}
