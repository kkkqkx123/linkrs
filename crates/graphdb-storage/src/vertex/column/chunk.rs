//! Column chunk: fixed-size segment within a column for chunk-local
//! encodings and point-update overlays.
//!
//! Each chunk owns the raw row buffers for its contiguous range of rows
//! (default one zone-group of 4,096) alongside its own encoding, MVCC
//! metadata, and compression profile. Chunking lets point writes land in a
//! row-level overlay without decoding the whole column, and lets flush
//! profile and encode each chunk independently. Chunk size also bounds
//! eviction: releasing a chunk's decoded buffers externalizes exactly one
//! chunk worth of rows.
//!
//! # Latching
//!
//! A chunk is the segment latch domain: `row_offset` / `row_count` describe
//! the window and are only mutated by structural operations (growth,
//! split, shrink, load), while every per-row payload in [`ChunkState`] is
//! guarded by `state`. Point readers and writers on different chunks never
//! share a lock; structural operations take the column's chunk-vector lock
//! first, so container-before-member order holds on every path.

use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};

use bitvec::order::Lsb0;
use bitvec::vec::BitVec;
use parking_lot::{RwLock, RwLockReadGuard, RwLockWriteGuard};

use crate::encoding::{ChunkEncodingMeta, ColumnEncoding, EncodingType};
use crate::vertex::column::chunk_encoding::{overlay_capacity_for, UpdateOverlay};
use crate::vertex::column::chunk_residency::{next_tick, ChunkResidency};
use crate::vertex::column::column::ColumnInner;
use crate::vertex::column::mvcc::{RowVisibility, VersionEntry};
use crate::vertex::column::overflow::OverflowHandle;
use crate::vertex::latch_order::{Guard, RANK_SEGMENT};
use graphdb_core::{DataType, Value};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Default number of rows per chunk.
///
/// Four row-pages wide, so the eviction unit stays aligned with the
/// zone-map and dirty-page addressing (`ROWS_PER_PAGE`); a chunk is the
/// smallest range that can be released and reloaded without touching a
/// neighbour's zone bounds.
pub const DEFAULT_CHUNK_ROWS: usize = 4_096;

// ---------------------------------------------------------------------------
// ChunkState
// ---------------------------------------------------------------------------

/// Everything a chunk owns about its rows, guarded by the segment latch.
///
/// `version_chains` and `visibility` are indexed by chunk-local row; the
/// column layer translates global rows on entry so concurrent writers on
/// different chunks never touch the same entry. `overflow_rows` maps local
/// rows to side-store handles; the store itself lives at the column level
/// behind a short critical section.
pub struct ChunkState {
    /// Owned raw row buffers for this chunk's window (fixed-width flat
    /// bytes or variable-width payload plus offsets). Row `r` of the column
    /// lives at local index `r - row_offset`. Empty while the chunk is
    /// encoded or evicted; sized to `row_count` while raw.
    pub raw: ColumnInner,
    /// Per-chunk encoding (independent from column-level encoding).
    pub encoding: ColumnEncoding,
    /// Row-level overwrite buffer on top of the encoded base.
    pub overlay: UpdateOverlay,
    /// Per-chunk compression metadata backing selection and update checks.
    pub encoding_meta: ChunkEncodingMeta,
    /// Overlay writes since the last encode; feeds the hot-update signal.
    pub updates_since_encode: u64,
    /// MVCC version chains for rows in this chunk, chunk-local indexed.
    pub version_chains: Option<Vec<Vec<VersionEntry>>>,
    /// Row-level visibility metadata for this chunk, chunk-local indexed.
    pub visibility: RowVisibility,
    /// Global dirty page ids touched through this chunk.
    pub dirty_pages: BTreeSet<u32>,
    /// Rows whose payload lives in the overflow store (local row to handle).
    pub overflow_rows: HashMap<u32, OverflowHandle>,
    /// Memory residency state (resident vs evicted to disk).
    pub residency: ChunkResidency,
}

// ---------------------------------------------------------------------------
// ColumnChunk
// ---------------------------------------------------------------------------

/// A fixed-size segment within a column for chunk-local encodings and
/// point-update overlays.
///
/// Each chunk stores a contiguous range of rows `[row_offset, row_offset + row_count)`.
/// The chunk owns its raw row buffers outright: there is no column-level
/// raw store, so every row byte lives in exactly one chunk.
pub struct ColumnChunk {
    /// First row index in this chunk (absolute row index in the column).
    pub row_offset: usize,
    /// Number of active rows in this chunk (may be < capacity for the last chunk).
    pub row_count: usize,
    /// Element size for fixed-width types (0 for variable-width).
    pub element_size: usize,
    /// Segment latch guarding all per-row payload.
    pub state: RwLock<ChunkState>,
    /// Last access tick for eviction ordering. Atomic so point reads on the
    /// shared-reference path can stamp recency without a lock upgrade.
    pub last_access: AtomicU64,
}

impl ColumnChunk {
    /// Create a new resident chunk with the given row offset and capacity.
    ///
    /// The owned raw buffers start zero-filled for fixed-width types and
    /// empty for variable-width types, matching the historical fresh-chunk
    /// contents; callers fill or resize them before serving reads.
    pub fn new(row_offset: usize, row_count: usize, data_type: &DataType, nullable: bool) -> Self {
        use super::column::ColumnInner;
        use super::fixed_width::FixedWidthColumn;
        use super::variable_width::VariableWidthColumn;
        let raw = if super::is_variable_length_type(data_type) {
            ColumnInner::Variable(VariableWidthColumn::new(data_type.clone(), nullable))
        } else {
            let mut fixed = FixedWidthColumn::new(data_type.clone(), nullable);
            fixed.data = vec![0u8; row_count * fixed.element_size];
            fixed.row_count = row_count;
            ColumnInner::Fixed(fixed)
        };

        Self {
            row_offset,
            row_count,
            element_size: super::fixed_width::element_size(data_type),
            state: RwLock::new(ChunkState {
                raw,
                encoding: ColumnEncoding::None,
                overlay: UpdateOverlay::new(overlay_capacity_for(row_count)),
                encoding_meta: ChunkEncodingMeta::default(),
                updates_since_encode: 0,
                version_chains: None,
                visibility: RowVisibility::new(),
                dirty_pages: BTreeSet::new(),
                overflow_rows: HashMap::new(),
                residency: ChunkResidency::Resident,
            }),
            last_access: AtomicU64::new(0),
        }
    }

    /// Shared payload access.
    #[inline]
    pub fn read_state(&self) -> Guard<RwLockReadGuard<'_, ChunkState>> {
        Guard::claim(
            self.state.read(),
            RANK_SEGMENT,
            "column segment latch (read)",
        )
    }

    /// Exclusive payload access.
    #[inline]
    pub fn write_state(&self) -> Guard<RwLockWriteGuard<'_, ChunkState>> {
        Guard::claim(
            self.state.write(),
            RANK_SEGMENT,
            "column segment latch (write)",
        )
    }

    /// Stamp recency for eviction ordering. Lock-free so shared-reference
    /// reads participate without upgrading to a write lock.
    pub fn touch(&self) {
        self.last_access.store(next_tick(), Ordering::Relaxed);
    }

    /// Effective encoding scheme: live encoding when resident, pre-evict
    /// scheme when evicted.
    pub fn evicted_encoding(&self) -> EncodingType {
        let state = self.read_state();
        state
            .residency
            .effective_encoding(state.encoding.encoding_type())
    }

    /// Whether this chunk may be evicted: resident, encoded, with no
    /// unmerged overlay writes and no live version-chain entries. Raw chunks
    /// stay resident until the flush encoding pass decides their scheme, so
    /// eviction never preempts encoding and promotion restores the encoded
    /// form directly.
    pub fn is_evictable(&self) -> bool {
        let state = self.read_state();
        matches!(state.residency, ChunkResidency::Resident)
            && state.encoding.is_encoded()
            && state.overlay.len() == 0
            && state
                .version_chains
                .as_ref()
                .is_none_or(|chains| chains.iter().all(|chain| chain.is_empty()))
    }

    /// Write one row: in-place when the encoding absorbs it, otherwise into
    /// the overlay. Returns the decision taken. Operates on already-locked
    /// state so point-write cores holding the segment latch never re-lock.
    pub fn set_value(
        state: &mut ChunkState,
        local_row: u32,
        value: Option<Value>,
        data_type: &graphdb_core::DataType,
    ) -> crate::vertex::column::chunk_encoding::UpdateDecision {
        use crate::vertex::column::chunk_encoding::{can_update_in_place, UpdateDecision};
        if !state.encoding.is_encoded() {
            return UpdateDecision::InPlace;
        }
        let decision = can_update_in_place(
            &state.encoding,
            data_type,
            &state.encoding_meta,
            local_row,
            value.as_ref(),
            state.overlay.is_full(),
        );
        match decision {
            UpdateDecision::InPlace => {
                if state
                    .encoding
                    .set(local_row as usize, value.as_ref())
                    .is_ok()
                {
                    return UpdateDecision::InPlace;
                }
                state.overlay.put(local_row, value);
                state.updates_since_encode += 1;
                UpdateDecision::Overlay
            }
            UpdateDecision::Overlay | UpdateDecision::OverlayAndRecode => {
                state.overlay.put(local_row, value);
                state.updates_since_encode += 1;
                decision
            }
        }
    }

    /// Mark the chunk hot after a recode verdict: the next flush (or an
    /// early merge) re-encodes it instead of letting the overlay grow.
    pub fn mark_hot(&self) {
        let mut state = self.write_state();
        state.updates_since_encode = state
            .updates_since_encode
            .max(state.overlay.capacity() as u64);
    }

    /// Whether this chunk should be re-encoded on the next flush.
    ///
    /// Both signals read the chunk's own overlay budget: distinct rows
    /// waiting in the overlay, or the total number of absorbed writes since
    /// the last encode. One budget therefore scales hotness with chunk size
    /// instead of leaving it a column-independent constant.
    pub fn needs_recode(&self) -> bool {
        let state = self.read_state();
        state.overlay.is_full() || state.updates_since_encode >= state.overlay.capacity() as u64
    }

    /// Refresh the cached compression metadata after re-encoding.
    pub fn refresh_encoding_meta(
        &self,
        num_values: u32,
        all_null: bool,
        min: Option<Value>,
        max: Option<Value>,
        compressed_size: u64,
        raw_size: u64,
    ) {
        let mut state = self.write_state();
        state.encoding_meta.scheme = state.encoding.encoding_type();
        state.encoding_meta.num_values = num_values;
        state.encoding_meta.all_null = all_null;
        state.encoding_meta.min = min;
        state.encoding_meta.max = max;
        state.encoding_meta.compressed_size = compressed_size;
        state.encoding_meta.raw_size = raw_size;
    }
}

impl Clone for ColumnChunk {
    fn clone(&self) -> Self {
        let state = self.read_state();
        Self {
            row_offset: self.row_offset,
            row_count: self.row_count,
            element_size: self.element_size,
            state: RwLock::new(ChunkState {
                raw: state.raw.clone(),
                encoding: state.encoding.clone(),
                overlay: state.overlay.clone(),
                encoding_meta: state.encoding_meta.clone(),
                updates_since_encode: state.updates_since_encode,
                version_chains: state.version_chains.clone(),
                visibility: state.visibility.clone(),
                dirty_pages: state.dirty_pages.clone(),
                overflow_rows: state.overflow_rows.clone(),
                residency: state.residency.clone(),
            }),
            last_access: AtomicU64::new(self.last_access.load(Ordering::Relaxed)),
        }
    }
}

impl std::fmt::Debug for ColumnChunk {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let state = self.read_state();
        f.debug_struct("ColumnChunk")
            .field("row_offset", &self.row_offset)
            .field("row_count", &self.row_count)
            .field("raw_len", &state.raw.as_storage().len())
            .field("encoding", &state.encoding.encoding_type())
            .field("resident", &state.residency.is_resident())
            .finish()
    }
}

// ---------------------------------------------------------------------------
// ChunkFlushView
// ---------------------------------------------------------------------------

/// Chunk-consistent flush descriptor: window plus cloned payload metadata
/// for persistence serialization, taken under one segment read latch.
/// Raw chunks carry their owned buffers (`raw_form` with data, offsets and
/// null bitmap); encoded chunks carry the encoding instead and leave the
/// raw fields empty.
#[derive(Debug, Clone)]
pub struct ChunkFlushView {
    pub row_offset: usize,
    pub row_count: usize,
    pub encoding_meta: ChunkEncodingMeta,
    pub encoding: ColumnEncoding,
    pub overlay: Vec<(u32, Option<Value>)>,
    pub raw_form: bool,
    pub raw_data: Vec<u8>,
    pub raw_offsets: Vec<u64>,
    pub raw_bitmap: Option<BitVec<u8, Lsb0>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chunk_fixed_width_create() {
        let chunk = ColumnChunk::new(0, 100, &DataType::Int, true);
        assert_eq!(chunk.row_offset, 0);
        assert_eq!(chunk.row_count, 100);
        assert_eq!(chunk.read_state().raw.as_storage().len(), 100);
        // Fresh fixed buffers are zero-filled and non-null.
        assert_eq!(
            chunk.read_state().raw.as_storage().get(0),
            Some(Value::Int(0))
        );
    }

    #[test]
    fn test_chunk_variable_width_create() {
        let chunk = ColumnChunk::new(0, 50, &DataType::String, false);
        assert_eq!(chunk.row_offset, 0);
        assert_eq!(chunk.row_count, 50);
        assert_eq!(chunk.read_state().raw.as_storage().len(), 0);
    }
}
