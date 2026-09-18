//! Edge write-ahead log: logical redo for committed but uncheckpointed writes.
//!
//! One log per edge table directory (`edge_wal.bin`). Commits append logical
//! operations before returning success; checkpoints truncate the log after the
//! new snapshot is durable. Recovery loads the checkpoint base then replays
//! the log in order; replay is idempotent so a repeated replay yields the
//! same state. A torn tail fails the load instead of entering service with a
//! partial prefix. Single format version, old versions are rejected.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use graphdb_core::{StorageError, StorageResult, Value};
use graphdb_core::types::Timestamp;

pub(crate) const EDGE_WAL_VERSION: u32 = 1;

pub(crate) fn wal_path(dir: &Path) -> PathBuf {
    dir.join("edge_wal.bin")
}

/// Logical redo operation for one committed edge write.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) enum EdgeWalOp {
    Insert {
        src: u32,
        dst: u32,
        rank: i64,
        properties: Vec<(String, Value)>,
        create_ts: Timestamp,
    },
    Delete {
        src: u32,
        dst: u32,
        rank: i64,
        delete_ts: Timestamp,
    },
    PropertyUpdate {
        src: u32,
        dst: u32,
        rank: i64,
        prop_name: String,
        value: Value,
        ts: Timestamp,
    },
    SchemaAdd {
        name: String,
        data_type: graphdb_core::DataType,
        nullable: bool,
        default: Option<Value>,
    },
    SchemaDrop {
        name: String,
    },
}

/// Append `ops` to the table log, creating it with a version header when
/// missing. The file is fsynced before returning so a returned commit is
/// durable; commit success and log durability share one atomic point.
pub(crate) fn append_ops(dir: &Path, ops: &[EdgeWalOp]) -> StorageResult<()> {
    if ops.is_empty() {
        return Ok(());
    }
    std::fs::create_dir_all(dir)?;
    let path = wal_path(dir);
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| StorageError::io_error(format!("Failed to open edge WAL: {}", e)))?;
    if file
        .metadata()
        .map(|meta| meta.len())
        .unwrap_or(0)
        == 0
    {
        file.write_all(&EDGE_WAL_VERSION.to_le_bytes())
            .map_err(|e| StorageError::io_error(format!("Failed to write edge WAL header: {}", e)))?;
    }
    for op in ops {
        let bytes = postcard::to_allocvec(op)
            .map_err(|e| StorageError::serialize_error(e.to_string()))?;
        file.write_all(&(bytes.len() as u64).to_le_bytes())
            .map_err(|e| StorageError::io_error(format!("Failed to write edge WAL entry: {}", e)))?;
        file.write_all(&bytes)
            .map_err(|e| StorageError::io_error(format!("Failed to write edge WAL entry: {}", e)))?;
    }
    file.sync_all()
        .map_err(|e| StorageError::io_error(format!("Failed to sync edge WAL: {}", e)))?;
    Ok(())
}

/// Read the log operations in order. A missing log reads as empty. A version
/// mismatch or a torn trailing entry fails closed so recovery never enters
/// service with a partial prefix.
pub(crate) fn read_ops(dir: &Path) -> StorageResult<Vec<EdgeWalOp>> {
    let path = wal_path(dir);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let mut file = std::fs::File::open(&path)
        .map_err(|e| StorageError::io_error(format!("Failed to open edge WAL: {}", e)))?;
    let mut version_bytes = [0u8; 4];
    file.read_exact(&mut version_bytes).map_err(|_| {
        StorageError::deserialize_error("edge WAL too short for version".to_string())
    })?;
    let version = u32::from_le_bytes(version_bytes);
    if version != EDGE_WAL_VERSION {
        return Err(StorageError::deserialize_error(format!(
            "unsupported edge WAL version: {}",
            version
        )));
    }
    let mut ops = Vec::new();
    loop {
        let mut len_bytes = [0u8; 8];
        match file.read_exact(&mut len_bytes) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => {
                return Err(StorageError::deserialize_error(format!(
                    "edge WAL entry length unreadable: {}",
                    e
                )));
            }
        }
        let len = u64::from_le_bytes(len_bytes) as usize;
        if len == 0 || len > 64 * 1024 * 1024 {
            return Err(StorageError::deserialize_error(format!(
                "edge WAL entry has invalid length: {}",
                len
            )));
        }
        let mut data = vec![0u8; len];
        file.read_exact(&mut data).map_err(|_| {
            StorageError::deserialize_error("torn edge WAL tail entry".to_string())
        })?;
        let op: EdgeWalOp = postcard::from_bytes(&data)
            .map_err(|e| StorageError::deserialize_error(format!("edge WAL entry corrupt: {}", e)))?;
        ops.push(op);
    }
    let mut trailer = [0u8; 1];
    if file.read(&mut trailer).unwrap_or(0) != 0 {
        return Err(StorageError::deserialize_error(
            "unexpected trailing data in edge WAL".to_string(),
        ));
    }
    Ok(ops)
}

/// Delete the log after the checkpoint covering its entries is durable.
pub(crate) fn truncate(dir: &Path) -> StorageResult<()> {
    let path = wal_path(dir);
    if path.exists() {
        std::fs::remove_file(&path)
            .map_err(|e| StorageError::io_error(format!("Failed to truncate edge WAL: {}", e)))?;
    }
    Ok(())
}
