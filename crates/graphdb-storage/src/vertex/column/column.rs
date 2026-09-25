use graphdb_core::types::Timestamp;
use graphdb_core::{DataType, StorageError, StorageResult, Value};

use crate::column_stats::ColumnStats;
use crate::encoding::ColumnEncoding;
use crate::stats::HyperLogLog;

use super::chunk::{ColumnChunk, DEFAULT_CHUNK_ROWS};
use super::chunk_encoding::{UpdateOverlay, DEFAULT_OVERLAY_CAPACITY};
use super::fixed_width::FixedWidthColumn;
use super::overflow::{OverflowHandle, OverflowStore, DEFAULT_OVERFLOW_THRESHOLD};
use super::variable_width::VariableWidthColumn;
use super::zone_map::ZoneBounds;

use bitvec::prelude::*;
use std::collections::HashMap;
use std::sync::atomic::Ordering;

use super::chunk_residency::{ChunkResidency, EvictedSnapshot};

/// Unified column storage interface.
pub trait ColumnStorage: Send + Sync + std::fmt::Debug {
    fn get(&self, row_idx: usize) -> Option<Value>;
    fn set(&mut self, row_idx: usize, value: Option<&Value>) -> StorageResult<()>;
    fn len(&self) -> usize;
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
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
#[derive(Debug, Clone)]
pub enum ColumnInner {
    Fixed(FixedWidthColumn),
    Variable(VariableWidthColumn),
}

/// Column storage that automatically selects fixed-width or variable-width
/// layout based on the `DataType` at construction time.
///
/// # Variant Selection
///
/// | `DataType` | Storage variant |
/// |---|---|
/// | Bool, SmallInt, Int, BigInt, Float, Double, Date, Time, DateTime, Uuid | `FixedWidthColumn` |
/// | All other types (String, FixedString, Blob, Geography, Vector family, Json/JsonB, Interval, Decimal family, Union, containers, composites, graph values) | `VariableWidthColumn` (length-prefixed base; per-chunk encodings such as dictionary/FSST plus zone maps and HLL stats apply on top) |
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
/// Version chain access follows a shared/exclusive pattern:
/// - **Reads** ([`Column::get_at_ts`], [`Column::with_version_chains_read`]):
///   Take `&self`, allowing multiple concurrent readers.
/// - **Writes** ([`Column::set_versioned`], [`Column::gc_versions`]):
///   Take `&mut self`, exclusive access required.
///
/// The table-level `RwLock<ShardedVertexTable>` provides shard-granularity
/// concurrency. Within a shard, version chain operations are serialized by
/// `&mut self`. For future optimization, version chains can be wrapped in
/// `parking_lot::RwLock` to allow concurrent reads during metadata updates.
///
/// Background eviction quota: one eviction segment releases at most this many
/// bytes before re-selecting victims, so over-quota background work proceeds
/// in segments instead of one burst.
pub const EVICTION_SEGMENT_BYTES: u64 = 16 * 1024 * 1024;

/// Background load quota: at most this many evicted chunks are promoted per
/// batch-load call; over-quota scans continue with the remainder.
pub const MAX_BACKGROUND_LOAD_CHUNKS: usize = 64;

#[derive(Debug, Clone)]
pub struct Column {
    pub name: String,
    pub col_id: i32,
    pub data_type: DataType,
    pub nullable: bool,
    pub(super) inner: ColumnInner,
    pub(super) encoding: ColumnEncoding,
    pub(super) stats: Option<ColumnStats>,
    /// Per-chunk min/max bounds over written values, used by scans for
    /// zone-map pruning. Bounds only ever widen after writes (deletes and
    /// nulls leave them stale but conservative), so pruning stays correct
    /// for any MVCC snapshot.
    pub(super) zone_maps: Vec<ZoneBounds>,
    /// Per-chunk length summaries for complex and variable-length values,
    /// parallel to `zone_maps`. Used for equality length pre-pruning when
    /// whole-value ordering alone cannot skip a chunk.
    pub(super) zone_complex: Vec<super::zone_map::ComplexZoneSummary>,
    /// Versioned writes since the last exact zone rebuild. Feeds the
    /// shrink trigger so long update histories do not leave pruning
    /// permanently stale.
    pub(super) zone_stale_writes: u64,
    /// Per-row version chains (before-images), lazily allocated.
    /// `None` means no updates have occurred and no version history is retained.
    pub(super) version_chains: Option<Vec<Vec<super::mvcc::VersionEntry>>>,
    /// Lightweight row-level visibility metadata (Layer 1).
    /// Always present; used for fast transaction isolation checks.
    pub(super) visibility: super::mvcc::RowVisibility,
    pub(super) dirty_tracker: crate::persistence::dirty_page::DirtyPageTracker,
    /// Lazy-loaded segments of this column. When non-empty, chunk-level
    /// methods route reads/writes through the resident chunk covering the
    /// target row. The `inner` buffer remains authoritative for raw chunks
    /// and single-buffer columns.
    pub(super) chunks: Vec<ColumnChunk>,
    /// Rows per chunk used when materializing `chunks`.
    pub(super) chunk_capacity: usize,
    /// In-memory HLL estimator maintained incrementally on writes.
    pub(super) hll: Option<HyperLogLog>,
    /// Large-string overflow area for this column.
    pub(super) overflow_store: OverflowStore,
    /// Rows whose payload lives in the overflow store.
    pub(super) overflow_rows: HashMap<usize, OverflowHandle>,
    /// Payload size above which strings spill to the overflow store.
    /// `usize::MAX` disables overflow routing (inline storage).
    pub(super) overflow_threshold: usize,
}

impl Column {
    pub fn new(name: String, col_id: i32, data_type: DataType, nullable: bool) -> Self {
        let inner = if super::is_variable_length_type(&data_type) {
            ColumnInner::Variable(VariableWidthColumn::new(data_type.clone(), nullable))
        } else {
            ColumnInner::Fixed(FixedWidthColumn::new(data_type.clone(), nullable))
        };

        Self {
            name,
            col_id,
            data_type,
            nullable,
            inner,
            encoding: ColumnEncoding::None,
            stats: None,
            zone_maps: Vec::new(),
            zone_complex: Vec::new(),
            zone_stale_writes: 0,
            version_chains: None,
            visibility: super::mvcc::RowVisibility::new(),
            dirty_tracker: crate::persistence::dirty_page::DirtyPageTracker::new(0),
            chunks: Vec::new(),
            chunk_capacity: DEFAULT_CHUNK_ROWS,
            hll: Some(HyperLogLog::new()),
            overflow_store: OverflowStore::new(DEFAULT_OVERFLOW_THRESHOLD),
            overflow_rows: HashMap::new(),
            overflow_threshold: DEFAULT_OVERFLOW_THRESHOLD,
        }
    }

    pub(super) fn inner(&self) -> &dyn ColumnStorage {
        match &self.inner {
            ColumnInner::Fixed(c) => c,
            ColumnInner::Variable(c) => c,
        }
    }

    pub(super) fn inner_mut(&mut self) -> &mut dyn ColumnStorage {
        match &mut self.inner {
            ColumnInner::Fixed(c) => c,
            ColumnInner::Variable(c) => c,
        }
    }

    /// Mark the page containing `row_idx` as dirty.
    #[inline]
    pub fn mark_dirty(&mut self, row_idx: usize) {
        let page_id = crate::persistence::dirty_page::DirtyPageTracker::row_to_page(row_idx);
        self.dirty_tracker.mark_page(page_id);
        self.dirty_tracker.ensure_rows(self.len().max(row_idx + 1));
    }

    pub fn dirty_pages(&self) -> Vec<usize> {
        self.dirty_tracker.dirty_pages()
    }

    pub fn dirty_count(&self) -> usize {
        self.dirty_tracker.dirty_count()
    }

    pub fn clear_dirty(&mut self) {
        self.dirty_tracker.clear();
    }

    /// Clear the dirty mark for a single row-page (keeps other dirty pages).
    #[inline]
    pub fn clear_page_dirty(&mut self, page_id: usize) {
        self.dirty_tracker.clear_page(page_id);
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
    pub fn deserialize_page(&mut self, data: &[u8]) -> StorageResult<()> {
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
        self.dirty_tracker.clear_page(page.header.page_id as usize);
        Ok(())
    }

    /// Whether any chunk carries its own encoding.
    fn any_chunk_encoded(&self) -> bool {
        self.chunks.iter().any(|c| c.encoding.is_encoded())
    }

    /// Write through the chunk layer (overlay or in-place chunk encoding).
    ///
    /// Chunk encodings are authoritative once chunking is active; the
    /// column-level encoding is only a compatibility marker. Returns true
    /// when the chunk path absorbed the write.
    fn write_via_chunks(&mut self, row_idx: usize, value: Option<&Value>) -> bool {
        if self.chunks.is_empty() {
            return false;
        }
        // Appends past the end: grow the raw buffer and the last chunk, and
        // decode the tail chunk to raw so the append stays O(chunk).
        if row_idx >= self.len() {
            self.inner_mut().resize(row_idx + 1);
            self.ensure_row_meta(row_idx + 1);
            self.visibility.ensure_len(row_idx + 1);
            if let Some(last) = self.chunks.last_mut() {
                last.row_count = (row_idx + 1).saturating_sub(last.row_offset).max(1);
                last.encoding = ColumnEncoding::None;
            }
            return self.write_raw_inner(row_idx, value).is_ok();
        }
        let capacity = self.chunk_capacity.max(1);
        let chunk_idx = row_idx / capacity;
        if chunk_idx >= self.chunks.len() {
            return false;
        }
        let local = (row_idx - self.chunks[chunk_idx].row_offset) as u32;
        let data_type = self.data_type.clone();
        let decision = self.chunks[chunk_idx].set_value(local, value.cloned(), &data_type);
        // A recode verdict means the encoding cannot absorb more point
        // writes: mark the chunk hot immediately so the next flush (or an
        // early merge) re-encodes it instead of letting the overlay grow
        // to full and forcing a whole-column fallback.
        if decision == crate::vertex::column::chunk_encoding::UpdateDecision::OverlayAndRecode {
            self.chunks[chunk_idx].updates_since_encode =
                self.chunks[chunk_idx].updates_since_encode.max(
                    crate::encoding::selector::EncodingThresholds::default().hot_update_threshold
                        + 1,
                );
        }
        // Raw chunks have no encoded base: mirror the write into the raw
        // buffer that serves as their storage.
        if decision == crate::vertex::column::chunk_encoding::UpdateDecision::InPlace
            && !self.chunks[chunk_idx].encoding.is_encoded()
        {
            return self.write_raw_inner(row_idx, value).is_ok();
        }
        // Nullability still enforced even for overlay writes.
        if value.is_none() && !self.nullable {
            return false;
        }
        if let Some(v) = value {
            if v.is_null() && !self.nullable {
                return false;
            }
        }
        true
    }

    fn write_raw_inner(&mut self, row_idx: usize, value: Option<&Value>) -> StorageResult<()> {
        if let Some(v) = value {
            if v.is_null() {
                if !self.nullable {
                    return Err(StorageError::null_value_not_allowed(self.name.clone()));
                }
                self.inner_mut().set(row_idx, None)?;
            } else {
                self.inner_mut().set(row_idx, Some(v))?;
            }
        } else {
            if !self.nullable {
                return Err(StorageError::null_value_not_allowed(self.name.clone()));
            }
            self.inner_mut().set(row_idx, None)?;
        }
        Ok(())
    }

    fn observe_write(&mut self, row_idx: usize, value: Option<&Value>) {
        self.update_zone_maps(row_idx, value);
        self.zone_stale_writes = self.zone_stale_writes.saturating_add(1);
        if let Some(ref mut hll) = self.hll {
            if let Some(v) = value {
                if !v.is_null() {
                    hll.add_value(v);
                }
            }
        }
    }

    /// Internal write without dirty marking (used by page deserialization).
    fn write_value_without_dirty(
        &mut self,
        row_idx: usize,
        value: Option<&Value>,
    ) -> StorageResult<()> {
        // Overlay writes to an evicted chunk load it first so point-write
        // semantics never change under eviction.
        if let Some(chunk_idx) = self.chunk_index_for_row(row_idx) {
            self.ensure_resident(chunk_idx)?;
        }
        // Chunked columns always route through the chunk layer so a point
        // write never decodes the whole column.
        if !self.chunks.is_empty()
            && (self.encoding.is_encoded() || self.any_chunk_encoded())
            && self.write_via_chunks(row_idx, value)
        {
            self.observe_write(row_idx, value);
            return Ok(());
        }
        if self.encoding.is_encoded() && self.chunks.is_empty() {
            if self.encoding.set(row_idx, value).is_ok() {
                if row_idx >= self.len() {
                    self.sync_row_count_from_encoding();
                }
                self.observe_write(row_idx, value);
                return Ok(());
            }
            // Encoded set failed: materialize chunk-local encodings and route
            // through the overlay instead of decoding the whole column.
            self.materialize_chunks();
            if self.write_via_chunks(row_idx, value) {
                self.observe_write(row_idx, value);
                return Ok(());
            }
        }
        self.write_raw_inner(row_idx, value)?;
        self.observe_write(row_idx, value);
        Ok(())
    }

    /// Write `value` into the column, handling the encoded and raw paths.
    /// Does not touch the MVCC metadata.
    pub(super) fn write_value(
        &mut self,
        row_idx: usize,
        value: Option<&Value>,
    ) -> StorageResult<()> {
        // Large-string overflow routing happens before encoding checks so the
        // main buffers only ever hold the small inline placeholder.
        if matches!(self.data_type, DataType::String | DataType::Blob) {
            let payload: Option<&[u8]> = match value {
                Some(Value::String(s)) => Some(s.as_bytes()),
                Some(Value::Blob(b)) => Some(b.as_slice()),
                _ => None,
            };
            if let Some(bytes) = payload {
                if self.overflow_store.should_overflow(bytes.len()) {
                    self.mark_dirty(row_idx);
                    let handle = self.overflow_store.append(bytes);
                    self.overflow_rows.insert(row_idx, handle);
                    // The side store is authoritative for this row: main
                    // buffers keep only an inline placeholder, and any
                    // chunk overlay entry for the row is stale.
                    self.clear_chunk_overlay_entry(row_idx);
                    let placeholder = match self.data_type {
                        DataType::Blob => Value::Blob(Vec::new()),
                        _ => Value::string(""),
                    };
                    self.write_raw_inner(row_idx, Some(&placeholder))?;
                    self.observe_write(row_idx, value);
                    return Ok(());
                }
            }
            if self.overflow_rows.remove(&row_idx).is_some() {
                // Stale payload stays in the store until flush rebuilds it.
            }
        }
        self.mark_dirty(row_idx);
        // Overlay writes to an evicted chunk load it first so point-write
        // semantics never change under eviction. Load failures propagate
        // as storage errors instead of silently dropping the write.
        if let Some(chunk_idx) = self.chunk_index_for_row(row_idx) {
            self.ensure_resident(chunk_idx)?;
        }
        // Chunked columns always route through the chunk layer so a point
        // write never decodes the whole column.
        if !self.chunks.is_empty()
            && (self.encoding.is_encoded() || self.any_chunk_encoded())
            && self.write_via_chunks(row_idx, value)
        {
            self.observe_write(row_idx, value);
            if let Some(chunk_idx) = self.chunk_index_for_row(row_idx) {
                let hot =
                    crate::encoding::selector::EncodingThresholds::default().hot_update_threshold;
                let _ = self.maybe_merge_hot_chunk(chunk_idx, hot);
            }
            return Ok(());
        }
        if self.encoding.is_encoded() && self.chunks.is_empty() {
            if self.encoding.set(row_idx, value).is_ok() {
                if row_idx >= self.len() {
                    self.sync_row_count_from_encoding();
                }
                self.observe_write(row_idx, value);
                return Ok(());
            }
            // Encoded set failed (e.g., row_idx >= row_count during WAL replay
            // or an append-only RLE encoding). Materialize chunk-local
            // encodings and route through the overlay instead of decoding
            // the whole column.
            self.materialize_chunks();
            if self.write_via_chunks(row_idx, value) {
                self.observe_write(row_idx, value);
                return Ok(());
            }
        }

        self.write_raw_inner(row_idx, value)?;
        self.observe_write(row_idx, value);
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Core read / write
    // -----------------------------------------------------------------------

    pub fn set(&mut self, row_idx: usize, value: Option<&Value>) -> StorageResult<()> {
        // Plain set treats the value as "current from the beginning": it
        // resets the row's MVCC metadata (no historical version recorded).
        self.ensure_row_meta(row_idx + 1);
        self.with_version_chains_write(|chains| {
            if let Some(chains) = chains.as_mut() {
                if row_idx < chains.len() {
                    chains[row_idx].clear();
                }
            }
        });
        self.visibility.mark_created(row_idx, 0);
        self.write_value(row_idx, value)
    }

    /// Write a value with a specific creation timestamp without generating
    /// a version chain entry. This is used when initializing a new row where
    /// the initial value should be visible from `create_ts` onward and no
    /// before-image exists.
    pub fn set_with_timestamp(
        &mut self,
        row_idx: usize,
        value: Option<&Value>,
        create_ts: Timestamp,
    ) -> StorageResult<()> {
        self.ensure_row_meta(row_idx + 1);
        // Clear any existing version chain for this row
        self.with_version_chains_write(|chains| {
            if let Some(chains) = chains.as_mut() {
                if row_idx < chains.len() {
                    chains[row_idx].clear();
                }
            }
        });
        // Set the correct creation timestamp
        self.visibility.mark_created(row_idx, create_ts);
        // Write the value without generating a version chain entry
        self.write_value(row_idx, value)
    }

    pub fn get(&self, row_idx: usize) -> Option<Value> {
        // Overflow rows are authoritative in the side store.
        if matches!(self.data_type, DataType::String | DataType::Blob) {
            if let Some(handle) = self.overflow_rows.get(&row_idx) {
                if let Some(bytes) = self.overflow_store.get(handle) {
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
        if !self.chunks.is_empty() {
            let capacity = self.chunk_capacity.max(1);
            let chunk_idx = row_idx / capacity;
            if let Some(chunk) = self.chunks.get(chunk_idx) {
                // Recency stamp for watermark-ordered eviction. Atomic so
                // shared-reference reads participate without a lock upgrade.
                chunk.touch();
                match &chunk.residency {
                    ChunkResidency::Resident => {
                        let local = (row_idx.saturating_sub(chunk.row_offset)) as u32;
                        if let Some(hit) = chunk.overlay.get(local) {
                            return hit;
                        }
                        // Chunk-local encodings are authoritative when present;
                        // raw chunks fall through to the inner buffer below.
                        if chunk.encoding.is_encoded() {
                            return self.restore_string_type(chunk.encoding.get(local as usize));
                        }
                    }
                    ChunkResidency::Evicted(snapshot) => {
                        // Cold miss served from the compressed snapshot
                        // without promoting: promotion happens on `&mut`
                        // paths (writes, batch prefetch, encode, flush).
                        // The snapshot is checksummed in memory, so a
                        // decode failure here is unreachable; warn and fall
                        // through rather than failing the read.
                        match snapshot.decode_row(row_idx) {
                            Ok(value) => return value,
                            Err(e) => {
                                log::warn!(
                                    "evicted chunk {} row {} snapshot decode failed: {}; falling back to base buffers",
                                    chunk_idx,
                                    row_idx,
                                    e
                                );
                            }
                        }
                    }
                }
            }
        }
        if self.encoding.is_encoded() {
            return self.restore_string_type(self.encoding.get(row_idx));
        }
        self.inner().get(row_idx)
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
        self.inner().is_null(row_idx)
    }

    pub fn null_count(&self) -> usize {
        self.inner().null_count()
    }

    pub fn len(&self) -> usize {
        self.inner().len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner().is_empty()
    }

    pub fn null_bitmap(&self) -> Option<&BitVec<u8, Lsb0>> {
        self.inner().null_bitmap()
    }

    pub fn memory_usage(&self) -> usize {
        let version_bytes = self.with_version_chains_read(|chains| {
            chains
                .map(|c| {
                    c.iter()
                        .map(|chain| {
                            chain.len() * std::mem::size_of::<super::mvcc::VersionEntry>()
                                + chain
                                    .iter()
                                    .map(|entry| {
                                        entry
                                            .value
                                            .as_ref()
                                            .map(super::value_payload_bytes)
                                            .unwrap_or(0)
                                    })
                                    .sum::<usize>()
                        })
                        .sum::<usize>()
                })
                .unwrap_or(0)
                + self.visibility.memory_usage()
        });
        let chunk_bytes: usize = self
            .chunks
            .iter()
            .map(|c| {
                c.overlay.memory_usage()
                    + c.encoding_meta.memory_usage()
                    + c.data.len()
                    + c.offsets.len() * 8
                    + c.null_bitmap
                        .as_ref()
                        .map(|bm| bm.as_raw_slice().len())
                        .unwrap_or(0)
                    + c.encoding.memory_usage()
                    + c.residency
                        .evicted_snapshot()
                        .map(|snapshot| snapshot.resident_bytes())
                        .unwrap_or(0)
            })
            .sum();
        let hll_bytes = self.hll.as_ref().map(|_| 64usize).unwrap_or(0);
        self.inner().memory_usage()
            + self.encoding.memory_usage()
            + version_bytes
            + chunk_bytes
            + hll_bytes
            + self.overflow_store.memory_usage()
    }

    pub fn memory_size(&self) -> usize {
        self.memory_usage() + std::mem::size_of::<Self>()
    }

    pub fn used_memory_size(&self) -> usize {
        let non_null_count = self.len() - self.null_count();
        let elem_size = super::element_size(&self.data_type);
        non_null_count * elem_size + std::mem::size_of::<Self>()
    }

    pub fn clear(&mut self) {
        self.inner_mut().clear();
        self.encoding = ColumnEncoding::None;
        self.zone_maps.clear();
        self.zone_complex.clear();
        self.zone_stale_writes = 0;
        self.with_version_chains_write(|chains| {
            *chains = None;
        });
        self.visibility.clear();
        self.dirty_tracker.clear();
        self.dirty_tracker.set_total_pages(0);
        self.chunks.clear();
        self.hll = Some(HyperLogLog::new());
        self.overflow_store = OverflowStore::new(self.overflow_threshold);
        self.overflow_rows.clear();
    }

    /// Pre-allocate capacity for `additional` more rows in the underlying
    /// storage buffers (used by batch inserts).
    pub fn reserve(&mut self, additional: usize) {
        self.inner_mut().reserve(additional);
        self.with_version_chains_write(|chains| {
            if let Some(chains) = chains.as_mut() {
                chains.reserve(additional);
            }
        });
        self.visibility.reserve(additional);
        let needed =
            (self.len() + additional).div_ceil(crate::persistence::dirty_page::ROWS_PER_PAGE);
        self.dirty_tracker
            .set_total_pages(self.dirty_tracker.total_pages().max(needed));
    }

    pub fn resize(&mut self, new_count: usize) {
        self.inner_mut().resize(new_count);
        self.ensure_row_meta(new_count);
        self.with_version_chains_write(|chains| {
            if let Some(chains) = chains.as_mut() {
                chains.resize(new_count, Vec::new());
            }
        });
        self.visibility.resize(new_count);
        self.dirty_tracker.ensure_rows(new_count);
        // Drop overflow mappings for truncated rows.
        self.overflow_rows.retain(|row, _| *row < new_count);
    }

    pub fn load_data_from_raw(
        &mut self,
        data: Vec<u8>,
        offsets: Vec<u64>,
        null_bitmap_raw: Option<Vec<u8>>,
        bitmap_bit_len: usize,
    ) {
        self.inner_mut()
            .load_data_from_raw(data, offsets, null_bitmap_raw, bitmap_bit_len);
        // MVCC metadata is intentionally left untouched: a freshly-loaded
        // column starts with empty metadata (rows read as "current"), and
        // in-memory decode paths must preserve existing version chains.
        self.dirty_tracker.clear();
        self.dirty_tracker.ensure_rows(self.len());
    }

    pub fn get_flush_data(&self) -> (Vec<u8>, Vec<u64>, Option<BitVec<u8, Lsb0>>) {
        if !self.encoding.is_encoded() && !self.any_chunk_encoded() {
            return self.inner().get_flush_data();
        }

        let row_count = self.len();
        let mut new_data = Vec::new();
        let mut new_offsets = Vec::new();
        let mut new_bitmap = self.null_bitmap().map(|_| BitVec::with_capacity(row_count));

        let is_var = super::is_variable_length_type(&self.data_type);
        let elem_size = super::element_size(&self.data_type);
        let chunked = self.has_chunks();

        // Stream row by row without materializing a full Vec<Option<Value>>:
        // each row is resolved (overlay/chunk-encoding merged, overflow rows
        // as inline placeholders) and encoded directly into the output
        // buffers, so flush peak memory stays O(row) instead of O(column).
        for row in 0..row_count {
            let value: Option<Value> = if chunked {
                if self.overflow_rows.contains_key(&row) {
                    self.inner().get(row)
                } else {
                    self.get(row)
                }
            } else {
                self.encoding.get(row)
            };
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
                        let _ = super::fixed_width::write_fixed_value(
                            &mut new_data,
                            start,
                            elem_size,
                            &v,
                        );
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

    // -----------------------------------------------------------------------
    // Chunk routing (chunk-local encodings and update overlays)
    // -----------------------------------------------------------------------

    /// Returns true when chunk-level routing is active.
    pub fn has_chunks(&self) -> bool {
        !self.chunks.is_empty()
    }

    /// Number of chunks (0 when chunking is inactive).
    pub fn chunk_count(&self) -> usize {
        self.chunks.len()
    }

    /// Rows per chunk used for materialization.
    pub fn chunk_capacity(&self) -> usize {
        self.chunk_capacity
    }

    /// Override the rows-per-chunk capacity for future materialization.
    pub fn set_chunk_capacity(&mut self, capacity: usize) {
        if capacity > 0 {
            self.chunk_capacity = capacity;
        }
    }

    /// Index of the chunk containing `row_idx`, if chunking is active.
    pub fn chunk_index_for_row(&self, row_idx: usize) -> Option<usize> {
        if self.chunks.is_empty() {
            return None;
        }
        let idx = row_idx / self.chunk_capacity.max(1);
        if idx < self.chunks.len() {
            Some(idx)
        } else {
            None
        }
    }

    /// Borrow one chunk by index.
    pub(crate) fn chunk_at(&self, idx: usize) -> Option<&ColumnChunk> {
        self.chunks.get(idx)
    }

    /// Whether any row maps into the overflow store.
    pub(crate) fn has_overflow(&self) -> bool {
        !self.overflow_rows.is_empty()
    }

    /// Replace the chunk vector (used when restoring per-chunk encodings).
    pub(crate) fn set_chunks(&mut self, chunks: Vec<ColumnChunk>) {
        self.chunks = chunks;
    }

    /// Borrow the chunk containing `row_idx`.
    pub fn chunk_for_row(&self, row_idx: usize) -> Option<&ColumnChunk> {
        self.chunk_index_for_row(row_idx)
            .and_then(|i| self.chunks.get(i))
    }

    /// Overflow-store accessors (large-string tier).
    pub fn set_overflow_threshold(&mut self, threshold: usize) {
        self.overflow_threshold = threshold;
        self.overflow_store.set_threshold(threshold);
    }

    // -----------------------------------------------------------------------
    // Chunk eviction / reload
    // -----------------------------------------------------------------------

    /// Whether the chunk may be evicted: chunk-level checks plus exclusion
    /// of chunks overlapped by column-level version chains.
    pub fn chunk_evictable(&self, chunk_idx: usize) -> bool {
        let Some(chunk) = self.chunks.get(chunk_idx) else {
            return false;
        };
        if !chunk.is_evictable() {
            return false;
        }
        let start = chunk.row_offset;
        let end = start.saturating_add(chunk.row_count);
        self.with_version_chains_read(|chains| match chains {
            None => true,
            Some(entries) => entries
                .iter()
                .take(end)
                .skip(start.min(entries.len()))
                .all(|chain| chain.is_empty()),
        })
    }

    /// Synchronously load an evicted chunk and mark it hot. Returns whether
    /// a load happened. Load failures propagate as storage errors.
    pub fn ensure_resident(&mut self, chunk_idx: usize) -> StorageResult<bool> {
        let snapshot = match self.chunks.get(chunk_idx) {
            Some(chunk) => match &chunk.residency {
                ChunkResidency::Resident => return Ok(false),
                ChunkResidency::Evicted(snapshot) => snapshot.clone(),
            },
            None => return Ok(false),
        };
        let pairs = snapshot.decode_all()?;
        let is_var = super::is_variable_length_type(&self.data_type);
        let (row_offset, row_count, nullable) = {
            let chunk = &self.chunks[chunk_idx];
            (chunk.row_offset, chunk.row_count, self.nullable)
        };
        let fresh = if is_var {
            ColumnChunk::new_variable(row_offset, row_count, nullable)
        } else {
            ColumnChunk::new(
                row_offset,
                row_count,
                super::element_size(&self.data_type),
                nullable,
            )
        };
        fresh.touch();
        self.chunks[chunk_idx] = fresh;
        for (row, value) in &pairs {
            self.write_raw_inner(*row, value.as_ref())?;
        }
        // Restore the pre-evict encoding from the same values so promotion
        // never leaves a raw chunk under a stale column-level mirror: mixed
        // raw/encoded states would misroute reads through the mirror.
        // Profiles (min/max/raw size) come from the snapshot; counts and
        // the compressed size reflect the fresh encoding. Values come from
        // the decoded snapshot (overflow rows as placeholders, mirroring
        // the encode-time base) rather than re-reading through the mirror.
        if snapshot.encoding != crate::encoding::EncodingType::None {
            let values: Vec<Option<Value>> = pairs
                .iter()
                .map(|(row, value)| {
                    if matches!(self.data_type, DataType::String | DataType::Blob)
                        && self.overflow_rows.contains_key(row)
                    {
                        match self.data_type {
                            DataType::Blob => Some(Value::Blob(Vec::new())),
                            _ => Some(Value::string("")),
                        }
                    } else {
                        value.clone()
                    }
                })
                .collect();
            let encoded = Self::encode_slice(
                &values,
                &self.data_type,
                self.nullable,
                snapshot.encoding,
                255,
            );
            if encoded.is_encoded() {
                let num_values = values.iter().filter(|v| v.is_some()).count() as u32;
                if let Some(chunk) = self.chunks.get_mut(chunk_idx) {
                    chunk.encoding = encoded;
                    chunk.clear_overlay_after_flush();
                    chunk.encoding_meta = snapshot.meta.clone();
                    chunk.encoding_meta.scheme = chunk.encoding.encoding_type();
                    chunk.encoding_meta.num_values = num_values;
                    chunk.encoding_meta.all_null = num_values == 0;
                    chunk.encoding_meta.compressed_size = chunk.encoding.memory_usage() as u64;
                }
            }
        }
        Ok(true)
    }

    /// Batch miss-load: promote every evicted chunk covering `rows` once
    /// before a grouped decode, avoiding per-row page faults. Returns the
    /// number of chunks loaded. Background batch loads behind this entry are
    /// charged against the task quota in [`super::MAX_BACKGROUND_LOAD_CHUNKS`]
    /// segments.
    pub fn ensure_resident_range(&mut self, rows: &[usize]) -> StorageResult<usize> {
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
    pub fn ensure_all_resident(&mut self) -> StorageResult<usize> {
        let mut loaded = 0usize;
        for idx in 0..self.chunks.len() {
            if self.ensure_resident(idx)? {
                loaded += 1;
            }
        }
        Ok(loaded)
    }

    /// Release one cold chunk's decoded buffers, spilling the compressed
    /// snapshot to a spill file and retaining only row range, encoding
    /// scheme, and profiles in memory. Returns bytes released, or 0 when
    /// the chunk is not evictable. Zone maps and HLL stay resident at the
    /// column level and keep serving. A spill failure reports an error so
    /// the caller skips the chunk instead of retaining heap pages.
    pub fn evict_chunk(&mut self, chunk_idx: usize) -> StorageResult<u64> {
        if !self.chunk_evictable(chunk_idx) {
            return Ok(0);
        }
        let (start, count, encoding, meta) = {
            let chunk = &self.chunks[chunk_idx];
            (
                chunk.row_offset,
                chunk.row_count,
                chunk.encoding.encoding_type(),
                chunk.encoding_meta.clone(),
            )
        };
        let values: Vec<Option<Value>> = (start..start.saturating_add(count))
            .map(|row| self.get(row))
            .collect();
        let snapshot = EvictedSnapshot::capture_spilled(start, values, encoding, meta)?;
        let released = {
            let chunk = &self.chunks[chunk_idx];
            (chunk.data.len()
                + chunk.offsets.len() * 8
                + chunk
                    .null_bitmap
                    .as_ref()
                    .map(|bm| bm.as_raw_slice().len())
                    .unwrap_or(0)
                + chunk.encoding.memory_usage()) as u64
        };
        {
            let chunk = &mut self.chunks[chunk_idx];
            chunk.data = Vec::new();
            chunk.offsets = Vec::new();
            chunk.null_bitmap = None;
            chunk.encoding = ColumnEncoding::None;
            chunk.residency = ChunkResidency::Evicted(snapshot);
        }
        Ok(released)
    }

    /// Evict resident cold chunks oldest-first until `budget` bytes are
    /// released. Returns `(chunks_evicted, bytes_released)`. Per-chunk
    /// failures are skipped with a warning so one corruptible chunk never
    /// blocks the watermark pass.
    pub fn evict_cold_chunks(&mut self, budget: u64) -> (usize, u64) {
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
        &mut self,
        budget: u64,
        task_quota: u64,
    ) -> (usize, u64, usize) {
        if budget == 0 || self.chunks.is_empty() {
            return (0, 0, 0);
        }
        let segment = task_quota.min(EVICTION_SEGMENT_BYTES).max(1).min(budget);
        let mut count = 0usize;
        let mut freed = 0u64;
        let mut segments = 0usize;
        while freed < budget {
            let target = freed.saturating_add(segment).min(budget);
            let mut order: Vec<(u64, usize)> = (0..self.chunks.len())
                .filter(|&idx| self.chunk_evictable(idx))
                .map(|idx| (self.chunks[idx].last_access.load(Ordering::Relaxed), idx))
                .collect();
            if order.is_empty() {
                break;
            }
            order.sort_unstable();
            segments += 1;
            let mut progress = false;
            for (_, idx) in order {
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
        &mut self,
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

    /// Resident decoded bytes (excludes retained eviction snapshots).
    pub fn resident_memory_usage(&self) -> usize {
        self.memory_usage().saturating_sub(self.evicted_bytes())
    }

    /// Chunk indexes whose overlay or update count makes them recode
    /// candidates at the current hot threshold.
    pub fn pending_recode_chunks(&self, hot_threshold: u64) -> Vec<usize> {
        self.chunks
            .iter()
            .enumerate()
            .filter(|(_, c)| c.needs_recode(hot_threshold))
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
    pub fn maybe_merge_hot_chunk(&mut self, chunk_idx: usize, hot_threshold: u64) -> bool {
        let (overlay_len, updates, encoded, evicted) = match self.chunks.get(chunk_idx) {
            Some(c) => (
                c.overlay.len(),
                c.updates_since_encode,
                c.encoding.is_encoded(),
                !c.residency.is_resident(),
            ),
            None => return false,
        };
        if evicted {
            return false;
        }
        let half_overlay = super::chunk_encoding::DEFAULT_OVERLAY_CAPACITY / 2;
        let half_updates = hot_threshold / 2;
        if overlay_len < half_overlay && updates < half_updates {
            return false;
        }
        if !encoded {
            let merged: Vec<(u32, Option<Value>)> = self.chunks[chunk_idx]
                .overlay
                .iter()
                .map(|(k, v)| (*k, v.clone()))
                .collect();
            let mut applied = 0usize;
            for (local, value) in merged {
                let row = self.chunks[chunk_idx].row_offset + local as usize;
                if self.write_raw_inner(row, value.as_ref()).is_ok() {
                    applied += 1;
                }
            }
            if applied > 0 {
                if let Some(chunk) = self.chunks.get_mut(chunk_idx) {
                    chunk.overlay.clear();
                    chunk.updates_since_encode = 0;
                }
                return true;
            }
            return false;
        }
        if let Some(chunk) = self.chunks.get_mut(chunk_idx) {
            if !chunk.needs_recode(hot_threshold) {
                chunk.updates_since_encode = hot_threshold + 1;
                return true;
            }
        }
        false
    }

    /// Compressed snapshot payload bytes for evicted chunks, wherever they
    /// live (heap or spill files). Used for eviction observability; spilled
    /// bytes no longer count toward heap memory.
    pub fn evicted_bytes(&self) -> usize {
        self.chunks
            .iter()
            .filter_map(|chunk| chunk.residency.evicted_snapshot())
            .map(|snapshot| snapshot.compressed_bytes())
            .sum()
    }

    /// Chunks with decoded data in memory.
    pub fn resident_chunk_count(&self) -> usize {
        self.chunks
            .iter()
            .filter(|chunk| chunk.residency.is_resident())
            .count()
    }

    /// Chunks released with only the snapshot retained.
    pub fn evicted_chunk_count(&self) -> usize {
        self.chunks
            .iter()
            .filter(|chunk| chunk.residency.is_evicted())
            .count()
    }

    /// Base value for encoding inputs and persisted buffers: overflow rows
    /// contribute their inline placeholder (the payload travels in the
    /// sidecar). Point reads use `get`, which serves the side store.
    fn encoding_base_value(&self, row_idx: usize) -> Option<Value> {
        if matches!(self.data_type, DataType::String | DataType::Blob)
            && self.overflow_rows.contains_key(&row_idx)
        {
            return match self.data_type {
                DataType::Blob => Some(Value::Blob(Vec::new())),
                _ => Some(Value::string("")),
            };
        }
        self.get(row_idx)
    }

    /// Drop a stale chunk overlay entry for a row whose newest value moved
    /// to the overflow side store.
    fn clear_chunk_overlay_entry(&mut self, row_idx: usize) {
        if self.chunks.is_empty() {
            return;
        }
        let capacity = self.chunk_capacity.max(1);
        let idx = row_idx / capacity;
        if let Some(chunk) = self.chunks.get_mut(idx) {
            if row_idx >= chunk.row_offset && row_idx < chunk.row_offset + chunk.row_count {
                chunk.overlay.remove((row_idx - chunk.row_offset) as u32);
            }
        }
    }

    /// Rebuild the overflow store from live rows only (flush-time GC).
    pub fn rebuild_overflow(&mut self) {
        if self.overflow_rows.is_empty() {
            return;
        }
        // Main buffers hold placeholders for overflow rows, so payloads are
        // re-read from the side store itself; rows whose payload vanished
        // (overwritten with a short value since) are dropped.
        let mut live_rows: Vec<usize> = Vec::new();
        let mut live_payloads: Vec<Vec<u8>> = Vec::new();
        for (row, handle) in self.overflow_rows.iter() {
            match self.overflow_store.get(handle) {
                Some(payload) if payload.len() > self.overflow_threshold => {
                    live_rows.push(*row);
                    live_payloads.push(payload);
                }
                _ => {}
            }
        }
        let mut order: Vec<usize> = (0..live_rows.len()).collect();
        order.sort_by_key(|&i| live_rows[i]);
        let sorted_payloads: Vec<Vec<u8>> =
            order.iter().map(|&i| live_payloads[i].clone()).collect();
        self.overflow_store.rebuild_from_live(&sorted_payloads);
        // Rebuild preserves row order, so entry ids follow the sorted rows.
        let mut rows = HashMap::new();
        let mut sorted_rows: Vec<usize> = live_rows;
        sorted_rows.sort_unstable();
        for (entry_id, row) in sorted_rows.into_iter().enumerate() {
            rows.insert(
                row,
                OverflowHandle {
                    entry_id: entry_id as u32,
                },
            );
        }
        self.overflow_rows = rows;
    }

    /// Serialize overflow state for the `<col>.overflow` sidecar.
    pub fn serialize_overflow(&self) -> StorageResult<Vec<u8>> {
        let mut store_buf = Vec::new();
        {
            let tmp = self.overflow_store.clone();
            tmp.flush_to_sidecar_buffer(&mut store_buf)?;
        }
        let mut buf = Vec::new();
        buf.push(1u8);
        buf.extend_from_slice(&(store_buf.len() as u32).to_le_bytes());
        buf.extend_from_slice(&store_buf);
        buf.extend_from_slice(&(self.overflow_rows.len() as u32).to_le_bytes());
        let mut rows: Vec<(&usize, &OverflowHandle)> = self.overflow_rows.iter().collect();
        rows.sort_by_key(|(row, _)| **row);
        for (row, handle) in rows {
            buf.extend_from_slice(&(*row as u32).to_le_bytes());
            buf.extend_from_slice(&handle.entry_id.to_le_bytes());
        }
        let crc = crc32fast::hash(&buf);
        buf.extend_from_slice(&crc.to_le_bytes());
        Ok(buf)
    }

    /// Restore overflow state from a sidecar buffer.
    pub fn load_overflow_bytes(&mut self, bytes: &[u8]) -> StorageResult<()> {
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
        let mut store = OverflowStore::new(self.overflow_threshold);
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
        self.overflow_store = store;
        self.overflow_rows = rows;
        Ok(())
    }

    /// Restore a column-level encoding after load without touching raw data.
    pub(crate) fn restore_encoding(&mut self, encoding: ColumnEncoding) {
        self.encoding = encoding;
    }

    /// Encode one row slice into a chunk-local encoding of the given type.
    fn encode_slice(
        values: &[Option<Value>],
        data_type: &DataType,
        nullable: bool,
        encoding_type: crate::encoding::EncodingType,
        fsst_max_symbols: usize,
    ) -> ColumnEncoding {
        use crate::encoding::EncodingType as ET;
        if values.is_empty() {
            return ColumnEncoding::None;
        }
        let mut tmp = Column::new("slice".to_string(), 0, data_type.clone(), nullable);
        for (off, v) in values.iter().enumerate() {
            tmp.resize(off + 1);
            let _ = tmp.write_value_without_dirty(off, v.as_ref());
        }
        let applied = match encoding_type {
            ET::Fsst => tmp.apply_fsst_encoding(fsst_max_symbols).is_ok(),
            ET::Dictionary => tmp.apply_dictionary_encoding().is_ok(),
            ET::Rle => tmp.apply_rle_encoding().is_ok(),
            ET::BitPacking => tmp.apply_bitpacking_encoding().is_ok(),
            ET::Alp => tmp.apply_alp_encoding().is_ok(),
            ET::Constant => tmp.apply_constant_encoding().is_ok(),
            ET::None => false,
        };
        if applied {
            tmp.encoding.clone()
        } else {
            ColumnEncoding::None
        }
    }

    /// Split the current column contents into fixed-size chunks.
    ///
    /// Each chunk carries its own encoding/overlay/meta; raw row bytes stay
    /// in `inner` which remains authoritative for raw chunks. No full-column
    /// value vector or raw-buffer copy is materialized here: each chunk slice
    /// is resolved per chunk (bounded by `chunk_capacity`).
    pub fn materialize_chunks(&mut self) {
        let total = self.len();
        if total == 0 {
            self.chunks.clear();
            return;
        }
        let column_encoding_type = self.encoding.encoding_type();
        let capacity = self.chunk_capacity.max(1);
        let is_var = super::is_variable_length_type(&self.data_type);
        let elem_size = super::element_size(&self.data_type);
        let mut chunks = Vec::new();
        let n = total.div_ceil(capacity);
        for ci in 0..n {
            let start = ci * capacity;
            let end = (start + capacity).min(total);
            let count = end - start;
            let mut chunk = if is_var {
                ColumnChunk::new_variable(start, count, self.nullable)
            } else {
                ColumnChunk::new(start, count, elem_size, self.nullable)
            };
            // Chunk-local encoding sliced from the per-chunk base values
            // (overlay-merged; overflow rows as placeholders).
            let slice: Vec<Option<Value>> =
                (start..end).map(|r| self.encoding_base_value(r)).collect();
            chunk.encoding = if self.encoding.is_encoded() {
                Self::encode_slice(
                    &slice,
                    &self.data_type,
                    self.nullable,
                    column_encoding_type,
                    255,
                )
            } else {
                ColumnEncoding::None
            };
            chunk.overlay = UpdateOverlay::new(DEFAULT_OVERLAY_CAPACITY);
            let num_values = slice.iter().filter(|v| v.is_some()).count() as u32;
            chunk.encoding_meta = crate::encoding::ChunkEncodingMeta {
                scheme: chunk.encoding.encoding_type(),
                num_values,
                all_null: num_values == 0,
                ..Default::default()
            };
            chunk.updates_since_encode = 0;
            chunks.push(chunk);
        }
        self.chunks = chunks;
    }

    // -----------------------------------------------------------------------
    // Chunk-level encoding
    // -----------------------------------------------------------------------

    /// Per-chunk encoding metadata: (chunk_idx, encoding type, row count).
    /// Evicted chunks report their pre-evict scheme so sidecars and chunk
    /// profiles keep describing the flushed layout.
    pub fn chunk_encoding_metadata(&self) -> Vec<(usize, crate::encoding::EncodingType, usize)> {
        if self.chunks.is_empty() {
            return vec![(0, self.encoding.encoding_type(), self.len())];
        }
        self.chunks
            .iter()
            .enumerate()
            .map(|(i, c)| (i, c.evicted_encoding(), c.row_count))
            .collect()
    }

    /// Re-evict one resident chunk from checkpoint sidecar pages without
    /// decoding them onto the heap. Matches by row window; a mismatch or
    /// an already-evicted chunk keeps current state and reports false so
    /// the caller stays resident. Used by reload to restore the persisted
    /// eviction state.
    pub fn restore_mapped_chunk(
        &mut self,
        record: super::chunk_residency::MappedChunk,
        map: &std::sync::Arc<memmap2::Mmap>,
    ) -> bool {
        let Some(chunk) = self.chunks.iter_mut().find(|c| {
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
        if !chunk.residency.is_resident() {
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
        chunk.data = Vec::new();
        chunk.offsets = Vec::new();
        chunk.null_bitmap = None;
        chunk.encoding = crate::encoding::ColumnEncoding::None;
        chunk.residency = ChunkResidency::Evicted(snapshot);
        true
    }

    /// Apply one selected encoding to this column.
    ///
    /// Single-column form of the store-level dispatch: chunked columns go
    /// through the per-chunk path, unchunked columns through the matching
    /// column-level encoder. Empty columns are a no-op.
    pub fn apply_selected_encoding(
        &mut self,
        encoding_type: crate::encoding::EncodingType,
        fsst_max_symbols: usize,
    ) -> StorageResult<()> {
        if self.is_empty() {
            return Ok(());
        }

        // Chunk-level path: each resident chunk selects and stores its own
        // encoding so point updates only decode the affected chunk.
        if self.has_chunks() {
            return self.apply_encoding_to_chunks(encoding_type, fsst_max_symbols);
        }

        match encoding_type {
            crate::encoding::EncodingType::Fsst => {
                if self.data_type != DataType::String
                    && self.data_type != DataType::Json
                    && !matches!(self.data_type, DataType::FixedString(_))
                {
                    return Err(StorageError::not_supported(format!(
                        "FSST encoding does not support type {:?}",
                        self.data_type
                    )));
                }
                self.apply_fsst_encoding(fsst_max_symbols)?;
            }
            crate::encoding::EncodingType::Dictionary => {
                self.apply_dictionary_encoding()?;
            }
            crate::encoding::EncodingType::Rle => {
                self.apply_rle_encoding()?;
            }
            crate::encoding::EncodingType::BitPacking => {
                self.apply_bitpacking_encoding()?;
            }
            crate::encoding::EncodingType::Alp => {
                self.apply_alp_encoding()?;
            }
            crate::encoding::EncodingType::Constant => {
                self.apply_constant_encoding()?;
            }
            crate::encoding::EncodingType::None => {}
        }
        // Encodings are built from placeholder base values; overflow rows
        // keep snapshot from the side store, so mappings are preserved.

        Ok(())
    }

    /// Apply an encoding type independently per chunk.
    ///
    /// Each chunk is resolved and encoded in isolation (bounded by chunk
    /// capacity): no full-column value vector is ever materialized. When a
    /// chunk selects `None`, its overlay-merged values are written back to
    /// the raw buffer before the overlay is cleared so no update is lost.
    pub fn apply_encoding_to_chunks(
        &mut self,
        encoding_type: crate::encoding::EncodingType,
        fsst_max_symbols: usize,
    ) -> StorageResult<()> {
        if self.chunks.is_empty() {
            self.materialize_chunks();
        }
        if self.chunks.is_empty() {
            return Ok(());
        }
        // Encoding rebuilds need decoded buffers: promote evicted chunks
        // first so every chunk below is resident. Load failures propagate
        // instead of silently encoding a partial column.
        for idx in 0..self.chunks.len() {
            self.ensure_resident(idx)?;
        }
        let total = self.len();
        for idx in 0..self.chunks.len() {
            let start = self.chunks[idx].row_offset;
            let end = start.saturating_add(self.chunks[idx].row_count).min(total);
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
            if selected == crate::encoding::EncodingType::None {
                // Preserve overlay-merged values: the raw buffer still holds
                // the pre-overlay base, so write merged values back first.
                for (off, v) in values.iter().enumerate() {
                    let row = start + off;
                    let _ = self.write_raw_inner(row, v.as_ref());
                }
                self.chunks[idx].encoding = ColumnEncoding::None;
                self.chunks[idx].clear_overlay_after_flush();
                self.chunks[idx].encoding_meta.scheme = crate::encoding::EncodingType::None;
                self.chunks[idx].encoding_meta.num_values =
                    values.iter().filter(|v| v.is_some()).count() as u32;
                self.chunks[idx].encoding_meta.all_null =
                    self.chunks[idx].encoding_meta.num_values == 0;
                continue;
            }
            // Encode this chunk slice in isolation (chunk-local indexes).
            let encoded = Self::encode_slice(
                &values,
                &self.data_type,
                self.nullable,
                selected,
                fsst_max_symbols,
            );
            if encoded.is_encoded() {
                self.chunks[idx].encoding = encoded;
                self.chunks[idx].clear_overlay_after_flush();
                let num_values = values.iter().filter(|v| v.is_some()).count() as u32;
                self.chunks[idx].encoding_meta.scheme = self.chunks[idx].encoding.encoding_type();
                self.chunks[idx].encoding_meta.num_values = num_values;
                self.chunks[idx].encoding_meta.all_null = num_values == 0;
                self.chunks[idx].encoding_meta.compressed_size =
                    self.chunks[idx].encoding.memory_usage() as u64;
            }
        }
        // Mirror the first chunk as the column-level encoding so
        // single-buffer readers observe the active scheme.
        if let Some(first) = self.chunks.first() {
            self.encoding = first.encoding.clone();
        }
        Ok(())
    }
}
