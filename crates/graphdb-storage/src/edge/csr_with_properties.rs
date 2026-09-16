//! CSR with Properties — ladybug-style columnar storage.
//!
//! Properties are stored in parallel column arrays keyed by `EdgeId` through
//! the edge-to-row map. The topology CSR owns the only adjacency index; this
//! store keeps no per-vertex offsets, lengths, or append heads.
//!
//! This implementation uses `Column` (continuous arrays) instead of
//! `HashMap<u32, Value>` for cache-friendly scans and lower memory overhead.
//!
//! Visibility authority lives in `MVCCManager` (`edge_timestamps`): these CSR
//! row stamps are only a physical projection kept in sync on the write path
//! for garbage collection. They must never decide query visibility alone.

use std::collections::{HashMap, HashSet};

use graphdb_core::types::{EdgeId, Timestamp};
use graphdb_core::{DataType, StorageError, StorageResult, Value};

use crate::edge::property_schema::PropertySchema;
use crate::vertex::column::Column;

/// Row visibility for MVCC.
#[derive(Debug, Clone, Copy)]
struct RowVisibility {
    create_ts: Timestamp,
    delete_ts: Option<Timestamp>,
}

impl RowVisibility {
    fn new(create_ts: Timestamp) -> Self {
        Self {
            create_ts,
            delete_ts: None,
        }
    }

    fn is_visible_at(&self, query_ts: Timestamp) -> bool {
        if query_ts < self.create_ts {
            return false;
        }
        if let Some(del) = self.delete_ts {
            if query_ts >= del {
                return false;
            }
        }
        true
    }

    fn mark_deleted(&mut self, ts: Timestamp) {
        if self.delete_ts.is_none() {
            self.delete_ts = Some(ts);
        }
    }
}

/// Columnar property storage keyed by edge id.
///
/// Every edge owns exactly one row, including edges without properties.
/// Row identity is the edge-to-row map; there is no per-vertex addressing.
///
/// Memory-only version and encoding semantics: property before-images and
/// column encodings accelerate live reads but are not persisted.
/// `dump` serializes current values plus row visibility only; a reload
/// collapses history to the latest value and restores plain columns.
/// Attribute time travel is therefore valid within a checkpoint epoch and
/// must be re-encoded after a reload when needed.
#[derive(Debug, Clone)]
pub struct CsrWithProperties {
    property_schema: Vec<PropertySchema>,
    property_columns: Vec<Column>,
    visibility: Vec<RowVisibility>,
    edge_to_row: HashMap<EdgeId, u32>,
    /// Reverse index for O(1) row-to-edge lookup. Authoritative with
    /// `edge_to_row`; rebuilt on load, never persisted separately.
    row_to_edge: Vec<Option<EdgeId>>,
    free_list: Vec<u32>,
    /// O(1) membership for free slots; rebuilt from `free_list` on load.
    free_set: HashSet<u32>,
    row_count: usize,
    /// Columns mutated since the last stats refresh or checkpoint.
    /// Drives per-column stats refresh so clean columns never pay recompute.
    dirty_columns: HashSet<String>,
}

impl CsrWithProperties {
    pub fn new(property_schema: Vec<PropertySchema>) -> Self {
        let mut property_columns = Vec::with_capacity(property_schema.len());
        for schema in &property_schema {
            let col = Column::new(
                schema.name.clone(),
                schema.prop_id,
                schema.data_type.clone(),
                schema.nullable,
            );
            property_columns.push(col);
        }
        Self {
            property_schema,
            property_columns,
            visibility: Vec::new(),
            edge_to_row: HashMap::new(),
            row_to_edge: Vec::new(),
            free_list: Vec::new(),
            free_set: HashSet::new(),
            row_count: 0,
            dirty_columns: HashSet::new(),
        }
    }

    fn ensure_row_aux_len(&mut self, len: usize) {
        if self.row_to_edge.len() < len {
            self.row_to_edge.resize(len, None);
        }
    }

    fn mark_column_dirty(&mut self, name: &str) {
        self.dirty_columns.insert(name.to_string());
    }

    fn mark_all_columns_dirty(&mut self) {
        for schema in &self.property_schema {
            self.dirty_columns.insert(schema.name.clone());
        }
    }

    /// Clear per-column dirt after a successful checkpoint.
    pub fn clear_dirty_columns(&mut self) {
        self.dirty_columns.clear();
    }

    /// Whether any property column changed since the last checkpoint.
    pub fn has_dirty_columns(&self) -> bool {
        !self.dirty_columns.is_empty()
    }

    pub fn property_schema(&self) -> &[PropertySchema] {
        &self.property_schema
    }

    pub fn row_count(&self) -> usize {
        self.row_count
    }

    /// Allocate a new row and populate it with the given values.
    /// Returns the row index (0-based).
    fn allocate_row(
        &mut self,
        values: &[(String, Value)],
        create_ts: Timestamp,
    ) -> StorageResult<usize> {
        let row_idx = if let Some(free_off) = self.free_list.pop() {
            let idx = free_off as usize;
            self.free_set.remove(&free_off);
            if idx >= self.visibility.len() {
                self.visibility.resize(idx + 1, RowVisibility::new(0));
            }
            self.visibility[idx] = RowVisibility::new(create_ts);
            self.row_count += 1;
            idx
        } else {
            let idx = self.visibility.len();
            self.visibility.push(RowVisibility::new(create_ts));
            self.row_count += 1;
            idx
        };
        self.ensure_row_aux_len(self.visibility.len());
        self.row_to_edge[row_idx] = None;
        let names: Vec<String> = self
            .property_schema
            .iter()
            .map(|s| s.name.clone())
            .collect();
        for (i, schema) in self.property_schema.iter().enumerate() {
            let col = &mut self.property_columns[i];
            // Extend column data buffer for the new row without generating
            // a spurious [0, create_ts) version chain entry. We do this by
            // writing directly to the column's internal buffer and setting
            // the correct visibility timestamp.
            let value_opt = values
                .iter()
                .find(|(k, _)| k == &schema.name)
                .map(|(_, v)| v);
            match value_opt {
                Some(v) => {
                    // Value provided: versioned write with the given value
                    col.set_versioned(row_idx, Some(v), create_ts)?;
                }
                None => {
                    // No value provided: use default value if available, otherwise None
                    let default_val = schema.default_value.as_ref();
                    col.set_with_timestamp(row_idx, default_val, create_ts)?;
                }
            }
        }
        for name in names {
            self.mark_column_dirty(&name);
        }
        Ok(row_idx)
    }

    /// Release a row back to the free list without leaving an orphan.
    ///
    /// Clears the visibility stamp so the slot is skipped by reads and GC
    /// scans, drops the edge mapping via the reverse index, and queues the
    /// slot for reuse. Reused slots are fully overwritten by `allocate_row`.
    /// Idempotent: releasing an already-free or out-of-range row is a no-op.
    pub fn release_row(&mut self, row_idx: usize) {
        if row_idx < self.visibility.len()
            && (self.visibility[row_idx].create_ts != 0
                || self.visibility[row_idx].delete_ts.is_some())
        {
            self.visibility[row_idx].create_ts = 0;
            self.visibility[row_idx].delete_ts = None;
            self.row_count = self.row_count.saturating_sub(1);
        }
        if let Some(slot) = self.row_to_edge.get_mut(row_idx) {
            if let Some(edge_id) = slot.take() {
                self.edge_to_row.remove(&edge_id);
            }
        } else {
            self.edge_to_row.retain(|_, pos| *pos as usize != row_idx);
        }
        let slot = row_idx as u32;
        if self.free_set.insert(slot) {
            self.free_list.push(slot);
        }
    }

    /// Read the property row for `edge_id` at `query_ts`, decoding only the
    /// projected columns.
    ///
    /// `projection` selects which columns to decode: `None` decodes every
    /// column, `Some(&[])` decodes none (topology-only read). Unknown names
    /// are skipped. Visibility is still enforced: an invisible edge yields
    /// `None`, a visible one yields `Some` (possibly empty).
    pub fn get_projected_by_edge_id(
        &self,
        edge_id: EdgeId,
        query_ts: Timestamp,
        projection: Option<&[String]>,
    ) -> Option<Vec<(String, Option<Value>)>> {
        let pos = *self.edge_to_row.get(&edge_id)? as usize;
        let vis = self.visibility.get(pos)?;
        if !vis.is_visible_at(query_ts) {
            return None;
        }
        match projection {
            None => Some(
                self.property_schema
                    .iter()
                    .enumerate()
                    .map(|(i, s)| {
                        let v = self.property_columns[i].get_at_ts(pos, query_ts);
                        (s.name.clone(), v)
                    })
                    .collect(),
            ),
            Some(names) => {
                if names.is_empty() {
                    return Some(Vec::new());
                }
                Some(
                    self.property_schema
                        .iter()
                        .enumerate()
                        .filter(|(_, s)| names.iter().any(|n| n == &s.name))
                        .map(|(i, s)| {
                            let v = self.property_columns[i].get_at_ts(pos, query_ts);
                            (s.name.clone(), v)
                        })
                        .collect(),
                )
            }
        }
    }

    /// Insert properties for an edge and associate the row with `edge_id`.
    pub fn get_by_edge_id(
        &self,
        edge_id: EdgeId,
        query_ts: Timestamp,
    ) -> Option<Vec<(String, Option<Value>)>> {
        self.get_projected_by_edge_id(edge_id, query_ts, None)
    }

    /// Read non-nullable properties for an edge by its EdgeId (no MVCC filtering).
    pub fn read_properties_by_edge_id(&self, edge_id: EdgeId) -> Option<Vec<(String, Value)>> {
        let pos = *self.edge_to_row.get(&edge_id)? as usize;
        let result: Vec<(String, Value)> = self
            .property_schema
            .iter()
            .enumerate()
            .filter_map(|(i, s)| {
                let v = self.property_columns[i].get(pos)?;
                Some((s.name.clone(), v))
            })
            .collect();
        if result.is_empty() {
            None
        } else {
            Some(result)
        }
    }

    pub fn mark_deleted(&mut self, edge_id: EdgeId, ts: Timestamp) -> bool {
        if let Some(&pos) = self.edge_to_row.get(&edge_id) {
            if let Some(vis) = self.visibility.get_mut(pos as usize) {
                if vis.delete_ts.is_some() {
                    return false;
                }
                vis.mark_deleted(ts);
                return true;
            }
        }
        false
    }

    /// Insert properties for an edge and associate the row with `edge_id`.
    pub fn insert_for_edge(
        &mut self,
        edge_id: EdgeId,
        values: &[(String, Value)],
        create_ts: Timestamp,
    ) -> StorageResult<()> {
        let row_idx = self.allocate_row(values, create_ts)?;
        self.edge_to_row.insert(edge_id, row_idx as u32);
        self.ensure_row_aux_len(row_idx + 1);
        self.row_to_edge[row_idx] = Some(edge_id);
        Ok(())
    }

    /// Associate an existing row index with an edge id.
    pub fn associate_edge(&mut self, edge_id: EdgeId, row_idx: usize) {
        self.edge_to_row.insert(edge_id, row_idx as u32);
        self.ensure_row_aux_len(row_idx + 1);
        self.row_to_edge[row_idx] = Some(edge_id);
    }

    /// Get the row index for an edge.
    pub fn get_row_for_edge(&self, edge_id: EdgeId) -> Option<usize> {
        self.edge_to_row.get(&edge_id).map(|&pos| pos as usize)
    }

    /// Remove edge-to-row mapping and return the row index.
    pub fn remove_edge_mapping(&mut self, edge_id: EdgeId) -> Option<usize> {
        let pos = self.edge_to_row.remove(&edge_id).map(|pos| pos as usize)?;
        if let Some(slot) = self.row_to_edge.get_mut(pos) {
            if *slot == Some(edge_id) {
                *slot = None;
            }
        }
        Some(pos)
    }

    /// Edge-aware property update: lookup row via `edge_id`.
    pub fn set_property_for_edge(
        &mut self,
        edge_id: EdgeId,
        name: &str,
        value: Option<Value>,
        ts: Timestamp,
    ) -> StorageResult<()> {
        let pos = *self
            .edge_to_row
            .get(&edge_id)
            .ok_or_else(|| StorageError::invalid_offset(0))?;
        self.set_property_at_row(pos as usize, name, value, ts)
    }

    /// Edge-aware bulk property update: lookup row via `edge_id` and update all properties.
    pub fn update_properties_for_edge(
        &mut self,
        edge_id: EdgeId,
        properties: &[(String, Value)],
        ts: Timestamp,
    ) -> StorageResult<()> {
        let pos = *self
            .edge_to_row
            .get(&edge_id)
            .ok_or_else(|| StorageError::invalid_offset(0))?;
        self.update_at_row(pos as usize, properties, ts)
    }

    pub fn set_property_by_id_for_edge(
        &mut self,
        edge_id: EdgeId,
        prop_id: crate::types::PropertyId,
        value: Option<Value>,
        ts: Timestamp,
    ) -> StorageResult<()> {
        let pos = *self
            .edge_to_row
            .get(&edge_id)
            .ok_or_else(|| StorageError::invalid_offset(0))?;
        let idx = prop_id.as_usize();
        if idx >= self.property_schema.len() {
            return Err(StorageError::column_not_found(format!(
                "prop_id={}",
                prop_id.0
            )));
        }
        let name = self.property_schema[idx].name.clone();
        self.set_property_at_row(pos as usize, &name, value, ts)
    }

    pub fn revert_deletion_for_edge(&mut self, edge_id: EdgeId) -> bool {
        if let Some(&pos) = self.edge_to_row.get(&edge_id) {
            return self.revert_deletion_at_row(pos as usize);
        }
        false
    }

    /// Iterate over all edge->row mappings (for compaction).
    pub fn edge_mappings(&self) -> impl Iterator<Item = (&EdgeId, &u32)> {
        self.edge_to_row.iter()
    }

    pub fn edge_ids(&self) -> impl Iterator<Item = EdgeId> + '_ {
        self.edge_to_row.keys().copied()
    }

    pub fn mark_deleted_at_row(&mut self, row_idx: usize, ts: Timestamp) -> StorageResult<()> {
        if row_idx >= self.visibility.len() {
            return Ok(());
        }
        if self.visibility[row_idx].delete_ts.is_some() {
            return Err(StorageError::invalid_operation(
                "record already marked deleted",
            ));
        }
        self.visibility[row_idx].mark_deleted(ts);
        Ok(())
    }

    pub fn is_deleted_at_row(&self, row_idx: usize) -> bool {
        if let Some(vis) = self.visibility.get(row_idx) {
            return vis.delete_ts.is_some();
        }
        false
    }

    pub fn revert_deletion_at_row(&mut self, row_idx: usize) -> bool {
        if let Some(vis) = self.visibility.get_mut(row_idx) {
            if vis.delete_ts.is_some() {
                vis.delete_ts = None;
                return true;
            }
        }
        false
    }

    pub fn has_property(&self, name: &str) -> bool {
        self.property_schema.iter().any(|s| s.name == name)
    }

    pub fn get_property_id(&self, name: &str) -> Option<crate::types::PropertyId> {
        self.property_schema
            .iter()
            .position(|s| s.name == name)
            .map(|i| crate::types::PropertyId::new(i as u16))
    }

    pub fn add_property(
        &mut self,
        name: String,
        data_type: DataType,
        nullable: bool,
    ) -> StorageResult<crate::types::PropertyId> {
        if self.has_property(&name) {
            return Err(StorageError::column_already_exists(name));
        }
        let prop_id = self.property_schema.len() as i32;
        let schema =
            PropertySchema::new(name.clone(), prop_id, data_type.clone()).nullable(nullable);
        self.property_schema.push(schema);
        let mut col = Column::new(name.clone(), prop_id, data_type, nullable);
        let rows = self.visibility.len();
        if rows > 0 {
            col.resize(rows);
        }
        self.property_columns.push(col);
        self.mark_column_dirty(&name);
        Ok(crate::types::PropertyId::new(prop_id as u16))
    }

    pub fn remove_property(&mut self, name: &str) -> StorageResult<()> {
        let idx = self
            .property_schema
            .iter()
            .position(|p| p.name == name)
            .ok_or_else(|| StorageError::column_not_found(name.to_string()))?;
        self.property_schema.remove(idx);
        self.property_columns.remove(idx);
        for (i, s) in self.property_schema.iter_mut().enumerate() {
            s.prop_id = i as i32;
            if let Some(col) = self.property_columns.get_mut(i) {
                col.col_id = i as i32;
            }
        }
        self.dirty_columns.remove(name);
        Ok(())
    }

    pub fn rename_property(&mut self, old_name: &str, new_name: &str) -> StorageResult<()> {
        if self.has_property(new_name) {
            return Err(StorageError::column_already_exists(new_name.to_string()));
        }
        let idx = self
            .property_schema
            .iter()
            .position(|p| p.name == old_name)
            .ok_or_else(|| StorageError::column_not_found(old_name.to_string()))?;
        self.property_schema[idx].name = new_name.to_string();
        if let Some(col) = self.property_columns.get_mut(idx) {
            col.name = new_name.to_string();
        }
        if self.dirty_columns.remove(old_name) {
            self.dirty_columns.insert(new_name.to_string());
        }
        Ok(())
    }

    pub fn set_property_at_row(
        &mut self,
        row_idx: usize,
        name: &str,
        value: Option<Value>,
        ts: Timestamp,
    ) -> StorageResult<()> {
        if row_idx >= self.visibility.len() || self.visibility[row_idx].create_ts == 0 {
            return Err(StorageError::invalid_offset(row_idx as u32));
        }
        let col_idx = self
            .property_schema
            .iter()
            .position(|s| s.name == name)
            .ok_or_else(|| StorageError::column_not_found(name.to_string()))?;
        let col = &mut self.property_columns[col_idx];
        col.set_versioned(row_idx, value.as_ref(), ts)?;
        self.mark_column_dirty(name);
        Ok(())
    }

    /// Bulk update properties at a given row index.
    pub fn update_at_row(
        &mut self,
        row_idx: usize,
        properties: &[(String, Value)],
        ts: Timestamp,
    ) -> StorageResult<()> {
        if row_idx >= self.visibility.len() || self.visibility[row_idx].create_ts == 0 {
            return Err(StorageError::invalid_offset(row_idx as u32));
        }
        for (name, value) in properties {
            let col_idx = self
                .property_schema
                .iter()
                .position(|s| s.name == *name)
                .ok_or_else(|| StorageError::column_not_found(name.to_string()))?;
            let col = &mut self.property_columns[col_idx];
            col.set_versioned(row_idx, Some(value), ts)?;
            self.mark_column_dirty(name);
        }
        Ok(())
    }

    pub fn compaction_stats(&self) -> crate::edge::property_schema::PropertyCompactionStats {
        let tombstone_count = self
            .visibility
            .iter()
            .filter(|v| v.delete_ts.is_some())
            .count();
        let live_records = self
            .visibility
            .iter()
            .filter(|v| v.create_ts != 0 && v.delete_ts.is_none())
            .count();
        let mut reclaimable_bytes = 0usize;
        for v in &self.visibility {
            if v.delete_ts.is_some() {
                reclaimable_bytes += 32 * self.property_schema.len();
            }
        }
        crate::edge::property_schema::PropertyCompactionStats {
            tombstone_count,
            total_records: self.visibility.len(),
            live_records,
            free_list_size: self.free_list.len(),
            reclaimable_bytes,
        }
    }

    /// Garbage-collect property version-chain entries that no active snapshot
    /// can observe. Returns the total number of before-images removed.
    pub fn gc_property_versions(&mut self, min_active_snapshot_ts: Timestamp) -> usize {
        self.property_columns
            .iter_mut()
            .map(|col| col.gc_versions(min_active_snapshot_ts))
            .sum()
    }

    /// Aggregate version-chain statistics across all property columns.
    pub fn property_version_stats(&self) -> crate::vertex::column::mvcc::VersionChainStats {
        let mut total_rows = 0usize;
        let mut total_entries = 0usize;
        let mut max_len = 0usize;
        let mut memory_bytes = 0usize;
        for col in &self.property_columns {
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
        crate::vertex::column::mvcc::VersionChainStats {
            total_rows,
            total_entries,
            max_len,
            avg_len,
            memory_bytes,
        }
    }

    pub fn is_schema_fixed_size(&self) -> bool {
        self.property_schema.iter().all(|s| {
            matches!(
                s.data_type,
                DataType::Bool
                    | DataType::SmallInt
                    | DataType::Int
                    | DataType::BigInt
                    | DataType::Float
                    | DataType::Double
            )
        })
    }

    pub fn used_memory_size(&self) -> usize {
        let mut total = std::mem::size_of::<Self>();
        total += self.visibility.capacity() * std::mem::size_of::<RowVisibility>();
        total +=
            self.edge_to_row.len() * (std::mem::size_of::<EdgeId>() + std::mem::size_of::<u32>());
        total += self.free_list.capacity() * std::mem::size_of::<u32>();
        total += self.row_to_edge.capacity() * std::mem::size_of::<Option<EdgeId>>();
        total += self.free_set.len() * std::mem::size_of::<u32>();
        for col in &self.property_columns {
            total += col.memory_size();
        }
        total += self.property_schema.len() * std::mem::size_of::<PropertySchema>();
        total
    }

    pub fn dump(&self) -> Vec<u8> {
        // Current-value snapshot only: version chains and column encodings
        // stay memory-only by design. Load restores plain columns holding
        // the latest value with the row creation stamp.
        let mut buf = Vec::new();
        buf.push(2u8); // version
        buf.extend_from_slice(&(self.visibility.len() as u32).to_le_bytes());
        for vis in &self.visibility {
            buf.extend_from_slice(&vis.create_ts.to_le_bytes());
            if let Some(del) = vis.delete_ts {
                buf.push(1);
                buf.extend_from_slice(&del.to_le_bytes());
            } else {
                buf.push(0);
            }
        }
        buf.extend_from_slice(&(self.row_count as u32).to_le_bytes());
        buf.extend_from_slice(&(self.edge_to_row.len() as u32).to_le_bytes());
        for (eid, pos) in &self.edge_to_row {
            buf.extend_from_slice(&eid.0.to_le_bytes());
            buf.extend_from_slice(&pos.to_le_bytes());
        }
        buf.extend_from_slice(&(self.free_list.len() as u32).to_le_bytes());
        for &off in &self.free_list {
            buf.extend_from_slice(&off.to_le_bytes());
        }
        // Serialize current column values (without version history).
        buf.extend_from_slice(&(self.property_columns.len() as u32).to_le_bytes());
        for col in &self.property_columns {
            // Column name keys the payload to the schema entry on load.
            buf.extend_from_slice(&(col.name.len() as u32).to_le_bytes());
            buf.extend_from_slice(col.name.as_bytes());
            let rows = self.visibility.len();
            buf.extend_from_slice(&(rows as u32).to_le_bytes());
            for row_idx in 0..rows {
                let val = col.get(row_idx);
                if let Some(v) = val {
                    buf.push(1);
                    if let Ok(bytes) = postcard::to_allocvec(&v) {
                        buf.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
                        buf.extend_from_slice(&bytes);
                    } else {
                        buf.extend_from_slice(&0u32.to_le_bytes());
                    }
                } else {
                    buf.push(0);
                }
            }
        }
        buf
    }

    pub fn load(&mut self, data: &[u8]) -> StorageResult<()> {
        if data.is_empty() {
            return Ok(());
        }
        let mut offset = 0usize;
        if offset >= data.len() {
            return Ok(());
        }
        let version = data[offset];
        offset += 1;
        if version != 2 {
            return Err(StorageError::deserialize_error(format!(
                "Unsupported CsrWithProperties version: {}, only version 2 is accepted",
                version
            )));
        }
        if offset + 4 > data.len() {
            return Ok(());
        }
        let vis_len = u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap()) as usize;
        offset += 4;
        self.visibility.clear();
        self.visibility.reserve(vis_len);
        for _ in 0..vis_len {
            if offset + 8 > data.len() {
                break;
            }
            let create = u64::from_le_bytes(data[offset..offset + 8].try_into().unwrap());
            offset += 8;
            if offset >= data.len() {
                break;
            }
            let has_del = data[offset];
            offset += 1;
            let del = if has_del == 1 {
                if offset + 8 > data.len() {
                    None
                } else {
                    let d = u64::from_le_bytes(data[offset..offset + 8].try_into().unwrap());
                    offset += 8;
                    Some(d)
                }
            } else {
                None
            };
            self.visibility.push(RowVisibility {
                create_ts: create,
                delete_ts: del,
            });
        }
        if offset + 4 <= data.len() {
            self.row_count =
                u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap()) as usize;
            offset += 4;
        }
        if offset + 4 <= data.len() {
            let map_len = u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap()) as usize;
            offset += 4;
            self.edge_to_row.clear();
            for _ in 0..map_len {
                if offset + 12 > data.len() {
                    break;
                }
                let eid = u64::from_le_bytes(data[offset..offset + 8].try_into().unwrap());
                offset += 8;
                let pos = u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap());
                offset += 4;
                self.edge_to_row.insert(EdgeId(eid), pos);
            }
        }
        if offset + 4 <= data.len() {
            let free_len =
                u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap()) as usize;
            offset += 4;
            self.free_list.clear();
            for _ in 0..free_len {
                if offset + 4 > data.len() {
                    break;
                }
                let off = u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap());
                offset += 4;
                self.free_list.push(off);
            }
        }
        if offset + 4 <= data.len() {
            let col_count =
                u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap()) as usize;
            offset += 4;
            for _ in 0..col_count {
                if offset + 4 > data.len() {
                    break;
                }
                let name_len =
                    u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap()) as usize;
                offset += 4;
                if offset + name_len > data.len() {
                    break;
                }
                let name = String::from_utf8_lossy(&data[offset..offset + name_len]).to_string();
                offset += name_len;
                if offset + 4 > data.len() {
                    break;
                }
                let rows =
                    u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap()) as usize;
                offset += 4;
                if let Some(col_idx) = self.property_schema.iter().position(|s| s.name == name) {
                    let col = &mut self.property_columns[col_idx];
                    if col.len() < rows {
                        col.resize(rows);
                    }
                    for row_idx in 0..rows {
                        if offset >= data.len() {
                            break;
                        }
                        let has = data[offset];
                        offset += 1;
                        if has == 1 {
                            if offset + 4 > data.len() {
                                break;
                            }
                            let vlen =
                                u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap())
                                    as usize;
                            offset += 4;
                            if offset + vlen > data.len() {
                                break;
                            }
                            let vbytes = &data[offset..offset + vlen];
                            offset += vlen;
                            if let Ok(val) = postcard::from_bytes::<Value>(vbytes) {
                                let _ = col.set(row_idx, Some(&val));
                            }
                        } else {
                            let _ = col.set(row_idx, None);
                        }
                    }
                } else {
                    // skip unknown column values
                    for _ in 0..rows {
                        if offset >= data.len() {
                            break;
                        }
                        let has = data[offset];
                        offset += 1;
                        if has == 1 {
                            if offset + 4 > data.len() {
                                break;
                            }
                            let vlen =
                                u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap())
                                    as usize;
                            offset += 4;
                            if offset + vlen <= data.len() {
                                offset += vlen;
                            } else {
                                break;
                            }
                        }
                    }
                }
            }
        }
        if offset != data.len() {
            return Err(StorageError::deserialize_error(
                "unexpected trailing data in properties payload".to_string(),
            ));
        }
        self.rebuild_aux_indexes();
        self.dirty_columns.clear();
        Ok(())
    }

    fn rebuild_aux_indexes(&mut self) {
        self.row_to_edge.clear();
        self.row_to_edge.resize(self.visibility.len(), None);
        for (edge_id, pos) in &self.edge_to_row {
            let idx = *pos as usize;
            if idx < self.row_to_edge.len() {
                self.row_to_edge[idx] = Some(*edge_id);
            }
        }
        self.free_set.clear();
        for &slot in &self.free_list {
            self.free_set.insert(slot);
        }
    }

    pub fn reclaim_slots(
        &mut self,
        valid_edge_ids: &HashSet<EdgeId>,
        retention_bound: Timestamp,
    ) -> usize {
        if retention_bound == Timestamp::MAX {
            return 0;
        }
        let mut to_reclaim = Vec::new();
        for (idx, vis) in self.visibility.iter().enumerate() {
            if vis.create_ts == 0 {
                continue;
            }
            // O(1) ownership check via the reverse index instead of scanning
            // the full edge map for every row.
            let has_live_edge = self
                .row_to_edge
                .get(idx)
                .and_then(|slot| *slot)
                .is_some_and(|eid| valid_edge_ids.contains(&eid));
            if has_live_edge {
                continue;
            }
            if let Some(del_ts) = vis.delete_ts {
                // Exclusive waterfront: deletable exactly when invisible to
                // every snapshot at or past the cutoff.
                if crate::mvcc_visibility::Visibility::is_gc_eligible(del_ts, retention_bound) {
                    to_reclaim.push(idx);
                }
            }
        }
        if to_reclaim.is_empty() {
            return 0;
        }
        let reclaim_set: HashSet<u32> = to_reclaim.iter().map(|&i| i as u32).collect();
        for &idx in &to_reclaim {
            self.visibility[idx].create_ts = 0;
            self.visibility[idx].delete_ts = None;
            if let Some(slot) = self.row_to_edge.get_mut(idx) {
                slot.take();
            }
            if self.free_set.insert(idx as u32) {
                self.free_list.push(idx as u32);
            }
            self.row_count = self.row_count.saturating_sub(1);
        }
        self.edge_to_row.retain(|_, pos| !reclaim_set.contains(pos));
        to_reclaim.len()
    }

    pub fn column_stats_snapshot(
        &self,
        column: &str,
    ) -> Option<crate::stats_reader::ColumnStatsSnapshot> {
        let col = self.property_columns.iter().find(|c| c.name == column)?;
        let mut min: Option<Value> = None;
        let mut max: Option<Value> = None;
        for zone in col.zone_maps() {
            if let Some(v) = &zone.min {
                match &min {
                    Some(cur)
                        if crate::vertex::column::compare_values(cur, v)
                            != std::cmp::Ordering::Greater => {}
                    _ => min = Some(v.clone()),
                }
            }
            if let Some(v) = &zone.max {
                match &max {
                    Some(cur)
                        if crate::vertex::column::compare_values(cur, v)
                            != std::cmp::Ordering::Less => {}
                    _ => max = Some(v.clone()),
                }
            }
        }
        let persisted = col.stats();
        let null_count = persisted.map(|s| s.null_count);
        let (distinct_count, hll) = match persisted.and_then(|s| s.hll.clone()) {
            Some(h) => {
                let est = h.estimate();
                (Some(est), persisted.and_then(|s| s.hll.clone()))
            }
            None => (None, None),
        };
        Some(crate::stats_reader::ColumnStatsSnapshot {
            row_count: self.row_count as u64,
            null_count,
            distinct_count,
            hll,
            min_value: min,
            max_value: max,
        })
    }

    /// Encoding applied to one property column, if the column exists.
    pub fn column_encoding_type(&self, column: &str) -> Option<crate::encoding::EncodingType> {
        self.property_columns
            .iter()
            .find(|c| c.name == column)
            .map(|c| c.encoding_type())
    }

    /// Apply one encoding to a single property column.
    ///
    /// Chunked columns take the chunk-local path so point updates keep
    /// decoding only the affected chunk; plain columns dispatch by type.
    /// Empty columns are a no-op. Unknown columns are an explicit error.
    pub fn apply_encoding_to_column(
        &mut self,
        column: &str,
        encoding_type: crate::encoding::EncodingType,
        fsst_max_symbols: usize,
    ) -> StorageResult<()> {
        let col = self
            .property_columns
            .iter_mut()
            .find(|c| c.name == column)
            .ok_or_else(|| StorageError::column_not_found(column.to_string()))?;
        if col.is_empty() {
            return Ok(());
        }
        if col.has_chunks() {
            return col.apply_encoding_to_chunks(encoding_type, fsst_max_symbols);
        }
        match encoding_type {
            crate::encoding::EncodingType::Fsst => {
                col.apply_fsst_encoding(fsst_max_symbols)?;
            }
            crate::encoding::EncodingType::Dictionary => {
                col.apply_dictionary_encoding()?;
            }
            crate::encoding::EncodingType::Rle => {
                col.apply_rle_encoding()?;
            }
            crate::encoding::EncodingType::BitPacking => {
                col.apply_bitpacking_encoding()?;
            }
            crate::encoding::EncodingType::Alp => {
                col.apply_alp_encoding()?;
            }
            crate::encoding::EncodingType::Constant => {
                col.apply_constant_encoding()?;
            }
            crate::encoding::EncodingType::None => {}
        }
        Ok(())
    }

    /// Select and apply one encoding per property column from current values.
    ///
    /// Explicit maintenance operation: hot columns stay unencoded between runs
    /// by design so everyday writes never pay re-encoding. Returns the number
    /// of columns that received an encoding.
    pub fn auto_encode_properties(&mut self) -> usize {
        let selector = crate::encoding::EncodingSelector::default();
        let mut encoded = 0usize;
        for idx in 0..self.property_columns.len() {
            let data_type = self.property_columns[idx].data_type.clone();
            let values: Vec<Option<Value>> = (0..self.property_columns[idx].len())
                .map(|row| self.property_columns[idx].get(row))
                .collect();
            if values.is_empty() {
                continue;
            }
            let selected = selector.select_for_column(&data_type, &values);
            if selected == crate::encoding::EncodingType::None {
                continue;
            }
            let name = self.property_columns[idx].name.clone();
            if self
                .apply_encoding_to_column(name.as_str(), selected, 255)
                .is_ok()
            {
                encoded += 1;
            }
        }
        encoded
    }

    /// Recompute persisted per-column statistics from flush buffers.
    ///
    /// Called before the property file is serialized so statistics follow the
    /// checkpoint instead of drifting. Only columns marked dirty are
    /// recomputed; clean columns keep their persisted statistics. Columns
    /// that fail to compute keep their previous statistics.
    pub fn refresh_column_stats(&mut self) {
        if self.dirty_columns.is_empty() {
            for col in &mut self.property_columns {
                if let Ok(stats) = col.compute_stats() {
                    col.set_stats(stats);
                }
            }
            return;
        }
        for col in &mut self.property_columns {
            if !self.dirty_columns.contains(&col.name) {
                continue;
            }
            if let Ok(stats) = col.compute_stats() {
                col.set_stats(stats);
            }
        }
    }

    /// Refresh statistics for one column only. Used by column-level
    /// checkpoint follow-up when only a subset changed.
    pub fn refresh_column_stats_for(&mut self, column: &str) {
        if let Some(col) = self.property_columns.iter_mut().find(|c| c.name == column) {
            if let Ok(stats) = col.compute_stats() {
                col.set_stats(stats);
            }
        }
    }

    /// Backfill one column with a default value on every existing row.
    ///
    /// Used by the staged add-column fill step. Unknown columns are an
    /// explicit error; a single row failure aborts with the error.
    pub fn backfill_column(&mut self, column: &str, default: &Value) -> StorageResult<()> {
        let rows = self.visibility.len();
        let col = self
            .property_columns
            .iter_mut()
            .find(|c| c.name == column)
            .ok_or_else(|| StorageError::column_not_found(column.to_string()))?;
        for row in 0..rows {
            col.set(row, Some(default))?;
        }
        self.mark_column_dirty(column);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encoding::EncodingType;
    use graphdb_core::DataType;

    fn schema() -> Vec<PropertySchema> {
        vec![
            PropertySchema::new("weight".to_string(), 0, DataType::Double),
            PropertySchema::new("label".to_string(), 1, DataType::String).nullable(true),
        ]
    }

    #[test]
    fn edge_id_access() {
        let mut csr = CsrWithProperties::new(schema());
        let eid0 = EdgeId(1);
        let eid1 = EdgeId(2);
        csr.insert_for_edge(eid0, &[("weight".to_string(), Value::Double(1.5))], 10)
            .unwrap();
        csr.insert_for_edge(eid1, &[("weight".to_string(), Value::Double(2.5))], 10)
            .unwrap();
        let p0 = csr.get_by_edge_id(eid0, 10).unwrap();
        assert!(p0
            .iter()
            .any(|(k, v)| k == "weight" && v == &Some(Value::Double(1.5))));
        let by_id = csr.get_by_edge_id(eid1, 10).unwrap();
        assert!(by_id
            .iter()
            .any(|(k, v)| k == "weight" && v == &Some(Value::Double(2.5))));
        assert_eq!(csr.row_count(), 2);
    }

    #[test]
    fn visibility() {
        let mut csr = CsrWithProperties::new(schema());
        let eid = EdgeId(99);
        csr.insert_for_edge(eid, &[("weight".to_string(), Value::Double(3.0))], 100)
            .unwrap();
        assert!(csr.get_by_edge_id(eid, 99).is_none());
        assert!(csr.get_by_edge_id(eid, 100).is_some());
        csr.mark_deleted(eid, 150);
        assert!(csr.get_by_edge_id(eid, 149).is_some());
        assert!(csr.get_by_edge_id(eid, 150).is_none());
    }

    #[test]
    fn columnar_repeatable_read() {
        let mut csr = CsrWithProperties::new(schema());
        let eid = EdgeId(42);
        csr.insert_for_edge(eid, &[("weight".to_string(), Value::Double(1.0))], 100)
            .unwrap();
        csr.set_property_for_edge(eid, "weight", Some(Value::Double(2.0)), 200)
            .unwrap();
        // Snapshot read at 150 still observes the before-image (RepeatableRead).
        let old = csr.get_by_edge_id(eid, 150).unwrap();
        assert!(old
            .iter()
            .any(|(k, v)| k == "weight" && v == &Some(Value::Double(1.0))));
        // Newer readers observe the latest write.
        let got = csr.get_by_edge_id(eid, 250).unwrap();
        assert!(got
            .iter()
            .any(|(k, v)| k == "weight" && v == &Some(Value::Double(2.0))));
    }

    #[test]
    fn columnar_property_version_gc_keeps_visible_snapshots() {
        let mut csr = CsrWithProperties::new(schema());
        let eid = EdgeId(7);
        csr.insert_for_edge(eid, &[("weight".to_string(), Value::Double(1.0))], 100)
            .unwrap();
        csr.set_property_for_edge(eid, "weight", Some(Value::Double(2.0)), 200)
            .unwrap();
        csr.set_property_for_edge(eid, "weight", Some(Value::Double(3.0)), 300)
            .unwrap();
        // Entries ending at or before the cutoff are eligible; the chain
        // covering ts=200 must survive a GC at 150.
        assert_eq!(csr.gc_property_versions(150), 0);
        let at_200 = csr.get_by_edge_id(eid, 200).unwrap();
        assert!(at_200
            .iter()
            .any(|(k, v)| k == "weight" && v == &Some(Value::Double(2.0))));
        // Past every active snapshot, history is reclaimed but the latest
        // value stays readable.
        assert!(csr.gc_property_versions(300) >= 1);
        let at_300 = csr.get_by_edge_id(eid, 300).unwrap();
        assert!(at_300
            .iter()
            .any(|(k, v)| k == "weight" && v == &Some(Value::Double(3.0))));
    }

    #[test]
    fn projected_read_returns_only_requested_columns() {
        let mut csr = CsrWithProperties::new(schema());
        let eid = EdgeId(5);
        csr.insert_for_edge(
            eid,
            &[
                ("weight".to_string(), Value::Double(1.5)),
                ("label".to_string(), Value::String("a".into())),
            ],
            100,
        )
        .unwrap();

        let all = csr.get_projected_by_edge_id(eid, 100, None).unwrap();
        assert_eq!(all.len(), 2);

        let subset = csr
            .get_projected_by_edge_id(eid, 100, Some(&["label".to_string()]))
            .unwrap();
        assert_eq!(
            subset,
            vec![("label".to_string(), Some(Value::String("a".into())))]
        );

        let topology_only = csr.get_projected_by_edge_id(eid, 100, Some(&[])).unwrap();
        assert!(topology_only.is_empty());

        let unknown = csr
            .get_projected_by_edge_id(eid, 100, Some(&["missing".to_string()]))
            .unwrap();
        assert!(unknown.is_empty());

        assert!(csr.get_projected_by_edge_id(eid, 99, None).is_none());
        assert!(csr
            .get_projected_by_edge_id(eid, 99, Some(&["weight".to_string()]))
            .is_none());
    }

    #[test]
    fn dump_load_roundtrip() {
        let mut csr = CsrWithProperties::new(schema());
        let eid0 = EdgeId(10);
        let eid1 = EdgeId(11);

        csr.insert_for_edge(eid0, &[("weight".to_string(), Value::Double(1.0))], 100)
            .unwrap();
        csr.insert_for_edge(eid1, &[("weight".to_string(), Value::Double(2.0))], 100)
            .unwrap();
        csr.mark_deleted(eid0, 150);

        let bytes = csr.dump();
        let mut loaded = CsrWithProperties::new(schema());
        loaded.load(&bytes).unwrap();

        assert_eq!(loaded.row_count(), 2);
        assert!(loaded
            .get_by_edge_id(eid1, 100)
            .unwrap()
            .iter()
            .any(|(k, v)| k == "weight" && v == &Some(Value::Double(2.0))));
        assert!(loaded.get_by_edge_id(eid0, 149).is_some());
        assert!(loaded.get_by_edge_id(eid0, 150).is_none());
    }

    #[test]
    fn legacy_version_is_rejected() {
        let mut csr = CsrWithProperties::new(schema());
        let eid = EdgeId(10);
        csr.insert_for_edge(eid, &[("weight".to_string(), Value::Double(1.0))], 100)
            .unwrap();
        let mut bytes = csr.dump();
        bytes[0] = 1;
        let mut loaded = CsrWithProperties::new(schema());
        assert!(loaded.load(&bytes).is_err());
    }

    #[test]
    fn trailing_bytes_are_rejected() {
        let mut csr = CsrWithProperties::new(schema());
        csr.insert_for_edge(
            EdgeId(10),
            &[("weight".to_string(), Value::Double(1.0))],
            100,
        )
        .unwrap();
        let mut bytes = csr.dump();
        bytes.push(0xff);
        let mut loaded = CsrWithProperties::new(schema());
        assert!(loaded.load(&bytes).is_err());
    }

    fn typed_store() -> CsrWithProperties {
        CsrWithProperties::new(vec![
            PropertySchema::new("count".to_string(), 0, DataType::Int),
            PropertySchema::new("flag".to_string(), 1, DataType::Bool),
            PropertySchema::new("tag".to_string(), 2, DataType::String).nullable(true),
        ])
    }

    fn fill_typed_store(rows: i64) -> CsrWithProperties {
        let mut csr = typed_store();
        for i in 0..rows {
            csr.insert_for_edge(
                EdgeId(i as u64),
                &[
                    ("count".to_string(), Value::Int(i as i32)),
                    ("flag".to_string(), Value::Bool(i % 2 == 0)),
                    (
                        "tag".to_string(),
                        Value::String(format!("tag{}", i % 4).into()),
                    ),
                ],
                100,
            )
            .expect("typed insert should succeed");
        }
        csr
    }

    #[test]
    fn dump_collapses_version_history_to_latest() {
        let mut csr = CsrWithProperties::new(schema());
        let eid = EdgeId(42);
        csr.insert_for_edge(eid, &[("weight".to_string(), Value::Double(1.0))], 100)
            .unwrap();
        csr.set_property_for_edge(eid, "weight", Some(Value::Double(2.0)), 200)
            .unwrap();
        let bytes = csr.dump();
        let mut loaded = CsrWithProperties::new(schema());
        loaded.load(&bytes).unwrap();
        let collapsed = loaded
            .get_by_edge_id(eid, 150)
            .expect("row survives reload");
        assert!(collapsed
            .iter()
            .any(|(k, v)| k == "weight" && v == &Some(Value::Double(2.0))));
    }

    #[test]
    fn dump_restores_plain_columns_with_current_values() {
        let mut csr = fill_typed_store(20);
        assert!(csr.auto_encode_properties() > 0);
        let bytes = csr.dump();
        let mut loaded = typed_store();
        loaded.load(&bytes).unwrap();
        assert_eq!(
            loaded.column_encoding_type("count"),
            Some(EncodingType::None)
        );
        let got = loaded
            .get_by_edge_id(EdgeId(3), 200)
            .expect("row should read");
        assert!(got
            .iter()
            .any(|(k, v)| k == "count" && v == &Some(Value::Int(3))));
    }

    #[test]
    fn auto_encode_selects_expected_schemes() {
        let mut csr = fill_typed_store(20);
        assert_eq!(csr.auto_encode_properties(), 3);
        assert_eq!(
            csr.column_encoding_type("count"),
            Some(EncodingType::BitPacking)
        );
        assert_eq!(csr.column_encoding_type("flag"), Some(EncodingType::Rle));
        assert_eq!(
            csr.column_encoding_type("tag"),
            Some(EncodingType::Dictionary)
        );
    }

    #[test]
    fn auto_encode_preserves_values() {
        let mut csr = fill_typed_store(20);
        csr.auto_encode_properties();
        for i in 0..20 {
            let got = csr
                .get_by_edge_id(EdgeId(i as u64), 200)
                .expect("encoded row should stay readable");
            assert!(got
                .iter()
                .any(|(k, v)| k == "count" && v == &Some(Value::Int(i as i32))));
            assert!(got
                .iter()
                .any(|(k, v)| k == "flag" && v == &Some(Value::Bool(i % 2 == 0))));
            let expected_tag = Value::String(format!("tag{}", i % 4).into());
            assert!(got
                .iter()
                .any(|(k, v)| k == "tag" && v == &Some(expected_tag.clone())));
        }
    }

    #[test]
    fn auto_encode_constant_column_uses_single_value_storage() {
        let mut csr = CsrWithProperties::new(vec![PropertySchema::new(
            "level".to_string(),
            0,
            DataType::Int,
        )]);
        for i in 0..60 {
            csr.insert_for_edge(EdgeId(i), &[("level".to_string(), Value::Int(7))], 100)
                .expect("constant insert should succeed");
        }
        assert_eq!(csr.auto_encode_properties(), 1);
        assert_eq!(
            csr.column_encoding_type("level"),
            Some(EncodingType::Constant)
        );
        let got = csr.get_by_edge_id(EdgeId(3), 200).expect("row should read");
        assert!(got
            .iter()
            .any(|(k, v)| k == "level" && v == &Some(Value::Int(7))));
    }

    #[test]
    fn encode_rejects_unknown_column_and_skips_empty() {
        let mut csr = typed_store();
        assert_eq!(csr.auto_encode_properties(), 0);
        assert!(csr
            .apply_encoding_to_column("missing", EncodingType::Rle, 255)
            .is_err());
        assert_eq!(csr.column_encoding_type("missing"), None);
    }

    #[test]
    fn refresh_stats_feeds_snapshot() {
        let mut csr = CsrWithProperties::new(vec![PropertySchema::new(
            "count".to_string(),
            0,
            DataType::Int,
        )
        .nullable(true)]);
        for i in 0..5 {
            csr.insert_for_edge(
                EdgeId(i),
                &[("count".to_string(), Value::Int((i as i32 + 1) * 10))],
                100,
            )
            .expect("stat insert should succeed");
        }
        csr.insert_for_edge(EdgeId(99), &[], 100)
            .expect("null insert should succeed");
        // Materialize the absent cell as an explicit null so the flush-time
        // statistics count it, matching the read path that serves it as null.
        csr.set_property_for_edge(EdgeId(99), "count", None, 150)
            .expect("null write should succeed");
        // Zone-map bounds are live before any refresh, but persisted counts
        // only exist after the flush-time refresh.
        let before = csr.column_stats_snapshot("count").expect("snapshot exists");
        assert_eq!(before.row_count, 6);
        assert_eq!(before.min_value, Some(Value::Int(10)));
        assert_eq!(before.max_value, Some(Value::Int(50)));
        assert_eq!(before.null_count, None);
        csr.refresh_column_stats();
        let after = csr.column_stats_snapshot("count").expect("snapshot exists");
        assert_eq!(after.row_count, 6);
        assert_eq!(after.min_value, Some(Value::Int(10)));
        assert_eq!(after.max_value, Some(Value::Int(50)));
        assert_eq!(after.null_count, Some(1));
        assert!(after.distinct_count.is_some());
        assert!(csr.column_stats_snapshot("missing").is_none());
    }
}
