//! checkpoint.meta text-format codec: save, parse, file collection and
//! checksum verification for checkpoint directories.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use graphdb_core::types::Timestamp;
use graphdb_core::{StorageError, StorageResult};
use graphdb_transaction::wal::Lsn;

use crate::engine::persistence_coordinator::PersistenceCoordinator;
use crate::persistence::dirty_page::IncrementalCheckpointMeta;

pub const CHECKPOINT_FORMAT_VERSION: u32 = 1;
pub const INCREMENTAL_CHECKPOINT_FORMAT_VERSION: u32 = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CheckpointFileEntry {
    pub(crate) path: PathBuf,
    pub(crate) size: u64,
    pub(crate) checksum: u32,
}

#[derive(Debug, Clone)]
pub struct CheckpointInfo {
    pub checkpoint_id: u64,
    pub lsn: Lsn,
    pub timestamp: Timestamp,
}

impl PersistenceCoordinator {
    pub(super) fn save_checkpoint_metadata(
        &self,
        dir: &Path,
        checkpoint: &graphdb_transaction::wal::Checkpoint,
        data: &crate::engine::persistence_coordinator::CheckpointData,
        files: &[CheckpointFileEntry],
    ) -> StorageResult<()> {
        self.save_checkpoint_metadata_extended(dir, checkpoint, data, files, None)
    }

    pub(super) fn save_checkpoint_metadata_extended(
        &self,
        dir: &Path,
        checkpoint: &graphdb_transaction::wal::Checkpoint,
        data: &crate::engine::persistence_coordinator::CheckpointData,
        files: &[CheckpointFileEntry],
        incremental: Option<&IncrementalCheckpointMeta>,
    ) -> StorageResult<()> {
        use std::fs::File;
        use std::io::Write;

        let metadata_path = dir.join("checkpoint.meta");
        let mut file = File::create(metadata_path)?;

        let version = if incremental.is_some() {
            INCREMENTAL_CHECKPOINT_FORMAT_VERSION
        } else {
            CHECKPOINT_FORMAT_VERSION
        };
        writeln!(file, "format_version={}", version)?;
        writeln!(file, "checkpoint_id={}", checkpoint.seq)?;
        writeln!(file, "timestamp={}", checkpoint.timestamp)?;
        writeln!(file, "wal_lsn={}", checkpoint.lsn.0)?;
        writeln!(file, "vertex_count={}", data.vertex_count)?;
        writeln!(file, "edge_count={}", data.edge_count)?;
        writeln!(file, "data_size={}", data.data_size)?;
        writeln!(file, "created_at={:?}", SystemTime::now())?;
        if let Some(meta) = incremental {
            writeln!(file, "strategy={}", meta.strategy.as_str())?;
            if let Some(base) = meta.base_checkpoint_id {
                writeln!(file, "base_checkpoint_id={}", base)?;
            }
            writeln!(file, "dirty_pages={}", meta.dirty_pages.len())?;
            for page in &meta.dirty_pages {
                writeln!(
                    file,
                    "dirty_page={}:{}",
                    page.component.as_str(),
                    page.page_id
                )?;
            }
            for (page, checksum) in &meta.page_checksums {
                writeln!(
                    file,
                    "page_checksum={}:{}:{}",
                    page.component.as_str(),
                    page.page_id,
                    checksum
                )?;
            }
            writeln!(file, "total_pages={}", meta.total_pages)?;
            writeln!(file, "dirty_ratio={}", meta.dirty_ratio)?;
        } else {
            writeln!(file, "strategy=full")?;
        }
        for entry in files {
            writeln!(
                file,
                "file={}|{}|{}",
                entry.path.display(),
                entry.size,
                entry.checksum
            )?;
        }
        file.sync_all()?;

        Ok(())
    }

    pub(crate) fn collect_checkpoint_files(root: &Path) -> StorageResult<Vec<CheckpointFileEntry>> {
        fn visit(
            root: &Path,
            directory: &Path,
            entries: &mut Vec<CheckpointFileEntry>,
        ) -> StorageResult<()> {
            for item in std::fs::read_dir(directory)? {
                let item = item?;
                let path = item.path();
                if path.is_dir() {
                    visit(root, &path, entries)?;
                } else if path.is_file() {
                    let relative = path.strip_prefix(root).map_err(|error| {
                        StorageError::invalid_operation(format!(
                            "checkpoint file is outside its root: {}",
                            error
                        ))
                    })?;
                    let bytes = std::fs::read(&path)?;
                    entries.push(CheckpointFileEntry {
                        path: relative.to_path_buf(),
                        size: bytes.len() as u64,
                        checksum: crc32fast::hash(&bytes),
                    });
                }
            }
            Ok(())
        }

        let mut entries = Vec::new();
        visit(root, root, &mut entries)?;
        entries.sort_by(|left, right| left.path.cmp(&right.path));
        Ok(entries)
    }

    pub(super) fn verify_checkpoint_files(
        checkpoint_dir: &Path,
        files: &[CheckpointFileEntry],
    ) -> StorageResult<()> {
        for entry in files {
            if entry.path.is_absolute()
                || entry
                    .path
                    .components()
                    .any(|component| component == std::path::Component::ParentDir)
            {
                return Err(StorageError::deserialize_error(format!(
                    "Invalid checkpoint file path: {}",
                    entry.path.display()
                )));
            }
            let path = checkpoint_dir.join(&entry.path);
            let bytes = std::fs::read(&path)?;
            if bytes.len() as u64 != entry.size || crc32fast::hash(&bytes) != entry.checksum {
                return Err(StorageError::deserialize_error(format!(
                    "Checkpoint file verification failed: {}",
                    entry.path.display()
                )));
            }
        }
        Ok(())
    }

    pub(super) fn load_checkpoint_metadata(
        &self,
        dir: &Path,
    ) -> StorageResult<(CheckpointInfo, Vec<CheckpointFileEntry>)> {
        use std::fs::File;
        use std::io::{BufRead, BufReader};

        let metadata_path = dir.join("checkpoint.meta");
        let file = File::open(metadata_path)?;
        let reader = BufReader::new(file);

        let mut checkpoint_id: Option<u64> = None;
        let mut lsn: Option<u64> = None;
        let mut timestamp: Option<Timestamp> = None;
        let mut format_version: Option<u32> = None;
        let mut files = Vec::new();

        for line in reader.lines() {
            let line = line?;
            let parts: Vec<&str> = line.splitn(2, '=').collect();
            if parts.len() != 2 {
                return Err(StorageError::deserialize_error(format!(
                    "Invalid checkpoint metadata line: {}",
                    line
                )));
            }

            match parts[0] {
                "format_version" => {
                    format_version = Some(parts[1].parse().map_err(|e| {
                        StorageError::deserialize_error(format!(
                            "Invalid format_version in checkpoint metadata: {}",
                            e
                        ))
                    })?);
                }
                "checkpoint_id" => {
                    checkpoint_id = Some(parts[1].parse().map_err(|e| {
                        StorageError::deserialize_error(format!(
                            "Invalid checkpoint_id in checkpoint metadata: {}",
                            e
                        ))
                    })?);
                }
                "wal_lsn" => {
                    lsn = Some(parts[1].parse().map_err(|e| {
                        StorageError::deserialize_error(format!(
                            "Invalid wal_lsn in checkpoint metadata: {}",
                            e
                        ))
                    })?);
                }
                "timestamp" => {
                    timestamp = Some(parts[1].parse().map_err(|e| {
                        StorageError::deserialize_error(format!(
                            "Invalid timestamp in checkpoint metadata: {}",
                            e
                        ))
                    })?);
                }
                "file" => {
                    let mut fields = parts[1].rsplitn(3, '|');
                    let checksum = fields.next().and_then(|value| value.parse::<u32>().ok());
                    let size = fields.next().and_then(|value| value.parse::<u64>().ok());
                    let path = fields.next().map(PathBuf::from);
                    match (path, size, checksum) {
                        (Some(path), Some(size), Some(checksum)) => {
                            files.push(CheckpointFileEntry {
                                path,
                                size,
                                checksum,
                            })
                        }
                        _ => {
                            return Err(StorageError::deserialize_error(
                                "Invalid file entry in checkpoint metadata".to_string(),
                            ));
                        }
                    }
                }
                _ => {}
            }
        }

        let format_version = format_version.ok_or_else(|| {
            StorageError::deserialize_error(
                "Missing format_version in checkpoint metadata".to_string(),
            )
        })?;
        if format_version != CHECKPOINT_FORMAT_VERSION
            && format_version != INCREMENTAL_CHECKPOINT_FORMAT_VERSION
        {
            return Err(StorageError::deserialize_error(format!(
                "Unsupported checkpoint format version: {}",
                format_version
            )));
        }
        if files.is_empty() {
            return Err(StorageError::deserialize_error(
                "Checkpoint metadata contains no files".to_string(),
            ));
        }

        let checkpoint_id = checkpoint_id.ok_or_else(|| {
            StorageError::deserialize_error(
                "Missing checkpoint_id in checkpoint metadata".to_string(),
            )
        })?;
        let lsn = lsn.ok_or_else(|| {
            StorageError::deserialize_error("Missing wal_lsn in checkpoint metadata".to_string())
        })?;

        Ok((
            CheckpointInfo {
                checkpoint_id,
                lsn: Lsn::new(lsn),
                timestamp: timestamp.unwrap_or(0),
            },
            files,
        ))
    }
}
