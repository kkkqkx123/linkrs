use linkrs_core::{DataType, StorageError, StorageResult, Value};
use std::sync::Arc;

use super::column::Column;
use super::mvcc::VersionChainStats;
use super::zone_map::ZoneBounds;
use crate::cursor::ColumnValues;
use crate::encoding::EncodingType;

use linkrs_core::types::Timestamp;

/// One projected row: ordered `(column name, value-or-null)` pairs.
type ProjectedRow = Vec<(Arc<str>, Option<Value>)>;
/// Projected rows in request order: one entry per requested row.
type ProjectedRowBatch = Vec<ProjectedRow>;

/// Eviction sidecar load result with corrupt and mismatch counted separately.
/// Missing files are normal and absent from all counters.
#[derive(Debug, Clone, Copy, Default)]
pub struct SnapshotLoadOutcome {
    pub restored: usize,
    pub corrupt: usize,
    pub mismatched: usize,
}

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
    use linkrs_core::value::{Geography, VectorValue};
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

/// Per-table column set with its own latch for structural changes.
///
/// Point operations only need shared access to individual columns (each
/// column carries its own segment latches), so they take the store shared.
/// Structural operations (add/remove/rename column) take it exclusively;
/// they run under the shard write lock, where no point traffic exists.
/// Lock order is always name map before column vector, then the column's
/// own latches.
#[derive(Debug)]
pub struct ColumnStore {
    columns: parking_lot::RwLock<Vec<Column>>,
    name_to_index: parking_lot::RwLock<std::collections::HashMap<String, usize>>,
    unavailable: parking_lot::RwLock<std::collections::HashMap<String, String>>,
}

impl Clone for ColumnStore {
    fn clone(&self) -> Self {
        Self {
            columns: parking_lot::RwLock::new(self.columns.read().clone()),
            name_to_index: parking_lot::RwLock::new(self.name_to_index.read().clone()),
            unavailable: parking_lot::RwLock::new(self.unavailable.read().clone()),
        }
    }
}

impl ColumnStore {
    pub fn new() -> Self {
        Self {
            columns: parking_lot::RwLock::new(Vec::new()),
            name_to_index: parking_lot::RwLock::new(std::collections::HashMap::new()),
            unavailable: parking_lot::RwLock::new(std::collections::HashMap::new()),
        }
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            columns: parking_lot::RwLock::new(Vec::with_capacity(capacity)),
            name_to_index: parking_lot::RwLock::new(std::collections::HashMap::with_capacity(
                capacity,
            )),
            unavailable: parking_lot::RwLock::new(std::collections::HashMap::new()),
        }
    }

    /// Mark one column unavailable with a human-readable reason. Later strict
    /// reads and writes that touch the column fail with the column name;
    /// projections over healthy columns keep serving.
    pub fn mark_column_unavailable(&self, name: &str, reason: String) {
        self.unavailable.write().insert(name.to_string(), reason);
    }

    /// Unavailable columns with their reasons, sorted by name for stable output.
    pub fn unavailable_columns(&self) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = self
            .unavailable
            .read()
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    pub fn is_column_unavailable(&self, name: &str) -> bool {
        self.unavailable.read().contains_key(name)
    }

    /// Fail when the named column is marked unavailable. Descriptive lookups
    /// (names, declared types, zone bounds of healthy columns) bypass this;
    /// every data path that would decode or write the column calls it first
    /// so the error names the column instead of refusing the whole table.
    pub fn check_column_available(&self, name: &str) -> StorageResult<()> {
        if let Some(reason) = self.unavailable.read().get(name) {
            return Err(StorageError::deserialize_error(format!(
                "column {} unavailable: {}",
                name, reason
            )));
        }
        Ok(())
    }

    /// Drop all unavailable marks. Full baseline loads call this first so a
    /// retry after repairing the files starts from a clean slate; delta
    /// applies preserve existing marks and only add new ones.
    pub fn clear_unavailable_columns(&self) {
        self.unavailable.write().clear();
    }

    /// Per-zone min/max bounds of one column, for zone-map pruning.
    /// `None` when the column does not exist in this store.
    pub fn zone_maps_for_column(&self, name: &str) -> Option<Vec<ZoneBounds>> {
        self.get_column(name).map(|col| col.zone_maps())
    }

    /// Borrow-based zone prune covering range and equality probes.
    ///
    /// Equality probes on length-carrying values first check the per-chunk
    /// length summary; a probe length outside the recorded interval skips
    /// the chunk without decoding. Nested equality probes then check the
    /// scalar-leaf interval and the key bloom: disjoint leaf ranges or
    /// probe key bits outside the chunk fingerprint skip the chunk even
    /// when outer lengths coincide. All probes then fall back to
    /// whole-value min/max ordering, preserving the conservative contract.
    /// No `ZoneBounds` or summary clone crosses the probe.
    pub fn zone_prunes_in_borrowed(
        &self,
        chunk: usize,
        range: &crate::cursor::PredicateRange,
    ) -> bool {
        let Some(column) = self.get_column(&range.column) else {
            return true;
        };
        if let Some(probe_len) = range.equality_len() {
            let complex_prunes = column.with_zone_complex_for_chunk(chunk, |summary| {
                let Some(summary) = summary else {
                    return false;
                };
                if let (Some(lo), Some(hi)) = (summary.len_min, summary.len_max) {
                    if probe_len < lo || probe_len > hi {
                        return true;
                    }
                }
                if let Some((probe_lo, probe_hi)) = range.equality_leaf_range() {
                    if let (Some(lo), Some(hi)) = (&summary.leaf_min, &summary.leaf_max) {
                        if super::zone_map::compare_values(&probe_hi, lo)
                            == std::cmp::Ordering::Less
                            || super::zone_map::compare_values(&probe_lo, hi)
                                == std::cmp::Ordering::Greater
                        {
                            return true;
                        }
                    }
                }
                if let Some(probe_fp) = range.equality_key_fp() {
                    if summary.key_fp & probe_fp != probe_fp {
                        return true;
                    }
                }
                false
            });
            if complex_prunes {
                return false;
            }
        }
        column.with_zone_for_chunk(chunk, |zb| {
            let Some(zb) = zb else { return true };
            let (Some(min), Some(max)) = (&zb.min, &zb.max) else {
                return true;
            };
            range.overlaps(min, max)
        })
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

    pub fn add_column(&self, name: String, data_type: DataType, nullable: bool) -> i32 {
        let mut columns = self.columns.write();
        let mut name_to_index = self.name_to_index.write();
        let col_id = columns.len() as i32;
        let column = Column::new(name.clone(), col_id, data_type, nullable);
        name_to_index.insert(name, columns.len());
        columns.push(column);
        col_id
    }

    /// Shared access to one column. The returned guard derefs to `Column`,
    /// so point-path call sites work unchanged; the guard must not be held
    /// across structural store operations.
    pub fn get_column(&self, name: &str) -> Option<parking_lot::MappedRwLockReadGuard<'_, Column>> {
        let idx = *self.name_to_index.read().get(name)?;
        parking_lot::RwLockReadGuard::try_map(self.columns.read(), |cols| cols.get(idx)).ok()
    }

    /// The declared data type of the column `name`, if it exists.
    pub fn data_type_of(&self, name: &str) -> Option<DataType> {
        self.get_column(name).map(|c| c.data_type.clone())
    }

    /// Run `f` against every column in order, one shared guard at a time.
    /// The guard is released between columns so long scans never pin the
    /// store latch.
    pub fn for_each_column<R>(&self, f: impl FnMut(&Column) -> R) -> Vec<R> {
        let columns = self.columns.read();
        columns.iter().map(f).collect()
    }

    pub fn set(&self, row_idx: usize, values: &[(Arc<str>, Value)]) -> StorageResult<()> {
        for (name, value) in values {
            self.check_column_available(name)?;
            if let Some(col) = self.get_column(name) {
                col.set(row_idx, Some(value))?;
            }
        }
        Ok(())
    }

    pub fn get(&self, row_idx: usize) -> ProjectedRow {
        self.for_each_column(|col| (col.name.as_str().into(), col.get(row_idx)))
    }

    // -----------------------------------------------------------------------
    // MVCC (versioned) read / write
    // -----------------------------------------------------------------------

    /// Versioned write of multiple properties for one row at `ts`.
    pub fn set_versioned(
        &self,
        row_idx: usize,
        values: &[(Arc<str>, Value)],
        ts: Timestamp,
    ) -> StorageResult<()> {
        for (name, value) in values {
            self.check_column_available(name)?;
            if let Some(col) = self.get_column(name) {
                col.set_versioned(row_idx, Some(value), ts)?;
            }
        }
        Ok(())
    }

    /// Versioned write of a single property for one row at `ts` with row
    /// liveness rechecked inside the segment latch.
    pub fn set_property_versioned_checked<F: Fn() -> bool>(
        &self,
        row_idx: usize,
        col_name: &str,
        value: Option<&Value>,
        ts: Timestamp,
        row_alive: F,
    ) -> StorageResult<()> {
        self.check_column_available(col_name)?;
        let col = self
            .get_column(col_name)
            .ok_or_else(|| StorageError::column_not_found(col_name.to_string()))?;
        col.set_versioned_checked(row_idx, value, ts, row_alive)
    }

    /// Read all columns for one row as visible at `query_ts`.
    pub fn get_at_ts(&self, row_idx: usize, query_ts: Timestamp) -> ProjectedRow {
        self.for_each_column(|col| (col.name.as_str().into(), col.get_at_ts(row_idx, query_ts)))
    }

    /// Start timestamps of the per-column versions covering `query_ts`.
    ///
    /// Internal companion of [`ColumnStore::get_at_ts`] for record cache
    /// fences: the stamps travel with a cached record and a hit is accepted
    /// only when they still match live storage.
    pub fn picked_starts_at(&self, row_idx: usize, query_ts: Timestamp) -> Vec<Timestamp> {
        self.for_each_column(|col| col.start_ts_at(row_idx, query_ts))
    }

    /// Combined values plus covering stamps for one row under one pass per
    /// column. Merges the projected decode and the fence-stamp read so
    /// fenced point reads pay one chunk locate per column instead of two.
    pub fn get_projected_with_stamps_at_ts(
        &self,
        row_idx: usize,
        projection: &[Arc<str>],
        query_ts: Timestamp,
    ) -> (ProjectedRow, Vec<Timestamp>) {
        let mut values = Vec::with_capacity(projection.len());
        let mut stamps = Vec::with_capacity(projection.len());
        for name in projection {
            match self.get_column(name) {
                Some(column) => {
                    let (stamp, value) = column.get_with_stamp(row_idx, query_ts);
                    values.push((name.clone(), value));
                    stamps.push(stamp);
                }
                None => {
                    values.push((name.clone(), None));
                    stamps.push(0);
                }
            }
        }
        (values, stamps)
    }

    /// Read only the requested columns for one row as visible at `query_ts`.
    pub fn get_projected_at_ts(
        &self,
        row_idx: usize,
        projection: &[Arc<str>],
        query_ts: Timestamp,
    ) -> ProjectedRow {
        projection
            .iter()
            .filter_map(|name| {
                self.get_column(name)
                    .map(|column| (name.clone(), column.get_at_ts(row_idx, query_ts)))
            })
            .collect()
    }

    /// Strict batch read: corrupt payloads fail with column and row context.
    ///
    /// Query entry for explicit failure semantics. Never-written windows
    /// yield `None`; snapshot, side-store and raw decode failures yield
    /// `Err` so scans never observe silent nulls.
    pub fn try_get_batch_at_ts(
        &self,
        rows: &[usize],
        query_ts: Timestamp,
    ) -> StorageResult<ProjectedRowBatch> {
        let degraded: Vec<(String, String)> = self.unavailable_columns();
        if let Some((name, reason)) = degraded.into_iter().next() {
            return Err(StorageError::deserialize_error(format!(
                "column {} unavailable: {}",
                name, reason
            )));
        }
        let columns = self.columns.read();
        let mut out = vec![Vec::with_capacity(columns.len()); rows.len()];
        for col in columns.iter() {
            for (ri, &row) in rows.iter().enumerate() {
                let value = col.try_get_at_ts(row, query_ts).map_err(|e| {
                    StorageError::deserialize_error(format!(
                        "column {} batch decode failed at row {}: {}",
                        col.name, row, e
                    ))
                })?;
                out[ri].push((col.name.as_str().into(), value));
            }
        }
        Ok(out)
    }

    /// Strict projected batch read: corrupt payloads fail with column and
    /// row context, mirroring [`Self::try_get_batch_at_ts`] for projections.
    pub fn try_get_projected_batch_at_ts(
        &self,
        rows: &[usize],
        projection: &[Arc<str>],
        query_ts: Timestamp,
    ) -> StorageResult<ProjectedRowBatch> {
        let mut out = vec![Vec::with_capacity(projection.len()); rows.len()];
        for name in projection {
            self.check_column_available(name)?;
            let column = self
                .get_column(name)
                .ok_or_else(|| StorageError::column_not_found(name.to_string()))?;
            for (ri, &row) in rows.iter().enumerate() {
                let value = column.try_get_at_ts(row, query_ts).map_err(|e| {
                    StorageError::deserialize_error(format!(
                        "column {} batch decode failed at row {}: {}",
                        name, row, e
                    ))
                })?;
                out[ri].push((name.clone(), value));
            }
        }
        Ok(out)
    }

    /// Column-major batch decode at `query_ts` (A1 column-block path).
    pub fn get_projected_columns_at_ts(
        &self,
        rows: &[usize],
        names: &[Arc<str>],
        query_ts: Timestamp,
    ) -> Vec<(Arc<str>, ColumnValues)> {
        if names.is_empty() {
            self.for_each_column(|column| {
                if self.is_column_unavailable(&column.name) {
                    return (
                        column.name.as_str().into(),
                        ColumnValues::General(vec![None; rows.len()]),
                    );
                }
                let values = decode_column_values_at_ts(column, rows, query_ts);
                (column.name.as_str().into(), values)
            })
        } else {
            names
                .iter()
                .map(|name| {
                    if self.is_column_unavailable(name) {
                        return (name.clone(), ColumnValues::General(vec![None; rows.len()]));
                    }
                    let values = match self.get_column(name) {
                        Some(column) => decode_column_values_at_ts(&column, rows, query_ts),
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
        let columns = self.columns.read();
        for col in columns.iter() {
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

    /// Buffered overwrite entries across all columns awaiting re-encode.
    #[allow(dead_code)]
    pub fn overlay_entry_count(&self) -> usize {
        let columns = self.columns.read();
        let mut total = 0usize;
        for col in columns.iter() {
            total += col.overlay_entry_count();
        }
        total
    }

    /// Garbage-collect version chains across all columns, returning the total
    /// number of before-images removed.
    pub fn gc_versions(&self, min_active_snapshot_ts: Timestamp) -> usize {
        let columns = self.columns.read();
        let mut removed = 0;
        for col in columns.iter() {
            removed += col.gc_versions(min_active_snapshot_ts);
        }
        removed
    }

    /// Exact zone rebuild for columns past the stale-write threshold.
    /// Called from watermark-coordinated maintenance after version chains
    /// fold, so long update histories regain pruning precision.
    pub fn maybe_rebuild_zones_exact(&self) -> usize {
        let columns = self.columns.read();
        let mut rebuilt = 0;
        for col in columns.iter() {
            if col.maybe_rebuild_zone_maps_exact() {
                rebuilt += 1;
            }
        }
        rebuilt
    }

    /// Copy the MVCC row state (creation timestamp + before-image chain) of
    /// every column from another store's row, used by table compaction to
    /// preserve version history when rows move into a rebuilt store.
    pub(crate) fn clone_row_state_from(&self, src: &ColumnStore, from: usize, to: usize) {
        let columns = self.columns.read();
        for dst_col in columns.iter() {
            let name = dst_col.name.clone();
            if let Some(src_col) = src.get_column(&name) {
                dst_col.clone_row_state_from(&src_col, from, to);
            }
        }
    }

    /// Drop a column. Exclusive-only (schema change path): it rewrites
    /// the column vector while no point traffic exists.
    pub fn remove_column(&self, name: &str) -> StorageResult<()> {
        let mut columns = self.columns.write();
        let mut name_to_index = self.name_to_index.write();
        let index = name_to_index
            .get(name)
            .copied()
            .ok_or_else(|| StorageError::column_not_found(name.to_string()))?;

        columns.remove(index);

        name_to_index.clear();
        for (idx, column) in columns.iter_mut().enumerate() {
            column.col_id = idx as i32;
            name_to_index.insert(column.name.clone(), idx);
        }
        self.unavailable.write().remove(name);

        Ok(())
    }

    /// Rename a column. Exclusive-only (schema change path).
    pub fn rename_column(&self, old_name: &str, new_name: String) -> StorageResult<()> {
        let mut columns = self.columns.write();
        let mut name_to_index = self.name_to_index.write();
        if name_to_index.contains_key(&new_name) {
            return Err(StorageError::column_already_exists(new_name));
        }

        let index = name_to_index
            .get(old_name)
            .copied()
            .ok_or_else(|| StorageError::column_not_found(old_name.to_string()))?;

        if let Some(column) = columns.get_mut(index) {
            column.name = new_name.clone();
        }

        name_to_index.clear();
        for (idx, column) in columns.iter().enumerate() {
            name_to_index.insert(column.name.clone(), idx);
        }
        if let Some(reason) = self.unavailable.write().remove(old_name) {
            self.unavailable.write().insert(new_name, reason);
        }

        Ok(())
    }

    pub fn column_count(&self) -> usize {
        self.columns.read().len()
    }

    /// Pre-allocate capacity for `additional` more rows in every column.
    pub fn reserve(&self, additional: usize) {
        let columns = self.columns.read();
        for column in columns.iter() {
            column.reserve(additional);
        }
    }

    pub fn row_count(&self) -> usize {
        self.columns.read().first().map(|c| c.len()).unwrap_or(0)
    }

    pub fn clear(&self) {
        let columns = self.columns.read();
        for col in columns.iter() {
            col.clear();
        }
    }

    pub fn resize(&self, new_count: usize) -> bool {
        let columns = self.columns.read();
        let mut ok = true;
        for col in columns.iter() {
            ok &= col.resize(new_count);
        }
        ok
    }

    /// Column names in store order.
    pub fn column_names(&self) -> Vec<String> {
        self.for_each_column(|col| col.name.clone())
    }

    /// Collect all dirty pages across columns.
    pub fn collect_dirty_pages(&self) -> Vec<crate::persistence::dirty_page::PageId> {
        use std::collections::HashSet;
        let columns = self.columns.read();
        let mut seen = HashSet::new();
        let mut pages = Vec::new();
        for col in columns.iter() {
            for pid in col.dirty_pages() {
                let id = crate::persistence::dirty_page::PageId::new(
                    crate::persistence::dirty_page::ComponentType::VertexColumns,
                    pid as u64,
                );
                if seen.insert(id) {
                    pages.push(id);
                }
            }
        }
        pages
    }

    pub fn clear_dirty(&self) {
        let columns = self.columns.read();
        for col in columns.iter() {
            col.clear_dirty();
        }
    }

    /// Clear the dirty mark only for the given `(column_name, page_id)` pairs,
    /// leaving other dirty pages tracked for a later flush.
    pub fn clear_pages(&self, pages: &[(String, usize)]) {
        for (name, page_id) in pages {
            if let Some(col) = self.get_column(name) {
                col.clear_page_dirty(*page_id);
            }
        }
    }

    pub fn total_dirty_pages(&self) -> usize {
        self.buffer_ledger().dirty_pages
    }

    /// Evict cold column chunks oldest-first until `budget` bytes are
    /// released. Returns `(chunks_evicted, bytes_released)`.
    pub fn evict_cold_chunks(&self, budget: u64) -> (usize, u64) {
        let columns = self.columns.read();
        let mut count = 0usize;
        let mut freed = 0u64;
        for col in columns.iter() {
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
        use super::chunk_residency::{encode_snapshot_sidecar, SnapshotChunkPlan};

        let mut live = std::collections::HashSet::new();
        let columns = self.columns.read();
        for col in columns.iter() {
            let file_name = format!("{}.snapshot", col.name);
            live.insert(file_name.clone());
            let mut plans = Vec::new();
            let mut metas = Vec::new();
            // Own every snapshot up front: plans borrow `metas`, so no
            // `metas` push may happen after the first plan is built.
            let snapshots: Vec<(
                u32,
                u32,
                crate::encoding::EncodingType,
                crate::encoding::ChunkEncodingMeta,
                u64,
                Vec<super::chunk_residency::EvictedPage>,
            )> = {
                let chunks = col.chunks.read();
                let mut out = Vec::new();
                for chunk in chunks.iter() {
                    let snapshot = chunk.read_state().residency.evicted_snapshot().cloned();
                    let Some(snapshot) = snapshot else {
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
                    out.push((
                        row_offset,
                        rows,
                        snapshot.encoding,
                        snapshot.meta.clone(),
                        snapshot.uncompressed_bytes as u64,
                        pages,
                    ));
                }
                out
            };
            for (_, _, _, meta, _, _) in &snapshots {
                metas.push(meta.clone());
            }
            for (i, (row_offset, rows, encoding, _, uncompressed_bytes, pages)) in
                snapshots.iter().enumerate()
            {
                plans.push(SnapshotChunkPlan {
                    row_offset: *row_offset,
                    rows: *rows,
                    encoding: *encoding,
                    meta: &metas[i],
                    uncompressed_bytes: *uncompressed_bytes,
                    pages: pages.clone(),
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

    /// Derived sidecar inventory without mapping payloads: file count and
    /// bytes for one checkpoint directory. Missing files count as zero;
    /// corrupt files still count here and are distinguished at load time.
    /// Per-directory inventory for tooling; manifest-pinned verification
    /// lives in the commit health inspection.
    #[allow(dead_code)]
    pub fn snapshot_sidecar_stats(dir: &std::path::Path) -> (usize, u64) {
        let mut files = 0usize;
        let mut bytes = 0u64;
        let Ok(entries) = std::fs::read_dir(dir) else {
            return (0, 0);
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.ends_with(".snapshot") {
                continue;
            }
            files += 1;
            if let Ok(meta) = entry.metadata() {
                bytes += meta.len();
            }
        }
        (files, bytes)
    }

    /// Re-evict chunks persisted by [`Self::flush_evict_snapshots`] from
    /// memory-mapped sidecars. A missing sidecar is normal and stays silent;
    /// a corrupt sidecar and a window mismatch keep chunks resident but count
    /// separately so operators can tell cache loss from checkpoint drift.
    /// The open never fails over a derived cache.
    pub fn load_evict_snapshots(&self, dir: &std::path::Path) -> SnapshotLoadOutcome {
        use super::chunk_residency::open_snapshot_sidecar;

        let columns = self.columns.read();
        let mut outcome = SnapshotLoadOutcome::default();
        for col in columns.iter() {
            let path = dir.join(format!("{}.snapshot", col.name));
            if !path.exists() {
                continue;
            }
            let mapped = match open_snapshot_sidecar(&path) {
                Ok(mapped) => mapped,
                Err(e) => {
                    log::warn!(
                        "discarding corrupt snapshot sidecar for {} at {}: {}; keeping chunks resident",
                        col.name,
                        path.display(),
                        e
                    );
                    outcome.corrupt += 1;
                    continue;
                }
            };
            for record in mapped.chunks {
                if col.restore_mapped_chunk(record, &mapped.map) {
                    outcome.restored += 1;
                } else {
                    // Window mismatch against the checkpoint pages: the
                    // sidecar names rows the column no longer holds. Keep
                    // resident and count the drop instead of failing open.
                    log::warn!(
                        "discarding mismatched snapshot sidecar window for {} at {}: keeping chunks resident",
                        col.name,
                        path.display(),
                    );
                    outcome.mismatched += 1;
                }
            }
        }
        if outcome.corrupt > 0 || outcome.mismatched > 0 {
            log::warn!(
                "snapshot sidecar discards: restored={} corrupt={} mismatched={} dir={}",
                outcome.restored,
                outcome.corrupt,
                outcome.mismatched,
                dir.display(),
            );
        }
        outcome
    }

    /// Quota-segmented eviction across columns for background tasks.
    /// `task_quota` caps one segment; over-quota work proceeds in segments.
    /// Returns `(chunks_evicted, bytes_released, segments)`.
    pub fn evict_cold_chunks_with_quota(
        &self,
        budget: u64,
        task_quota: u64,
    ) -> (usize, u64, usize) {
        let columns = self.columns.read();
        let mut count = 0usize;
        let mut freed = 0u64;
        let mut segments = 0usize;
        for col in columns.iter() {
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

    /// Unified buffer ledger across columns: per-column ledgers summed under
    /// one unified accounting (resident including overflow, retained snapshots, overflow
    /// subset, dirty pages, chunk counts). Eviction quotas and
    /// observability share this instead of three separate tallies.
    pub fn buffer_ledger(&self) -> super::column::BufferLedger {
        let columns = self.columns.read();
        let mut acc = super::column::BufferLedger::default();
        for col in columns.iter() {
            let ledger = col.buffer_ledger();
            acc.resident_bytes += ledger.resident_bytes;
            acc.evicted_bytes += ledger.evicted_bytes;
            acc.overflow_bytes += ledger.overflow_bytes;
            acc.dirty_pages += ledger.dirty_pages;
            acc.resident_chunks += ledger.resident_chunks;
            acc.evicted_chunks += ledger.evicted_chunks;
        }
        acc
    }

    pub fn mark_row_dirty(&self, row_idx: usize) {
        let columns = self.columns.read();
        for col in columns.iter() {
            if col.len() <= row_idx {
                continue;
            }
            col.mark_dirty(row_idx);
        }
    }

    pub fn backdate_row(&self, row_idx: usize, ts: Timestamp) {
        let columns = self.columns.read();
        for col in columns.iter() {
            col.backdate_created(row_idx, ts);
        }
    }

    pub fn undo_last_versioned_write(
        &self,
        row_idx: usize,
        col_name: &str,
        ts: Timestamp,
    ) -> StorageResult<()> {
        let col = self
            .get_column(col_name)
            .ok_or_else(|| StorageError::column_not_found(col_name.to_string()))?;
        col.undo_last_versioned_write(row_idx, ts)
    }

    pub fn apply_encoding_to_column(
        &self,
        col_name: &str,
        encoding_type: EncodingType,
        fsst_max_symbols: usize,
    ) -> StorageResult<()> {
        let col = self
            .get_column(col_name)
            .ok_or_else(|| StorageError::column_not_found(col_name.to_string()))?;

        col.apply_selected_encoding(encoding_type, fsst_max_symbols)
    }

    pub fn memory_size(&self) -> usize {
        let mut total = std::mem::size_of::<Self>();
        let columns = self.columns.read();

        for col in columns.iter() {
            total += col.memory_size();
        }

        total += self.name_to_index.read().len()
            * (std::mem::size_of::<String>() + std::mem::size_of::<usize>());

        total
    }

    pub fn used_memory_size(&self) -> usize {
        let mut total = std::mem::size_of::<Self>();
        let columns = self.columns.read();

        for col in columns.iter() {
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
