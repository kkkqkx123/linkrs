//! Edge write-ahead log: logical redo for committed but uncheckpointed writes.
//!
//! One log per edge table directory (`edge_wal.bin`). Commits append logical
//! operations before returning success; checkpoints truncate the log after the
//! new snapshot is durable. Recovery loads the checkpoint base then replays
//! the log in order; replay is idempotent so a repeated replay yields the
//! same state.
//!
//! Torn-tail policy: [`read_ops`] fails the load instead of entering service
//! with a partial prefix. Repair is an explicit offline step,
//! [`discard_torn_tail`], which truncates the file at the last valid entry
//! and reports how many operations survived. It must only run while no
//! writer holds the table, under the same single-writer discipline as every
//! other mutation.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use graphdb_core::types::Timestamp;
use graphdb_core::{StorageError, StorageResult, Value};

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
    SchemaRename {
        old_name: String,
        new_name: String,
    },
}

/// Append `ops` to the table log. The file is fsynced before returning so
/// a returned commit is durable; commit success and log durability share one
/// atomic point.
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
    for op in ops {
        let bytes =
            postcard::to_allocvec(op).map_err(|e| StorageError::serialize_error(e.to_string()))?;
        file.write_all(&(bytes.len() as u64).to_le_bytes())
            .map_err(|e| {
                StorageError::io_error(format!("Failed to write edge WAL entry: {}", e))
            })?;
        file.write_all(&bytes).map_err(|e| {
            StorageError::io_error(format!("Failed to write edge WAL entry: {}", e))
        })?;
    }
    file.sync_all()
        .map_err(|e| StorageError::io_error(format!("Failed to sync edge WAL: {}", e)))?;
    Ok(())
}

/// Read the log operations in order. A missing log reads as empty. A torn
/// trailing entry fails closed so recovery never enters
/// service with a partial prefix.
pub(crate) fn read_ops(dir: &Path) -> StorageResult<Vec<EdgeWalOp>> {
    use std::io::Seek as _;
    let path = wal_path(dir);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let mut file = std::fs::File::open(&path)
        .map_err(|e| StorageError::io_error(format!("Failed to open edge WAL: {}", e)))?;
    let file_len = file
        .metadata()
        .map_err(|e| StorageError::io_error(format!("Failed to stat edge WAL: {}", e)))?
        .len();
    let mut ops = Vec::new();
    loop {
        // Clean EOF breaks before any read: a failed length read below
        // always means bytes remain but no full entry does. Checking the
        // position first (instead of probing with a read) keeps even a
        // sub-length fragment from being consumed and mistaken for clean.
        let pos = file
            .stream_position()
            .map_err(|e| StorageError::io_error(format!("Failed to stat edge WAL: {}", e)))?;
        if pos == file_len {
            break;
        }
        let mut len_bytes = [0u8; 8];
        file.read_exact(&mut len_bytes).map_err(|_| {
            StorageError::deserialize_error("torn edge WAL tail entry".to_string())
        })?;
        let len = u64::from_le_bytes(len_bytes) as usize;
        if len == 0 || len > 64 * 1024 * 1024 {
            return Err(StorageError::deserialize_error(format!(
                "edge WAL entry has invalid length: {}",
                len
            )));
        }
        let mut data = vec![0u8; len];
        file.read_exact(&mut data)
            .map_err(|_| StorageError::deserialize_error("torn edge WAL tail entry".to_string()))?;
        let op: EdgeWalOp = postcard::from_bytes(&data).map_err(|e| {
            StorageError::deserialize_error(format!("edge WAL entry corrupt: {}", e))
        })?;
        ops.push(op);
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

/// Offline repair for a torn tail: truncate the log at the last valid entry
/// boundary and return the salvaged operation count.
///
/// Only run while no writer holds the table. A clean log is left untouched
/// and reports its full count; a missing log reports zero. Every discarded
/// byte is logged, never silent.
///
/// Invoked explicitly by operators, never by the load path, so normal builds
/// report no in-crate callers.
#[allow(dead_code)]
pub(crate) fn discard_torn_tail(dir: &Path) -> StorageResult<usize> {
    const MAX_ENTRY_LEN: usize = 64 * 1024 * 1024;
    let path = wal_path(dir);
    if !path.exists() {
        return Ok(0);
    }
    let bytes = std::fs::read(&path)
        .map_err(|e| StorageError::io_error(format!("Failed to read edge WAL: {}", e)))?;
    let mut offset = 0usize;
    let mut salvaged = 0usize;
    while offset < bytes.len() {
        let remaining = bytes.len() - offset;
        if remaining < 8 {
            break;
        }
        let len = u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap()) as usize;
        if len == 0 || len > MAX_ENTRY_LEN || remaining - 8 < len {
            break;
        }
        if postcard::from_bytes::<EdgeWalOp>(&bytes[offset + 8..offset + 8 + len]).is_err() {
            break;
        }
        offset += 8 + len;
        salvaged += 1;
    }
    // A non-empty tail the reader would reject (torn entry or trailing
    // garbage) is damage only past the salvaged prefix.
    if offset != bytes.len() {
        log::warn!(
            "edge WAL repair: truncating {} torn tail bytes at offset {}, salvaged {} ops",
            bytes.len() - offset,
            offset,
            salvaged,
        );
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .map_err(|e| StorageError::io_error(format!("Failed to open edge WAL: {}", e)))?;
        file.set_len(offset as u64)
            .map_err(|e| StorageError::io_error(format!("Failed to truncate edge WAL: {}", e)))?;
    }
    Ok(salvaged)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wal_ops() -> Vec<EdgeWalOp> {
        vec![
            EdgeWalOp::Insert {
                src: 0,
                dst: 1,
                rank: 0,
                properties: Vec::new(),
                create_ts: 100,
            },
            EdgeWalOp::Delete {
                src: 0,
                dst: 1,
                rank: 0,
                delete_ts: 150,
            },
        ]
    }

    #[test]
    fn torn_tail_fails_load_and_repair_salvages_prefix() {
        let dir = tempfile::tempdir().expect("temporary WAL directory");
        append_ops(dir.path(), &wal_ops()).expect("append succeeds");
        // Simulate a crashed commit: half an entry lands on disk.
        {
            use std::io::Write as _;
            let mut file = std::fs::OpenOptions::new()
                .append(true)
                .open(wal_path(dir.path()))
                .expect("WAL opens for damage injection");
            file.write_all(&[0x09, 0x00]).expect("partial length lands");
        }
        assert!(read_ops(dir.path()).is_err());

        let salvaged = discard_torn_tail(dir.path()).expect("repair succeeds");
        assert_eq!(salvaged, wal_ops().len());
        let ops = read_ops(dir.path()).expect("load succeeds after repair");
        assert_eq!(ops.len(), wal_ops().len());
    }

    #[test]
    fn clean_log_repair_is_a_noop() {
        let dir = tempfile::tempdir().expect("temporary WAL directory");
        append_ops(dir.path(), &wal_ops()).expect("append succeeds");
        let salvaged = discard_torn_tail(dir.path()).expect("repair succeeds");
        assert_eq!(salvaged, wal_ops().len());
        assert_eq!(read_ops(dir.path()).expect("load succeeds").len(), wal_ops().len());

        let empty = tempfile::tempdir().expect("temporary empty directory");
        assert_eq!(discard_torn_tail(empty.path()).expect("missing log reads zero"), 0);
    }
}
