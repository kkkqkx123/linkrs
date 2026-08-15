//! File-backed undo log storage.
//!
//! When undo log entries exceed a configurable memory threshold, entries are
//! spilled to a temporary file in "blocks". On abort, entries are replayed in
//! LIFO order. On commit/abort completion, the temp file is deleted via `Drop`.
//!
//! ## Algorithm
//!
//! Entries accumulate in an in-memory `buffer`. When the buffer reaches the
//! threshold, it is serialized as a new block at the end of the temp file, and
//! the buffer is cleared. Block offsets are tracked in memory.
//!
//! `pop()` returns from the buffer first (newest). When the buffer is empty but
//! file blocks remain, the last block is read into the buffer in reversed order
//! (so pop() continues to return newest-first). The block is truncated from
//! the file. This keeps memory bounded to at most `threshold` entries resident.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;

use crate::core::types::UndoLogResult;
use crate::transaction::undo_log::{UndoLogEntry, UndoTarget};
use crate::transaction::wal::Timestamp;

/// Configuration for file-backed undo log.
#[derive(Debug, Clone)]
pub struct UndoLogConfig {
    /// Maximum number of entries to keep in memory before spilling to disk.
    pub memory_overflow_threshold: usize,
}

impl Default for UndoLogConfig {
    fn default() -> Self {
        Self {
            memory_overflow_threshold: 10_000,
        }
    }
}

/// File-backed undo log storage with memory buffer + temp file spill.
pub struct FileBackedUndoLog {
    /// In-memory buffer for the newest entries.
    buffer: Vec<UndoLogEntry>,
    /// Optional temp file holding spilled blocks.
    file: Option<File>,
    /// Path to temp file (held for cleanup on drop).
    file_path: Option<PathBuf>,
    /// Byte offset of each block in the file.
    block_offsets: Vec<u64>,
    /// Total entries across buffer and file.
    total_entries: usize,
    /// Memory threshold before spilling.
    threshold: usize,
}

impl FileBackedUndoLog {
    pub fn new(config: UndoLogConfig) -> Self {
        Self {
            buffer: Vec::new(),
            file: None,
            file_path: None,
            block_offsets: Vec::new(),
            total_entries: 0,
            // A zero threshold would spill every append and makes the configuration
            // unusable as a memory bound. Treat it as the smallest valid threshold.
            threshold: config.memory_overflow_threshold.max(1),
        }
    }

    pub fn add(&mut self, entry: UndoLogEntry) -> UndoLogResult<()> {
        self.buffer.push(entry);
        self.total_entries += 1;

        if self.buffer.len() >= self.threshold {
            self.spill_to_file()?;
        }

        Ok(())
    }

    pub fn is_empty(&self) -> bool {
        self.total_entries == 0
    }

    pub fn len(&self) -> usize {
        self.total_entries
    }

    pub fn clear(&mut self) -> UndoLogResult<()> {
        if let Some(ref mut f) = self.file {
            f.set_len(0).map_err(|error| {
                crate::core::types::UndoLogError::UndoFailed(format!(
                    "Failed to clear undo spill file: {error}"
                ))
            })?;
        }
        self.buffer.clear();
        self.block_offsets.clear();
        self.total_entries = 0;
        Ok(())
    }

    /// Pop the newest entry (LIFO order).
    pub fn pop(&mut self) -> UndoLogResult<Option<UndoLogEntry>> {
        if let Some(entry) = self.buffer.pop() {
            self.total_entries = self.total_entries.saturating_sub(1);
            Ok(Some(entry))
        } else {
            self.pop_from_file()
        }
    }

    /// Execute all undo entries in LIFO order.
    pub fn execute_undo<T: UndoTarget + ?Sized>(
        &mut self,
        graph: &T,
        ts: Timestamp,
    ) -> UndoLogResult<()> {
        while self.execute_next(graph, ts)? {}
        Ok(())
    }

    fn execute_next<T: UndoTarget + ?Sized>(
        &mut self,
        graph: &T,
        ts: Timestamp,
    ) -> UndoLogResult<bool> {
        let Some(entry) = self.pop()? else {
            return Ok(false);
        };

        match entry.undo(graph, ts) {
            Ok(()) => Ok(true),
            Err(error) => {
                // Keep the failed entry available for diagnostics or a controlled retry.
                self.buffer.push(entry);
                self.total_entries += 1;
                Err(error)
            }
        }
    }

    /// Execute undo entries from `start_index` (0-based) onward, preserving
    /// entries before that index.
    pub fn execute_undo_from_index<T: UndoTarget + ?Sized>(
        &mut self,
        graph: &T,
        ts: Timestamp,
        start_index: usize,
    ) -> UndoLogResult<()> {
        if start_index > self.total_entries {
            return Err(crate::core::types::UndoLogError::UndoFailed(format!(
                "Invalid undo log rollback index: {}, undo log length: {}",
                start_index, self.total_entries
            )));
        }

        let entries_to_undo = self.total_entries - start_index;

        for _ in 0..entries_to_undo {
            if !self.execute_next(graph, ts)? {
                return Err(crate::core::types::UndoLogError::UndoFailed(
                    "Unexpected: not enough entries during undo".to_string(),
                ));
            }
        }

        Ok(())
    }

    /// Serialize the current buffer to a new block at the end of the file.
    fn spill_to_file(&mut self) -> UndoLogResult<()> {
        let block_data = serialize_block(&self.buffer)?;

        if self.file.is_none() {
            self.create_file().map_err(|error| {
                crate::core::types::UndoLogError::UndoFailed(format!(
                    "Failed to create undo spill file: {error}"
                ))
            })?;
        }

        let file = self.file.as_mut().ok_or_else(|| {
            crate::core::types::UndoLogError::UndoFailed(
                "Undo spill file was not initialized".to_string(),
            )
        })?;

        // Compute new block offset (end of current file)
        let new_offset = file.seek(SeekFrom::End(0)).map_err(|error| {
            crate::core::types::UndoLogError::UndoFailed(format!(
                "Failed to seek undo spill file: {error}"
            ))
        })?;

        // Append new block
        file.write_all(&block_data).map_err(|error| {
            let _ = file.set_len(new_offset);
            crate::core::types::UndoLogError::UndoFailed(format!(
                "Failed to write undo spill block: {error}"
            ))
        })?;

        self.block_offsets.push(new_offset);
        self.buffer.clear();
        Ok(())
    }

    fn create_file(&mut self) -> std::io::Result<()> {
        let tmp = tempfile::NamedTempFile::new()?;
        let path = tmp.path().to_path_buf();
        let file = tmp.keep()?.0;
        self.file_path = Some(path);
        self.file = Some(file);
        Ok(())
    }

    /// Pop the newest entry from the file (last block, last entry).
    fn pop_from_file(&mut self) -> UndoLogResult<Option<UndoLogEntry>> {
        let Some(file) = self.file.as_mut() else {
            return Ok(None);
        };

        let Some(last_idx) = self.block_offsets.len().checked_sub(1) else {
            return Ok(None);
        };
        let last_offset = self.block_offsets[last_idx];

        if self.total_entries == 0 {
            return Err(crate::core::types::UndoLogError::UndoFailed(
                "Undo log has a file block but no entries".to_string(),
            ));
        }

        // Read last block
        file.seek(SeekFrom::Start(last_offset)).map_err(|error| {
            crate::core::types::UndoLogError::UndoFailed(format!(
                "Failed to seek undo spill block: {error}"
            ))
        })?;

        let file_len = file
            .metadata()
            .map_err(|error| {
                crate::core::types::UndoLogError::UndoFailed(format!(
                    "Failed to inspect undo spill file: {error}"
                ))
            })?
            .len();

        let mut count_buf = [0u8; 8];
        file.read_exact(&mut count_buf).map_err(|error| {
            crate::core::types::UndoLogError::UndoFailed(format!(
                "Failed to read undo spill block header: {error}"
            ))
        })?;
        let entry_count = u64::from_le_bytes(count_buf);
        if entry_count == 0 {
            return Err(crate::core::types::UndoLogError::UndoFailed(
                "Undo spill block contains no entries".to_string(),
            ));
        }

        let entry_start = file.stream_position().map_err(|error| {
            crate::core::types::UndoLogError::UndoFailed(format!(
                "Failed to locate undo spill payload: {error}"
            ))
        })?;
        let remaining = file_len.saturating_sub(entry_start);
        if entry_count > remaining / 8 {
            return Err(crate::core::types::UndoLogError::UndoFailed(
                "Undo spill block has an invalid entry count".to_string(),
            ));
        }

        let mut entries = Vec::new();
        for _ in 0..entry_count {
            file.read_exact(&mut count_buf).map_err(|error| {
                crate::core::types::UndoLogError::UndoFailed(format!(
                    "Failed to read undo entry length: {error}"
                ))
            })?;
            let len_u64 = u64::from_le_bytes(count_buf);
            let entry_start = file.stream_position().map_err(|error| {
                crate::core::types::UndoLogError::UndoFailed(format!(
                    "Failed to locate undo entry: {error}"
                ))
            })?;
            if len_u64 > file_len.saturating_sub(entry_start) {
                return Err(crate::core::types::UndoLogError::UndoFailed(
                    "Undo spill entry exceeds the file boundary".to_string(),
                ));
            }
            let len = usize::try_from(len_u64).map_err(|_| {
                crate::core::types::UndoLogError::UndoFailed(
                    "Undo spill entry is too large for this platform".to_string(),
                )
            })?;
            let mut entry_buf = vec![0u8; len];
            file.read_exact(&mut entry_buf).map_err(|error| {
                crate::core::types::UndoLogError::UndoFailed(format!(
                    "Failed to read undo entry: {error}"
                ))
            })?;
            let entry = postcard::from_bytes(&entry_buf).map_err(|error| {
                crate::core::types::UndoLogError::UndoFailed(format!(
                    "Failed to deserialize undo entry: {error}"
                ))
            })?;
            entries.push(entry);
        }

        let block_end = file.stream_position().map_err(|error| {
            crate::core::types::UndoLogError::UndoFailed(format!(
                "Failed to locate undo spill block end: {error}"
            ))
        })?;
        if block_end != file_len {
            return Err(crate::core::types::UndoLogError::UndoFailed(
                "Undo spill block contains trailing data".to_string(),
            ));
        }

        // The newest entry is the last one in the block
        let newest = entries.pop().ok_or_else(|| {
            crate::core::types::UndoLogError::UndoFailed(
                "Undo spill block contains no decoded entries".to_string(),
            )
        })?;

        // Truncate only after the whole block has been decoded successfully.
        file.set_len(last_offset).map_err(|error| {
            crate::core::types::UndoLogError::UndoFailed(format!(
                "Failed to remove consumed undo spill block: {error}"
            ))
        })?;

        // Remove the consumed block from tracking — its remaining entries
        // (if any) are now in the buffer. This prevents double-counting.
        self.block_offsets.pop();

        // Remaining entries stay in forward order so Vec::pop() returns
        // the next-newest entry first (LIFO).
        self.buffer = entries;

        self.total_entries = self.total_entries.saturating_sub(1);
        Ok(Some(newest))
    }
}

/// Serialize entries into a block: num_entries(u64) + entries (length-prefixed).
fn serialize_block(entries: &[UndoLogEntry]) -> UndoLogResult<Vec<u8>> {
    let mut buf = Vec::new();
    buf.extend_from_slice(&0u64.to_le_bytes());
    let mut serialized_count = 0u64;
    for entry in entries {
        let bytes = postcard::to_stdvec(entry).map_err(|error| {
            crate::core::types::UndoLogError::UndoFailed(format!(
                "Failed to serialize undo entry: {error}"
            ))
        })?;
        buf.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
        buf.extend_from_slice(&bytes);
        serialized_count += 1;
    }
    buf[..8].copy_from_slice(&serialized_count.to_le_bytes());
    Ok(buf)
}

impl Drop for FileBackedUndoLog {
    fn drop(&mut self) {
        if let Some(path) = self.file_path.take() {
            std::fs::remove_file(&path).ok();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::{
        ColumnId, EdgeDeletionContext, EdgeIdentifier, EdgeKey, UndoLogError, VertexIdentifier,
    };
    use crate::transaction::undo_log::UpdateVertexPropUndo;
    use crate::transaction::wal::{LabelId, VertexId};
    use std::sync::Mutex;

    struct MockUndoTarget {
        calls: Mutex<Vec<String>>,
        fail: bool,
    }

    impl MockUndoTarget {
        fn new() -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                fail: false,
            }
        }

        fn failing() -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                fail: true,
            }
        }

        fn record(&self, call: String) -> UndoLogResult<()> {
            self.calls
                .lock()
                .expect("Undo target call log was poisoned")
                .push(call);
            if self.fail {
                Err(UndoLogError::UndoFailed("mock undo failure".to_string()))
            } else {
                Ok(())
            }
        }

        fn calls(&self) -> Vec<String> {
            self.calls
                .lock()
                .expect("Undo target call log was poisoned")
                .clone()
        }
    }

    impl UndoTarget for MockUndoTarget {
        fn delete_vertex_type(&self, label: LabelId) -> UndoLogResult<()> {
            self.record(format!("delete_vertex_type:{label}"))
        }

        fn delete_edge_type(&self, edge_key: EdgeKey) -> UndoLogResult<()> {
            self.record(format!("delete_edge_type:{edge_key:?}"))
        }

        fn delete_vertex(&self, vertex: VertexIdentifier, ts: Timestamp) -> UndoLogResult<()> {
            self.record(format!("delete_vertex:{vertex:?}:{ts}"))
        }

        fn delete_edge(&self, edge_ctx: EdgeDeletionContext) -> UndoLogResult<()> {
            self.record(format!("delete_edge:{edge_ctx:?}"))
        }

        fn undo_update_vertex_property(
            &self,
            vertex: VertexIdentifier,
            col_id: ColumnId,
            value: crate::core::Value,
            ts: Timestamp,
        ) -> UndoLogResult<()> {
            self.record(format!(
                "undo_update_vertex_property:{vertex:?}:{col_id:?}:{value:?}:{ts}"
            ))
        }

        fn undo_update_edge_property(
            &self,
            edge_id: EdgeIdentifier,
            col_id: ColumnId,
            value: crate::core::Value,
            ts: Timestamp,
        ) -> UndoLogResult<()> {
            self.record(format!(
                "undo_update_edge_property:{edge_id:?}:{col_id:?}:{value:?}:{ts}"
            ))
        }

        fn revert_delete_vertex(
            &self,
            vertex: VertexIdentifier,
            ts: Timestamp,
        ) -> UndoLogResult<()> {
            self.record(format!("revert_delete_vertex:{vertex:?}:{ts}"))
        }

        fn revert_delete_edge(&self, edge_ctx: EdgeDeletionContext) -> UndoLogResult<()> {
            self.record(format!("revert_delete_edge:{edge_ctx:?}"))
        }

        fn revert_delete_vertex_properties(
            &self,
            label_name: &str,
            prop_names: &[String],
        ) -> UndoLogResult<()> {
            self.record(format!(
                "revert_delete_vertex_properties:{label_name}:{prop_names:?}"
            ))
        }

        fn revert_delete_edge_properties(
            &self,
            src_label: &str,
            dst_label: &str,
            edge_label: &str,
            prop_names: &[String],
        ) -> UndoLogResult<()> {
            self.record(format!(
                "revert_delete_edge_properties:{src_label}:{dst_label}:{edge_label}:{prop_names:?}"
            ))
        }

        fn revert_delete_vertex_label(&self, label_name: &str) -> UndoLogResult<()> {
            self.record(format!("revert_delete_vertex_label:{label_name}"))
        }

        fn revert_delete_edge_label(
            &self,
            src_label: &str,
            dst_label: &str,
            edge_label: &str,
        ) -> UndoLogResult<()> {
            self.record(format!(
                "revert_delete_edge_label:{src_label}:{dst_label}:{edge_label}"
            ))
        }

        fn revert_rename_vertex_properties(
            &self,
            label_name: &str,
            current_names: &[String],
            original_names: &[String],
        ) -> UndoLogResult<()> {
            self.record(format!(
                "revert_rename_vertex_properties:{label_name}:{current_names:?}:{original_names:?}"
            ))
        }

        fn revert_rename_edge_properties(
            &self,
            src_label: &str,
            dst_label: &str,
            edge_label: &str,
            current_names: &[String],
            original_names: &[String],
        ) -> UndoLogResult<()> {
            self.record(format!(
                "revert_rename_edge_properties:{src_label}:{dst_label}:{edge_label}:{current_names:?}:{original_names:?}"
            ))
        }
    }

    fn make_entry(id: i64) -> UndoLogEntry {
        UndoLogEntry::InsertVertex(crate::transaction::undo_log::InsertVertexUndo {
            v_label: 1,
            vid: VertexId::from_int64(id),
        })
    }

    #[test]
    fn test_file_backed_basic() {
        let config = UndoLogConfig {
            memory_overflow_threshold: 4,
        };
        let mut log = FileBackedUndoLog::new(config);

        for i in 0..10 {
            log.add(make_entry(i)).expect("Failed to append undo log");
        }

        assert_eq!(log.len(), 10);
        assert!(!log.is_empty());

        let target = MockUndoTarget::new();
        log.execute_undo(&target, 1).expect("Undo failed");
        assert!(log.is_empty());
        assert_eq!(log.len(), 0);
    }

    #[test]
    fn test_file_backed_forwards_spilled_entry_arguments() {
        let config = UndoLogConfig {
            memory_overflow_threshold: 1,
        };
        let mut log = FileBackedUndoLog::new(config);
        log.add(UndoLogEntry::UpdateVertexProp(UpdateVertexPropUndo {
            v_label: 7,
            vid: VertexId::from_int64(99),
            col_id: ColumnId(4),
            old_value: crate::core::Value::BigInt(42),
        }))
        .expect("Failed to append undo log");

        let target = MockUndoTarget::new();
        log.execute_undo(&target, 55).expect("Undo failed");

        let calls = target.calls();
        assert_eq!(calls.len(), 1);
        assert!(calls[0].contains("7"));
        assert!(calls[0].contains("99"));
        assert!(calls[0].contains("BigInt(42)"));
        assert!(calls[0].ends_with(":55"));
    }

    #[test]
    fn test_failed_undo_keeps_entry_for_retry() {
        let config = UndoLogConfig {
            memory_overflow_threshold: 2,
        };
        let mut log = FileBackedUndoLog::new(config);
        log.add(make_entry(1)).expect("Failed to append undo log");
        log.add(make_entry(2)).expect("Failed to append undo log");

        let target = MockUndoTarget::failing();
        assert!(log.execute_undo(&target, 1).is_err());
        assert_eq!(log.len(), 2);

        let entry = log
            .pop()
            .expect("Failed to pop preserved undo log")
            .expect("Expected the failed entry to remain");
        match entry {
            UndoLogEntry::InsertVertex(undo) => {
                assert_eq!(undo.vid, VertexId::from_int64(2));
            }
            _ => panic!("Expected InsertVertex"),
        }
    }

    #[test]
    fn test_corrupt_spill_block_returns_error_without_consuming_log() {
        let config = UndoLogConfig {
            memory_overflow_threshold: 1,
        };
        let mut log = FileBackedUndoLog::new(config);
        log.add(make_entry(1)).expect("Failed to append undo log");

        log.file
            .as_mut()
            .expect("Expected spill file")
            .set_len(4)
            .expect("Failed to corrupt spill file for test");

        assert!(log.pop().is_err());
        assert_eq!(log.len(), 1);
    }

    #[test]
    fn test_spill_block_trailing_data_returns_error_without_consuming_log() {
        let config = UndoLogConfig {
            memory_overflow_threshold: 1,
        };
        let mut log = FileBackedUndoLog::new(config);
        log.add(make_entry(1)).expect("Failed to append undo log");

        let file = log.file.as_mut().expect("Expected spill file");
        let file_len = file.metadata().expect("Failed to inspect spill file").len();
        file.set_len(file_len + 1)
            .expect("Failed to corrupt spill file for test");

        assert!(log.pop().is_err());
        assert_eq!(log.len(), 1);
    }

    #[test]
    fn test_file_backed_clear() {
        let config = UndoLogConfig {
            memory_overflow_threshold: 4,
        };
        let mut log = FileBackedUndoLog::new(config);

        for i in 0..10 {
            log.add(make_entry(i)).expect("Failed to append undo log");
        }

        log.clear().expect("Failed to clear undo log");
        assert!(log.is_empty());
        assert_eq!(log.len(), 0);
    }

    #[test]
    fn test_file_backed_undo_from_index() {
        let config = UndoLogConfig {
            memory_overflow_threshold: 4,
        };
        let mut log = FileBackedUndoLog::new(config);

        for i in 0..10 {
            log.add(make_entry(i)).expect("Failed to append undo log");
        }

        let target = MockUndoTarget::new();
        log.execute_undo_from_index(&target, 1, 5)
            .expect("Undo from index failed");

        assert_eq!(log.len(), 5);
    }

    #[test]
    fn test_file_backed_undo_from_invalid_index() {
        let config = UndoLogConfig {
            memory_overflow_threshold: 4,
        };
        let mut log = FileBackedUndoLog::new(config);

        for i in 0..5 {
            log.add(make_entry(i)).expect("Failed to append undo log");
        }

        let target = MockUndoTarget::new();
        let result = log.execute_undo_from_index(&target, 1, 10);
        assert!(result.is_err());
        match result {
            Err(UndoLogError::UndoFailed(_)) => {}
            _ => panic!("Expected UndoFailed error"),
        }
    }

    #[test]
    fn test_file_backed_lifo_order() {
        let config = UndoLogConfig {
            memory_overflow_threshold: 4,
        };
        let mut log = FileBackedUndoLog::new(config);

        for i in 0..10 {
            log.add(make_entry(i)).expect("Failed to append undo log");
        }

        // Pop should return newest first (LIFO): 9, 8, 7, ..., 0
        for expected_id in (0..10).rev() {
            let entry = log
                .pop()
                .expect("Failed to pop undo log")
                .expect("Expected an undo entry");
            match &entry {
                UndoLogEntry::InsertVertex(u) => {
                    assert_eq!(
                        u.vid,
                        VertexId::from_int64(expected_id),
                        "Expected entry {}, got {}",
                        expected_id,
                        u.vid
                    );
                }
                _ => panic!("Expected InsertVertex"),
            }
        }
        assert!(log.is_empty());
    }

    #[test]
    fn test_file_backed_all_in_memory() {
        // When entries never exceed threshold, everything stays in memory
        let config = UndoLogConfig {
            memory_overflow_threshold: 100,
        };
        let mut log = FileBackedUndoLog::new(config);

        for i in 0..10 {
            log.add(make_entry(i)).expect("Failed to append undo log");
        }

        let target = MockUndoTarget::new();
        log.execute_undo(&target, 1).expect("Undo failed");
        assert!(log.is_empty());
    }
}
