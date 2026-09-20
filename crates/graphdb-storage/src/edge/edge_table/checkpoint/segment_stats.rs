//! Segment-statistics snapshot persistence.

use super::super::core::EdgeStore;
use super::layout::{file_bytes, segment_stats_path};
use graphdb_core::{StorageError, StorageResult};
use std::path::Path;

impl EdgeStore {
    /// Persist the checkpoint-collected segment statistics snapshot.
    ///
    /// Written on every checkpoint after the collection step so scans prune
    /// from the same epoch as the topology and property shards. The snapshot
    /// shares the manifest commit point: it lands in a shadow file before
    /// the metadata commit, and a crash before the manifest publish leaves
    /// only the previous consistent snapshot.
    pub(crate) fn flush_segment_stats(
        &self,
        dir: &Path,
        page_size: usize,
        level: i32,
    ) -> StorageResult<u64> {
        use super::super::stats::encode_segment_snapshot;
        let snapshot = encode_segment_snapshot(&self.segment_stats);
        let mut payload = Vec::new();
        crate::persistence::write_header_to(
            &mut payload,
            crate::persistence::section::EDGE_SEGMENT_STATS,
        )
        .map_err(|e| {
            StorageError::io_error(format!("Failed to write segment stats header: {}", e))
        })?;
        payload.extend_from_slice(&(snapshot.len() as u64).to_le_bytes());
        payload.extend_from_slice(&snapshot);
        let path = segment_stats_path(dir);
        super::super::persistence::write_pages_to_file(
            &path,
            &payload,
            page_size,
            level,
            self.segment_stats.len() as u32,
        )?;
        Ok(file_bytes(&path))
    }

    /// Load the segment-statistics snapshot, failing closed on section
    /// or trailing mismatches. A missing snapshot fails the load,
    /// never rebuilt silently.
    pub(crate) fn load_segment_stats(&mut self, dir: &Path) -> StorageResult<()> {
        use super::super::stats::decode_segment_snapshot;
        use std::io::Read as _;
        let path = segment_stats_path(dir);
        let (raw, _) = super::super::persistence::read_pages_from_file(&path).map_err(|e| {
            StorageError::deserialize_error(format!(
                "missing segment statistics snapshot at {}: {}",
                path.display(),
                e
            ))
        })?;
        let mut cursor = &raw[..];
        let mut header_buf = [0u8; crate::persistence::HEADER_SIZE];
        cursor.read_exact(&mut header_buf)?;
        {
            let mut slice = &header_buf[..];
            let sid = crate::persistence::read_header(&mut slice)?;
            if sid != crate::persistence::section::EDGE_SEGMENT_STATS {
                return Err(StorageError::deserialize_error(format!(
                    "unexpected section id in segment stats: expected {:#06x}, got {:#06x}",
                    crate::persistence::section::EDGE_SEGMENT_STATS,
                    sid
                )));
            }
        }
        let mut len_bytes = [0u8; 8];
        cursor.read_exact(&mut len_bytes)?;
        let len = u64::from_le_bytes(len_bytes) as usize;
        let mut data = vec![0u8; len];
        cursor.read_exact(&mut data)?;
        if !cursor.is_empty() {
            return Err(StorageError::deserialize_error(
                "unexpected trailing data in segment stats".to_string(),
            ));
        }
        let stats = decode_segment_snapshot(&data)?;
        self.restore_segment_stats(stats);
        Ok(())
    }
}
