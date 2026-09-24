//! Column chunk: fixed-size segment within a column for chunk-local
//! encodings and point-update overlays.
//!
//! Each chunk holds a contiguous range of rows (default 65,536) with its own
//! data buffer, encoding, MVCC metadata, and compression profile. Chunking
//! lets point writes land in a row-level overlay without decoding the whole
//! column, and lets flush profile and encode each chunk independently.

use std::sync::atomic::{AtomicU64, Ordering};

use crate::encoding::{ChunkEncodingMeta, ColumnEncoding, EncodingType};
use crate::persistence::dirty_page::DirtyPageTracker;
use crate::vertex::column::chunk_encoding::{UpdateOverlay, DEFAULT_OVERLAY_CAPACITY};
use crate::vertex::column::chunk_residency::{next_tick, ChunkResidency};
use crate::vertex::column::mvcc::{RowVisibility, VersionEntry};
use graphdb_core::Value;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Default number of rows per chunk.
pub const DEFAULT_CHUNK_ROWS: usize = 65_536;

// ---------------------------------------------------------------------------
// ColumnChunk
// ---------------------------------------------------------------------------

/// A fixed-size segment within a column for chunk-local encodings and
/// point-update overlays.
///
/// Each chunk stores a contiguous range of rows `[row_offset, row_offset + row_count)`.
pub struct ColumnChunk {
    /// First row index in this chunk (absolute row index in the column).
    pub row_offset: usize,
    /// Number of active rows in this chunk (may be < capacity for the last chunk).
    pub row_count: usize,
    /// Raw data buffer (fixed-width: flat bytes; variable-width: value bytes).
    pub data: Vec<u8>,
    /// Offset array for variable-width columns (empty for fixed-width).
    pub offsets: Vec<u64>,
    /// Null bitmap (None if column is not nullable).
    pub null_bitmap: Option<bitvec::vec::BitVec<u8, bitvec::order::Lsb0>>,
    /// Per-chunk encoding (independent from column-level encoding).
    pub encoding: ColumnEncoding,
    /// Dirty page tracking for this chunk.
    pub dirty_tracker: DirtyPageTracker,
    /// MVCC version chains for rows in this chunk.
    pub version_chains: Option<Vec<Vec<VersionEntry>>>,
    /// Row-level visibility metadata for this chunk.
    pub visibility: RowVisibility,
    /// Element size for fixed-width types (0 for variable-width).
    pub element_size: usize,
    /// Row-level overwrite buffer on top of the encoded base.
    pub overlay: UpdateOverlay,
    /// Per-chunk compression metadata backing selection and update checks.
    pub encoding_meta: ChunkEncodingMeta,
    /// Overlay writes since the last encode; feeds the hot-update signal.
    pub updates_since_encode: u64,
    /// Memory residency state (resident vs evicted to disk).
    pub residency: ChunkResidency,
    /// Last access tick for eviction ordering. Atomic so point reads on the
    /// shared-reference path can stamp recency without a lock upgrade.
    pub last_access: AtomicU64,
}

impl ColumnChunk {
    /// Create a new resident chunk with the given row offset and capacity.
    pub fn new(row_offset: usize, row_count: usize, element_size: usize, nullable: bool) -> Self {
        let data = if element_size > 0 {
            vec![0u8; row_count * element_size]
        } else {
            Vec::new()
        };
        let null_bitmap = if nullable {
            Some(bitvec::vec::BitVec::with_capacity(row_count))
        } else {
            None
        };

        Self {
            row_offset,
            row_count,
            data,
            offsets: Vec::new(),
            null_bitmap,
            encoding: ColumnEncoding::None,
            dirty_tracker: DirtyPageTracker::new(0),
            version_chains: None,
            visibility: RowVisibility::new(),
            element_size,
            overlay: UpdateOverlay::new(DEFAULT_OVERLAY_CAPACITY),
            encoding_meta: ChunkEncodingMeta::default(),
            updates_since_encode: 0,
            residency: ChunkResidency::Resident,
            last_access: AtomicU64::new(0),
        }
    }

    /// Create a new variable-width chunk.
    pub fn new_variable(row_offset: usize, row_count: usize, nullable: bool) -> Self {
        let null_bitmap = if nullable {
            Some(bitvec::vec::BitVec::with_capacity(row_count))
        } else {
            None
        };

        Self {
            row_offset,
            row_count,
            data: Vec::new(),
            offsets: Vec::with_capacity(row_count),
            null_bitmap,
            encoding: ColumnEncoding::None,
            dirty_tracker: DirtyPageTracker::new(0),
            version_chains: None,
            visibility: RowVisibility::new(),
            element_size: 0,
            overlay: UpdateOverlay::new(DEFAULT_OVERLAY_CAPACITY),
            encoding_meta: ChunkEncodingMeta::default(),
            updates_since_encode: 0,
            residency: ChunkResidency::Resident,
            last_access: AtomicU64::new(0),
        }
    }

    /// Stamp recency for eviction ordering. Lock-free so shared-reference
    /// reads participate without upgrading to a write lock.
    pub fn touch(&self) {
        self.last_access.store(next_tick(), Ordering::Relaxed);
    }

    /// Effective encoding scheme: live encoding when resident, pre-evict
    /// scheme when evicted.
    pub fn evicted_encoding(&self) -> EncodingType {
        self.residency
            .effective_encoding(self.encoding.encoding_type())
    }

    /// Whether this chunk may be evicted: resident, encoded, with no
    /// unmerged overlay writes and no live version-chain entries. Raw and
    /// dirty-overlay chunks stay resident; the column layer additionally
    /// excludes chunks overlapped by column-level version chains.
    pub fn is_evictable(&self) -> bool {
        matches!(self.residency, ChunkResidency::Resident)
            && self.encoding.is_encoded()
            && self.overlay.len() == 0
            && self
                .version_chains
                .as_ref()
                .is_none_or(|chains| chains.iter().all(|chain| chain.is_empty()))
    }

    /// Write one row: in-place when the encoding absorbs it, otherwise into
    /// the overlay. Returns the decision taken.
    pub fn set_value(
        &mut self,
        local_row: u32,
        value: Option<Value>,
        data_type: &graphdb_core::DataType,
    ) -> crate::vertex::column::chunk_encoding::UpdateDecision {
        use crate::vertex::column::chunk_encoding::{can_update_in_place, UpdateDecision};
        if !self.encoding.is_encoded() {
            return UpdateDecision::InPlace;
        }
        let decision = can_update_in_place(
            &self.encoding,
            data_type,
            &self.encoding_meta,
            local_row,
            value.as_ref(),
            self.overlay.is_full(),
        );
        match decision {
            UpdateDecision::InPlace => {
                if self
                    .encoding
                    .set(local_row as usize, value.as_ref())
                    .is_ok()
                {
                    return UpdateDecision::InPlace;
                }
                self.overlay.put(local_row, value);
                self.updates_since_encode += 1;
                UpdateDecision::Overlay
            }
            UpdateDecision::Overlay | UpdateDecision::OverlayAndRecode => {
                self.overlay.put(local_row, value);
                self.updates_since_encode += 1;
                decision
            }
        }
    }

    /// Drop the overlay after a successful flush and reset the hot counter.
    pub fn clear_overlay_after_flush(&mut self) {
        self.overlay.clear();
        self.updates_since_encode = 0;
    }

    /// Whether this chunk should be re-encoded on the next flush.
    pub fn needs_recode(&self, hot_threshold: u64) -> bool {
        self.overlay.is_full() || self.updates_since_encode > hot_threshold
    }

    /// Refresh the cached compression metadata after re-encoding.
    pub fn refresh_encoding_meta(
        &mut self,
        num_values: u32,
        all_null: bool,
        min: Option<Value>,
        max: Option<Value>,
        compressed_size: u64,
        raw_size: u64,
    ) {
        self.encoding_meta.scheme = self.encoding.encoding_type();
        self.encoding_meta.num_values = num_values;
        self.encoding_meta.all_null = all_null;
        self.encoding_meta.min = min;
        self.encoding_meta.max = max;
        self.encoding_meta.compressed_size = compressed_size;
        self.encoding_meta.raw_size = raw_size;
    }
}

impl Clone for ColumnChunk {
    fn clone(&self) -> Self {
        Self {
            row_offset: self.row_offset,
            row_count: self.row_count,
            data: self.data.clone(),
            offsets: self.offsets.clone(),
            null_bitmap: self.null_bitmap.clone(),
            encoding: self.encoding.clone(),
            dirty_tracker: self.dirty_tracker.clone(),
            version_chains: self.version_chains.clone(),
            visibility: self.visibility.clone(),
            element_size: self.element_size,
            overlay: self.overlay.clone(),
            encoding_meta: self.encoding_meta.clone(),
            updates_since_encode: self.updates_since_encode,
            residency: self.residency.clone(),
            last_access: AtomicU64::new(self.last_access.load(Ordering::Relaxed)),
        }
    }
}

impl std::fmt::Debug for ColumnChunk {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ColumnChunk")
            .field("row_offset", &self.row_offset)
            .field("row_count", &self.row_count)
            .field("data_len", &self.data.len())
            .field("offsets_len", &self.offsets.len())
            .field("has_bitmap", &self.null_bitmap.is_some())
            .field("encoding", &self.encoding.encoding_type())
            .field("resident", &self.residency.is_resident())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chunk_fixed_width_create() {
        let chunk = ColumnChunk::new(0, 100, 4, true);
        assert_eq!(chunk.row_offset, 0);
        assert_eq!(chunk.row_count, 100);
        assert_eq!(chunk.data.len(), 400); // 100 * 4 bytes
        assert!(chunk.null_bitmap.is_some());
    }

    #[test]
    fn test_chunk_variable_width_create() {
        let chunk = ColumnChunk::new_variable(0, 50, false);
        assert_eq!(chunk.row_offset, 0);
        assert_eq!(chunk.row_count, 50);
        assert!(chunk.data.is_empty());
        assert!(chunk.offsets.is_empty());
        assert!(chunk.null_bitmap.is_none());
    }
}
