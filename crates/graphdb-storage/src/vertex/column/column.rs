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
/// | Bool, SmallInt, Int, BigInt, Float, Double, Date, Time, Uuid | `FixedWidthColumn` |
/// | String | `VariableWidthColumn` |
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
        // Ensure the target chunk is resident before writing.
        if !self.chunks[chunk_idx].is_resident() {
            let spill_path = self.chunks[chunk_idx].spill_path.clone();
            if let Some(path) = spill_path {
                use super::chunk_residency::{reload_chunk, ChunkResidency};
                if let Ok(bytes) = std::fs::read(&path) {
                    if let Ok(reloaded) = reload_chunk(&bytes) {
                        self.chunks[chunk_idx] = reloaded;
                        self.chunks[chunk_idx].residency = ChunkResidency::Resident;
                        self.chunks[chunk_idx].spill_path = None;
                        self.chunks[chunk_idx].spill_size = 0;
                    }
                }
            }
        }
        let local = (row_idx - self.chunks[chunk_idx].row_offset) as u32;
        let data_type = self.data_type.clone();
        let decision = self.chunks[chunk_idx].set_value(local, value.cloned(), &data_type);
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
        // Ensure the target chunk is resident before any write path.
        if !self.chunks.is_empty() {
            let capacity = self.chunk_capacity.max(1);
            let chunk_idx = row_idx / capacity;
            if chunk_idx < self.chunks.len() && !self.chunks[chunk_idx].is_resident() {
                self.ensure_chunk_resident(chunk_idx);
            }
        }
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
                // Evicted chunks: this is a &self method, so we cannot reload
                // in-place. The caller must use ensure_chunk_resident() before
                // calling get() on a column that may have evicted chunks.
                // Fall through to inner/encoding path for evicted chunks.
                if chunk.is_resident() {
                    let local = (row_idx.saturating_sub(chunk.row_offset)) as u32;
                    if let Some(hit) = chunk.overlay.get(local) {
                        return hit;
                    }
                    // Chunk-local encodings are authoritative when present;
                    // raw chunks fall through to the inner buffer below.
                    if chunk.encoding.is_encoded() {
                        return chunk.encoding.get(local as usize);
                    }
                }
            }
        }
        if self.encoding.is_encoded() {
            return self.encoding.get(row_idx);
        }
        self.inner().get(row_idx)
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
                if c.is_resident() {
                    c.overlay.memory_usage() + c.encoding_meta.memory_usage()
                } else {
                    // Evicted chunks: account for spill_size as "still占用" for memory pressure
                    c.spill_size as usize
                }
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
    // Chunk eviction / reload (lazy loading)
    // -----------------------------------------------------------------------

    /// Evict a single chunk to disk. The chunk's data is serialized to a
    /// sidecar file under `spill_dir` and its in-memory buffers are freed.
    /// Chunks with dirty overlays, active version chains, or dirty pages
    /// are skipped (returns `false`).
    ///
    /// On success the chunk transitions to `Evicted` state and reads targeting
    /// its row range will trigger an on-demand reload.
    pub fn evict_chunk(&mut self, chunk_idx: usize, spill_dir: &std::path::Path) -> bool {
        use super::chunk_residency::{spill_chunk, ChunkResidency};

        let chunk = match self.chunks.get(chunk_idx) {
            Some(c) => c,
            None => return false,
        };

        // Don't evict chunks that are dirty or have active MVCC state.
        if chunk.overlay.len() > 0 {
            return false;
        }
        if chunk.version_chains.as_ref().is_some_and(|chains| {
            chains
                .get(chunk.row_offset / self.chunk_capacity.max(1))
                .is_some_and(|c| !c.is_empty())
        }) {
            return false;
        }
        if !chunk.is_resident() {
            return false; // already evicted
        }

        // Serialize the chunk to a spill file.
        let file_name = format!("{}_{}.spill", self.name, chunk_idx);
        let spill_path = spill_dir.join(&file_name);
        let data = match spill_chunk(chunk, &self.data_type) {
            Ok(d) => d,
            Err(_) => return false,
        };
        let spill_size = data.len() as u64;

        // Write to disk.
        if std::fs::write(&spill_path, &data).is_err() {
            return false;
        }

        // Transition the chunk to evicted state, dropping in-memory buffers.
        let chunk = &mut self.chunks[chunk_idx];
        chunk.data = Vec::new();
        chunk.offsets = Vec::new();
        chunk.null_bitmap = None;
        chunk.encoding = ColumnEncoding::None;
        chunk.overlay.clear();
        chunk.version_chains = None;
        chunk.residency = ChunkResidency::Evicted {
            spill_path: spill_path.clone(),
            spill_size,
        };
        chunk.spill_path = Some(spill_path);
        chunk.spill_size = spill_size;

        true
    }

    /// Ensure the chunk at `chunk_idx` is resident in memory. If it is
    /// currently evicted, reload it from the spill file. Returns `true`
    /// if the chunk is (or was made) resident.
    pub fn ensure_chunk_resident(&mut self, chunk_idx: usize) -> bool {
        use super::chunk_residency::{reload_chunk, ChunkResidency};

        let chunk = match self.chunks.get(chunk_idx) {
            Some(c) => c,
            None => return false,
        };

        if chunk.is_resident() {
            return true;
        }

        let spill_path = match &chunk.residency {
            ChunkResidency::Evicted { spill_path, .. } => spill_path.clone(),
            ChunkResidency::Resident => return true,
        };

        // Read the spill file.
        let bytes = match std::fs::read(&spill_path) {
            Ok(b) => b,
            Err(_) => return false,
        };

        let reloaded = match reload_chunk(&bytes) {
            Ok(c) => c,
            Err(_) => return false,
        };

        // Preserve the original row_offset (reload_chunk restores it, but be safe).
        self.chunks[chunk_idx] = reloaded;
        self.chunks[chunk_idx].residency = ChunkResidency::Resident;
        self.chunks[chunk_idx].spill_path = None;
        self.chunks[chunk_idx].spill_size = 0;

        true
    }

    /// Evict idle chunks to free memory. Iterates all chunks and evicts
    /// those that are clean (no overlay, no dirty version chains). Returns
    /// the number of chunks evicted. Caller should provide a `spill_dir`.
    pub fn evict_idle_chunks(&mut self, spill_dir: &std::path::Path) -> usize {
        let n = self.chunks.len();
        let mut evicted = 0;
        for idx in 0..n {
            if self.evict_chunk(idx, spill_dir) {
                evicted += 1;
            }
        }
        evicted
    }

    /// Evict all chunks that are resident and clean. Returns the number
    /// of chunks evicted.
    pub fn evict_all_clean_chunks(&mut self, spill_dir: &std::path::Path) -> usize {
        self.evict_idle_chunks(spill_dir)
    }

    /// Total memory used by evicted chunks (spill_size accounting).
    pub fn evicted_memory_usage(&self) -> usize {
        self.chunks
            .iter()
            .filter(|c| !c.is_resident())
            .map(|c| c.spill_size as usize)
            .sum()
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
        let stored_crc = u32::from_le_bytes(bytes[bytes.len() - 4..].try_into().unwrap());
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
    pub fn chunk_encoding_metadata(&self) -> Vec<(usize, crate::encoding::EncodingType, usize)> {
        if self.chunks.is_empty() {
            return vec![(0, self.encoding.encoding_type(), self.len())];
        }
        self.chunks
            .iter()
            .enumerate()
            .map(|(i, c)| (i, c.encoding.encoding_type(), c.row_count))
            .collect()
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
