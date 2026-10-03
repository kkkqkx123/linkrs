use graphdb_core::types::Timestamp;
use graphdb_core::{DataType, StorageError, StorageResult, Value};

use crate::column_stats::ColumnStats;
use crate::encoding::ColumnEncoding;
use crate::stats::HyperLogLog;

use super::chunk::{ChunkState, ColumnChunk, DEFAULT_CHUNK_ROWS};
use super::chunk_residency::ChunkResidency;
use super::fixed_width::FixedWidthColumn;
use super::overflow::{OverflowHandle, OverflowStore, DEFAULT_OVERFLOW_THRESHOLD};
use super::variable_width::VariableWidthColumn;

use bitvec::prelude::*;
use parking_lot::{Mutex, RwLock};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

/// Unified column storage interface.
pub trait ColumnStorage: Send + Sync + std::fmt::Debug {
    fn get(&self, row_idx: usize) -> Option<Value>;
    /// Strict read distinguishing never-written windows from corrupt payloads.
    ///
    /// Never-written rows (out of range, gaps, null slots) yield `Ok(None)`.
    /// Truncated buffers, length or dimension mismatches and undecodable
    /// opaque payloads yield `Err` so queries never observe silent nulls.
    fn try_get(&self, row_idx: usize) -> StorageResult<Option<Value>> {
        Ok(self.get(row_idx))
    }
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
        offsets: Vec<u32>,
        null_bitmap_raw: Option<Vec<u8>>,
        bitmap_bit_len: usize,
    );
    fn get_flush_data(&self) -> (Vec<u8>, Vec<u32>, Option<BitVec<u8, Lsb0>>);
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
    /// Cached null count invalidated on every write. Reads populate it on
    /// the slow path so repeated statistics passes pay one scan per epoch.
    pub(super) null_count_cache: parking_lot::RwLock<Option<usize>>,
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
            null_count_cache: parking_lot::RwLock::new(*self.null_count_cache.read()),
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
            null_count_cache: parking_lot::RwLock::new(None),
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
                let mut left_offsets: Vec<u32> = Vec::new();
                let mut left_bits: Option<BitVec<u8, Lsb0>> =
                    var.null_bitmap.as_ref().map(|_| BitVec::new());
                let mut right_data = Vec::new();
                let mut right_offsets: Vec<u32> = Vec::new();
                let mut right_bits: Option<BitVec<u8, Lsb0>> =
                    var.null_bitmap.as_ref().map(|_| BitVec::new());
                let bits: Vec<bool> = var
                    .null_bitmap
                    .as_ref()
                    .map(|b| b.iter().by_vals().collect())
                    .unwrap_or_default();
                for local in 0..var.row_count {
                    let off = var.offsets.get(local).copied().unwrap_or(u32::MAX) as usize;
                    let bit = bits.get(local).copied().unwrap_or(false);
                    let (data, offsets, bits) = if local < at {
                        (&mut left_data, &mut left_offsets, &mut left_bits)
                    } else {
                        (&mut right_data, &mut right_offsets, &mut right_bits)
                    };
                    if off == u32::MAX as usize || off + 8 > var.data.len() {
                        offsets.push(u32::MAX);
                    } else {
                        let len_bytes: [u8; 8] =
                            var.data[off..off + 8].try_into().unwrap_or([0u8; 8]);
                        let len = u64::from_le_bytes(len_bytes) as usize;
                        if off + 8 + len > var.data.len() {
                            offsets.push(u32::MAX);
                        } else {
                            offsets.push(data.len() as u32);
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
        // Unified overflow routing by payload bytes happens before encoding
        // checks so the main buffers only ever hold small payloads plus
        // placeholders. The side store stays a short critical section: one
        // lock for the threshold check plus handle allocation, never nesting
        // back into a segment latch. Segment work below only updates row
        // payloads and metadata. Threshold stays per-column configurable;
        // `usize::MAX` disables routing with the same small-inline behavior.
        if super::overflow::OverflowStore::routes_for(&self.data_type) {
            let payload: Option<Vec<u8>> = match value {
                Some(v) if !v.is_null() => super::overflow::overflow_payload_bytes(v),
                _ => None,
            };
            if let Some(bytes) = payload {
                let handle = {
                    let mut store = self.overflow_store.lock();
                    if !store.should_overflow(bytes.len()) {
                        None
                    } else {
                        Some(store.append(&bytes)?)
                    }
                };
                if let Some(handle) = handle {
                    state.overflow_rows.insert(local, handle);
                    // The side store is authoritative for this row: main
                    // buffers keep only an inline placeholder, and any
                    // chunk overlay entry for the row is stale.
                    state.overlay.remove(local);
                    let placeholder = Value::string("");
                    let normalized = match Some(&placeholder) {
                        Some(v) if v.is_null() => None,
                        other => other,
                    };
                    let _ = state.raw.as_storage_mut().set(local as usize, normalized);
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
    pub(super) fn write_raw_inner(
        &self,
        row_idx: usize,
        value: Option<&Value>,
    ) -> StorageResult<()> {
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
        *self.null_count_cache.write() = None;
        if let Some(v) = value {
            if !v.is_null() {
                if let Some(hll) = self.hll.lock().as_mut() {
                    hll.add_value(v);
                }
            }
        }
    }

    /// Internal write without dirty marking (used by page deserialization).
    pub(super) fn write_value_without_dirty(
        &self,
        row_idx: usize,
        value: Option<&Value>,
    ) -> StorageResult<()> {
        let use_chunk_layer = self.chunk_layer_routes(&self.chunks.read());
        let absorbed = self.with_resident_chunk(row_idx, |chunk, state| {
            // Fresh coverage has no side state yet; size visibility to the
            // window. Sparse chains allocate only on real history writes.
            state.visibility.ensure_len(chunk.row_count);
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
                chains.remove(&local);
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
                chains.remove(&local);
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

    /// Strict point read distinguishing never-written windows from corrupt
    /// payloads.
    ///
    /// Never-written rows (out of range, gaps, null slots) yield `Ok(None)`.
    /// Evicted-snapshot decode failures, side-store corruptions and raw
    /// length or dimension mismatches yield `Err` carrying the column name
    /// and row so queries fail loudly instead of observing silent nulls.
    pub fn try_get(&self, row_idx: usize) -> StorageResult<Option<Value>> {
        let chunks = self.chunks.read();
        self.try_get_in(&chunks, row_idx)
    }

    pub(super) fn try_get_in(
        &self,
        chunks: &[ColumnChunk],
        row_idx: usize,
    ) -> StorageResult<Option<Value>> {
        let capacity = self.chunk_capacity();
        let Some(chunk) = chunks.get(row_idx / capacity.max(1)) else {
            return Ok(None);
        };
        if row_idx < chunk.row_offset || row_idx >= chunk.row_offset + chunk.row_count {
            return Ok(None);
        }
        let local = (row_idx - chunk.row_offset) as u32;
        if super::overflow::OverflowStore::routes_for(&self.data_type) {
            let handle = chunk.read_state().overflow_rows.get(&local).copied();
            if let Some(handle) = handle {
                let bytes = self.overflow_store.lock().get(&handle).ok_or_else(|| {
                    StorageError::deserialize_error(format!(
                        "column {} overflow payload missing at row {}",
                        self.name, row_idx
                    ))
                })?;
                let value = super::overflow::decode_overflow_payload(&self.data_type, bytes)
                    .map_err(|e| {
                        StorageError::deserialize_error(format!(
                            "column {} overflow decode failed at row {}: {}",
                            self.name, row_idx, e
                        ))
                    })?;
                return Ok(Some(value));
            }
        }
        chunk.touch();
        let state = chunk.read_state();
        match &state.residency {
            ChunkResidency::Resident => {
                if let Some(hit) = state.overlay.get(local) {
                    return Ok(hit);
                }
                if state.encoding.is_encoded() {
                    return Ok(self.restore_string_type(state.encoding.get(local as usize)));
                }
                state.raw.as_storage().try_get(local as usize).map_err(|e| {
                    StorageError::deserialize_error(format!(
                        "column {} raw decode failed at rows {}-{}: {}",
                        self.name,
                        chunk.row_offset,
                        chunk.row_offset + chunk.row_count,
                        e
                    ))
                })
            }
            ChunkResidency::Evicted(snapshot) => snapshot.decode_row(row_idx).map_err(|e| {
                StorageError::deserialize_error(format!(
                    "column {} evicted snapshot decode failed at row {}: {}",
                    self.name, row_idx, e
                ))
            }),
        }
    }

    /// [`Self::get`] against an already-locked chunk vector.
    ///
    /// Lenient wrapper over [`Self::try_get_in`]: never-written windows read
    /// as missing, while corrupt payloads log and read as missing. Query
    /// paths needing explicit errors must call `try_get_in` directly.
    pub(super) fn get_in(&self, chunks: &[ColumnChunk], row_idx: usize) -> Option<Value> {
        match self.try_get_in(chunks, row_idx) {
            Ok(value) => value,
            Err(e) => {
                log::warn!(
                    "column {} row {} lenient read failed: {}; reading as missing",
                    self.name,
                    row_idx,
                    e
                );
                None
            }
        }
    }

    /// Restore the declared string type for values served from a
    /// type-agnostic string encoding (dictionary / FSST), which always
    /// decodes to `Value::String`. Raw reads already carry the declared
    /// type, so only encoded reads pass through here.
    pub(super) fn restore_string_type(&self, value: Option<Value>) -> Option<Value> {
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
            return true;
        };
        if row_idx < chunk.row_offset || row_idx >= chunk.row_offset + chunk.row_count {
            return true;
        }
        let local = (row_idx - chunk.row_offset) as u32;
        if super::overflow::OverflowStore::routes_for(&column.data_type)
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
                // Corrupt snapshots must not prune: report non-null so the
                // row survives to the strict projection path which errors.
                Some(Err(e)) => {
                    log::warn!(
                        "column {} row {} evicted snapshot decode failed in null check: {}; treating as non-null",
                        column.name,
                        row_idx,
                        e
                    );
                    return false;
                }
                None => return true,
            }
        }
        if state.encoding.is_encoded() {
            return state.encoding.get(local as usize).is_none();
        }
        state.raw.as_storage().is_null(local as usize)
    }

    pub fn null_count(&self) -> usize {
        if let Some(cached) = *self.null_count_cache.read() {
            return cached;
        }
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
        *self.null_count_cache.write() = Some(total);
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
                for chain in chains.values() {
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
        let elem_size = super::element_size(&self.data_type);
        if elem_size == 0 {
            // Variable-length encodings have no fixed element size; fall back
            // to allocated bytes instead of reporting zero live bytes.
            return self.memory_usage() + std::mem::size_of::<Self>();
        }
        let non_null_count = self.len() - self.null_count();
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
        *self.null_count_cache.write() = Some(0);
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
                chains.reserve(additional);
            }
        }
        drop(chunks);
        let needed =
            (self.len() + additional).div_ceil(crate::persistence::dirty_page::ROWS_PER_PAGE);
        self.total_dirty_pages.fetch_max(needed, Ordering::Relaxed);
    }

    /// Whether truncated rows still hold version history.
    ///
    /// Conservative snapshot guard: any retained before-image in the cut
    /// range may still serve an active snapshot, so the shrink refuses
    /// instead of dropping history silently. Callers GC versions to the
    /// watermark first; an empty cut range always allows the shrink.
    fn shrink_blocked_by_history(&self, new_count: usize, current: usize) -> bool {
        let chunks = self.chunks.read();
        for chunk in chunks.iter() {
            let chunk_start = chunk.row_offset;
            let chunk_end = chunk.row_offset + chunk.row_count;
            if chunk_end <= new_count || chunk_start >= current {
                continue;
            }
            let state = chunk.read_state();
            if let Some(chains) = state.version_chains.as_ref() {
                for (local, chain) in chains.iter() {
                    if chunk_start + local >= new_count && !chain.is_empty() {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Grow or shrink the column to `new_count` rows.
    ///
    /// Growth only appends the tail and is safe under the shard read lock.
    /// Shrinking rewrites windows and is exclusive-only (shard write lock),
    /// like `materialize_chunks`. Shrinks align to the reclamation watermark:
    /// rows at or past `new_count` holding version history are treated as
    /// still referenced by active snapshots and refuse the shrink so history
    /// is never dropped silently. Returns false when the shrink is refused.
    pub fn resize(&self, new_count: usize) -> bool {
        *self.null_count_cache.write() = None;
        let current = self.len();
        if new_count > current {
            // Grow through the coverage path so windows stay
            // capacity-aligned and gap rows read back as null.
            self.ensure_coverage(new_count - 1);
        } else if new_count < current {
            if self.shrink_blocked_by_history(new_count, current) {
                log::warn!(
                    "column {} refuses shrink to {} rows: version history still references truncated range",
                    self.name,
                    new_count
                );
                return false;
            }
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
                        chains.retain(|local, _| *local < keep);
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
                chains.retain(|local, _| *local < count);
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
        true
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
        if super::overflow::OverflowStore::routes_for(&self.data_type) {
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
        if super::overflow::OverflowStore::routes_for(&self.data_type) {
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
    pub(super) fn values_into_buffers(
        &self,
        values: impl Iterator<Item = Option<Value>>,
    ) -> (Vec<u8>, Vec<u32>, Option<BitVec<u8, Lsb0>>) {
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
                        new_offsets.push(new_data.len() as u32);
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
                                new_offsets.push(u32::MAX);
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
                        new_offsets.push(u32::MAX);
                    }
                }
            }
        }

        (new_data, new_offsets, new_bitmap)
    }

    pub fn get_flush_data(&self) -> (Vec<u8>, Vec<u32>, Option<BitVec<u8, Lsb0>>) {
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
                    let base = data.len() as u32;
                    for off in chunk_offsets {
                        offsets.push(if off == u32::MAX {
                            u32::MAX
                        } else {
                            base.saturating_add(off)
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
}
