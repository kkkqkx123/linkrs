use graphdb_core::{DataType, StorageError, StorageResult, Value};

use super::column::Column;
use super::mvcc::VersionChainStats;
use super::zone_map::ZoneBounds;
use crate::cursor::ColumnValues;
use crate::encoding::EncodingType;

use graphdb_core::types::Timestamp;

// ---------------------------------------------------------------------------
// Internal helpers (used by ColumnStore and Column)
// ---------------------------------------------------------------------------

pub(crate) fn ensure_bitmap_len(
    bitmap: &mut bitvec::vec::BitVec<u8, bitvec::order::Lsb0>,
    min_len: usize,
) {
    if bitmap.len() < min_len {
        bitmap.resize(min_len, false);
    }
}

/// Rough heap footprint of a `Value`'s payload (used for MVCC memory
/// accounting of retained version chains). For heap-allocated types, we
/// estimate the payload size based on the inner data structure.
pub(crate) fn value_payload_bytes(value: &Value) -> usize {
    use graphdb_core::value::{Geography, VectorValue};
    match value {
        Value::String(s) => s.len(),
        Value::FixedString(s) => s.len(),
        Value::Blob(b) => b.len(),
        Value::List(l) => l.len() * std::mem::size_of::<Value>(),
        Value::Map(m) => {
            m.len() * (std::mem::size_of::<Value>() * 2) // key + value per entry
        }
        Value::Set(s) => s.len() * std::mem::size_of::<Value>(),
        Value::Geography(geo) => {
            // Geography contains coordinate data; estimate based on point count
            match geo {
                Geography::Point(_) => 24, // 2 x f64 + srid
                Geography::LineString(ls) => ls.points.len() * 24,
                Geography::Polygon(pg) => {
                    pg.exterior.points.len() * 24
                        + pg.holes.iter().map(|r| r.points.len() * 24).sum::<usize>()
                }
                Geography::MultiPoint(mp) => mp.points.len() * 24,
                Geography::MultiLineString(ml) => ml
                    .linestrings
                    .iter()
                    .map(|ls| ls.points.len() * 24)
                    .sum::<usize>(),
                Geography::MultiPolygon(mpg) => mpg
                    .polygons
                    .iter()
                    .map(|pg| {
                        pg.exterior.points.len() * 24
                            + pg.holes.iter().map(|r| r.points.len() * 24).sum::<usize>()
                    })
                    .sum::<usize>(),
            }
        }
        Value::Vector(v) => match v {
            VectorValue::Dense(data) => data.len() * std::mem::size_of::<f32>(),
            VectorValue::Sparse { indices, values } => {
                indices.len() * std::mem::size_of::<u32>()
                    + values.len() * std::mem::size_of::<f32>()
            }
        },
        Value::Json(j) => j.as_str().len(),
        Value::JsonB(j) => j.estimated_size(),
        Value::DataSet(ds) => {
            ds.col_names.len() * ds.rows.len() * 8 // rough estimate
        }
        Value::Struct(sv) => sv.fields.len() * std::mem::size_of::<Value>(),
        Value::Array(av) => av.values.len() * std::mem::size_of::<Value>(),
        Value::Vertex(_) => 64,         // fixed-size vertex record
        Value::Edge(_) => 64,           // fixed-size edge record
        Value::Path(p) => p.len() * 32, // per-hop estimate
        _ => 0, // fixed-width types (Bool, Int, Float, etc.) have no heap payload
    }
}

/// Timestamp-aware variant of [`decode_column_values`]: decodes each row's
/// value as visible at `query_ts` through the MVCC version chain.
fn decode_column_values_at_ts(
    column: &Column,
    rows: &[usize],
    query_ts: Timestamp,
) -> ColumnValues {
    match &column.data_type {
        DataType::BigInt => {
            let mut values = Vec::with_capacity(rows.len());
            let mut valid = vec![0u8; rows.len()];
            for (i, &row) in rows.iter().enumerate() {
                match column.get_at_ts(row, query_ts) {
                    Some(Value::BigInt(v)) => {
                        values.push(v);
                        valid[i] = 1;
                    }
                    Some(_) => return general_column_at_ts(column, rows, query_ts),
                    None => values.push(0),
                }
            }
            ColumnValues::I64 { values, valid }
        }
        DataType::Double => {
            let mut values = Vec::with_capacity(rows.len());
            let mut valid = vec![0u8; rows.len()];
            for (i, &row) in rows.iter().enumerate() {
                match column.get_at_ts(row, query_ts) {
                    Some(Value::Double(v)) => {
                        values.push(v);
                        valid[i] = 1;
                    }
                    Some(_) => return general_column_at_ts(column, rows, query_ts),
                    None => values.push(0.0),
                }
            }
            ColumnValues::F64 { values, valid }
        }
        DataType::Int => {
            let mut values = Vec::with_capacity(rows.len());
            let mut valid = vec![0u8; rows.len()];
            for (i, &row) in rows.iter().enumerate() {
                match column.get_at_ts(row, query_ts) {
                    Some(Value::Int(v)) => {
                        values.push(v);
                        valid[i] = 1;
                    }
                    Some(_) => return general_column_at_ts(column, rows, query_ts),
                    None => values.push(0),
                }
            }
            ColumnValues::I32 { values, valid }
        }
        DataType::Bool => {
            let mut values = Vec::with_capacity(rows.len());
            let mut valid = vec![0u8; rows.len()];
            for (i, &row) in rows.iter().enumerate() {
                match column.get_at_ts(row, query_ts) {
                    Some(Value::Bool(v)) => {
                        values.push(u8::from(v));
                        valid[i] = 1;
                    }
                    Some(_) => return general_column_at_ts(column, rows, query_ts),
                    None => values.push(0),
                }
            }
            ColumnValues::Bool { values, valid }
        }
        DataType::SmallInt => {
            let mut values = Vec::with_capacity(rows.len());
            let mut valid = vec![0u8; rows.len()];
            for (i, &row) in rows.iter().enumerate() {
                match column.get_at_ts(row, query_ts) {
                    Some(Value::SmallInt(v)) => {
                        values.push(v);
                        valid[i] = 1;
                    }
                    Some(_) => return general_column_at_ts(column, rows, query_ts),
                    None => values.push(0),
                }
            }
            ColumnValues::I16 { values, valid }
        }
        DataType::Float => {
            let mut values = Vec::with_capacity(rows.len());
            let mut valid = vec![0u8; rows.len()];
            for (i, &row) in rows.iter().enumerate() {
                match column.get_at_ts(row, query_ts) {
                    Some(Value::Float(v)) => {
                        values.push(v);
                        valid[i] = 1;
                    }
                    Some(_) => return general_column_at_ts(column, rows, query_ts),
                    None => values.push(0.0),
                }
            }
            ColumnValues::F32 { values, valid }
        }
        _ => general_column_at_ts(column, rows, query_ts),
    }
}

fn general_column_at_ts(column: &Column, rows: &[usize], query_ts: Timestamp) -> ColumnValues {
    ColumnValues::General(
        rows.iter()
            .map(|&r| column.get_at_ts(r, query_ts))
            .collect(),
    )
}

// ---------------------------------------------------------------------------
// ColumnStore
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct ColumnStore {
    columns: Vec<Column>,
    name_to_index: std::collections::HashMap<String, usize>,
}

impl ColumnStore {
    pub fn new() -> Self {
        Self {
            columns: Vec::new(),
            name_to_index: std::collections::HashMap::new(),
        }
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            columns: Vec::with_capacity(capacity),
            name_to_index: std::collections::HashMap::with_capacity(capacity),
        }
    }

    /// Per-chunk min/max bounds of one column, for zone-map pruning.
    /// `None` when the column does not exist in this store.
    pub fn zone_maps_for_column(&self, name: &str) -> Option<&[ZoneBounds]> {
        self.name_to_index
            .get(name)
            .map(|&index| self.columns[index].zone_maps())
    }

    /// Per-chunk length summaries of one column, for complex equality
    /// pre-pruning. `None` when the column does not exist.
    pub fn zone_complex_for_column(
        &self,
        name: &str,
    ) -> Option<&[super::zone_map::ComplexZoneSummary]> {
        self.name_to_index
            .get(name)
            .map(|&index| self.columns[index].zone_complex())
    }

    /// Whether the zone chunk covering `chunk` may contain rows matching
    /// `range`. Returns true unless the chunk provably lies outside.
    ///
    /// Equality probes on length-carrying values first check the per-chunk
    /// length summary; a probe length outside the recorded interval skips
    /// the chunk without decoding. Nested equality probes then check the
    /// scalar-leaf interval and the key bloom: disjoint leaf ranges or
    /// probe key bits outside the chunk fingerprint skip the chunk even
    /// when outer lengths coincide. All probes then fall back to
    /// whole-value min/max ordering, preserving the conservative contract.
    pub fn zone_prunes_in(&self, chunk: usize, range: &crate::cursor::PredicateRange) -> bool {
        if let Some(probe_len) = range.equality_len() {
            if let Some(summary) = self
                .zone_complex_for_column(&range.column)
                .and_then(|s| s.get(chunk))
            {
                if let (Some(lo), Some(hi)) = (summary.len_min, summary.len_max) {
                    if probe_len < lo || probe_len > hi {
                        return false;
                    }
                }
                if let Some((probe_lo, probe_hi)) = range.equality_leaf_range() {
                    if let (Some(lo), Some(hi)) = (&summary.leaf_min, &summary.leaf_max) {
                        use super::zone_map::compare_values;
                        if compare_values(&probe_hi, lo) == std::cmp::Ordering::Less
                            || compare_values(&probe_lo, hi) == std::cmp::Ordering::Greater
                        {
                            return false;
                        }
                    }
                }
                if let Some(probe_fp) = range.equality_key_fp() {
                    if summary.key_fp & probe_fp != probe_fp {
                        return false;
                    }
                }
            }
        }
        let Some(bounds) = self.zone_maps_for_column(&range.column) else {
            return true;
        };
        let Some(zb) = bounds.get(chunk) else {
            return true;
        };
        let (Some(min), Some(max)) = (&zb.min, &zb.max) else {
            return true;
        };
        range.overlaps(min, max)
    }

    /// Global min/max bounds of one column, merged across all chunks with the
    /// same numeric comparison semantics used by pushed-predicate evaluation.
    /// `None` when the column is absent or has no recorded bounds.
    pub fn aggregate_zone_bounds(&self, name: &str) -> Option<ZoneBounds> {
        let zones = self.zone_maps_for_column(name)?;
        let mut merged = ZoneBounds::default();
        for zone in zones {
            if let Some(v) = &zone.min {
                match &merged.min {
                    Some(cur)
                        if super::zone_map::compare_values(cur, v)
                            != std::cmp::Ordering::Greater => {}
                    _ => merged.min = Some(v.clone()),
                }
            }
            if let Some(v) = &zone.max {
                match &merged.max {
                    Some(cur)
                        if super::zone_map::compare_values(cur, v) != std::cmp::Ordering::Less => {}
                    _ => merged.max = Some(v.clone()),
                }
            }
        }
        (merged.min.is_some() || merged.max.is_some()).then_some(merged)
    }

    pub fn add_column(&mut self, name: String, data_type: DataType, nullable: bool) -> i32 {
        let col_id = self.columns.len() as i32;
        let column = Column::new(name.clone(), col_id, data_type, nullable);
        self.name_to_index.insert(name, self.columns.len());
        self.columns.push(column);
        col_id
    }

    pub fn get_column(&self, name: &str) -> Option<&Column> {
        self.name_to_index
            .get(name)
            .and_then(|&idx| self.columns.get(idx))
    }

    /// The declared data type of the column `name`, if it exists.
    pub fn data_type_of(&self, name: &str) -> Option<DataType> {
        self.get_column(name).map(|c| c.data_type.clone())
    }

    pub fn get_column_mut(&mut self, name: &str) -> Option<&mut Column> {
        self.name_to_index
            .get(name)
            .and_then(|&idx| self.columns.get_mut(idx))
    }

    pub fn get_column_by_id(&self, col_id: i32) -> Option<&Column> {
        self.columns.get(col_id as usize)
    }

    pub fn get_column_by_id_mut(&mut self, col_id: i32) -> Option<&mut Column> {
        self.columns.get_mut(col_id as usize)
    }

    pub fn set(&mut self, row_idx: usize, values: &[(String, Value)]) -> StorageResult<()> {
        for (name, value) in values {
            if let Some(col) = self.get_column_mut(name) {
                col.set(row_idx, Some(value))?;
            }
        }
        Ok(())
    }

    pub fn get(&self, row_idx: usize) -> Vec<(String, Option<Value>)> {
        self.columns
            .iter()
            .map(|col| (col.name.clone(), col.get(row_idx)))
            .collect()
    }

    // -----------------------------------------------------------------------
    // MVCC (versioned) read / write
    // -----------------------------------------------------------------------

    /// Versioned write of multiple properties for one row at `ts`.
    pub fn set_versioned(
        &mut self,
        row_idx: usize,
        values: &[(String, Value)],
        ts: Timestamp,
    ) -> StorageResult<()> {
        for (name, value) in values {
            if let Some(col) = self.get_column_mut(name) {
                col.set_versioned(row_idx, Some(value), ts)?;
            }
        }
        Ok(())
    }

    /// Versioned write of a single property for one row at `ts`.
    pub fn set_property_versioned(
        &mut self,
        row_idx: usize,
        col_name: &str,
        value: Option<&Value>,
        ts: Timestamp,
    ) -> StorageResult<()> {
        let col = self
            .get_column_mut(col_name)
            .ok_or_else(|| StorageError::column_not_found(col_name.to_string()))?;
        col.set_versioned(row_idx, value, ts)
    }

    /// Read all columns for one row as visible at `query_ts`.
    pub fn get_at_ts(&self, row_idx: usize, query_ts: Timestamp) -> Vec<(String, Option<Value>)> {
        self.columns
            .iter()
            .map(|col| (col.name.clone(), col.get_at_ts(row_idx, query_ts)))
            .collect()
    }

    /// Start timestamps of the per-column versions covering `query_ts`.
    ///
    /// Internal companion of [`ColumnStore::get_at_ts`] for pending-aware
    /// point lookups: when any covering stamp belongs to a foreign
    /// uncommitted write the caller re-reads at `stamp - 1`.
    pub fn picked_starts_at(&self, row_idx: usize, query_ts: Timestamp) -> Vec<Timestamp> {
        self.columns
            .iter()
            .map(|col| col.start_ts_at(row_idx, query_ts))
            .collect()
    }

    /// Read only the requested columns for one row as visible at `query_ts`.
    pub fn get_projected_at_ts(
        &self,
        row_idx: usize,
        projection: &[String],
        query_ts: Timestamp,
    ) -> Vec<(String, Option<Value>)> {
        projection
            .iter()
            .filter_map(|name| {
                self.get_column(name)
                    .map(|column| (name.clone(), column.get_at_ts(row_idx, query_ts)))
            })
            .collect()
    }

    /// Batch read of all columns for multiple rows at `query_ts`.
    pub fn get_batch_at_ts(
        &self,
        rows: &[usize],
        query_ts: Timestamp,
    ) -> Vec<Vec<(String, Option<Value>)>> {
        let mut out = vec![Vec::with_capacity(self.columns.len()); rows.len()];
        for col in &self.columns {
            for (ri, &row) in rows.iter().enumerate() {
                out[ri].push((col.name.clone(), col.get_at_ts(row, query_ts)));
            }
        }
        out
    }

    /// Batch variant of [`get_projected_at_ts`].
    pub fn get_projected_batch_at_ts(
        &self,
        rows: &[usize],
        projection: &[String],
        query_ts: Timestamp,
    ) -> Vec<Vec<(String, Option<Value>)>> {
        let mut out = vec![Vec::with_capacity(projection.len()); rows.len()];
        for name in projection {
            if let Some(column) = self.get_column(name) {
                for (ri, &row) in rows.iter().enumerate() {
                    out[ri].push((name.clone(), column.get_at_ts(row, query_ts)));
                }
            }
        }
        out
    }

    /// Column-major batch decode at `query_ts` (A1 column-block path).
    pub fn get_projected_columns_at_ts(
        &self,
        rows: &[usize],
        names: &[String],
        query_ts: Timestamp,
    ) -> Vec<(String, ColumnValues)> {
        if names.is_empty() {
            self.columns
                .iter()
                .map(|column| {
                    let values = decode_column_values_at_ts(column, rows, query_ts);
                    (column.name.clone(), values)
                })
                .collect()
        } else {
            names
                .iter()
                .map(|name| {
                    let values = match self.get_column(name) {
                        Some(column) => decode_column_values_at_ts(column, rows, query_ts),
                        None => ColumnValues::General(vec![None; rows.len()]),
                    };
                    (name.clone(), values)
                })
                .collect()
        }
    }

    /// Aggregate version-chain statistics across all columns.
    pub fn version_chain_stats(&self) -> VersionChainStats {
        let mut total_rows = 0usize;
        let mut total_entries = 0usize;
        let mut max_len = 0usize;
        let mut memory_bytes = 0usize;
        for col in &self.columns {
            let stats = col.version_chain_stats();
            total_rows = total_rows.max(stats.total_rows);
            total_entries += stats.total_entries;
            max_len = max_len.max(stats.max_len);
            memory_bytes += stats.memory_bytes;
        }
        let avg_len = if total_rows > 0 {
            total_entries as f64 / total_rows as f64
        } else {
            0.0
        };
        VersionChainStats {
            total_rows,
            total_entries,
            max_len,
            avg_len,
            memory_bytes,
        }
    }

    /// Garbage-collect version chains across all columns, returning the total
    /// number of before-images removed.
    pub fn gc_versions(&mut self, min_active_snapshot_ts: Timestamp) -> usize {
        let mut removed = 0;
        for col in &mut self.columns {
            removed += col.gc_versions(min_active_snapshot_ts);
        }
        removed
    }

    /// Exact zone rebuild for columns past the stale-write threshold.
    /// Called from watermark-coordinated maintenance after version chains
    /// fold, so long update histories regain pruning precision.
    pub fn maybe_rebuild_zones_exact(&mut self) -> usize {
        let mut rebuilt = 0;
        for col in &mut self.columns {
            if col.maybe_rebuild_zone_maps_exact() {
                rebuilt += 1;
            }
        }
        rebuilt
    }

    /// Copy the MVCC row state (creation timestamp + before-image chain) of
    /// every column from another store's row, used by table compaction to
    /// preserve version history when rows move into a rebuilt store.
    pub(crate) fn clone_row_state_from(&mut self, src: &ColumnStore, from: usize, to: usize) {
        for dst_col in &mut self.columns {
            let name = dst_col.name.clone();
            if let Some(src_col) = src.get_column(&name) {
                dst_col.clone_row_state_from(src_col, from, to);
            }
        }
    }

    pub fn remove_column(&mut self, name: &str) -> StorageResult<()> {
        let index = self
            .name_to_index
            .get(name)
            .copied()
            .ok_or_else(|| StorageError::column_not_found(name.to_string()))?;

        self.columns.remove(index);

        self.name_to_index.clear();
        for (idx, column) in self.columns.iter_mut().enumerate() {
            column.col_id = idx as i32;
            self.name_to_index.insert(column.name.clone(), idx);
        }

        Ok(())
    }

    pub fn rename_column(&mut self, old_name: &str, new_name: String) -> StorageResult<()> {
        if self.name_to_index.contains_key(&new_name) {
            return Err(StorageError::column_already_exists(new_name));
        }

        let index = self
            .name_to_index
            .get(old_name)
            .copied()
            .ok_or_else(|| StorageError::column_not_found(old_name.to_string()))?;

        if let Some(column) = self.columns.get_mut(index) {
            column.name = new_name;
        }

        self.name_to_index.clear();
        for (idx, column) in self.columns.iter().enumerate() {
            self.name_to_index.insert(column.name.clone(), idx);
        }

        Ok(())
    }

    pub fn column_count(&self) -> usize {
        self.columns.len()
    }

    /// Pre-allocate capacity for `additional` more rows in every column.
    pub fn reserve(&mut self, additional: usize) {
        for column in &mut self.columns {
            column.reserve(additional);
        }
    }

    pub fn row_count(&self) -> usize {
        self.columns.first().map(|c| c.len()).unwrap_or(0)
    }

    pub fn clear(&mut self) {
        for col in &mut self.columns {
            col.clear();
        }
    }

    pub fn resize(&mut self, new_count: usize) {
        for col in &mut self.columns {
            col.resize(new_count);
        }
    }

    pub fn columns(&self) -> &[Column] {
        &self.columns
    }

    /// Collect all dirty pages across columns.
    pub fn collect_dirty_pages(&self) -> Vec<crate::persistence::dirty_page::PageId> {
        let mut pages = Vec::new();
        for col in &self.columns {
            for pid in col.dirty_pages() {
                pages.push(crate::persistence::dirty_page::PageId::new(
                    crate::persistence::dirty_page::ComponentType::VertexColumns,
                    pid as u64,
                ));
            }
        }
        pages
    }

    pub fn clear_dirty(&mut self) {
        for col in &mut self.columns {
            col.clear_dirty();
        }
    }

    /// Clear the dirty mark only for the given `(column_name, page_id)` pairs,
    /// leaving other dirty pages tracked for a later flush.
    pub fn clear_pages(&mut self, pages: &[(String, usize)]) {
        for (name, page_id) in pages {
            if let Some(col) = self.get_column_mut(name) {
                col.clear_page_dirty(*page_id);
            }
        }
    }

    pub fn total_dirty_pages(&self) -> usize {
        self.columns.iter().map(|c| c.dirty_count()).sum()
    }

    /// Evict cold column chunks oldest-first until `budget` bytes are
    /// released. Returns `(chunks_evicted, bytes_released)`.
    pub fn evict_cold_chunks(&mut self, budget: u64) -> (usize, u64) {
        let mut count = 0usize;
        let mut freed = 0u64;
        for col in &mut self.columns {
            if freed >= budget {
                break;
            }
            let (n, bytes) = col.evict_cold_chunks(budget.saturating_sub(freed));
            count += n;
            freed += bytes;
        }
        (count, freed)
    }

    /// Persist every evicted chunk into a `{column}.snapshot` checkpoint
    /// sidecar, reusing the already compressed pages without promoting the
    /// live chunks. Columns without evicted chunks drop their sidecar;
    /// sidecars of dropped columns are swept. Failures of the structural
    /// encoding fail the flush; a single unreadable snapshot only warns
    /// and stays resident on reload.
    pub fn flush_evict_snapshots(&self, dir: &std::path::Path) -> StorageResult<()> {
        use super::chunk_residency::{encode_snapshot_sidecar, ChunkResidency, SnapshotChunkPlan};

        let mut live = std::collections::HashSet::new();
        for col in &self.columns {
            let file_name = format!("{}.snapshot", col.name);
            live.insert(file_name.clone());
            let mut plans = Vec::new();
            for chunk in col.chunks.iter() {
                let ChunkResidency::Evicted(snapshot) = &chunk.residency else {
                    continue;
                };
                let Ok(row_offset) = u32::try_from(chunk.row_offset) else {
                    log::warn!(
                        "snapshot sidecar skips chunk at row {} of column {}: offset exceeds u32",
                        chunk.row_offset,
                        col.name,
                    );
                    continue;
                };
                let Ok(rows) = u32::try_from(chunk.row_count) else {
                    log::warn!(
                        "snapshot sidecar skips chunk of column {}: row count exceeds u32",
                        col.name,
                    );
                    continue;
                };
                let pages = match snapshot.compressed_pages() {
                    Ok(pages) => pages,
                    Err(e) => {
                        log::warn!(
                            "snapshot sidecar skips evicted chunk of column {}: {}",
                            col.name,
                            e
                        );
                        continue;
                    }
                };
                plans.push(SnapshotChunkPlan {
                    row_offset,
                    rows,
                    encoding: snapshot.encoding,
                    meta: &snapshot.meta,
                    uncompressed_bytes: snapshot.uncompressed_bytes as u64,
                    pages,
                });
            }
            if plans.is_empty() {
                let stale = dir.join(&file_name);
                if stale.exists() {
                    if let Err(e) = std::fs::remove_file(&stale) {
                        log::warn!(
                            "cannot remove stale snapshot sidecar {}: {}",
                            stale.display(),
                            e
                        );
                    }
                }
                continue;
            }
            let bytes = encode_snapshot_sidecar(&plans)?;
            crate::compression::write_shadow_file(dir.join(&file_name), &bytes)?;
        }
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.ends_with(".snapshot") && !live.contains(&name) {
                    log::debug!("removing orphan snapshot sidecar {}", name);
                    if let Err(e) = std::fs::remove_file(entry.path()) {
                        log::warn!("cannot remove orphan snapshot sidecar {}: {}", name, e);
                    }
                }
            }
        }
        Ok(())
    }

    /// Re-evict chunks persisted by [`Self::flush_evict_snapshots`] from
    /// memory-mapped sidecars. A missing or corrupt sidecar only warns and
    /// keeps the affected chunks resident; the open never fails over a
    /// derived cache.
    pub fn load_evict_snapshots(&mut self, dir: &std::path::Path) {
        use super::chunk_residency::open_snapshot_sidecar;

        for col in &mut self.columns {
            let path = dir.join(format!("{}.snapshot", col.name));
            if !path.exists() {
                continue;
            }
            let mapped = match open_snapshot_sidecar(&path) {
                Ok(mapped) => mapped,
                Err(e) => {
                    log::warn!("ignoring corrupt snapshot sidecar for {}: {}", col.name, e);
                    continue;
                }
            };
            for record in mapped.chunks {
                col.restore_mapped_chunk(record, &mapped.map);
            }
        }
    }

    /// Quota-segmented eviction across columns for background tasks.
    /// `task_quota` caps one segment; over-quota work proceeds in segments.
    /// Returns `(chunks_evicted, bytes_released, segments)`.
    pub fn evict_cold_chunks_with_quota(
        &mut self,
        budget: u64,
        task_quota: u64,
    ) -> (usize, u64, usize) {
        let mut count = 0usize;
        let mut freed = 0u64;
        let mut segments = 0usize;
        for col in &mut self.columns {
            if freed >= budget {
                break;
            }
            let (n, bytes, segs) =
                col.evict_cold_chunks_with_quota(budget.saturating_sub(freed), task_quota);
            count += n;
            freed += bytes;
            segments += segs;
        }
        (count, freed, segments)
    }

    /// Resident decoded bytes across columns.
    pub fn resident_memory_usage(&self) -> usize {
        self.columns.iter().map(|c| c.resident_memory_usage()).sum()
    }

    /// Compressed snapshot bytes retained for evicted chunks.
    pub fn evicted_bytes(&self) -> usize {
        self.columns.iter().map(|c| c.evicted_bytes()).sum()
    }

    /// Chunks with decoded data in memory.
    pub fn resident_chunk_count(&self) -> usize {
        self.columns.iter().map(|c| c.resident_chunk_count()).sum()
    }

    /// Chunks released with only the snapshot retained.
    pub fn evicted_chunk_count(&self) -> usize {
        self.columns.iter().map(|c| c.evicted_chunk_count()).sum()
    }

    pub fn mark_row_dirty(&mut self, row_idx: usize) {
        for col in &mut self.columns {
            col.mark_dirty(row_idx);
        }
    }

    pub fn load_column_from_raw(
        &mut self,
        name: &str,
        data: Vec<u8>,
        offsets: Vec<u64>,
        null_bitmap_raw: Option<Vec<u8>>,
        bitmap_bit_len: usize,
    ) -> StorageResult<()> {
        if let Some(col) = self.get_column_mut(name) {
            col.load_data_from_raw(data, offsets, null_bitmap_raw, bitmap_bit_len);
            Ok(())
        } else {
            Err(StorageError::column_not_found(name.to_string()))
        }
    }

    pub fn apply_encoding_to_column(
        &mut self,
        col_name: &str,
        encoding_type: EncodingType,
        fsst_max_symbols: usize,
    ) -> StorageResult<()> {
        let col = self
            .get_column_mut(col_name)
            .ok_or_else(|| StorageError::column_not_found(col_name.to_string()))?;

        col.apply_selected_encoding(encoding_type, fsst_max_symbols)
    }

    pub fn memory_size(&self) -> usize {
        let mut total = std::mem::size_of::<Self>();

        for col in &self.columns {
            total += col.memory_size();
        }

        total += self.name_to_index.len()
            * (std::mem::size_of::<String>() + std::mem::size_of::<usize>());

        total
    }

    pub fn used_memory_size(&self) -> usize {
        let mut total = std::mem::size_of::<Self>();

        for col in &self.columns {
            total += col.used_memory_size();
        }

        total
    }
}

impl Default for ColumnStore {
    fn default() -> Self {
        Self::new()
    }
}
