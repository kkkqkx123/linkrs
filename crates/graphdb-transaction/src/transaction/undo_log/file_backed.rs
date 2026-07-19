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
use std::io::{BufReader, Read, Seek, SeekFrom, Write};
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
            threshold: config.memory_overflow_threshold,
        }
    }

    pub fn add(&mut self, entry: UndoLogEntry) {
        self.buffer.push(entry);
        self.total_entries += 1;

        if self.buffer.len() >= self.threshold {
            self.spill_to_file();
        }
    }

    pub fn is_empty(&self) -> bool {
        self.total_entries == 0
    }

    pub fn len(&self) -> usize {
        self.total_entries
    }

    pub fn clear(&mut self) {
        self.buffer.clear();
        self.block_offsets.clear();
        if let Some(ref mut f) = self.file {
            let _ = f.set_len(0);
            let _ = f.seek(SeekFrom::Start(0));
        }
        self.total_entries = 0;
    }

    /// Pop the newest entry (LIFO order).
    pub fn pop(&mut self) -> Option<UndoLogEntry> {
        if let Some(entry) = self.buffer.pop() {
            self.total_entries = self.total_entries.saturating_sub(1);
            Some(entry)
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
        while let Some(entry) = self.pop() {
            entry.undo(graph, ts)?;
        }
        Ok(())
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
            let entry = self.pop().ok_or_else(|| {
                crate::core::types::UndoLogError::UndoFailed(
                    "Unexpected: not enough entries during undo".to_string(),
                )
            })?;
            entry.undo(graph, ts)?;
        }

        Ok(())
    }

    /// Serialize the current buffer to a new block at the end of the file.
    fn spill_to_file(&mut self) {
        let block_data = serialize_block(&self.buffer);
        if block_data.is_empty() {
            return;
        }

        if self.file.is_none() {
            if let Err(e) = self.create_file() {
                log::error!("Failed to create undo spill file: {}", e);
                return;
            }
        }

        let file = self.file.as_mut().expect("file just created");

        // Compute new block offset (end of current file)
        let new_offset = file.seek(SeekFrom::End(0)).unwrap_or(0);

        // Append new block
        if file.write_all(&block_data).is_err() {
            return;
        }

        self.block_offsets.push(new_offset);
        self.buffer.clear();
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
    fn pop_from_file(&mut self) -> Option<UndoLogEntry> {
        let file = self.file.as_mut()?;

        let last_idx = self.block_offsets.len().checked_sub(1)?;
        let last_offset = self.block_offsets[last_idx];

        // Read last block
        if file.seek(SeekFrom::Start(last_offset)).is_err() {
            return None;
        }

        let mut count_buf = [0u8; 8];
        if file.read_exact(&mut count_buf).is_err() {
            return None;
        }
        let entry_count = u64::from_le_bytes(count_buf) as usize;

        let mut entries = Vec::with_capacity(entry_count);
        for _ in 0..entry_count {
            if file.read_exact(&mut count_buf).is_err() {
                break;
            }
            let len = u64::from_le_bytes(count_buf) as usize;
            let mut entry_buf = vec![0u8; len];
            if file.read_exact(&mut entry_buf).is_err() {
                break;
            }
            if let Ok(entry) = postcard::from_bytes(&entry_buf) {
                entries.push(entry);
            }
        }

        if entries.is_empty() {
            return None;
        }

        // The newest entry is the last one in the block
        let newest = entries.pop().expect("entries not empty");

        // Remove the consumed block from tracking — its remaining entries
        // (if any) are now in the buffer. This prevents double-counting.
        self.block_offsets.pop();

        // Remaining entries stay in forward order so Vec::pop() returns
        // the next-newest entry first (LIFO).
        self.buffer = entries;

        // Truncate file to remove the consumed block
        let truncate_to = if self.block_offsets.is_empty() {
            0
        } else {
            let prev_last = self.block_offsets[self.block_offsets.len() - 1];
            prev_last + block_size_at(file, prev_last)
        };
        file.set_len(truncate_to).ok();
        file.seek(SeekFrom::Start(truncate_to)).ok();

        self.total_entries = self.total_entries.saturating_sub(1);
        Some(newest)
    }
}

/// Compute the byte size of a block at `offset`.
fn block_size_at(file: &File, offset: u64) -> u64 {
    let mut reader = BufReader::new(file);
    if reader.seek(SeekFrom::Start(offset)).is_err() {
        return 0;
    }
    let mut buf = [0u8; 8];
    if reader.read_exact(&mut buf).is_err() {
        return 0;
    }
    let entry_count = u64::from_le_bytes(buf) as usize;

    let mut size = 8u64;
    for _ in 0..entry_count {
        if reader.read_exact(&mut buf).is_err() {
            return size;
        }
        let len = u64::from_le_bytes(buf) as usize;
        size += 8 + len as u64;
        let mut skip = vec![0u8; len];
        if reader.read_exact(&mut skip).is_err() {
            return size;
        }
    }
    size
}

/// Serialize entries into a block: num_entries(u64) + entries (length-prefixed).
fn serialize_block(entries: &[UndoLogEntry]) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.extend_from_slice(&(entries.len() as u64).to_le_bytes());
    for entry in entries {
        match postcard::to_stdvec(entry) {
            Ok(bytes) => {
                buf.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
                buf.extend_from_slice(&bytes);
            }
            Err(e) => {
                log::error!("Failed to serialize undo entry: {}", e);
            }
        }
    }
    buf
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
        ColumnId, EdgeDeletionContext, EdgeIdentifier, EdgeKey,
        PropertyValue, UndoLogError, VertexIdentifier,
    };
    use crate::transaction::wal::{LabelId, VertexId};

    struct MockUndoTarget;

    impl UndoTarget for MockUndoTarget {
        fn delete_vertex_type(&self, _label: LabelId) -> UndoLogResult<()> {
            Ok(())
        }

        fn delete_edge_type(&self, _edge_key: EdgeKey) -> UndoLogResult<()> {
            Ok(())
        }

        fn delete_vertex(
            &self,
            _vertex: VertexIdentifier,
            _ts: Timestamp,
        ) -> UndoLogResult<()> {
            Ok(())
        }

        fn delete_edge(&self, _edge_ctx: EdgeDeletionContext) -> UndoLogResult<()> {
            Ok(())
        }

        fn undo_update_vertex_property(
            &self,
            _vertex: VertexIdentifier,
            _col_id: ColumnId,
            _value: PropertyValue,
            _ts: Timestamp,
        ) -> UndoLogResult<()> {
            Ok(())
        }

        fn undo_update_edge_property(
            &self,
            _edge_id: EdgeIdentifier,
            _oe_offset: i32,
            _ie_offset: i32,
            _col_id: ColumnId,
            _value: PropertyValue,
            _ts: Timestamp,
        ) -> UndoLogResult<()> {
            Ok(())
        }

        fn revert_delete_vertex(
            &self,
            _vertex: VertexIdentifier,
            _ts: Timestamp,
        ) -> UndoLogResult<()> {
            Ok(())
        }

        fn revert_delete_edge(
            &self,
            _edge_ctx: EdgeDeletionContext,
        ) -> UndoLogResult<()> {
            Ok(())
        }

        fn revert_delete_vertex_properties(
            &self,
            _label_name: &str,
            _prop_names: &[String],
        ) -> UndoLogResult<()> {
            Ok(())
        }

        fn revert_delete_edge_properties(
            &self,
            _src_label: &str,
            _dst_label: &str,
            _edge_label: &str,
            _prop_names: &[String],
        ) -> UndoLogResult<()> {
            Ok(())
        }

        fn revert_delete_vertex_label(&self, _label_name: &str) -> UndoLogResult<()> {
            Ok(())
        }

        fn revert_delete_edge_label(
            &self,
            _src_label: &str,
            _dst_label: &str,
            _edge_label: &str,
        ) -> UndoLogResult<()> {
            Ok(())
        }

        fn revert_rename_vertex_properties(
            &self,
            _label_name: &str,
            _current_names: &[String],
            _original_names: &[String],
        ) -> UndoLogResult<()> {
            Ok(())
        }

        fn revert_rename_edge_properties(
            &self,
            _src_label: &str,
            _dst_label: &str,
            _edge_label: &str,
            _current_names: &[String],
            _original_names: &[String],
        ) -> UndoLogResult<()> {
            Ok(())
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
            log.add(make_entry(i));
        }

        assert_eq!(log.len(), 10);
        assert!(!log.is_empty());

        let target = MockUndoTarget;
        log.execute_undo(&target, 1).expect("Undo failed");
        assert!(log.is_empty());
        assert_eq!(log.len(), 0);
    }

    #[test]
    fn test_file_backed_clear() {
        let config = UndoLogConfig {
            memory_overflow_threshold: 4,
        };
        let mut log = FileBackedUndoLog::new(config);

        for i in 0..10 {
            log.add(make_entry(i));
        }

        log.clear();
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
            log.add(make_entry(i));
        }

        let target = MockUndoTarget;
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
            log.add(make_entry(i));
        }

        let target = MockUndoTarget;
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
            log.add(make_entry(i));
        }

        // Pop should return newest first (LIFO): 9, 8, 7, ..., 0
        for expected_id in (0..10).rev() {
            let entry = log.pop().unwrap();
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
            log.add(make_entry(i));
        }

        let target = MockUndoTarget;
        log.execute_undo(&target, 1).expect("Undo failed");
        assert!(log.is_empty());
    }
}
