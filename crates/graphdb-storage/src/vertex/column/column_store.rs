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

    /// Fold oldest entries only for the specified columns.
    /// This reduces write amplification when only a subset of columns was updated.
    pub fn fold_oldest_for_row_filtered(
        &mut self,
        row_idx: usize,
        cap: usize,
        horizon: Timestamp,
        names: &[String],
    ) {
        if cap == 0 || names.is_empty() {
            return;
        }
        for name in names {
            if let Some(col) = self.get_column_mut(name) {
                if col.version_chains_opt().is_none() {
                    continue;
                }
                col.fold_oldest(row_idx, cap, horizon);
            }
        }
    }

    /// Copy the MVCC row state (current version timestamp + version chain)
    /// from `from` to `to`, used by table compaction to preserve version
    /// history when rows are remapped.
    pub(crate) fn copy_row_state(&mut self, from: usize, to: usize) {
        for col in &mut self.columns {
            col.copy_row_state(from, to);
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

    pub fn total_dirty_pages(&self) -> usize {
        self.columns.iter().map(|c| c.dirty_count()).sum()
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

        if col.is_empty() {
            return Ok(());
        }

        match encoding_type {
            EncodingType::Fsst => {
                if col.data_type != DataType::String && col.data_type != DataType::Json {
                    return Err(StorageError::not_supported(format!(
                        "FSST encoding does not support type {:?}",
                        col.data_type
                    )));
                }
                col.apply_fsst_encoding(fsst_max_symbols)?;
            }
            EncodingType::Dictionary => {
                col.apply_dictionary_encoding()?;
            }
            EncodingType::Rle => {
                col.apply_rle_encoding()?;
            }
            EncodingType::BitPacking => {
                col.apply_bitpacking_encoding()?;
            }
            EncodingType::Alp => {
                col.apply_alp_encoding()?;
            }
            EncodingType::Constant => {
                col.apply_constant_encoding()?;
            }
            EncodingType::None => {}
        }

        Ok(())
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
