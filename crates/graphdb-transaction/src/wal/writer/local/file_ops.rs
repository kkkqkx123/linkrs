//! Local WAL writer - file_ops module

use std::fs::{File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

use crate::wal::parser::{LocalWalParser, WalParser};
use graphdb_core::wal::types::{
    ArchiveMode, Lsn, WalError, WalFileHeader, WalResult, WAL_FILE_HEADER_SIZE,
};

use super::LocalWalWriter;

impl LocalWalWriter {
    /// Get the WAL directory path
    pub(crate) fn get_wal_dir(&self) -> PathBuf {
        PathBuf::from(&self.wal_uri)
    }

    /// Find next available file path
    pub(crate) fn find_available_path(&self) -> WalResult<PathBuf> {
        let wal_dir = self.get_wal_dir();

        if !wal_dir.exists() {
            std::fs::create_dir_all(&wal_dir).map_err(|e| WalError::IoError(e.to_string()))?;
        }

        for version in self.version..65536 {
            let path = self.get_wal_file_path(version);
            if !path.exists() {
                return Ok(path);
            }
        }

        Err(WalError::IoError(
            "No available WAL file version".to_string(),
        ))
    }

    /// Generate WAL file path for a given version
    pub(crate) fn get_wal_file_path(&self, version: u32) -> PathBuf {
        PathBuf::from(&self.wal_uri).join(format!("thread_{}_wal_{:08X}", self.thread_id, version))
    }

    /// Determine whether a path belongs to this writer's WAL set.
    pub(crate) fn is_managed_wal_file(&self, path: &Path) -> bool {
        path.file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|name| {
                name.starts_with(&format!("thread_{}_wal_", self.thread_id))
                    || (name.starts_with("wal_") && name.len() == 12)
            })
    }

    /// Return the currently open WAL file path, if any.
    pub(crate) fn current_file_path(&self) -> Option<PathBuf> {
        self.file_path.clone()
    }

    /// Read the WAL file header from disk.
    pub(crate) fn read_file_header(&self, path: &Path) -> WalResult<Option<WalFileHeader>> {
        use std::io::Read;

        let mut file = match std::fs::File::open(path) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(WalError::IoError(e.to_string())),
        };

        let mut buffer = [0u8; WAL_FILE_HEADER_SIZE];
        if let Err(e) = file.read_exact(&mut buffer) {
            return Err(WalError::IoError(e.to_string()));
        }

        Ok(WalFileHeader::from_bytes(&buffer))
    }

    /// Check if rotation is needed
    pub(crate) fn rotate_if_needed(&mut self) -> WalResult<()> {
        if self.file_used >= self.config.max_file_size {
            self.rotate()?;
        }
        Ok(())
    }

    /// Rotate to a new WAL file
    pub(crate) fn rotate(&mut self) -> WalResult<()> {
        log::info!(
            "Rotating WAL file: used={}, max_size={}, version={}",
            self.file_used,
            self.config.max_file_size,
            self.version
        );

        if let Some(ref file) = self.file {
            file.sync_all()?;
        }

        self.version += 1;

        let new_path = self.get_wal_file_path(self.version);

        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&new_path)?;

        file.set_len(self.config.truncate_size as u64)?;

        if let Some(ref coordinator) = self.group_commit {
            if let Ok(cloned) = file.try_clone() {
                coordinator.update_file(cloned);
            }
        }

        self.file = Some(file);
        self.file_path = Some(new_path);
        self.file_size = self.config.truncate_size;
        self.file_used = 0;
        self.file_start_lsn = Lsn::new(self.current_lsn.load(Ordering::SeqCst));

        self.write_file_header()?;

        // Record rotation statistics
        self.stats.record_rotation();

        log::info!(
            "WAL rotated to version {}, file: {:?}, start_lsn={}",
            self.version,
            self.file_path,
            self.file_start_lsn
        );

        Ok(())
    }

    /// Delete or archive a WAL file based on configuration
    pub(crate) fn delete_or_archive_file(&mut self, file: &Path) -> WalResult<()> {
        if let Some(ref archive_dir) = self.config.archive_dir {
            match self.config.archive_mode {
                ArchiveMode::None => {
                    std::fs::remove_file(file)?;
                    self.stats.record_file_deleted();
                }
                ArchiveMode::Move => {
                    self.archive_wal_file(file, archive_dir)?;
                    self.stats.record_file_archived();
                }
                ArchiveMode::Copy => {
                    self.copy_and_delete(file, archive_dir)?;
                    self.stats.record_file_archived();
                }
            }
        } else {
            std::fs::remove_file(file)?;
            self.stats.record_file_deleted();
        }
        Ok(())
    }

    /// Archive a WAL file to the archive directory
    pub(crate) fn archive_wal_file(&self, file: &Path, archive_dir: &str) -> WalResult<()> {
        std::fs::create_dir_all(archive_dir).map_err(|e| WalError::IoError(e.to_string()))?;

        let file_name = file
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown");

        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let archive_name = format!("{}_{}", file_name, timestamp);
        let archive_path = PathBuf::from(archive_dir).join(archive_name);

        std::fs::rename(file, &archive_path).map_err(|e| WalError::IoError(e.to_string()))?;

        log::debug!("Archived WAL file: {:?} -> {:?}", file, archive_path);

        Ok(())
    }

    /// Copy a file and delete the original
    pub(crate) fn copy_and_delete(&self, file: &Path, archive_dir: &str) -> WalResult<()> {
        std::fs::create_dir_all(archive_dir).map_err(|e| WalError::IoError(e.to_string()))?;

        let file_name = file
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown");

        let archive_path = PathBuf::from(archive_dir).join(file_name);

        std::fs::copy(file, &archive_path).map_err(|e| WalError::IoError(e.to_string()))?;

        std::fs::remove_file(file)?;

        log::debug!(
            "Copied and deleted WAL file: {:?} -> {:?}",
            file,
            archive_path
        );

        Ok(())
    }

    /// Remove WAL files that are older than the current checkpoint boundary.
    pub(crate) fn reclaim_before_checkpoint(&mut self) -> WalResult<usize> {
        let current_file = self.current_file_path();
        let checkpoint_seq = self.checkpoint_seq;

        if checkpoint_seq == 0 {
            return Ok(0);
        }

        let wal_dir = self.get_wal_dir();
        if !wal_dir.exists() {
            return Ok(0);
        }

        let mut deleted_count = 0;

        for entry in std::fs::read_dir(&wal_dir)? {
            let entry = entry?;
            let path = entry.path();

            if current_file
                .as_ref()
                .is_some_and(|current| current == &path)
            {
                continue;
            }

            if !self.is_managed_wal_file(&path) {
                continue;
            }

            let Some(header) = self.read_file_header(&path)? else {
                continue;
            };

            if header.thread_id != self.thread_id || header.checkpoint_seq >= checkpoint_seq {
                continue;
            }

            self.delete_or_archive_file(&path)?;
            deleted_count += 1;
        }

        if deleted_count > 0 {
            log::info!(
                "Reclaimed {} WAL files before checkpoint seq {}",
                deleted_count,
                checkpoint_seq
            );
        }

        Ok(deleted_count)
    }

    pub fn truncate(&mut self, lsn: Lsn) -> WalResult<usize> {
        let durable_lsn = self.durable_lsn();
        if lsn > durable_lsn {
            return Err(WalError::InvalidOperation(format!(
                "Cannot truncate WAL at {} beyond durable LSN {}",
                lsn, durable_lsn
            )));
        }

        if lsn == self.current_lsn() {
            self.set_current_lsn(lsn);
            if self.file.is_some() {
                self.refresh_file_header()?;
            }
        }

        self.reclaim_before_checkpoint()
    }
}
