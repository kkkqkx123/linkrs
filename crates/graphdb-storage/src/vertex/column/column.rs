use graphdb_core::{DataType, StorageError, StorageResult, Value};

use crate::column_stats::ColumnStats;
use crate::encoding::ColumnEncoding;

use super::fixed_width::FixedWidthColumn;
use super::variable_width::VariableWidthColumn;
use super::zone_map::ZoneBounds;

use bitvec::prelude::*;

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

    /// Internal write without dirty marking (used by page deserialization).
    fn write_value_without_dirty(
        &mut self,
        row_idx: usize,
        value: Option<&Value>,
    ) -> StorageResult<()> {
        if self.encoding.is_encoded() {
            if self.encoding.set(row_idx, value).is_ok() {
                if row_idx >= self.len() {
                    self.sync_row_count_from_encoding();
                }
                self.update_zone_maps(row_idx, value);
                return Ok(());
            }
            self.decode_encoding_to_raw()?;
        }
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
        self.update_zone_maps(row_idx, value);
        Ok(())
    }

    /// Write `value` into the column, handling the encoded and raw paths.
    /// Does not touch the MVCC metadata.
    pub(super) fn write_value(
        &mut self,
        row_idx: usize,
        value: Option<&Value>,
    ) -> StorageResult<()> {
        self.mark_dirty(row_idx);
        if self.encoding.is_encoded() {
            if self.encoding.set(row_idx, value).is_ok() {
                if row_idx >= self.len() {
                    self.sync_row_count_from_encoding();
                }
                self.update_zone_maps(row_idx, value);
                return Ok(());
            }
            // Encoded set failed (e.g., row_idx >= row_count during WAL replay).
            // Decode back to raw column format and fall through to the raw path.
            self.decode_encoding_to_raw()?;
        }

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

        self.update_zone_maps(row_idx, value);
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
        self.inner().memory_usage() + self.encoding.memory_usage() + version_bytes
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
        if !self.encoding.is_encoded() {
            return self.inner().get_flush_data();
        }

        let row_count = self.len();
        let mut new_data = Vec::new();
        let mut new_offsets = Vec::new();
        let mut new_bitmap = self.null_bitmap().map(|_| BitVec::with_capacity(row_count));

        let is_var = super::is_variable_length_type(&self.data_type);

        for i in 0..row_count {
            let value = self.encoding.get(i);
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
                        let elem_size = super::element_size(&self.data_type);
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
}
