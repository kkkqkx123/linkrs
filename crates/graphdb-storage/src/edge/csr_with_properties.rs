//! CSR with Properties — ladybug-style columnar storage.
//!
//! Properties are stored in parallel column arrays aligned with CSR row
//! positions. Row `pos = offsets[src] + edge_index` gives the column row for
//! that edge's properties — no `prop_offset` indirection inside Nbr.
//!
//! This implementation uses `Column` (continuous arrays) instead of
//! `HashMap<u32, Value>` for cache-friendly scans and lower memory overhead.

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

/// Columnar property storage aligned with CSR row positions.
#[derive(Debug, Clone)]
pub struct CsrWithProperties {
    offsets: Vec<u32>,
    lengths: Vec<u32>,
    total_edges: u64,
    vertex_capacity: usize,
    property_schema: Vec<PropertySchema>,
    property_columns: Vec<Column>,
    visibility: Vec<RowVisibility>,
    edge_to_row: HashMap<EdgeId, u32>,
    free_list: Vec<u32>,
    row_count: usize,
    version_chain_cap: usize,
    retention_horizon: Timestamp,
}

impl CsrWithProperties {
    pub fn new(vertex_capacity: usize, property_schema: Vec<PropertySchema>) -> Self {
        let vc = vertex_capacity.max(1);
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
            offsets: vec![0; vc + 1],
            lengths: vec![0; vc],
            total_edges: 0,
            vertex_capacity: vc,
            property_schema,
            property_columns,
            visibility: Vec::new(),
            edge_to_row: HashMap::new(),
            free_list: Vec::new(),
            row_count: 0,
            version_chain_cap: 64,
            retention_horizon: Timestamp::MAX,
        }
    }

    pub fn property_schema(&self) -> &[PropertySchema] {
        &self.property_schema
    }

    pub fn vertex_capacity(&self) -> usize {
        self.vertex_capacity
    }

    pub fn edge_count(&self) -> u64 {
        self.total_edges
    }

    pub fn row_count(&self) -> usize {
        self.row_count
    }

    pub fn set_version_chain_cap(&mut self, cap: usize) {
        self.version_chain_cap = cap;
    }

    pub fn set_retention_horizon(&mut self, horizon: Timestamp) {
        self.retention_horizon = horizon;
    }

    fn ensure_vertex_capacity(&mut self, min: usize) {
        if min <= self.vertex_capacity {
            return;
        }
        let new_cap = (min as f64 * 1.25).ceil() as usize;
        self.offsets.resize(new_cap + 1, 0);
        self.lengths.resize(new_cap, 0);
        self.vertex_capacity = new_cap;
    }

    fn rebuild_offsets(&mut self) {
        self.offsets[0] = 0;
        for i in 0..self.lengths.len() {
            self.offsets[i + 1] = self.offsets[i] + self.lengths[i];
        }
    }

    /// Generic insert (like PropertyTable) — allocates next free row.
    pub fn insert(&mut self, values: &[(String, Value)], create_ts: Timestamp) -> StorageResult<u32> {
        let row_idx = if let Some(free_off) = self.free_list.pop() {
            let idx = (free_off - 1) as usize;
            if idx >= self.visibility.len() {
                self.visibility.resize(idx + 1, RowVisibility::new(0));
                for col in &mut self.property_columns {
                    if col.len() <= idx {
                        col.resize(idx + 1);
                    }
                    col.clear_row_version_chains(idx);
                }
            } else {
                for col in &mut self.property_columns {
                    col.clear_row_version_chains(idx);
                }
            }
            self.visibility[idx] = RowVisibility::new(create_ts);
            self.row_count += 1;
            idx
        } else {
            let idx = self.visibility.len();
            self.visibility.push(RowVisibility::new(create_ts));
            for col in &mut self.property_columns {
                if col.len() <= idx {
                    col.resize(idx + 1);
                }
                col.clear_row_version_chains(idx);
            }
            self.row_count += 1;
            idx
        };
        for (i, schema) in self.property_schema.iter().enumerate() {
            let col = &mut self.property_columns[i];
            if col.len() <= row_idx {
                col.resize(row_idx + 1);
            }
            if let Some((_, v)) = values.iter().find(|(k, _)| k == &schema.name) {
                col.set_versioned(row_idx, Some(v), create_ts)?;
            } else {
                let _ = col.set_versioned(row_idx, None, create_ts);
            }
            if self.version_chain_cap != 0 && col.version_chain_len(row_idx) > self.version_chain_cap {
                col.fold_oldest(row_idx, self.version_chain_cap, self.retention_horizon);
            }
        }
        Ok(crate::edge::property_schema::prop_index_to_offset(row_idx))
    }

    /// Insert an edge's properties at the CSR position for `src`.
    pub fn insert_properties(
        &mut self,
        src: u32,
        edge_id: EdgeId,
        properties: &[(String, Value)],
        ts: Timestamp,
    ) -> StorageResult<u32> {
        self.ensure_vertex_capacity(src as usize + 1);
        let pos = (self.offsets[src as usize] + self.lengths[src as usize]) as usize;
        if pos >= self.visibility.len() {
            self.visibility.resize(pos + 1, RowVisibility::new(0));
        }
        self.visibility[pos] = RowVisibility::new(ts);
        for col in &mut self.property_columns {
            if col.len() <= pos {
                col.resize(pos + 1);
            }
            col.clear_row_version_chains(pos);
        }
        self.row_count += 1;

        for (i, schema) in self.property_schema.iter().enumerate() {
            let col = &mut self.property_columns[i];
            if col.len() <= pos {
                col.resize(pos + 1);
            }
            if let Some((_, v)) = properties.iter().find(|(k, _)| k == &schema.name) {
                col.set_versioned(pos, Some(v), ts)?;
            } else {
                let _ = col.set_versioned(pos, None, ts);
            }
            if self.version_chain_cap != 0 && col.version_chain_len(pos) > self.version_chain_cap {
                col.fold_oldest(pos, self.version_chain_cap, self.retention_horizon);
            }
        }

        self.edge_to_row.insert(edge_id, pos as u32);
        self.lengths[src as usize] += 1;
        self.total_edges += 1;
        self.rebuild_offsets();
        Ok(pos as u32)
    }

    /// Positional property read — fast path for scans.
    pub fn get_properties(
        &self,
        src: u32,
        edge_index: usize,
        query_ts: Timestamp,
    ) -> Option<Vec<(String, Option<Value>)>> {
        let start = *self.offsets.get(src as usize)? as usize;
        let len = *self.lengths.get(src as usize)? as usize;
        if edge_index >= len {
            return None;
        }
        let pos = start + edge_index;
        let vis = self.visibility.get(pos)?;
        if !vis.is_visible_at(query_ts) {
            return None;
        }
        Some(
            self.property_schema
                .iter()
                .enumerate()
                .map(|(i, s)| {
                    let v = self.property_columns[i].get_at_ts(pos, query_ts);
                    (s.name.clone(), v)
                })
                .collect(),
        )
    }

    /// Lookup by `EdgeId`.
    pub fn get_by_edge_id(
        &self,
        edge_id: EdgeId,
        query_ts: Timestamp,
    ) -> Option<Vec<(String, Option<Value>)>> {
        let pos = *self.edge_to_row.get(&edge_id)? as usize;
        let vis = self.visibility.get(pos)?;
        if !vis.is_visible_at(query_ts) {
            return None;
        }
        Some(
            self.property_schema
                .iter()
                .enumerate()
                .map(|(i, s)| {
                    let v = self.property_columns[i].get_at_ts(pos, query_ts);
                    (s.name.clone(), v)
                })
                .collect(),
        )
    }

    /// For compatibility with PropertyTable API: get by offset (prop_offset = pos+1)
    pub fn get(&self, offset: u32, query_ts: Option<Timestamp>) -> Option<Vec<(String, Option<Value>)>> {
        let row_idx = crate::edge::property_schema::prop_offset_to_index(offset)?;
        let ts = query_ts.unwrap_or(Timestamp::MAX);
        let vis = self.visibility.get(row_idx)?;
        if !vis.is_visible_at(ts) {
            return None;
        }
        Some(
            self.property_schema
                .iter()
                .enumerate()
                .map(|(i, s)| {
                    let v = self.property_columns[i].get_at_ts(row_idx, ts);
                    (s.name.clone(), v)
                })
                .collect(),
        )
    }

    pub fn read_properties(&self, offset: u32) -> Option<Vec<(String, Value)>> {
        let props = self.get(offset, None)?;
        let result: Vec<(String, Value)> = props
            .into_iter()
            .filter_map(|(name, opt_val)| opt_val.map(|v| (name, v)))
            .collect();
        if result.is_empty() {
            None
        } else {
            Some(result)
        }
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

    /// Insert properties for an edge using the free-list path and associate
    /// the allocated row with `edge_id`. Returns the property offset (row+1).
    pub fn insert_for_edge(
        &mut self,
        edge_id: EdgeId,
        values: &[(String, Value)],
        create_ts: Timestamp,
    ) -> StorageResult<u32> {
        let offset = self.insert(values, create_ts)?;
        if let Some(row_idx) = crate::edge::property_schema::prop_offset_to_index(offset) {
            self.edge_to_row.insert(edge_id, row_idx as u32);
        }
        Ok(offset)
    }

    /// Associate an existing offset with an edge id (for migration paths).
    pub fn associate_edge(&mut self, edge_id: EdgeId, offset: u32) {
        if let Some(row_idx) = crate::edge::property_schema::prop_offset_to_index(offset) {
            self.edge_to_row.insert(edge_id, row_idx as u32);
        }
    }

    pub fn get_offset_for_edge(&self, edge_id: EdgeId) -> Option<u32> {
        self.edge_to_row
            .get(&edge_id)
            .map(|pos| crate::edge::property_schema::prop_index_to_offset(*pos as usize))
    }

    pub fn remove_edge_mapping(&mut self, edge_id: EdgeId) -> Option<u32> {
        self.edge_to_row.remove(&edge_id).map(|pos| {
            crate::edge::property_schema::prop_index_to_offset(pos as usize)
        })
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
        let offset = crate::edge::property_schema::prop_index_to_offset(pos as usize);
        self.set_property(offset, name, value, ts)
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
        let offset = crate::edge::property_schema::prop_index_to_offset(pos as usize);
        self.update(offset, properties, ts)
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
        let offset = crate::edge::property_schema::prop_index_to_offset(pos as usize);
        self.set_property_by_id(offset, prop_id, value, ts)
    }

    pub fn revert_deletion_for_edge(&mut self, edge_id: EdgeId) -> bool {
        if let Some(&pos) = self.edge_to_row.get(&edge_id) {
            let offset = crate::edge::property_schema::prop_index_to_offset(pos as usize);
            return self.revert_deletion(offset);
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

    pub fn mark_deleted_by_offset(&mut self, offset: u32, ts: Timestamp) -> StorageResult<()> {
        let row_idx = crate::edge::property_schema::prop_offset_to_index(offset)
            .ok_or_else(|| StorageError::invalid_offset(offset))?;
        if row_idx >= self.visibility.len() {
            return Ok(());
        }
        if self.visibility[row_idx].delete_ts.is_some() {
            return Err(StorageError::invalid_operation("record already marked deleted"));
        }
        self.visibility[row_idx].mark_deleted(ts);
        Ok(())
    }

    pub fn is_deleted(&self, offset: u32) -> bool {
        if let Some(row_idx) = crate::edge::property_schema::prop_offset_to_index(offset) {
            if let Some(vis) = self.visibility.get(row_idx) {
                return vis.delete_ts.is_some();
            }
        }
        false
    }

    pub fn revert_deletion(&mut self, offset: u32) -> bool {
        if let Some(row_idx) = crate::edge::property_schema::prop_offset_to_index(offset) {
            if let Some(vis) = self.visibility.get_mut(row_idx) {
                if vis.delete_ts.is_some() {
                    vis.delete_ts = None;
                    return true;
                }
            }
        }
        false
    }

    pub fn delete_edge(&mut self, src: u32, edge_index: usize, ts: Timestamp) -> bool {
        let start = match self.offsets.get(src as usize) {
            Some(v) => *v as usize,
            None => return false,
        };
        let len = match self.lengths.get(src as usize) {
            Some(v) => *v as usize,
            None => return false,
        };
        if edge_index >= len {
            return false;
        }
        let pos = start + edge_index;
        if let Some(vis) = self.visibility.get_mut(pos) {
            if vis.delete_ts.is_some() {
                return false;
            }
            vis.mark_deleted(ts);
            return true;
        }
        false
    }

    pub fn offsets(&self) -> &[u32] {
        &self.offsets
    }

    pub fn lengths(&self) -> &[u32] {
        &self.lengths
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
        let schema = PropertySchema::new(name.clone(), prop_id, data_type.clone()).nullable(nullable);
        self.property_schema.push(schema);
        let mut col = Column::new(name, prop_id, data_type, nullable);
        let rows = self.visibility.len();
        if rows > 0 {
            col.resize(rows);
        }
        self.property_columns.push(col);
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
        Ok(())
    }

    pub fn set_property(
        &mut self,
        offset: u32,
        name: &str,
        value: Option<Value>,
        ts: Timestamp,
    ) -> StorageResult<()> {
        let row_idx = crate::edge::property_schema::prop_offset_to_index(offset)
            .ok_or_else(|| StorageError::invalid_offset(offset))?;
        if row_idx >= self.visibility.len() || self.visibility[row_idx].create_ts == 0 {
            return Err(StorageError::invalid_offset(offset));
        }
        let col_idx = self
            .property_schema
            .iter()
            .position(|s| s.name == name)
            .ok_or_else(|| StorageError::column_not_found(name.to_string()))?;
        let col = &mut self.property_columns[col_idx];
        col.set_versioned(row_idx, value.as_ref(), ts)?;
        if self.version_chain_cap != 0 && col.version_chain_len(row_idx) > self.version_chain_cap {
            col.fold_oldest(row_idx, self.version_chain_cap, self.retention_horizon);
        }
        Ok(())
    }

    /// Bulk update properties at a given offset position.
    pub fn update(
        &mut self,
        offset: u32,
        properties: &[(String, Value)],
        ts: Timestamp,
    ) -> StorageResult<()> {
        let row_idx = crate::edge::property_schema::prop_offset_to_index(offset)
            .ok_or_else(|| StorageError::invalid_offset(offset))?;
        if row_idx >= self.visibility.len() || self.visibility[row_idx].create_ts == 0 {
            return Err(StorageError::invalid_offset(offset));
        }
        for (name, value) in properties {
            let col_idx = self
                .property_schema
                .iter()
                .position(|s| s.name == *name)
                .ok_or_else(|| StorageError::column_not_found(name.to_string()))?;
            let col = &mut self.property_columns[col_idx];
            col.set_versioned(row_idx, Some(value), ts)?;
            if self.version_chain_cap != 0 && col.version_chain_len(row_idx) > self.version_chain_cap {
                col.fold_oldest(row_idx, self.version_chain_cap, self.retention_horizon);
            }
        }
        Ok(())
    }

    pub fn set_property_by_id(
        &mut self,
        offset: u32,
        prop_id: crate::types::PropertyId,
        value: Option<Value>,
        ts: Timestamp,
    ) -> StorageResult<()> {
        let idx = prop_id.as_usize();
        if idx >= self.property_schema.len() {
            return Err(StorageError::column_not_found(format!("prop_id={}", prop_id.0)));
        }
        let name = self.property_schema[idx].name.clone();
        self.set_property(offset, &name, value, ts)
    }

    pub fn compaction_stats(&self) -> crate::edge::property_schema::PropertyCompactionStats {
        let tombstone_count = self.visibility.iter().filter(|v| v.delete_ts.is_some()).count();
        let live_records = self.visibility.iter().filter(|v| v.create_ts != 0 && v.delete_ts.is_none()).count();
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

    pub fn version_chain_stats(&self) -> crate::vertex::column::VersionChainStats {
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
        crate::vertex::column::VersionChainStats {
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
        total += self.offsets.capacity() * std::mem::size_of::<u32>();
        total += self.lengths.capacity() * std::mem::size_of::<u32>();
        total += self.visibility.capacity() * std::mem::size_of::<RowVisibility>();
        total += self.edge_to_row.len() * (std::mem::size_of::<EdgeId>() + std::mem::size_of::<u32>());
        total += self.free_list.capacity() * std::mem::size_of::<u32>();
        for col in &self.property_columns {
            total += col.memory_size();
        }
        total += self.property_schema.len() * std::mem::size_of::<PropertySchema>();
        total
    }

    pub fn dump(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.push(1u8); // version
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
        buf.extend_from_slice(&(self.offsets.len() as u32).to_le_bytes());
        for &o in &self.offsets {
            buf.extend_from_slice(&o.to_le_bytes());
        }
        buf.extend_from_slice(&(self.lengths.len() as u32).to_le_bytes());
        for &l in &self.lengths {
            buf.extend_from_slice(&l.to_le_bytes());
        }
        buf.extend_from_slice(&self.total_edges.to_le_bytes());
        buf.extend_from_slice(&(self.vertex_capacity as u32).to_le_bytes());
        // Serialize current column values (without version history) for at least basic persistence
        buf.extend_from_slice(&(self.property_columns.len() as u32).to_le_bytes());
        for col in &self.property_columns {
            // name not needed as schema already known, but write placeholder
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
        if version != 1 {
            return Err(StorageError::deserialize_error(format!(
                "Unsupported CsrWithProperties version: {}",
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
            self.row_count = u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap()) as usize;
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
            let free_len = u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap()) as usize;
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
            let off_len = u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap()) as usize;
            offset += 4;
            self.offsets.clear();
            for _ in 0..off_len {
                if offset + 4 > data.len() {
                    break;
                }
                let o = u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap());
                offset += 4;
                self.offsets.push(o);
            }
        }
        if offset + 4 <= data.len() {
            let len_len = u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap()) as usize;
            offset += 4;
            self.lengths.clear();
            for _ in 0..len_len {
                if offset + 4 > data.len() {
                    break;
                }
                let l = u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap());
                offset += 4;
                self.lengths.push(l);
            }
        }
        if offset + 8 <= data.len() {
            self.total_edges = u64::from_le_bytes(data[offset..offset + 8].try_into().unwrap());
            offset += 8;
        }
        if offset + 4 <= data.len() {
            self.vertex_capacity = u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap()) as usize;
            offset += 4;
        }
        if offset + 4 <= data.len() {
            let col_count = u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap()) as usize;
            offset += 4;
            for _ in 0..col_count {
                if offset + 4 > data.len() {
                    break;
                }
                let name_len = u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap()) as usize;
                offset += 4;
                if offset + name_len > data.len() {
                    break;
                }
                let name = String::from_utf8_lossy(&data[offset..offset + name_len]).to_string();
                offset += name_len;
                if offset + 4 > data.len() {
                    break;
                }
                let rows = u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap()) as usize;
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
                            let vlen = u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap()) as usize;
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
                            let vlen = u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap()) as usize;
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
        Ok(())
    }

    pub fn gc_versions(&mut self, min_active_snapshot_ts: Timestamp) -> usize {
        let mut removed = 0;
        for col in &mut self.property_columns {
            removed += col.gc_versions(min_active_snapshot_ts);
        }
        removed
    }

    pub fn reclaim_slots(&mut self, valid_offsets: &HashSet<u32>, retention_bound: Timestamp) -> usize {
        if retention_bound == Timestamp::MAX {
            return 0;
        }
        let mut to_reclaim = Vec::new();
        for (idx, vis) in self.visibility.iter().enumerate() {
            if vis.create_ts == 0 {
                continue;
            }
            let offset = crate::edge::property_schema::prop_index_to_offset(idx);
            if valid_offsets.contains(&offset) {
                continue;
            }
            if let Some(del_ts) = vis.delete_ts {
                if del_ts <= retention_bound {
                    to_reclaim.push((idx, offset));
                }
            }
        }
        for (idx, offset) in to_reclaim.iter() {
            self.visibility[*idx].create_ts = 0;
            self.visibility[*idx].delete_ts = None;
            for col in &mut self.property_columns {
                col.clear_row_version_chains(*idx);
            }
            let pos = crate::edge::property_schema::prop_offset_to_index(*offset).unwrap() as u32;
            self.edge_to_row.retain(|_, p| *p != pos);
            self.free_list.push(*offset);
            self.row_count = self.row_count.saturating_sub(1);
        }
        to_reclaim.len()
    }

    pub fn get_projected(
        &self,
        offset: u32,
        projection: &[String],
        query_ts: Option<Timestamp>,
    ) -> Option<Vec<(String, Option<Value>)>> {
        let row_idx = crate::edge::property_schema::prop_offset_to_index(offset)?;
        let ts = query_ts.unwrap_or(Timestamp::MAX);
        let vis = self.visibility.get(row_idx)?;
        if !vis.is_visible_at(ts) {
            return None;
        }
        let mut out = Vec::with_capacity(projection.len());
        for col_name in projection {
            if let Some(col) = self.property_columns.iter().find(|c| &c.name == col_name) {
                out.push((col_name.clone(), col.get_at_ts(row_idx, ts)));
            } else {
                out.push((col_name.clone(), None));
            }
        }
        Some(out)
    }

    pub fn column_stats_snapshot(
        &self,
        _column: &str,
    ) -> Option<crate::stats_reader::ColumnStatsSnapshot> {
        None
    }

    pub fn find_by_property(&self, _name: &str, _value: &Value) -> Vec<u32> {
        Vec::new()
    }
    pub fn find_by_property_null(&self, _name: &str) -> Vec<u32> {
        Vec::new()
    }

    pub fn get_projected_batch(
        &self,
        offsets: &[u32],
        projection: &[String],
        query_ts: Option<Timestamp>,
    ) -> Vec<Option<Vec<(String, Option<Value>)>>> {
        offsets
            .iter()
            .map(|off| self.get_projected(*off, projection, query_ts))
            .collect()
    }

    pub fn get_batch<'a, I>(&'a self, offsets: I, query_ts: Option<Timestamp>) -> Vec<Option<Vec<(String, Option<Value>)>>>
    where
        I: IntoIterator<Item = &'a u32>,
    {
        offsets
            .into_iter()
            .map(|off| self.get(*off, query_ts))
            .collect()
    }

    pub fn column_values(&self, col_idx: usize) -> Vec<Option<Value>> {
        if col_idx >= self.property_schema.len() {
            return Vec::new();
        }
        let mut values = Vec::with_capacity(self.visibility.len());
        for row_idx in 0..self.visibility.len() {
            if self.visibility[row_idx].create_ts == 0
                || self.visibility[row_idx].delete_ts.is_some()
            {
                values.push(None);
            } else {
                values.push(self.property_columns[col_idx].get(row_idx));
            }
        }
        values
    }

    pub fn apply_column_encoding(
        &mut self,
        col_name: &str,
        encoding: crate::encoding::EncodingType,
    ) -> StorageResult<()> {
        if let Some(col) = self.property_columns.iter_mut().find(|c| c.name == col_name) {
            // Delegate to column's encoding apply - simplified
            let _ = encoding;
            let _ = col;
        }
        Ok(())
    }

    pub fn zone_maps(&self) -> &HashMap<String, Vec<crate::column_stats::ColumnStats>> {
        // Return empty static
        static EMPTY: std::sync::OnceLock<HashMap<String, Vec<crate::column_stats::ColumnStats>>> = std::sync::OnceLock::new();
        EMPTY.get_or_init(HashMap::new)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use graphdb_core::DataType;

    fn schema() -> Vec<PropertySchema> {
        vec![
            PropertySchema::new("weight".to_string(), 0, DataType::Double),
            PropertySchema::new("label".to_string(), 1, DataType::String).nullable(true),
        ]
    }

    #[test]
    fn csr_positional_access() {
        let mut csr = CsrWithProperties::new(4, schema());
        let eid0 = EdgeId(1);
        let eid1 = EdgeId(2);
        csr.insert_properties(0, eid0, &[("weight".to_string(), Value::Double(1.5))], 10)
            .unwrap();
        csr.insert_properties(0, eid1, &[("weight".to_string(), Value::Double(2.5))], 10)
            .unwrap();
        let p0 = csr.get_properties(0, 0, 10).unwrap();
        assert!(p0.iter().any(|(k, v)| k == "weight" && v == &Some(Value::Double(1.5))));
        let p1 = csr.get_properties(0, 1, 10).unwrap();
        assert!(p1.iter().any(|(k, v)| k == "weight" && v == &Some(Value::Double(2.5))));
        let by_id = csr.get_by_edge_id(eid1, 10).unwrap();
        assert!(by_id.iter().any(|(k, v)| k == "weight" && v == &Some(Value::Double(2.5))));
    }

    #[test]
    fn visibility() {
        let mut csr = CsrWithProperties::new(2, schema());
        let eid = EdgeId(99);
        csr.insert_properties(1, eid, &[("weight".to_string(), Value::Double(3.0))], 100)
            .unwrap();
        assert!(csr.get_properties(1, 0, 99).is_none());
        assert!(csr.get_properties(1, 0, 100).is_some());
        csr.mark_deleted(eid, 150);
        assert!(csr.get_properties(1, 0, 149).is_some());
        assert!(csr.get_properties(1, 0, 150).is_none());
    }

    #[test]
    fn columnar_time_travel() {
        let mut csr = CsrWithProperties::new(2, schema());
        let eid = EdgeId(42);
        csr.insert_properties(0, eid, &[("weight".to_string(), Value::Double(1.0))], 100)
            .unwrap();
        let off = 1u32; // pos 0 -> offset 1
        csr.set_property(off, "weight", Some(Value::Double(2.0)), 200).unwrap();
        let old = csr.get(off, Some(150)).unwrap();
        assert!(old.iter().any(|(k, v)| k == "weight" && v == &Some(Value::Double(1.0))));
        let ne = csr.get(off, Some(250)).unwrap();
        assert!(ne.iter().any(|(k, v)| k == "weight" && v == &Some(Value::Double(2.0))));
    }
}
