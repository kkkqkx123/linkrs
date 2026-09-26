use graphdb_core::types::Timestamp;
use graphdb_core::{DataType, StorageError, StorageResult, Value};

use crate::column_stats::ColumnStats;
use crate::encoding::ColumnEncoding;
use crate::stats::HyperLogLog;

use super::chunk::{ChunkState, ColumnChunk, DEFAULT_CHUNK_ROWS};
use super::fixed_width::FixedWidthColumn;
use super::overflow::{OverflowHandle, OverflowStore, DEFAULT_OVERFLOW_THRESHOLD};
use super::variable_width::VariableWidthColumn;

use bitvec::prelude::*;
use parking_lot::{Mutex, RwLock};
use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use super::chunk_residency::{next_tick, ChunkResidency, EvictedSnapshot};

/// Unified column storage interface.
pub trait ColumnStorage: Send + Sync + std::fmt::Debug {
    fn get(&self, row_idx: usize) -> Option<Value>;
    fn set(&mut self, row_idx: usize, value: Option<&Value>) -> StorageResult<()>;
    fn len(&self) -> usize;
    fn is_null(&self, row_idx: usize) -> bool;
    fn memory_usage(&self) -> usize;
    /// Pre-allocate capacity for `additional` more rows.
    ///
    /// Batch inserts call this once before writing a run of rows so the
    /// underlying buffers (raw data, offsets, null bitmap) avoid repeated
    /// reallocation. The default is a no-op so single-row/small-batch paths
    /// are unaffected.
    fn reserve(&mut self, additional: usize);
    fn clear(&mut self);
    fn resize(&mut self, new_count: usize);
    fn null_bitmap(&self) -> Option<&BitVec<u8, Lsb0>>;
    fn null_count(&self) -> usize;
    fn load_data_from_raw(
        &mut self,
        data: Vec<u8>,
        offsets: Vec<u64>,
        null_bitmap_raw: Option<Vec<u8>>,
        bitmap_bit_len: usize,
    );
    fn get_flush_data(&self) -> (Vec<u8>, Vec<u64>, Option<BitVec<u8, Lsb0>>);
}

/// Internal dispatch between fixed-width and variable-width storage.
///
/// A `ColumnInner` is the owned raw row store of exactly one chunk
/// ([`ColumnChunk::raw`]); there is no column-level raw store.
#[derive(Debug, Clone)]
pub enum ColumnInner {
    Fixed(FixedWidthColumn),
    Variable(VariableWidthColumn),
}

impl ColumnInner {
    pub(crate) fn as_storage(&self) -> &dyn ColumnStorage {
        match self {
            ColumnInner::Fixed(c) => c,
            ColumnInner::Variable(c) => c,
        }
    }

    pub(crate) fn as_storage_mut(&mut self) -> &mut dyn ColumnStorage {
        match self {
            ColumnInner::Fixed(c) => c,
            ColumnInner::Variable(c) => c,
        }
    }
}

/// Column storage that automatically selects fixed-width or variable-width
/// layout based on the `DataType` at construction time.
///
/// # Variant Selection
///
/// | `DataType` | Storage variant |
/// |---|---|
/// | Bool, SmallInt, Int, BigInt, Float, Double, Date, Time, DateTime, Uuid, short `FixedString(n)`, small `VectorDense(n)` | `FixedWidthColumn` (short fixed strings as zero-padded `n`-byte slots; small dense vectors as fixed `n * 4`-byte slots) |
/// | All other types (String, wide or zero-width FixedString, Blob, Geography, wide or unsized vectors, Json/JsonB, Interval, Decimal family, Union, containers, composites, graph values) | `VariableWidthColumn` (length-prefixed base; per-chunk encodings such as dictionary/FSST plus zone maps and HLL stats apply on top) |
///
/// # MVCC
///
/// Property updates are versioned through [`Column::set_versioned`] /
/// [`Column::get_at_ts`]: each row keeps a chain of before-images
/// (`visibility.create_ts` + optional `version_chains`), so a snapshot
/// read at a historical timestamp returns the value visible then instead
/// of the current one. Old versions are reclaimed by [`Column::gc_versions`].
///
/// # Concurrency Model
///
/// The per-shard `RwLock<VertexTable>` remains the outer latch: exclusive
/// operations (flush, load, schema change, GC, compaction, eviction
/// selection) run under the shard write lock, point operations under the
/// shard read lock. Inside a shard, latching is two-level:
///
/// - Identity operations (id allocation, row timestamps, schema state) go
///   through the table's identity lock.
/// - Every per-row payload lives in exactly one chunk's segment latch
///   ([`ColumnChunk::state`]); point readers and writers on different
///   chunks never share a lock.
///
/// Lock order on every path is chunk-vector before segment state before the
/// short shared critical sections (overflow store, HLL). Structural
/// operations (growth, split, shrink, load) take the chunk-vector write
/// lock; point paths take it shared and release it before locking a
/// segment, so member-before-container inversion cannot happen.
///
/// Background eviction quota: one eviction segment releases at most this many
/// bytes before re-selecting victims, so over-quota background work proceeds
/// in segments instead of one burst.
pub const EVICTION_SEGMENT_BYTES: u64 = 16 * 1024 * 1024;

/// Background load quota: at most this many evicted chunks are promoted per
/// batch-load call; over-quota scans continue with the remainder.
pub const MAX_BACKGROUND_LOAD_CHUNKS: usize = 64;

/// Unified buffer accounting for one column: decoded resident bytes
/// (including the overflow side store), retained eviction-snapshot bytes,
/// the overflow subset for breakdown, dirty pages and chunk counts under a
/// single unified accounting. The three former paths (chunk residency, overflow store, dirty
/// pages) plus the process spill directory backing evicted snapshots
/// share this ledger so eviction quotas and observability observe the
/// same totals. Spill files are owned by their snapshots and vanish with
/// the last dropped reference; `clear` drops chunks, overflow and dirty
/// marks together, leaving no cross-process residue except
/// crash-orphaned spill directories reaped by
/// `cleanup_stale_spill_dirs`.
#[derive(Debug, Clone, Copy, Default)]
pub struct BufferLedger {
    pub resident_bytes: usize,
    pub evicted_bytes: usize,
    pub overflow_bytes: usize,
    pub dirty_pages: usize,
    pub resident_chunks: usize,
    pub evicted_chunks: usize,
}

impl BufferLedger {
    pub fn total_bytes(&self) -> usize {
        self.resident_bytes.saturating_add(self.evicted_bytes)
    }
}

/// A column of one shard: metadata plus a latch-guarded chunk vector.
///
/// Plain fields are either immutable after creation (`name`, `col_id`,
/// `data_type`, `nullable`) or mutated only by exclusive operations running
/// under the shard write lock (`stats`). Everything a point operation
/// touches is either per-chunk state behind the segment latch or one of the
/// short shared critical sections below.
#[derive(Debug)]
pub struct Column {
    pub name: String,
    pub col_id: i32,
    pub data_type: DataType,
    pub nullable: bool,
    /// Column statistics refreshed by exclusive analyze/encode passes.
    pub(super) stats: RwLock<Option<ColumnStats>>,
    /// Versioned writes since the last exact zone rebuild. Feeds the
    /// shrink trigger so long update histories do not leave pruning
    /// permanently stale.
    pub(super) zone_stale_writes: AtomicU64,
    /// Rows per chunk used when materializing `chunks`. Written only where
    /// no concurrent point traffic exists (creation, load, offline
    /// rebuild); read atomically on every routing decision.
    pub(super) chunk_capacity: AtomicUsize,
    /// Payload size above which strings spill to the overflow store.
    /// `usize::MAX` disables overflow routing (inline storage).
    pub(super) overflow_threshold: AtomicUsize,
    /// In-memory HLL estimator maintained incrementally on writes. Short
    /// critical section shared across segments on purpose: the sketch is
    /// tiny and merge-on-read would cost more than one brief lock.
    pub(super) hll: Mutex<Option<HyperLogLog>>,
    /// Zone-map state shared across segments (zone granularity is
    /// independent of segment capacity, so one map serves all segments
    /// without per-chunk duplication).
    pub(super) zone: RwLock<super::zone_map::ZoneMaps>,
    /// Large-string overflow area for this column. Append-only in the point
    /// path; rebuilds happen under the shard write lock.
    pub(super) overflow_store: Mutex<OverflowStore>,
    /// High-water page count backing the dirty-ratio pre-pass. Monotonic
    /// maximum; reset only by exclusive `clear`.
    pub(super) total_dirty_pages: AtomicUsize,
    /// The column's row store, partitioned into contiguous fixed-size
    /// windows. Every row byte lives in exactly one chunk's owned buffers;
    /// there is no column-level raw store. Windows are contiguous from row
    /// zero and no chunk exceeds `chunk_capacity` (a trailing reserved chunk
    /// may hold zero rows).
    pub(super) chunks: RwLock<Vec<ColumnChunk>>,
}

impl Clone for Column {
    fn clone(&self) -> Self {
        Self {
            name: self.name.clone(),
            col_id: self.col_id,
            data_type: self.data_type.clone(),
            nullable: self.nullable,
            stats: RwLock::new(self.stats.read().clone()),
            zone_stale_writes: AtomicU64::new(self.zone_stale_writes.load(Ordering::Relaxed)),
            chunk_capacity: AtomicUsize::new(self.chunk_capacity.load(Ordering::Relaxed)),
            overflow_threshold: AtomicUsize::new(self.overflow_threshold.load(Ordering::Relaxed)),
            hll: Mutex::new(self.hll.lock().clone()),
            zone: RwLock::new(self.zone.read().clone()),
            overflow_store: Mutex::new(self.overflow_store.lock().clone()),
            total_dirty_pages: AtomicUsize::new(self.total_dirty_pages.load(Ordering::Relaxed)),
            chunks: RwLock::new(self.chunks.read().clone()),
        }
    }
}

impl Column {
    pub fn new(name: String, col_id: i32, data_type: DataType, nullable: bool) -> Self {
        Self {
            name,
            col_id,
            data_type,
            nullable,
            stats: RwLock::new(None),
            zone_stale_writes: AtomicU64::new(0),
            chunk_capacity: AtomicUsize::new(DEFAULT_CHUNK_ROWS),
            overflow_threshold: AtomicUsize::new(DEFAULT_OVERFLOW_THRESHOLD),
            hll: Mutex::new(Some(HyperLogLog::new())),
            zone: RwLock::new(super::zone_map::ZoneMaps::default()),
            overflow_store: Mutex::new(OverflowStore::new(DEFAULT_OVERFLOW_THRESHOLD)),
            total_dirty_pages: AtomicUsize::new(0),
            chunks: RwLock::new(Vec::new()),
        }
    }

    /// Rows per chunk used for materialization.
    pub fn chunk_capacity(&self) -> usize {
        self.chunk_capacity.load(Ordering::Relaxed)
    }

    /// Override the rows-per-chunk capacity for future materialization.
    ///
    /// Only call where no concurrent point traffic exists (creation, load,
    /// offline rebuild): in-flight point operations route by the capacity
    /// they loaded, and a mid-flight change would misroute them.
    pub fn set_chunk_capacity(&self, capacity: usize) {
        if capacity > 0 {
            self.chunk_capacity.store(capacity, Ordering::Relaxed);
        }
    }

    /// Payload size above which strings spill to the overflow store.
    pub fn overflow_threshold(&self) -> usize {
        self.overflow_threshold.load(Ordering::Relaxed)
    }

    /// Decode one chunk's readable base into its owned raw buffers and
    /// drop the encoding/eviction state. The window is unchanged, so kept
    /// MVCC metadata stays valid; the chunk simply becomes raw.
    ///
    /// The base is collected without holding the chunk's write latch, then
    /// merged with whatever overlay entries landed meanwhile before the
    /// overlay clears, so a concurrent point write can never be lost.
    pub(super) fn decode_locked(&self, chunks: &[ColumnChunk], idx: usize) {
        let Some(chunk) = chunks.get(idx) else {
            return;
        };
        {
            let state = chunk.read_state();
            if !state.encoding.is_encoded() && state.residency.is_resident() {
                return;
            }
        }
        let (offset, count) = (chunk.row_offset, chunk.row_count);
        let mut base: Vec<Option<Value>> = (offset..offset + count)
            .map(|row| self.encoding_base_value_in(chunks, row))
            .collect();
        let mut state = chunk.write_state();
        // Merge overlay entries that landed during collection: they are
        // newer than the collected base and must survive the decode.
        for (local, value) in state.overlay.iter() {
            if let Some(slot) = base.get_mut(*local as usize) {
                *slot = value.clone();
            }
        }
        let mut num_values = 0u32;
        for (local, value) in base.into_iter().enumerate() {
            if value.is_some() {
                num_values += 1;
            }
            let _ = state.raw.as_storage_mut().set(local, value.as_ref());
        }
        state.encoding = ColumnEncoding::None;
        state.residency = ChunkResidency::Resident;
        // The decoded base already merges the overlay, so the overlay entries
        // are redundant now; dropping them keeps single representation.
        state.overlay.clear();
        state.updates_since_encode = 0;
        state.encoding_meta = crate::encoding::ChunkEncodingMeta {
            num_values,
            all_null: num_values == 0,
            ..Default::default()
        };
    }

    pub(super) fn column_len(chunks: &[ColumnChunk]) -> usize {
        chunks
            .last()
            .map(|chunk| chunk.row_offset + chunk.row_count)
            .unwrap_or(0)
    }

    /// Grow chunk windows to cover `row_idx`, filling gaps with nulls.
    /// Returns the index of the chunk owning `row_idx`. Fresh chunks are
    /// raw (no encoding, empty overlay); gap rows read back as null.
    /// Windows stay capacity-aligned, so the owner is always `row / capacity`.
    ///
    /// Only the tail ever grows or is appended: existing windows are never
    /// moved, so point operations holding the container shared may keep
    /// their located chunk across a concurrent growth.
    pub(super) fn ensure_coverage(&self, row_idx: usize) -> usize {
        let capacity = self.chunk_capacity().max(1);
        {
            let chunks = self.chunks.read();
            if Self::column_len(&chunks) > row_idx {
                return row_idx / capacity;
            }
        }
        let mut chunks = self.chunks.write();
        let slice: &mut Vec<ColumnChunk> = &mut chunks;
        while Self::column_len(slice) <= row_idx {
            let tail_full = match slice.last() {
                None => true,
                Some(tail) => tail.row_count >= capacity,
            };
            if tail_full {
                let offset = Self::column_len(slice);
                slice.push(ColumnChunk::new(offset, 0, &self.data_type, self.nullable));
            } else {
                let last = slice.len() - 1;
                self.decode_locked(slice, last);
            }
            let last = slice.len() - 1;
            let window_end = (last + 1) * capacity;
            let end_target = (row_idx + 1).min(window_end);
            let chunk = &mut slice[last];
            let end = chunk.row_offset + chunk.row_count;
            if end_target > end {
                chunk.row_count += end_target - end;
                let row_count = chunk.row_count;
                chunk.write_state().raw.as_storage_mut().resize(row_count);
            }
        }
        row_idx / capacity
    }

    /// Split an oversized chunk at `at` local rows, returning the two halves
    /// with contiguous windows. Used when a capacity shrink strands a chunk
    /// wider than the routing width.
    pub(super) fn split_raw(raw: &ColumnInner, at: usize) -> (ColumnInner, ColumnInner) {
        match raw {
            ColumnInner::Fixed(fixed) => {
                let elem = fixed.element_size.max(1);
                let (left_data, right_data) = fixed.data.split_at(at * elem);
                let (left_bits, right_bits) = match &fixed.null_bitmap {
                    Some(b) => {
                        let mut l = BitVec::new();
                        let mut r = BitVec::new();
                        for (i, bit) in b.iter().by_vals().enumerate() {
                            if i < at {
                                l.push(bit);
                            } else {
                                r.push(bit);
                            }
                        }
                        (Some(l), Some(r))
                    }
                    None => (None, None),
                };
                let mut left =
                    FixedWidthColumn::new(fixed.data_type.clone(), fixed.null_bitmap.is_some());
                left.data = left_data.to_vec();
                left.null_bitmap = left_bits;
                left.row_count = at;
                left.null_count = left
                    .null_bitmap
                    .as_ref()
                    .map(|b| b.count_ones())
                    .unwrap_or(0);
                let mut right =
                    FixedWidthColumn::new(fixed.data_type.clone(), fixed.null_bitmap.is_some());
                right.data = right_data.to_vec();
                right.null_bitmap = right_bits;
                right.row_count = fixed.row_count.saturating_sub(at);
                right.null_count = right
                    .null_bitmap
                    .as_ref()
                    .map(|b| b.count_ones())
                    .unwrap_or(0);
                (ColumnInner::Fixed(left), ColumnInner::Fixed(right))
            }
            ColumnInner::Variable(var) => {
                let mut left_data = Vec::new();
                let mut left_offsets: Vec<u64> = Vec::new();
                let mut left_bits: Option<BitVec<u8, Lsb0>> =
                    var.null_bitmap.as_ref().map(|_| BitVec::new());
                let mut right_data = Vec::new();
                let mut right_offsets: Vec<u64> = Vec::new();
                let mut right_bits: Option<BitVec<u8, Lsb0>> =
                    var.null_bitmap.as_ref().map(|_| BitVec::new());
                let bits: Vec<bool> = var
                    .null_bitmap
                    .as_ref()
                    .map(|b| b.iter().by_vals().collect())
                    .unwrap_or_default();
                for local in 0..var.row_count {
                    let off = var.offsets.get(local).copied().unwrap_or(usize::MAX);
                    let bit = bits.get(local).copied().unwrap_or(false);
                    let (data, offsets, bits) = if local < at {
                        (&mut left_data, &mut left_offsets, &mut left_bits)
                    } else {
                        (&mut right_data, &mut right_offsets, &mut right_bits)
                    };
                    if off == usize::MAX || off + 8 > var.data.len() {
                        offsets.push(u64::MAX);
                    } else {
                        let len_bytes: [u8; 8] =
                            var.data[off..off + 8].try_into().unwrap_or([0u8; 8]);
                        let len = u64::from_le_bytes(len_bytes) as usize;
                        if off + 8 + len > var.data.len() {
                            offsets.push(u64::MAX);
                        } else {
                            offsets.push(data.len() as u64);
                            data.extend_from_slice(&var.data[off..off + 8 + len]);
                        }
                    }
                    if let Some(b) = bits {
                        b.push(bit);
                    }
                }
                let mut left =
                    VariableWidthColumn::new(var.data_type.clone(), var.null_bitmap.is_some());
                left.load_data_from_raw(
                    left_data,
                    left_offsets,
                    left_bits.map(|b| b.into_vec()),
                    at,
                );
                let mut right =
                    VariableWidthColumn::new(var.data_type.clone(), var.null_bitmap.is_some());
                right.load_data_from_raw(
                    right_data,
                    right_offsets,
                    right_bits.map(|b| b.into_vec()),
                    var.row_count.saturating_sub(at),
                );
                (ColumnInner::Variable(left), ColumnInner::Variable(right))
            }
        }
    }

    /// Mark the page containing `row_idx` as dirty. The mark lives in the
    /// owning chunk's segment state; the page id is global so flush
    /// aggregation stays a plain union.
    #[inline]
    pub fn mark_dirty(&self, row_idx: usize) {
        let page_id = crate::persistence::dirty_page::row_to_page(row_idx);
        self.total_dirty_pages
            .fetch_max(page_id + 1, Ordering::Relaxed);
        let chunks = self.chunks.read();
        if let Some(chunk) = Self::chunk_for_row_in(&chunks, row_idx, self.chunk_capacity()) {
            chunk.write_state().dirty_pages.insert(page_id as u32);
        }
    }

    /// High-water page count backing the dirty-ratio pre-pass.
    pub fn total_pages(&self) -> usize {
        self.total_dirty_pages.load(Ordering::Relaxed)
    }

    pub fn dirty_pages(&self) -> Vec<usize> {
        let chunks = self.chunks.read();
        let mut pages = BTreeSet::new();
        for chunk in chunks.iter() {
            pages.extend(chunk.read_state().dirty_pages.iter().copied());
        }
        pages.into_iter().map(|id| id as usize).collect()
    }

    pub fn dirty_count(&self) -> usize {
        let chunks = self.chunks.read();
        chunks
            .iter()
            .map(|chunk| chunk.read_state().dirty_pages.len())
            .sum()
    }

    pub fn clear_dirty(&self) {
        let chunks = self.chunks.read();
        for chunk in chunks.iter() {
            chunk.write_state().dirty_pages.clear();
        }
    }

    /// Clear the dirty mark for a single row-page (keeps other dirty pages).
    #[inline]
    pub fn clear_page_dirty(&self, page_id: usize) {
        let first_row = page_id.saturating_mul(crate::persistence::dirty_page::ROWS_PER_PAGE);
        let chunks = self.chunks.read();
        if let Some(chunk) = Self::chunk_for_row_in(&chunks, first_row, self.chunk_capacity()) {
            chunk.write_state().dirty_pages.remove(&(page_id as u32));
        }
    }

    /// Locate the chunk owning `row_idx` inside an already-locked chunk
    /// vector, without re-locking the container.
    fn chunk_for_row_in(
        chunks: &[ColumnChunk],
        row_idx: usize,
        capacity: usize,
    ) -> Option<&ColumnChunk> {
        if chunks.is_empty() {
            return None;
        }
        let idx = row_idx / capacity.max(1);
        chunks.get(idx).filter(|chunk| {
            row_idx >= chunk.row_offset && row_idx < chunk.row_offset + chunk.row_count
        })
    }

    /// Serialize a single page for incremental checkpoint.
    /// Returns `PageData` serialized bytes including header + payload.
    pub fn serialize_page(&self, page_id: usize) -> StorageResult<Vec<u8>> {
        let rows_per_page = crate::persistence::dirty_page::ROWS_PER_PAGE;
        let start = page_id * rows_per_page;
        let total = self.len();
        if start >= total {
            return Err(StorageError::invalid_input(format!(
                "page {} out of range (total rows {})",
                page_id, total
            )));
        }
        let end = (start + rows_per_page).min(total);
        let mut values = Vec::with_capacity(end - start);
        for row in start..end {
            values.push(self.get(row));
        }
        let payload = postcard::to_allocvec(&values)
            .map_err(|e| StorageError::serialize_error(e.to_string()))?;
        let page_data =
            crate::persistence::dirty_page::PageData::new(page_id as u32, payload, false);
        Ok(page_data.serialize())
    }

    /// Deserialize and apply a single page. Does not mark the page dirty
    /// (clean after checkpoint load).
    pub fn deserialize_page(&self, data: &[u8]) -> StorageResult<()> {
        let page =
            crate::persistence::dirty_page::PageData::deserialize(data).ok_or_else(|| {
                StorageError::deserialize_error(
                    "invalid page data or checksum mismatch".to_string(),
                )
            })?;
        let values: Vec<Option<Value>> = postcard::from_bytes(&page.data)
            .map_err(|e| StorageError::deserialize_error(e.to_string()))?;
        let rows_per_page = crate::persistence::dirty_page::ROWS_PER_PAGE;
        let start = page.header.page_id as usize * rows_per_page;
        // Ensure column can hold the restored rows without marking dirty.
        if start + values.len() > self.len() {
            self.resize(start + values.len());
        }
        for (offset, val) in values.into_iter().enumerate() {
            let row_idx = start + offset;
            self.write_value_without_dirty(row_idx, val.as_ref())?;
        }
        // Mark page clean after successful restore (was dirtied by writes if any).
        self.clear_page_dirty(page.header.page_id as usize);
        Ok(())
    }

    /// Core value write with the segment write latch already held.
    ///
    /// Never touches the chunk container: coverage growth and promotion are
    /// the caller's job (protocol steps before locking the segment), and
    /// the overflow store / zone / HLL locks taken here are leaves that
    /// never nest back into a segment or the container. Returns whether the
    /// chunk layer absorbed the write (overlay or in-place encoding).
    pub(super) fn write_core(
        &self,
        chunk: &ColumnChunk,
        state: &mut ChunkState,
        row_idx: usize,
        value: Option<&Value>,
        use_chunk_layer: bool,
    ) -> StorageResult<bool> {
        use crate::vertex::column::chunk_encoding::UpdateDecision;
        let in_window = row_idx >= chunk.row_offset && row_idx < chunk.row_offset + chunk.row_count;
        if !in_window {
            return Err(StorageError::invalid_input(format!(
                "column {} has no chunk covering row {}",
                self.name, row_idx
            )));
        }
        let local = (row_idx - chunk.row_offset) as u32;
        // Large-string overflow routing happens before encoding checks so the
        // main buffers only ever hold the small inline placeholder.
        if matches!(self.data_type, DataType::String | DataType::Blob) {
            let payload: Option<&[u8]> = match value {
                Some(Value::String(s)) => Some(s.as_bytes()),
                Some(Value::Blob(b)) => Some(b.as_slice()),
                _ => None,
            };
            if let Some(bytes) = payload {
                if self.overflow_store.lock().should_overflow(bytes.len()) {
                    let handle = self.overflow_store.lock().append(bytes);
                    state.overflow_rows.insert(local, handle);
                    // The side store is authoritative for this row: main
                    // buffers keep only an inline placeholder, and any
                    // chunk overlay entry for the row is stale.
                    state.overlay.remove(local);
                    let placeholder = match self.data_type {
                        DataType::Blob => Value::Blob(Vec::new()),
                        _ => Value::string(""),
                    };
                    let normalized = match Some(&placeholder) {
                        Some(v) if v.is_null() => None,
                        other => other,
                    };
                    state.raw.as_storage_mut().set(local as usize, normalized)?;
                    return Ok(false);
                }
            }
            // A fitting value drops any stale side-store mapping. Stale
            // payload stays in the store until flush rebuilds it.
            state.overflow_rows.remove(&local);
        }
        if use_chunk_layer {
            let decision = ColumnChunk::set_value(state, local, value.cloned(), &self.data_type);
            // A recode verdict means the encoding cannot absorb more point
            // writes: mark the chunk hot immediately so the next flush (or an
            // early merge) re-encodes it instead of letting the overlay grow
            // to full and forcing a whole-column fallback.
            if decision == UpdateDecision::OverlayAndRecode {
                state.updates_since_encode = state
                    .updates_since_encode
                    .max(state.overlay.capacity() as u64);
            }
            // Raw chunks have no encoded base: mirror the write into the
            // owning chunk's raw buffers.
            if decision == UpdateDecision::InPlace && !state.encoding.is_encoded() {
                let normalized = match value {
                    Some(v) if v.is_null() => None,
                    other => other,
                };
                if normalized.is_none() && !self.nullable {
                    return Err(StorageError::null_value_not_allowed(self.name.clone()));
                }
                state.raw.as_storage_mut().set(local as usize, normalized)?;
                return Ok(true);
            }
            // Nullability still enforced even for overlay writes.
            if value.is_none() && !self.nullable {
                return Err(StorageError::null_value_not_allowed(self.name.clone()));
            }
            if let Some(v) = value {
                if v.is_null() && !self.nullable {
                    return Err(StorageError::null_value_not_allowed(self.name.clone()));
                }
            }
            return Ok(true);
        }
        let normalized = match value {
            Some(v) if v.is_null() => None,
            other => other,
        };
        if normalized.is_none() && !self.nullable {
            return Err(StorageError::null_value_not_allowed(self.name.clone()));
        }
        state.raw.as_storage_mut().set(local as usize, normalized)?;
        Ok(false)
    }

    /// Whether the chunk layer routes this write (overlay or in-place chunk
    /// encoding). Chunk encodings are authoritative once present, and an
    /// evicted chunk still counts: promotion on the write path restores its
    /// encoding, so the write must stay on the chunk-layer route.
    pub(super) fn chunk_layer_routes(&self, chunks: &[ColumnChunk]) -> bool {
        chunks
            .iter()
            .any(|c| c.evicted_encoding() != crate::encoding::EncodingType::None)
    }

    /// Point-write protocol: grow coverage, promote the owner, then run the
    /// caller under the segment write latch. Steps 1-2 never run while a
    /// segment latch is held (container-before-member order).
    pub(super) fn with_resident_chunk<T>(
        &self,
        row_idx: usize,
        f: impl FnOnce(&ColumnChunk, &mut ChunkState) -> StorageResult<T>,
    ) -> StorageResult<T> {
        let chunk_idx = self.ensure_coverage(row_idx);
        // Overlay writes to an evicted chunk load it first so point-write
        // semantics never change under eviction. Load failures propagate
        // as storage errors instead of silently dropping the write.
        if self.chunk_index_for_row(row_idx).is_some() {
            self.ensure_resident(chunk_idx)?;
        }
        let chunks = self.chunks.read();
        let Some(chunk) = chunks.get(chunk_idx) else {
            return Err(StorageError::invalid_input(format!(
                "column {} has no chunk covering row {}",
                self.name, row_idx
            )));
        };
        let mut state = chunk.write_state();
        f(chunk, &mut state)
    }

    /// Write into the owning chunk's raw buffers, growing coverage first.
    /// Nullability is enforced here so raw appends cannot smuggle nulls
    /// into a non-nullable column.
    fn write_raw_inner(&self, row_idx: usize, value: Option<&Value>) -> StorageResult<()> {
        let normalized = match value {
            Some(v) if v.is_null() => None,
            other => other,
        };
        if normalized.is_none() && !self.nullable {
            return Err(StorageError::null_value_not_allowed(self.name.clone()));
        }
        self.with_resident_chunk(row_idx, |chunk, state| {
            let local = row_idx - chunk.row_offset;
            state.raw.as_storage_mut().set(local, normalized)?;
            Ok(())
        })
    }

    pub(super) fn observe_write(&self, row_idx: usize, value: Option<&Value>) {
        self.update_zone_maps(row_idx, value);
        self.zone_stale_writes.fetch_add(1, Ordering::Relaxed);
        if let Some(v) = value {
            if !v.is_null() {
                if let Some(hll) = self.hll.lock().as_mut() {
                    hll.add_value(v);
                }
            }
        }
    }

    /// Internal write without dirty marking (used by page deserialization).
    fn write_value_without_dirty(
        &self,
        row_idx: usize,
        value: Option<&Value>,
    ) -> StorageResult<()> {
        let use_chunk_layer = self.chunk_layer_routes(&self.chunks.read());
        let absorbed = self.with_resident_chunk(row_idx, |chunk, state| {
            // Fresh coverage has no side state yet; size it to the window.
            state.visibility.ensure_len(chunk.row_count);
            if let Some(chains) = state.version_chains.as_mut() {
                if chains.len() < chunk.row_count {
                    chains.resize(chunk.row_count, Vec::new());
                }
            }
            self.write_core(chunk, state, row_idx, value, use_chunk_layer)
        })?;
        let _ = absorbed;
        self.observe_write(row_idx, value);
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Core read / write
    // -----------------------------------------------------------------------

    pub fn set(&self, row_idx: usize, value: Option<&Value>) -> StorageResult<()> {
        // Plain set treats the value as "current from the beginning": it
        // resets the row's MVCC metadata (no historical version recorded).
        // Metadata reset and value write share one segment latch so a
        // concurrent snapshot read never observes half a set.
        if value.is_none() && !self.nullable {
            return Err(StorageError::null_value_not_allowed(self.name.clone()));
        }
        if let Some(v) = value {
            if v.is_null() && !self.nullable {
                return Err(StorageError::null_value_not_allowed(self.name.clone()));
            }
        }
        let use_chunk_layer = self.chunk_layer_routes(&self.chunks.read());
        let absorbed = self.with_resident_chunk(row_idx, |chunk, state| {
            let local = row_idx - chunk.row_offset;
            state.visibility.ensure_len(chunk.row_count);
            if let Some(chains) = state.version_chains.as_mut() {
                if chains.len() < chunk.row_count {
                    chains.resize(chunk.row_count, Vec::new());
                }
                if local < chains.len() {
                    chains[local].clear();
                }
            }
            state.visibility.mark_created(local, 0);
            self.write_core(chunk, state, row_idx, value, use_chunk_layer)
        })?;
        self.observe_write(row_idx, value);
        self.mark_dirty(row_idx);
        if absorbed {
            if let Some(chunk_idx) = self.chunk_index_for_row(row_idx) {
                let _ = self.maybe_merge_hot_chunk(chunk_idx);
            }
        }
        Ok(())
    }

    /// Write a value with a specific creation timestamp without generating
    /// a version chain entry. This is used when initializing a new row where
    /// the initial value should be visible from `create_ts` onward and no
    /// before-image exists.
    pub fn set_with_timestamp(
        &self,
        row_idx: usize,
        value: Option<&Value>,
        create_ts: Timestamp,
    ) -> StorageResult<()> {
        if value.is_none() && !self.nullable {
            return Err(StorageError::null_value_not_allowed(self.name.clone()));
        }
        if let Some(v) = value {
            if v.is_null() && !self.nullable {
                return Err(StorageError::null_value_not_allowed(self.name.clone()));
            }
        }
        let use_chunk_layer = self.chunk_layer_routes(&self.chunks.read());
        let absorbed = self.with_resident_chunk(row_idx, |chunk, state| {
            let local = row_idx - chunk.row_offset;
            state.visibility.ensure_len(chunk.row_count);
            // Clear any existing version chain for this row
            if let Some(chains) = state.version_chains.as_mut() {
                if chains.len() < chunk.row_count {
                    chains.resize(chunk.row_count, Vec::new());
                }
                if local < chains.len() {
                    chains[local].clear();
                }
            }
            // Set the correct creation timestamp
            state.visibility.mark_created(local, create_ts);
            // Write the value without generating a version chain entry
            self.write_core(chunk, state, row_idx, value, use_chunk_layer)
        })?;
        self.observe_write(row_idx, value);
        self.mark_dirty(row_idx);
        if absorbed {
            if let Some(chunk_idx) = self.chunk_index_for_row(row_idx) {
                let _ = self.maybe_merge_hot_chunk(chunk_idx);
            }
        }
        Ok(())
    }

    pub fn get(&self, row_idx: usize) -> Option<Value> {
        let chunks = self.chunks.read();
        self.get_in(&chunks, row_idx)
    }

    /// [`Self::get`] against an already-locked chunk vector.
    pub(super) fn get_in(&self, chunks: &[ColumnChunk], row_idx: usize) -> Option<Value> {
        let capacity = self.chunk_capacity();
        let chunk = chunks.get(row_idx / capacity.max(1))?;
        // Rows outside the owning window (only possible when the
        // chunk shrank under a concurrent-free exclusive op) read as
        // missing, matching out-of-range reads.
        if row_idx < chunk.row_offset || row_idx >= chunk.row_offset + chunk.row_count {
            return None;
        }
        let local = (row_idx - chunk.row_offset) as u32;
        // Overflow rows are authoritative in the side store. The handle is
        // copied out before locking the store so the segment latch never
        // nests inside the store lock.
        if matches!(self.data_type, DataType::String | DataType::Blob) {
            let handle = chunk.read_state().overflow_rows.get(&local).copied();
            if let Some(handle) = handle {
                if let Some(bytes) = self.overflow_store.lock().get(&handle) {
                    match self.data_type {
                        DataType::Blob => return Some(Value::Blob(bytes)),
                        _ => {
                            if let Ok(s) = String::from_utf8(bytes) {
                                return Some(Value::string(s));
                            }
                        }
                    }
                }
            }
        }
        // Recency stamp for watermark-ordered eviction. Atomic so
        // shared-reference reads participate without a lock upgrade.
        chunk.touch();
        let state = chunk.read_state();
        match &state.residency {
            ChunkResidency::Resident => {
                if let Some(hit) = state.overlay.get(local) {
                    return hit;
                }
                // Chunk-local encodings are authoritative when present;
                // raw chunks serve their owned buffers.
                if state.encoding.is_encoded() {
                    return self.restore_string_type(state.encoding.get(local as usize));
                }
                state.raw.as_storage().get(local as usize)
            }
            ChunkResidency::Evicted(snapshot) => {
                // Cold miss served from the compressed snapshot
                // without promoting: promotion happens on write
                // paths (writes, batch prefetch, encode, flush).
                // The snapshot is checksummed in memory, so a
                // decode failure here is unreachable; warn and miss
                // rather than failing the read.
                match snapshot.decode_row(row_idx) {
                    Ok(value) => value,
                    Err(e) => {
                        log::warn!(
                            "evicted chunk row {} snapshot decode failed: {}; reading as missing",
                            row_idx,
                            e
                        );
                        None
                    }
                }
            }
        }
    }

    /// Restore the declared string type for values served from a
    /// type-agnostic string encoding (dictionary / FSST), which always
    /// decodes to `Value::String`. Raw reads already carry the declared
    /// type, so only encoded reads pass through here.
    fn restore_string_type(&self, value: Option<Value>) -> Option<Value> {
        match (value, &self.data_type) {
            (Some(Value::String(s)), DataType::FixedString(_)) => {
                Some(Value::FixedString(s.to_string()))
            }
            (v, _) => v,
        }
    }

    pub fn is_null(&self, row_idx: usize) -> bool {
        let chunks = self.chunks.read();
        Self::is_null_in(self, &chunks, row_idx)
    }

    fn is_null_in(column: &Column, chunks: &[ColumnChunk], row_idx: usize) -> bool {
        let capacity = column.chunk_capacity();
        let Some(chunk) = chunks.get(row_idx / capacity.max(1)) else {
            return false;
        };
        if row_idx < chunk.row_offset || row_idx >= chunk.row_offset + chunk.row_count {
            return false;
        }
        let local = (row_idx - chunk.row_offset) as u32;
        if matches!(column.data_type, DataType::String | DataType::Blob)
            && chunk.read_state().overflow_rows.contains_key(&local)
        {
            return false;
        }
        let state = chunk.read_state();
        if let Some(hit) = state.overlay.get(local) {
            return hit.is_none();
        }
        if state.residency.is_evicted() {
            match state
                .residency
                .evicted_snapshot()
                .map(|snapshot| snapshot.decode_row(row_idx))
            {
                Some(Ok(value)) => return value.is_none(),
                Some(Err(_)) => return false,
                None => return false,
            }
        }
        if state.encoding.is_encoded() {
            return state.encoding.get(local as usize).is_none();
        }
        state.raw.as_storage().is_null(local as usize)
    }

    pub fn null_count(&self) -> usize {
        let chunks = self.chunks.read();
        let mut total = 0usize;
        for chunk in chunks.iter() {
            let state = chunk.read_state();
            let fast_raw = !state.encoding.is_encoded()
                && state.residency.is_resident()
                && state.overlay.len() == 0
                && state.overflow_rows.is_empty();
            drop(state);
            if fast_raw {
                total += chunk.read_state().raw.as_storage().null_count();
                continue;
            }
            for offset in 0..chunk.row_count {
                if Self::is_null_in(self, &chunks, chunk.row_offset + offset) {
                    total += 1;
                }
            }
        }
        total
    }

    pub fn len(&self) -> usize {
        Self::column_len(&self.chunks.read())
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn memory_usage(&self) -> usize {
        let chunks = self.chunks.read();
        let mut version_bytes = 0usize;
        let mut chunk_bytes = 0usize;
        for chunk in chunks.iter() {
            let state = chunk.read_state();
            if let Some(chains) = state.version_chains.as_ref() {
                for chain in chains.iter() {
                    version_bytes += chain.len() * std::mem::size_of::<super::mvcc::VersionEntry>();
                    for entry in chain.iter() {
                        version_bytes += entry
                            .value
                            .as_ref()
                            .map(super::value_payload_bytes)
                            .unwrap_or(0);
                    }
                }
            }
            version_bytes += state.visibility.memory_usage();
            // Every row byte is counted exactly once, inside its owning chunk.
            // The column-level encoding is a scheme marker mirroring (a subset
            // of) the first chunk's encoding, not separate storage.
            chunk_bytes += state.overlay.memory_usage()
                + state.encoding_meta.memory_usage()
                + state.raw.as_storage().memory_usage()
                + state.encoding.memory_usage()
                + state.overflow_rows.len() * std::mem::size_of::<(u32, OverflowHandle)>();
        }
        let hll_bytes = self.hll.lock().as_ref().map(|_| 64usize).unwrap_or(0);
        version_bytes + chunk_bytes + hll_bytes + self.overflow_store.lock().memory_usage()
    }

    pub fn memory_size(&self) -> usize {
        self.memory_usage() + std::mem::size_of::<Self>()
    }

    pub fn used_memory_size(&self) -> usize {
        let non_null_count = self.len() - self.null_count();
        let elem_size = super::element_size(&self.data_type);
        non_null_count * elem_size + std::mem::size_of::<Self>()
    }

    /// Reset the column to empty. Exclusive-only: it replaces windows and
    /// shared summaries while point operations route by them.
    pub fn clear(&self) {
        self.zone_stale_writes.store(0, Ordering::Relaxed);
        self.zone.write().maps.clear();
        self.zone.write().complex.clear();
        self.chunks.write().clear();
        self.total_dirty_pages.store(0, Ordering::Relaxed);
        *self.hll.lock() = Some(HyperLogLog::new());
        *self.overflow_store.lock() = OverflowStore::new(self.overflow_threshold());
    }

    /// Pre-allocate capacity for `additional` more rows in the underlying
    /// storage buffers (used by batch inserts). When the column is empty a
    /// reserved zero-row chunk is staged so the imminent append run lands in
    /// pre-sized buffers; it reads as empty and vanishes on materialization.
    ///
    /// Safe under the shard read lock: it only appends or grows the tail,
    /// never moves existing windows.
    pub fn reserve(&self, additional: usize) {
        let capacity = self.chunk_capacity().max(1);
        let mut chunks = self.chunks.write();
        if chunks.is_empty() || chunks.last().is_some_and(|c| c.row_count >= capacity) {
            let offset = Self::column_len(&chunks);
            chunks.push(ColumnChunk::new(offset, 0, &self.data_type, self.nullable));
        }
        if let Some(chunk) = chunks.last() {
            let mut state = chunk.write_state();
            state.raw.as_storage_mut().reserve(additional);
            state.visibility.ensure_len(chunk.row_count);
            if let Some(chains) = state.version_chains.as_mut() {
                if chains.len() < chunk.row_count {
                    chains.resize(chunk.row_count, Vec::new());
                }
                chains.reserve(additional);
            }
        }
        drop(chunks);
        let needed =
            (self.len() + additional).div_ceil(crate::persistence::dirty_page::ROWS_PER_PAGE);
        self.total_dirty_pages.fetch_max(needed, Ordering::Relaxed);
    }

    /// Grow or shrink the column to `new_count` rows.
    ///
    /// Growth only appends the tail and is safe under the shard read lock.
    /// Shrinking rewrites windows and is exclusive-only (shard write lock),
    /// like `materialize_chunks`.
    pub fn resize(&self, new_count: usize) {
        let current = self.len();
        if new_count > current {
            // Grow through the coverage path so windows stay
            // capacity-aligned and gap rows read back as null.
            self.ensure_coverage(new_count - 1);
        } else if new_count < current {
            // Shrink from the tail: drop whole windows past the cut, then
            // trim the straddling chunk and its owned buffers.
            let mut chunks = self.chunks.write();
            while let Some(chunk) = chunks.last() {
                if chunk.row_offset >= new_count {
                    chunks.pop();
                } else {
                    break;
                }
            }
            if let Some(chunk) = chunks.last_mut() {
                let keep = new_count.saturating_sub(chunk.row_offset);
                if keep < chunk.row_count {
                    chunk.row_count = keep;
                    let mut state = chunk.write_state();
                    // Encoded or evicted chunks hold no raw rows; only raw
                    // resident buffers track the window length.
                    if !state.encoding.is_encoded() && state.residency.is_resident() {
                        state.raw.as_storage_mut().resize(keep);
                    }
                    // Stale overlay entries past the cut are unreachable by
                    // reads but would pin the chunk against eviction.
                    let stale: Vec<u32> = state
                        .overlay
                        .iter()
                        .map(|(local, _)| *local)
                        .filter(|local| (*local as usize) >= keep)
                        .collect();
                    for local in stale {
                        state.overlay.remove(local);
                    }
                    state.visibility.truncate(keep);
                    if let Some(chains) = state.version_chains.as_mut() {
                        chains.truncate(keep);
                    }
                    state
                        .overflow_rows
                        .retain(|local, _| (*local as usize) < keep);
                }
            }
        }
        // MVCC side state follows the windows: grown windows read back as
        // current, truncated rows lose their history.
        let chunks = self.chunks.read();
        for chunk in chunks.iter() {
            let mut state = chunk.write_state();
            let count = chunk.row_count;
            state.visibility.ensure_len(count);
            if let Some(chains) = state.version_chains.as_mut() {
                if chains.len() < count {
                    chains.resize(count, Vec::new());
                } else {
                    chains.truncate(count);
                }
            }
            state
                .overflow_rows
                .retain(|local, _| (*local as usize) < count);
        }
        drop(chunks);
        self.total_dirty_pages.fetch_max(
            new_count.div_ceil(crate::persistence::dirty_page::ROWS_PER_PAGE),
            Ordering::Relaxed,
        );
    }

    /// Raw base value for encoding inputs and persisted buffers: overflow
    /// rows contribute their inline placeholder (the payload travels in the
    /// sidecar), all other rows read from their owning chunk.
    ///
    /// Resolves against an already-locked chunk vector.
    pub(super) fn raw_base_value_in(
        &self,
        chunks: &[ColumnChunk],
        row_idx: usize,
    ) -> Option<Value> {
        if matches!(self.data_type, DataType::String | DataType::Blob) {
            let capacity = self.chunk_capacity();
            if let Some(chunk) = chunks.get(row_idx / capacity.max(1)) {
                if row_idx >= chunk.row_offset && row_idx < chunk.row_offset + chunk.row_count {
                    let local = (row_idx - chunk.row_offset) as u32;
                    if chunk.read_state().overflow_rows.contains_key(&local) {
                        return chunk
                            .read_state()
                            .raw
                            .as_storage()
                            .get(row_idx - chunk.row_offset);
                    }
                }
            }
        }
        self.get_in(chunks, row_idx)
    }

    /// Base value for encoding inputs and persisted buffers: overflow rows
    /// contribute their inline placeholder (the payload travels in the
    /// sidecar). Point reads use `get`, which serves the side store.
    pub(super) fn encoding_base_value_in(
        &self,
        chunks: &[ColumnChunk],
        row_idx: usize,
    ) -> Option<Value> {
        if matches!(self.data_type, DataType::String | DataType::Blob) {
            let capacity = self.chunk_capacity();
            if let Some(chunk) = chunks.get(row_idx / capacity.max(1)) {
                if row_idx >= chunk.row_offset && row_idx < chunk.row_offset + chunk.row_count {
                    let local = (row_idx - chunk.row_offset) as u32;
                    if chunk.read_state().overflow_rows.contains_key(&local) {
                        return match self.data_type {
                            DataType::Blob => Some(Value::Blob(Vec::new())),
                            _ => Some(Value::string("")),
                        };
                    }
                }
            }
        }
        self.get_in(chunks, row_idx)
    }

    /// Encode row values into raw flush buffers for this column's type:
    /// `(data, offsets, null bitmap)`. The bitmap is produced only for
    /// nullable columns. Rows are streamed so peak memory stays O(row).
    fn values_into_buffers(
        &self,
        values: impl Iterator<Item = Option<Value>>,
    ) -> (Vec<u8>, Vec<u64>, Option<BitVec<u8, Lsb0>>) {
        let is_var = super::is_variable_length_type(&self.data_type);
        let elem_size = super::element_size(&self.data_type);
        let mut new_data = Vec::new();
        let mut new_offsets = Vec::new();
        let mut new_bitmap = self.nullable.then(|| BitVec::with_capacity(1024));

        for value in values {
            match value {
                Some(v) => {
                    if let Some(ref mut bm) = new_bitmap {
                        bm.push(false);
                    }
                    if is_var {
                        new_offsets.push(new_data.len() as u64);
                        match &v {
                            Value::String(s) => {
                                let bytes = s.as_bytes();
                                new_data.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
                                new_data.extend_from_slice(bytes);
                            }
                            Value::FixedString(s) => {
                                let bytes = s.as_bytes();
                                new_data.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
                                new_data.extend_from_slice(bytes);
                            }
                            _ => {
                                new_offsets.pop();
                                new_offsets.push(u64::MAX);
                            }
                        }
                    } else {
                        let start = new_data.len();
                        new_data.resize(start + elem_size, 0);
                        if let DataType::FixedString(limit) = &self.data_type {
                            let _ = super::fixed_width::write_fixed_string(
                                &mut new_data,
                                start,
                                *limit,
                                &v,
                            );
                        } else {
                            let _ = super::fixed_width::write_fixed_value(
                                &mut new_data,
                                start,
                                elem_size,
                                &v,
                            );
                        }
                    }
                }
                None => {
                    if let Some(ref mut bm) = new_bitmap {
                        bm.push(true);
                    }
                    if is_var {
                        new_offsets.push(u64::MAX);
                    }
                }
            }
        }

        (new_data, new_offsets, new_bitmap)
    }

    pub fn get_flush_data(&self) -> (Vec<u8>, Vec<u64>, Option<BitVec<u8, Lsb0>>) {
        // Fast path: concatenate owned chunk buffers. Only when every chunk
        // is resident and raw; evicted chunks (whose live encoding reads as
        // None) fall through to the row-wise path that serves snapshots.
        let chunks = self.chunks.read();
        let all_raw_resident = !chunks.is_empty()
            && chunks.iter().all(|c| {
                let state = c.read_state();
                !state.encoding.is_encoded() && state.residency.is_resident()
            });
        if all_raw_resident {
            let mut data = Vec::new();
            let mut offsets = Vec::new();
            let mut bitmap = self.nullable.then(BitVec::<u8, Lsb0>::new);
            let is_var = super::is_variable_length_type(&self.data_type);
            if is_var {
                for chunk in chunks.iter() {
                    let state = chunk.read_state();
                    let (chunk_data, chunk_offsets, _) = state.raw.as_storage().get_flush_data();
                    let base = data.len() as u64;
                    for off in chunk_offsets {
                        offsets.push(if off == u64::MAX {
                            u64::MAX
                        } else {
                            base + off
                        });
                    }
                    data.extend_from_slice(&chunk_data);
                    if let Some(bm) = bitmap.as_mut() {
                        if let Some(chunk_bits) = state.raw.as_storage().null_bitmap() {
                            bm.extend(chunk_bits.iter().by_vals());
                        }
                    }
                }
            } else {
                for chunk in chunks.iter() {
                    let state = chunk.read_state();
                    let (chunk_data, _, _) = state.raw.as_storage().get_flush_data();
                    data.extend_from_slice(&chunk_data);
                    if let Some(bm) = bitmap.as_mut() {
                        if let Some(chunk_bits) = state.raw.as_storage().null_bitmap() {
                            bm.extend(chunk_bits.iter().by_vals());
                        }
                    }
                }
            }
            return (data, offsets, bitmap);
        }

        let row_count = Self::column_len(&chunks);
        // Row-wise path: overlay/encoded bases and evicted snapshots all
        // resolve through the row reader, overflow rows as inline
        // placeholders.
        self.values_into_buffers((0..row_count).map(|row| self.raw_base_value_in(&chunks, row)))
    }

    // -----------------------------------------------------------------------
    // Chunk routing (chunk-local encodings and update overlays)
    // -----------------------------------------------------------------------

    /// Returns true when chunk-level routing is active.
    pub fn has_chunks(&self) -> bool {
        !self.chunks.read().is_empty()
    }

    /// Number of chunks (0 when chunking is inactive).
    pub fn chunk_count(&self) -> usize {
        self.chunks.read().len()
    }

    /// Index of the chunk containing `row_idx`, if chunking is active.
    pub fn chunk_index_for_row(&self, row_idx: usize) -> Option<usize> {
        let chunks = self.chunks.read();
        if chunks.is_empty() {
            return None;
        }
        let idx = row_idx / self.chunk_capacity().max(1);
        if idx < chunks.len() {
            Some(idx)
        } else {
            None
        }
    }

    /// Whether any row maps into the overflow store.
    pub(crate) fn has_overflow(&self) -> bool {
        let chunks = self.chunks.read();
        chunks
            .iter()
            .any(|c| !c.read_state().overflow_rows.is_empty())
    }

    /// Whether the chunk covering `row_idx` wants a re-encode.
    pub(crate) fn chunk_needs_recode_for_row(&self, row_idx: usize) -> bool {
        let chunks = self.chunks.read();
        let capacity = self.chunk_capacity();
        chunks
            .get(row_idx / capacity.max(1))
            .is_some_and(|chunk| chunk.needs_recode())
    }

    /// Replace the chunk vector (used when restoring per-chunk encodings).
    /// Exclusive-only (load path): it swaps whole windows.
    pub(crate) fn set_chunks(&self, chunks: Vec<ColumnChunk>) {
        *self.chunks.write() = chunks;
    }

    /// Overflow-store accessors (large-string tier).
    pub fn set_overflow_threshold(&self, threshold: usize) {
        self.overflow_threshold.store(threshold, Ordering::Relaxed);
        self.overflow_store.lock().set_threshold(threshold);
    }

    // -----------------------------------------------------------------------
    // Chunk eviction / reload
    // -----------------------------------------------------------------------

    /// Whether the chunk may be evicted: resident, encoded, with no
    /// unmerged overlay writes and no live version-chain entries. Version
    /// chains live in the segment state itself, so the chunk check is
    /// complete; callers recheck inside [`Self::evict_chunk`].
    pub fn chunk_evictable(&self, chunk_idx: usize) -> bool {
        let chunks = self.chunks.read();
        chunks
            .get(chunk_idx)
            .is_some_and(|chunk| chunk.is_evictable())
    }

    /// Synchronously load an evicted chunk and mark it hot. Returns whether
    /// a load happened. Load failures propagate as storage errors.
    ///
    /// Promotion preserves the chunk's MVCC side state (visibility,
    /// chains, zone, dirty marks, overflow mappings): only the payload
    /// (raw, encoding, overlay, residency) is rebuilt. Overlay entries that
    /// landed while the snapshot decoded win over snapshot values.
    pub fn ensure_resident(&self, chunk_idx: usize) -> StorageResult<bool> {
        let snapshot = {
            let chunks = self.chunks.read();
            match chunks.get(chunk_idx) {
                Some(chunk) => match &chunk.read_state().residency {
                    ChunkResidency::Resident => return Ok(false),
                    ChunkResidency::Evicted(snapshot) => snapshot.clone(),
                },
                None => return Ok(false),
            }
        };
        let pairs = snapshot.decode_all()?;
        // Snapshot values keyed by absolute row; overlay entries collected
        // under the write latch override them below.
        let chunks = self.chunks.read();
        let Some(chunk) = chunks.get(chunk_idx) else {
            return Ok(false);
        };
        let (row_offset, row_count) = (chunk.row_offset, chunk.row_count);
        let data_type = self.data_type.clone();
        let nullable = self.nullable;
        // Overflow rows decode as placeholders for the re-encode below,
        // mirroring the encode-time base; payloads stay in the sidecar.
        let overflow_rows: std::collections::HashSet<u32> = {
            let state = chunk.read_state();
            state.overflow_rows.keys().copied().collect()
        };
        let fresh = ColumnChunk::new(row_offset, row_count, &data_type, nullable);
        fresh.touch();
        {
            let mut fresh_state = fresh.write_state();
            for (row, value) in &pairs {
                let local = row.saturating_sub(row_offset);
                let value = if overflow_rows.contains(&(local as u32)) {
                    match data_type {
                        DataType::Blob => Some(Value::Blob(Vec::new())),
                        _ => Some(Value::string("")),
                    }
                } else {
                    value.clone()
                };
                let _ = fresh_state.raw.as_storage_mut().set(local, value.as_ref());
            }
        }
        // Restore the pre-evict encoding from the same values so promotion
        // returns the chunk to its encoded form instead of a raw copy.
        // Profiles (min/max/raw size) come from the snapshot; counts and
        // the compressed size reflect the fresh encoding.
        if snapshot.encoding != crate::encoding::EncodingType::None {
            let values: Vec<Option<Value>> = pairs
                .iter()
                .map(|(row, value)| {
                    let local = row.saturating_sub(row_offset) as u32;
                    if overflow_rows.contains(&local) {
                        match data_type {
                            DataType::Blob => Some(Value::Blob(Vec::new())),
                            _ => Some(Value::string("")),
                        }
                    } else {
                        value.clone()
                    }
                })
                .collect();
            let encoded = Self::encode_slice(&values, &data_type, snapshot.encoding, 255);
            if encoded.is_encoded() {
                let num_values = values.iter().filter(|v| v.is_some()).count() as u32;
                let mut fresh_state = fresh.write_state();
                fresh_state.encoding = encoded;
                fresh_state.encoding_meta = snapshot.meta.clone();
                fresh_state.encoding_meta.scheme = fresh_state.encoding.encoding_type();
                fresh_state.encoding_meta.num_values = num_values;
                fresh_state.encoding_meta.all_null = num_values == 0;
                fresh_state.encoding_meta.compressed_size =
                    fresh_state.encoding.memory_usage() as u64;
            }
        }
        // Publish under the segment write latch, merging overlay entries
        // that landed during the decode so no concurrent write is lost.
        // Only the payload fields move; MVCC side state stays in place.
        let mut state = chunk.write_state();
        if state.residency.is_resident() {
            // Another promoter won the race; keep its result.
            return Ok(false);
        }
        let mut fresh_state = fresh.state.into_inner();
        for (local, value) in state.overlay.iter() {
            let _ = fresh_state
                .raw
                .as_storage_mut()
                .set(*local as usize, value.as_ref());
        }
        state.raw = fresh_state.raw;
        state.encoding = fresh_state.encoding;
        state.encoding_meta = fresh_state.encoding_meta;
        if state.overlay.len() != 0 && state.encoding.is_encoded() {
            // Overlay writes landed during the decode and were merged into
            // the raw buffers above, but the restored encoding was built
            // from snapshot values only. Drop the encoding so the merged
            // raw buffers stay authoritative; the next encode pass relearns.
            state.encoding = ColumnEncoding::None;
            state.encoding_meta = crate::encoding::ChunkEncodingMeta::default();
        }
        state.updates_since_encode = 0;
        state.overlay.clear();
        state.residency = ChunkResidency::Resident;
        Ok(true)
    }

    /// Batch miss-load: promote every evicted chunk covering `rows` once
    /// before a grouped decode, avoiding per-row page faults. Returns the
    /// number of chunks loaded. Background batch loads behind this entry are
    /// charged against the task quota in [`super::MAX_BACKGROUND_LOAD_CHUNKS`]
    /// segments.
    pub fn ensure_resident_range(&self, rows: &[usize]) -> StorageResult<usize> {
        let mut loaded = 0usize;
        let mut remaining = rows.to_vec();
        while !remaining.is_empty() {
            let (n, rest) = self
                .ensure_resident_range_with_quota(&remaining, super::MAX_BACKGROUND_LOAD_CHUNKS)?;
            if n == 0 {
                break;
            }
            loaded += n;
            remaining = rest;
        }
        Ok(loaded)
    }

    /// Promote every evicted chunk. Used by encoding passes and flush
    /// snapshots so persisted output keeps full fidelity; the source table
    /// is untouched when this runs on its flush-time clone.
    pub fn ensure_all_resident(&self) -> StorageResult<usize> {
        let mut loaded = 0usize;
        let len = self.chunks.read().len();
        for idx in 0..len {
            if self.ensure_resident(idx)? {
                loaded += 1;
            }
        }
        Ok(loaded)
    }

    /// Release one cold chunk's decoded buffers, spilling the compressed
    /// snapshot to a spill file and retaining only row range, encoding
    /// scheme, and profiles in memory. Returns bytes released, or 0 when
    /// the chunk is not evictable. Zone maps stay resident in the segment
    /// state and keep serving; HLL stays at the column level. A spill
    /// failure reports an error so the caller skips the chunk instead of
    /// retaining heap pages.
    ///
    /// Exclusive-only (shard write lock): point writes never run
    /// concurrently, but evictability is still rechecked under the segment
    /// write latch so a racing promotion abandons the chunk for this pass.
    pub fn evict_chunk(&self, chunk_idx: usize) -> StorageResult<u64> {
        if !self.chunk_evictable(chunk_idx) {
            return Ok(0);
        }
        let chunks = self.chunks.read();
        let Some(chunk) = chunks.get(chunk_idx) else {
            return Ok(0);
        };
        let (start, count, encoding, meta) = {
            let state = chunk.read_state();
            (
                chunk.row_offset,
                chunk.row_count,
                state.encoding.encoding_type(),
                state.encoding_meta.clone(),
            )
        };
        let values: Vec<Option<Value>> = (start..start.saturating_add(count))
            .map(|row| self.get_in(&chunks, row))
            .collect();
        let snapshot = EvictedSnapshot::capture(start, values, encoding, meta)?;
        let mut state = chunk.write_state();
        // Recheck under the latch: a racing promotion or point write
        // abandons this chunk for this pass.
        if state.residency.is_evicted()
            || !state.encoding.is_encoded()
            || state.overlay.len() != 0
            || state
                .version_chains
                .as_ref()
                .is_some_and(|chains| chains.iter().any(|chain| !chain.is_empty()))
        {
            return Ok(0);
        }
        let released =
            (state.raw.as_storage().memory_usage() + state.encoding.memory_usage()) as u64;
        state.raw.as_storage_mut().clear();
        state.encoding = ColumnEncoding::None;
        state.residency = ChunkResidency::Evicted(snapshot);
        Ok(released)
    }

    /// Evict resident cold chunks oldest-first until `budget` bytes are
    /// released. Returns `(chunks_evicted, bytes_released)`. Per-chunk
    /// failures are skipped with a warning so one corruptible chunk never
    /// blocks the watermark pass.
    pub fn evict_cold_chunks(&self, budget: u64) -> (usize, u64) {
        let (evicted, freed, _) = self.evict_cold_chunks_with_quota(budget, u64::MAX);
        (evicted, freed)
    }

    /// Quota-segmented eviction for background tasks.
    ///
    /// `budget` is the total release target; `task_quota` caps one segment so
    /// over-quota background work proceeds in segments instead of one burst.
    /// Returns `(chunks_evicted, bytes_released, segments)`. Each segment
    /// re-selects evictable chunks oldest-first and rechecks evictability
    /// inside [`Self::evict_chunk`], so writes landing during confirmation
    /// abandon that chunk for this pass.
    pub fn evict_cold_chunks_with_quota(
        &self,
        budget: u64,
        task_quota: u64,
    ) -> (usize, u64, usize) {
        let chunks = self.chunks.read();
        if budget == 0 || chunks.is_empty() {
            return (0, 0, 0);
        }
        drop(chunks);
        let segment = task_quota.clamp(1, EVICTION_SEGMENT_BYTES).min(budget);
        let mut count = 0usize;
        let mut freed = 0u64;
        let mut segments = 0usize;
        while freed < budget {
            let target = freed.saturating_add(segment).min(budget);
            let order: Vec<(u64, usize)> = {
                let chunks = self.chunks.read();
                let mut order: Vec<(u64, usize)> = (0..chunks.len())
                    .filter(|&idx| chunks.get(idx).is_some_and(|chunk| chunk.is_evictable()))
                    .map(|idx| (chunks[idx].last_access.load(Ordering::Relaxed), idx))
                    .collect();
                order.sort_unstable();
                order
            };
            segments += 1;
            let mut progress = false;
            for (_, idx) in order {
                // Recheck under the pass: a racing promotion or write
                // abandons the chunk via the in-evict recheck.
                if !self.chunk_evictable(idx) {
                    continue;
                }
                if freed >= target {
                    break;
                }
                match self.evict_chunk(idx) {
                    Ok(0) => {}
                    Ok(released) => {
                        count += 1;
                        freed += released;
                        progress = true;
                    }
                    Err(e) => {
                        log::warn!("chunk eviction skipped for {}[{}]: {}", self.name, idx, e);
                    }
                }
            }
            if !progress {
                break;
            }
        }
        (count, freed, segments)
    }

    /// Quota-capped batch promotion for background scans.
    ///
    /// Loads at most `max_chunks` evicted chunks covering `rows`, returning
    /// `(chunks_loaded, remaining_rows)`. Callers with a task memory quota
    /// process the loaded prefix, release pressure, then continue with the
    /// remainder instead of promoting the whole working set at once.
    pub fn ensure_resident_range_with_quota(
        &self,
        rows: &[usize],
        max_chunks: usize,
    ) -> StorageResult<(usize, Vec<usize>)> {
        let mut idxs: Vec<usize> = rows
            .iter()
            .filter_map(|row| self.chunk_index_for_row(*row))
            .collect();
        idxs.sort_unstable();
        idxs.dedup();
        let mut loaded = 0usize;
        let mut done_through = 0usize;
        for (position, idx) in idxs.iter().enumerate() {
            if loaded >= max_chunks {
                break;
            }
            if self.ensure_resident(*idx)? {
                loaded += 1;
            }
            done_through = position + 1;
        }
        let remaining: Vec<usize> = if done_through >= idxs.len() {
            Vec::new()
        } else {
            let pending: std::collections::HashSet<usize> =
                idxs[done_through..].iter().copied().collect();
            rows.iter()
                .copied()
                .filter(|row| {
                    self.chunk_index_for_row(*row)
                        .is_some_and(|idx| pending.contains(&idx))
                })
                .collect()
        };
        Ok((loaded, remaining))
    }

    /// Unified buffer ledger for this column in one pass: resident bytes
    /// (decoded heap including the overflow side store), retained
    /// eviction-snapshot bytes, the overflow subset, dirty pages and chunk
    /// counts. One chunks read plus one overflow lock; eviction quotas
    /// and observability share these totals instead of three separate
    /// tallies.
    pub fn buffer_ledger(&self) -> BufferLedger {
        let chunks = self.chunks.read();
        let mut resident_chunks = 0usize;
        let mut evicted_chunks = 0usize;
        let mut evicted_bytes = 0usize;
        let mut dirty_pages = 0usize;
        for chunk in chunks.iter() {
            let state = chunk.read_state();
            if state.residency.is_resident() {
                resident_chunks += 1;
            } else {
                evicted_chunks += 1;
            }
            if let Some(snapshot) = state.residency.evicted_snapshot() {
                evicted_bytes += snapshot.compressed_bytes();
            }
            dirty_pages += state.dirty_pages.len();
        }
        let overflow_bytes = self.overflow_store.lock().memory_usage();
        let resident_bytes = self.memory_usage().saturating_sub(evicted_bytes);
        BufferLedger {
            resident_bytes,
            evicted_bytes,
            overflow_bytes,
            dirty_pages,
            resident_chunks,
            evicted_chunks,
        }
    }

    /// Resident decoded bytes (excludes retained eviction snapshots).
    pub fn resident_memory_usage(&self) -> usize {
        self.buffer_ledger().resident_bytes
    }

    /// Chunk indexes whose overlay load makes them recode candidates.
    pub fn pending_recode_chunks(&self) -> Vec<usize> {
        let chunks = self.chunks.read();
        chunks
            .iter()
            .enumerate()
            .filter(|(_, c)| c.needs_recode())
            .map(|(idx, _)| idx)
            .collect()
    }

    /// Early merge for one hot chunk, before its overlay fills.
    ///
    /// Raw chunks merge cheaply: overlay values are written back to the raw
    /// buffer and the overlay clears, so later writes stay in place.
    /// Encoded chunks are only marked hot here; the actual re-encode stays
    /// with the flush path, which has the full chunk profile. Returns
    /// whether any merge or hot-marking happened. Never fails the write:
    /// merge errors leave the overlay intact.
    ///
    /// The overlay is collected before merging so the segment latch is never
    /// held across the raw write path (container-before-member order).
    pub fn maybe_merge_hot_chunk(&self, chunk_idx: usize) -> bool {
        let chunks = self.chunks.read();
        let Some(chunk) = chunks.get(chunk_idx) else {
            return false;
        };
        let (overlay_len, updates, encoded, evicted, half_budget, row_offset) = {
            let state = chunk.read_state();
            (
                state.overlay.len(),
                state.updates_since_encode,
                state.encoding.is_encoded(),
                !state.residency.is_resident(),
                (state.overlay.capacity() / 2).max(1),
                chunk.row_offset,
            )
        };
        if evicted {
            return false;
        }
        if overlay_len < half_budget && updates < half_budget as u64 {
            return false;
        }
        if !encoded {
            let merged: Vec<(u32, Option<Value>)> = chunk
                .read_state()
                .overlay
                .iter()
                .map(|(k, v)| (*k, v.clone()))
                .collect();
            drop(chunks);
            let mut applied: std::collections::HashSet<u32> = std::collections::HashSet::new();
            for (local, value) in &merged {
                let row = row_offset + *local as usize;
                if self.write_raw_inner(row, value.as_ref()).is_ok() {
                    applied.insert(*local);
                }
            }
            if !applied.is_empty() {
                let chunks = self.chunks.read();
                if let Some(chunk) = chunks.get(chunk_idx) {
                    let mut state = chunk.write_state();
                    // Only clear entries this pass merged: entries that
                    // landed during the merge, or whose write failed, stay
                    // for the next pass.
                    for (local, value) in &merged {
                        if !applied.contains(local) {
                            continue;
                        }
                        let same = state
                            .overlay
                            .get(*local)
                            .is_some_and(|current| &current == value);
                        if same {
                            state.overlay.remove(*local);
                        }
                    }
                    if state.overlay.len() == 0 {
                        state.updates_since_encode = 0;
                    }
                }
                return true;
            }
            return false;
        }
        drop(chunks);
        let chunks = self.chunks.read();
        if let Some(chunk) = chunks.get(chunk_idx) {
            if !chunk.needs_recode() {
                chunk.mark_hot();
                return true;
            }
        }
        false
    }

    /// Compressed snapshot payload bytes for evicted chunks, wherever they
    /// live (heap or spill files). Used for eviction observability; spilled
    /// bytes no longer count toward heap memory.
    pub fn evicted_bytes(&self) -> usize {
        self.buffer_ledger().evicted_bytes
    }

    /// Chunks with decoded data in memory.
    pub fn resident_chunk_count(&self) -> usize {
        self.buffer_ledger().resident_chunks
    }

    /// Chunks released with only the snapshot retained.
    pub fn evicted_chunk_count(&self) -> usize {
        self.buffer_ledger().evicted_chunks
    }

    /// Base value for encoding inputs and persisted buffers: overflow rows
    /// contribute their inline placeholder (the payload travels in the
    /// sidecar). Point reads use `get`, which serves the side store.
    pub(super) fn encoding_base_value(&self, row_idx: usize) -> Option<Value> {
        let chunks = self.chunks.read();
        self.encoding_base_value_in(&chunks, row_idx)
    }

    /// Collect every overflow mapping as global rows. The segment latch is
    /// released before touching the side store.
    pub(super) fn collect_overflow_rows(&self) -> Vec<(usize, OverflowHandle)> {
        let chunks = self.chunks.read();
        let mut rows = Vec::new();
        for chunk in chunks.iter() {
            let state = chunk.read_state();
            rows.extend(
                state
                    .overflow_rows
                    .iter()
                    .map(|(local, handle)| (chunk.row_offset + *local as usize, *handle)),
            );
        }
        rows
    }

    /// Rebuild the overflow store from live rows only (flush-time GC).
    /// Exclusive-only (flush path): it replaces the store and repartitions
    /// every mapping.
    pub fn rebuild_overflow(&self) {
        let rows = self.collect_overflow_rows();
        if rows.is_empty() {
            return;
        }
        let threshold = self.overflow_threshold();
        // Main buffers hold placeholders for overflow rows, so payloads are
        // re-read from the side store itself; rows whose payload vanished
        // (overwritten with a short value since) are dropped.
        let mut live_rows: Vec<usize> = Vec::new();
        let mut live_payloads: Vec<Vec<u8>> = Vec::new();
        {
            let store = self.overflow_store.lock();
            for (row, handle) in &rows {
                match store.get(handle) {
                    Some(payload) if payload.len() > threshold => {
                        live_rows.push(*row);
                        live_payloads.push(payload);
                    }
                    _ => {}
                }
            }
        }
        let mut order: Vec<usize> = (0..live_rows.len()).collect();
        order.sort_by_key(|&i| live_rows[i]);
        let sorted_payloads: Vec<Vec<u8>> =
            order.iter().map(|&i| live_payloads[i].clone()).collect();
        self.overflow_store
            .lock()
            .rebuild_from_live(&sorted_payloads);
        // Rebuild preserves row order, so entry ids follow the sorted rows.
        let mut sorted_rows: Vec<usize> = live_rows;
        sorted_rows.sort_unstable();
        let capacity = self.chunk_capacity();
        let chunks = self.chunks.read();
        for chunk in chunks.iter() {
            chunk.write_state().overflow_rows.clear();
        }
        for (entry_id, row) in sorted_rows.into_iter().enumerate() {
            let handle = OverflowHandle {
                entry_id: entry_id as u32,
            };
            if let Some(chunk) = chunks.get(row / capacity.max(1)) {
                if row >= chunk.row_offset && row < chunk.row_offset + chunk.row_count {
                    chunk
                        .write_state()
                        .overflow_rows
                        .insert((row - chunk.row_offset) as u32, handle);
                }
            }
        }
    }

    /// Serialize overflow state for the `<col>.overflow` sidecar.
    pub fn serialize_overflow(&self) -> StorageResult<Vec<u8>> {
        let mut store_buf = Vec::new();
        {
            let store = self.overflow_store.lock();
            store.flush_to_sidecar_buffer(&mut store_buf)?;
        }
        let mut rows = self.collect_overflow_rows();
        let mut buf = Vec::new();
        buf.push(1u8);
        buf.extend_from_slice(&(store_buf.len() as u32).to_le_bytes());
        buf.extend_from_slice(&store_buf);
        buf.extend_from_slice(&(rows.len() as u32).to_le_bytes());
        rows.sort_by_key(|(row, _)| *row);
        for (row, handle) in rows {
            buf.extend_from_slice(&(row as u32).to_le_bytes());
            buf.extend_from_slice(&handle.entry_id.to_le_bytes());
        }
        let crc = crc32fast::hash(&buf);
        buf.extend_from_slice(&crc.to_le_bytes());
        Ok(buf)
    }

    /// Restore overflow state from a sidecar buffer.
    pub fn load_overflow_bytes(&self, bytes: &[u8]) -> StorageResult<()> {
        use std::io::Read;
        if bytes.len() < 10 {
            return Err(StorageError::deserialize_error(
                "overflow sidecar too small".to_string(),
            ));
        }
        let crc_tail: [u8; 4] = bytes[bytes.len() - 4..].try_into().map_err(|_| {
            StorageError::deserialize_error("overflow sidecar CRC tail malformed".to_string())
        })?;
        let stored_crc = u32::from_le_bytes(crc_tail);
        let computed = crc32fast::hash(&bytes[..bytes.len() - 4]);
        if stored_crc != computed {
            return Err(StorageError::deserialize_error(format!(
                "overflow sidecar CRC mismatch: stored={:#x} computed={:#x}",
                stored_crc, computed
            )));
        }
        let mut cursor = &bytes[..bytes.len() - 4];
        let mut ver = [0u8; 1];
        cursor.read_exact(&mut ver)?;
        if ver[0] != 1 {
            return Err(StorageError::deserialize_error(format!(
                "unsupported overflow version {}",
                ver[0]
            )));
        }
        let mut len_buf = [0u8; 4];
        cursor.read_exact(&mut len_buf)?;
        let store_len = u32::from_le_bytes(len_buf) as usize;
        if store_len > cursor.len() {
            return Err(StorageError::deserialize_error(
                "overflow sidecar truncated".to_string(),
            ));
        }
        let mut store = OverflowStore::new(self.overflow_threshold());
        store.load_from_bytes(&cursor[..store_len])?;
        cursor = &cursor[store_len..];
        cursor.read_exact(&mut len_buf)?;
        let map_len = u32::from_le_bytes(len_buf) as usize;
        let mut rows = HashMap::new();
        for _ in 0..map_len {
            let mut b = [0u8; 4];
            cursor.read_exact(&mut b)?;
            let row = u32::from_le_bytes(b) as usize;
            cursor.read_exact(&mut b)?;
            let entry = u32::from_le_bytes(b);
            rows.insert(row, OverflowHandle { entry_id: entry });
        }
        *self.overflow_store.lock() = store;
        // Repartition the global mappings into their owning segments.
        let capacity = self.chunk_capacity();
        let chunks = self.chunks.read();
        for chunk in chunks.iter() {
            chunk.write_state().overflow_rows.clear();
        }
        for (row, handle) in rows {
            if let Some(chunk) = chunks.get(row / capacity.max(1)) {
                if row >= chunk.row_offset && row < chunk.row_offset + chunk.row_count {
                    chunk
                        .write_state()
                        .overflow_rows
                        .insert((row - chunk.row_offset) as u32, handle);
                }
            }
        }
        Ok(())
    }

    /// Encode one row slice into a chunk-local encoding of the given type.
    /// A failed or infeasible build yields `None` (the chunk stays raw).
    fn encode_slice(
        values: &[Option<Value>],
        data_type: &DataType,
        encoding_type: crate::encoding::EncodingType,
        fsst_max_symbols: usize,
    ) -> ColumnEncoding {
        if values.is_empty() {
            return ColumnEncoding::None;
        }
        Self::build_chunk_encoding(data_type, values, encoding_type, fsst_max_symbols)
            .unwrap_or(ColumnEncoding::None)
    }

    /// Enforce the chunk window invariant: windows are contiguous from row
    /// zero and no chunk exceeds `chunk_capacity`.
    ///
    /// Writes and loads maintain this shape directly, so this is normally a
    /// no-op. It does real work only after a capacity shrink strands an
    /// oversized chunk (split at the capacity boundary) and it drops
    /// reserved zero-row tail chunks left behind by `reserve`.
    ///
    /// Exclusive-only: rewriting windows while point operations route by
    /// them would misroute reads and writes. All callers run under the
    /// shard write lock.
    pub fn materialize_chunks(&self) {
        let mut chunks = self.chunks.write();
        while chunks.last().is_some_and(|chunk| chunk.row_count == 0) {
            chunks.pop();
        }
        let capacity = self.chunk_capacity().max(1);
        let mut idx = 0;
        while idx < chunks.len() {
            if chunks[idx].row_count <= capacity {
                idx += 1;
                continue;
            }
            let offset = chunks[idx].row_offset;
            let at = capacity.min(chunks[idx].row_count);
            let right_count = chunks[idx].row_count - at;
            // Splits operate on raw buffers: decode first so the cut never
            // drops overlay entries (decode merges them into the base).
            self.decode_locked(&chunks, idx);
            let right_state = {
                let chunk = &chunks[idx];
                let mut state = chunk.write_state();
                debug_assert_eq!(state.overlay.len(), 0);
                let (left_raw, right_raw) = Self::split_raw(&state.raw, at);
                state.raw = left_raw;
                let right_chains = state.version_chains.as_mut().map(|c| c.split_off(at));
                let right_vis = state.visibility.split_off(at);
                // Dirty pages are global ids: a page stays with the side
                // holding its first row. A straddling page stays left; its
                // flush covers both sides at page granularity.
                let cut_row = offset + at;
                let mut right_dirty = BTreeSet::new();
                state.dirty_pages.retain(|page| {
                    if (*page as usize)
                        .saturating_mul(crate::persistence::dirty_page::ROWS_PER_PAGE)
                        < cut_row
                    {
                        true
                    } else {
                        right_dirty.insert(*page);
                        false
                    }
                });
                let mut right_overflow = HashMap::new();
                state.overflow_rows.retain(|local, handle| {
                    if (*local as usize) < at {
                        true
                    } else {
                        right_overflow.insert(*local - at as u32, *handle);
                        false
                    }
                });
                crate::vertex::column::chunk::ChunkState {
                    raw: right_raw,
                    encoding: ColumnEncoding::None,
                    overlay: super::chunk_encoding::UpdateOverlay::new(
                        super::chunk_encoding::overlay_capacity_for(right_count),
                    ),
                    encoding_meta: crate::encoding::ChunkEncodingMeta::default(),
                    updates_since_encode: 0,
                    version_chains: right_chains,
                    visibility: right_vis,
                    dirty_pages: right_dirty,
                    overflow_rows: right_overflow,
                    residency: ChunkResidency::Resident,
                }
            };
            chunks[idx].row_count = at;
            let right = ColumnChunk {
                row_offset: offset + at,
                row_count: right_count,
                element_size: chunks[idx].element_size,
                state: RwLock::new(right_state),
                last_access: AtomicU64::new(next_tick()),
            };
            chunks.insert(idx + 1, right);
            idx += 1;
        }
    }

    // -----------------------------------------------------------------------
    // Chunk-level encoding
    // -----------------------------------------------------------------------

    /// Per-chunk flush view: window plus cloned payload descriptors for
    /// persistence serialization. The clones are taken under one segment
    /// read latch each so the serialized record is chunk-consistent.
    /// Evicted chunks (possible only when flush-time promotion failed) are
    /// materialized row-wise from their snapshot into raw buffers.
    pub(crate) fn chunk_flush_view(&self, idx: usize) -> Option<super::ChunkFlushView> {
        let chunks = self.chunks.read();
        let chunk = chunks.get(idx)?;
        let state = chunk.read_state();
        let raw_form = !state.encoding.is_encoded();
        let (raw_data, raw_offsets, raw_bitmap) = if !raw_form {
            (Vec::new(), Vec::new(), None)
        } else if state.residency.is_evicted() {
            let (data, offsets, bitmap) = self.values_into_buffers(
                (chunk.row_offset..chunk.row_offset + chunk.row_count)
                    .map(|row| self.raw_base_value_in(&chunks, row)),
            );
            (data, offsets, bitmap)
        } else {
            state.raw.as_storage().get_flush_data()
        };
        let overlay: Vec<(u32, Option<Value>)> = if state.residency.is_evicted() {
            // Evicted windows carry no live overlay (writes promote first),
            // and the materialized buffers above already merged any values.
            Vec::new()
        } else {
            state.overlay.iter().map(|(k, v)| (*k, v.clone())).collect()
        };
        Some(super::ChunkFlushView {
            row_offset: chunk.row_offset,
            row_count: chunk.row_count,
            encoding_meta: state.encoding_meta.clone(),
            encoding: state.encoding.clone(),
            overlay,
            raw_form,
            raw_data,
            raw_offsets,
            raw_bitmap,
        })
    }

    /// Every chunk's flush view, in row order.
    pub(crate) fn chunk_flush_views(&self) -> Vec<super::ChunkFlushView> {
        let count = self.chunks.read().len();
        (0..count)
            .filter_map(|idx| self.chunk_flush_view(idx))
            .collect()
    }

    /// Per-chunk encoding metadata: (chunk_idx, encoding type, row count).
    /// Evicted chunks report their pre-evict scheme so sidecars and chunk
    /// profiles keep describing the flushed layout.
    pub fn chunk_encoding_metadata(&self) -> Vec<(usize, crate::encoding::EncodingType, usize)> {
        let chunks = self.chunks.read();
        chunks
            .iter()
            .enumerate()
            .map(|(i, c)| (i, c.evicted_encoding(), c.row_count))
            .collect()
    }

    /// Re-evict one resident chunk from checkpoint sidecar pages without
    /// decoding them onto the heap. Matches by row window; a mismatch or
    /// an already-evicted chunk keeps current state and reports false so
    /// the caller stays resident. Used by reload to restore the persisted
    /// eviction state. Exclusive-only (load path).
    pub fn restore_mapped_chunk(
        &self,
        record: super::chunk_residency::MappedChunk,
        map: &std::sync::Arc<memmap2::Mmap>,
    ) -> bool {
        let chunks = self.chunks.read();
        let Some(chunk) = chunks.iter().find(|c| {
            c.row_offset == record.row_offset as usize && c.row_count == record.rows as usize
        }) else {
            log::warn!(
                "snapshot sidecar window [{}, {}) matches no chunk of column {}",
                record.row_offset,
                record.row_offset as usize + record.rows as usize,
                self.name,
            );
            return false;
        };
        let mut state = chunk.write_state();
        if !state.residency.is_resident() {
            return false;
        }
        let snapshot = super::chunk_residency::EvictedSnapshot::from_mapped(
            record.rows,
            record.encoding,
            record.meta,
            record.uncompressed_bytes,
            map.clone(),
            record.frames,
        );
        state.raw.as_storage_mut().clear();
        state.encoding = crate::encoding::ColumnEncoding::None;
        state.residency = ChunkResidency::Evicted(snapshot);
        true
    }

    /// Apply one selected encoding to this column, chunk by chunk.
    ///
    /// Each resident chunk selects and stores its own encoding so point
    /// updates only decode the affected chunk. Exclusive-only (flush/encode
    /// path): it rewrites chunk payloads wholesale.
    pub fn apply_selected_encoding(
        &self,
        encoding_type: crate::encoding::EncodingType,
        fsst_max_symbols: usize,
    ) -> StorageResult<()> {
        if self.is_empty() {
            return Ok(());
        }
        self.apply_encoding_to_chunks(encoding_type, fsst_max_symbols)
    }

    /// Apply an encoding type independently per chunk.
    ///
    /// Each chunk is resolved and encoded in isolation (bounded by chunk
    /// capacity): no full-column value vector is ever materialized. When a
    /// chunk selects `None`, its overlay-merged values are written back to
    /// the raw buffer before the overlay is cleared so no update is lost.
    pub fn apply_encoding_to_chunks(
        &self,
        encoding_type: crate::encoding::EncodingType,
        fsst_max_symbols: usize,
    ) -> StorageResult<()> {
        // Bind the check so the read guard drops before materialize_chunks:
        // parking_lot locks are not reentrant.
        let empty = self.chunks.read().is_empty();
        if empty {
            self.materialize_chunks();
        }
        if self.chunks.read().is_empty() {
            return Ok(());
        }
        // Encoding rebuilds need decoded buffers: promote evicted chunks
        // first so every chunk below is resident. Load failures propagate
        // instead of silently encoding a partial column.
        let len = self.chunks.read().len();
        for idx in 0..len {
            self.ensure_resident(idx)?;
        }
        let total = self.len();
        let len = self.chunks.read().len();
        for idx in 0..len {
            let (start, end) = {
                let chunks = self.chunks.read();
                let Some(chunk) = chunks.get(idx) else {
                    continue;
                };
                let start = chunk.row_offset;
                (start, start.saturating_add(chunk.row_count).min(total))
            };
            // Per-chunk slice only (overlay-merged base; overflow rows as
            // placeholders whose payloads stay in the sidecar).
            let values: Vec<Option<Value>> =
                (start..end).map(|r| self.encoding_base_value(r)).collect();
            let selected = match encoding_type {
                crate::encoding::EncodingType::None => crate::encoding::EncodingType::None,
                _ => {
                    let selector = crate::encoding::EncodingSelector::default();
                    selector.select_for_chunk(&self.data_type, &values)
                }
            };
            let chunks = self.chunks.read();
            let Some(chunk) = chunks.get(idx) else {
                continue;
            };
            if selected == crate::encoding::EncodingType::None {
                // Preserve overlay-merged values: the raw buffer still holds
                // the pre-overlay base, so write merged values back first.
                drop(chunks);
                for (off, v) in values.iter().enumerate() {
                    let row = start + off;
                    let _ = self.write_raw_inner(row, v.as_ref());
                }
                let chunks = self.chunks.read();
                if let Some(chunk) = chunks.get(idx) {
                    let mut state = chunk.write_state();
                    state.encoding = ColumnEncoding::None;
                    state.overlay.clear();
                    state.updates_since_encode = 0;
                    state.encoding_meta.scheme = crate::encoding::EncodingType::None;
                    state.encoding_meta.num_values =
                        values.iter().filter(|v| v.is_some()).count() as u32;
                    state.encoding_meta.all_null = state.encoding_meta.num_values == 0;
                }
                continue;
            }
            // Encode this chunk slice in isolation (chunk-local indexes).
            let encoded = Self::encode_slice(&values, &self.data_type, selected, fsst_max_symbols);
            if encoded.is_encoded() {
                let num_values = values.iter().filter(|v| v.is_some()).count() as u32;
                let mut state = chunk.write_state();
                state.encoding = encoded;
                state.overlay.clear();
                state.updates_since_encode = 0;
                state.encoding_meta.scheme = state.encoding.encoding_type();
                state.encoding_meta.num_values = num_values;
                state.encoding_meta.all_null = num_values == 0;
                state.encoding_meta.compressed_size = state.encoding.memory_usage() as u64;
            }
        }
        Ok(())
    }
}
