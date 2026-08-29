//! Property Table for Edges
//!
//! Column-only MVCC storage for edge properties.
//!
//! The earlier row-oriented layout stored edge properties as whole-row blobs.
//! This version is columnar-only: each property column is stored independently
//! via `ColumnStore` (one `Column` per property, independent compression,
//! zero-copy scans, column pruning, predicate pushdown) and row liveness is
//! tracked via `row_create_ts` + `TieredTombstoneManager` + `free_list`.
//! No serialized row blob remains — `ColumnStore` is the single source of truth.

use std::collections::{HashMap, HashSet};
use std::io::{Cursor, Read};

use crate::column_stats::{compute_stats, ColumnStats};
use crate::encoding::EncodingType;
use crate::mvcc::TieredTombstoneManager;
use crate::naming::NameIndexer;
use crate::persistence::{read_header, read_u32_le, read_u64_le, section, write_header};
use crate::types::PropertyId;
use crate::vertex::column_store::ColumnStore;
use graphdb_core::types::EdgeId;
use graphdb_core::types::Timestamp;
use graphdb_core::{
    data_type_from_info, DataType, StorageError, StorageResult, TypeCodecError,
    TypeInfo, Value,
};

mod columnar;
mod mvcc;
mod serialization;
mod tombstone;
mod zone_map;

/// Current on-disk layout version: columnar-only (ColumnStore per property) +
/// zone maps + per-column encodings + row Create/Delete metadata.
const PROPERTY_TABLE_VERSION: u8 = 1;

/// Rows per zone-map chunk. Zone maps store min/max/ndv/null_count per chunk
/// for predicate pushdown (skip chunks whose zone cannot contain the predicate).
pub const ZONE_MAP_CHUNK_SIZE: usize = 1024;

pub use super::property_schema::{
    prop_index_to_offset, prop_offset_to_index, PropertyCompactionStats, PropertySchema,
};

/// A single projected row: optional list of `(column_name, optional_value)` pairs.
type ProjectedRow = Option<Vec<(String, Option<Value>)>>;

/// Property value index for fast edge lookup by property value.
#[derive(Debug, Clone, Default)]
pub struct PropertyValueIndex {
    index: HashMap<String, HashMap<Vec<u8>, HashSet<u32>>>,
}

impl PropertyValueIndex {
    pub fn new() -> Self {
        Self {
            index: HashMap::new(),
        }
    }

    pub fn insert(&mut self, name: &str, value: Option<&Value>, offset: u32) {
        let entry = self.index.entry(name.to_string()).or_default();
        let key = encode_value_for_index(value);
        entry.entry(key).or_default().insert(offset);
    }

    pub fn index_record(&mut self, props: &[(String, Option<Value>)], offset: u32) {
        for (name, val) in props {
            self.insert(name, val.as_ref(), offset);
        }
    }

    pub fn remove(&mut self, name: &str, value: Option<&Value>, offset: u32) {
        if let Some(entry) = self.index.get_mut(name) {
            let key = encode_value_for_index(value);
            if let Some(offsets) = entry.get_mut(&key) {
                offsets.remove(&offset);
                if offsets.is_empty() {
                    entry.remove(&key);
                }
            }
            if entry.is_empty() {
                self.index.remove(name);
            }
        }
    }

    pub fn remove_record(&mut self, props: &[(String, Option<Value>)], offset: u32) {
        for (name, val) in props {
            self.remove(name, val.as_ref(), offset);
        }
    }

    pub fn lookup(&self, name: &str, value: Option<&Value>) -> Vec<u32> {
        let key = encode_value_for_index(value);
        self.index
            .get(name)
            .and_then(|entry| entry.get(&key))
            .map(|offsets| offsets.iter().copied().collect())
            .unwrap_or_default()
    }

    pub fn clear(&mut self) {
        self.index.clear();
    }

    pub fn entry_count(&self) -> usize {
        self.index.values().map(|m| m.len()).sum()
    }

    /// Rebuild index from live columnar rows.
    pub fn rebuild_columnar(
        &mut self,
        schema: &[PropertySchema],
        column_store: &ColumnStore,
        row_create_ts: &[Timestamp],
        row_delete_ts: &[Option<Timestamp>],
        free_list: &[u32],
    ) {
        self.clear();
        let free_set: HashSet<u32> = free_list.iter().copied().collect();
        for row_idx in 0..row_create_ts.len() {
            let offset = prop_index_to_offset(row_idx);
            if free_set.contains(&offset) {
                continue;
            }
            if row_create_ts[row_idx] == 0 {
                continue;
            }
            if row_delete_ts.get(row_idx).and_then(|v| *v).is_some() {
                continue;
            }
            let mut props = Vec::new();
            for schema_entry in schema {
                if let Some(col) = column_store.get_column(&schema_entry.name) {
                    let val = col.get(row_idx);
                    props.push((schema_entry.name.clone(), val));
                } else {
                    props.push((schema_entry.name.clone(), None));
                }
            }
            for (name, val) in &props {
                self.insert(name, val.as_ref(), offset);
            }
        }
    }
}

fn encode_value_for_index(value: Option<&Value>) -> Vec<u8> {
    match value {
        None => vec![0],
        Some(v) => {
            let mut buf = vec![1];
            match v {
                Value::Bool(b) => {
                    buf.push(if *b { 1 } else { 0 });
                }
                Value::SmallInt(i) => buf.extend_from_slice(&i.to_le_bytes()),
                Value::Int(i) => buf.extend_from_slice(&i.to_le_bytes()),
                Value::BigInt(i) => buf.extend_from_slice(&i.to_le_bytes()),
                Value::Float(f) => buf.extend_from_slice(&f.to_le_bytes()),
                Value::Double(d) => buf.extend_from_slice(&d.to_le_bytes()),
                Value::String(s) => {
                    let bytes = s.as_bytes();
                    encode_varint(bytes.len() as u32, &mut buf);
                    buf.extend_from_slice(bytes);
                }
                Value::Date(d) => {
                    buf.extend_from_slice(&d.year.to_le_bytes());
                    buf.extend_from_slice(&d.month.to_le_bytes());
                    buf.extend_from_slice(&d.day.to_le_bytes());
                }
                _ => {}
            }
            buf
        }
    }
}

fn encode_varint(mut value: u32, buffer: &mut Vec<u8>) {
    while value >= 128 {
        buffer.push((value as u8) | 0x80);
        value >>= 7;
    }
    buffer.push(value as u8);
}

fn decode_varint(cursor: &mut Cursor<&[u8]>) -> StorageResult<u32> {
    let mut result = 0u32;
    let mut shift = 0;
    loop {
        let mut b = [0u8; 1];
        cursor
            .read_exact(&mut b)
            .map_err(|_| StorageError::deserialize_error("failed to decode varint"))?;
        result |= ((b[0] & 0x7F) as u32) << shift;
        if b[0] < 128 {
            break;
        }
        shift += 7;
    }
    Ok(result)
}

#[derive(Debug)]
pub struct PropertyTable {
    schema: Vec<PropertySchema>,
    name_indexer: NameIndexer,
    row_create_ts: Vec<Timestamp>,
    row_delete_ts: Vec<Option<Timestamp>>,
    row_count: usize,
    free_list: Vec<u32>,
    tombstones_manager: TieredTombstoneManager<u32>,
    value_index: PropertyValueIndex,
    edge_prop_map: HashMap<EdgeId, u32>,
    version_chain_cap: usize,
    retention_horizon: Timestamp,
    column_store: ColumnStore,
    zone_maps: HashMap<String, Vec<ColumnStats>>,
}

pub const DEFAULT_VERSION_CHAIN_CAP: usize = 64;

impl Clone for PropertyTable {
    fn clone(&self) -> Self {
        Self {
            schema: self.schema.clone(),
            name_indexer: self.name_indexer.clone(),
            row_create_ts: self.row_create_ts.clone(),
            row_delete_ts: self.row_delete_ts.clone(),
            row_count: self.row_count,
            free_list: self.free_list.clone(),
            tombstones_manager: self.tombstones_manager.clone(),
            value_index: self.value_index.clone(),
            edge_prop_map: self.edge_prop_map.clone(),
            version_chain_cap: self.version_chain_cap,
            retention_horizon: self.retention_horizon,
            column_store: self.column_store.clone(),
            zone_maps: self.zone_maps.clone(),
        }
    }
}

impl PropertyTable {
    pub fn new() -> Self {
        Self {
            schema: Vec::new(),
            name_indexer: NameIndexer::new(),
            row_create_ts: Vec::new(),
            row_delete_ts: Vec::new(),
            row_count: 0,
            free_list: Vec::new(),
            tombstones_manager: TieredTombstoneManager::new(10_000),
            value_index: PropertyValueIndex::new(),
            edge_prop_map: HashMap::new(),
            version_chain_cap: DEFAULT_VERSION_CHAIN_CAP,
            retention_horizon: Timestamp::MAX,
            column_store: ColumnStore::new(),
            zone_maps: HashMap::new(),
        }
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            schema: Vec::new(),
            name_indexer: NameIndexer::with_capacity(capacity),
            row_create_ts: Vec::with_capacity(capacity),
            row_delete_ts: Vec::with_capacity(capacity),
            row_count: 0,
            free_list: Vec::with_capacity(capacity / 10),
            tombstones_manager: TieredTombstoneManager::new(10_000),
            value_index: PropertyValueIndex::new(),
            edge_prop_map: HashMap::with_capacity(capacity),
            version_chain_cap: DEFAULT_VERSION_CHAIN_CAP,
            retention_horizon: Timestamp::MAX,
            column_store: ColumnStore::with_capacity(capacity),
            zone_maps: HashMap::new(),
        }
    }

    pub fn set_version_chain_cap(&mut self, cap: usize) {
        self.version_chain_cap = cap;
    }

    pub fn set_retention_horizon(&mut self, horizon: Timestamp) {
        self.retention_horizon = horizon;
    }

    pub fn add_property(
        &mut self,
        name: String,
        data_type: DataType,
        nullable: bool,
    ) -> StorageResult<PropertyId> {
        let prop_id = PropertyId::new(self.schema.len() as u16);
        let schema =
            PropertySchema::new(name.clone(), prop_id.as_usize() as i32, data_type.clone())
                .nullable(nullable);
        self.name_indexer.register(name.clone())?;
        self.schema.push(schema);
        self.column_store
            .add_column(name.clone(), data_type, nullable);
        self.zone_maps.insert(name, Vec::new());
        Ok(prop_id)
    }

    pub fn remove_property(&mut self, name: &str) -> StorageResult<()> {
        let index = self
            .schema
            .iter()
            .position(|prop| prop.name == name)
            .ok_or_else(|| StorageError::column_not_found(name.to_string()))?;

        self.schema.remove(index);
        self.name_indexer.clear();
        for (idx, schema) in self.schema.iter_mut().enumerate() {
            schema.prop_id = idx as i32;
            self.name_indexer.register(schema.name.clone())?;
        }
        let _ = self.column_store.remove_column(name);
        self.zone_maps.remove(name);

        Ok(())
    }

    pub fn rename_property(&mut self, old_name: &str, new_name: &str) -> StorageResult<()> {
        if self.has_property(new_name) {
            return Err(StorageError::column_already_exists(new_name.to_string()));
        }

        let index = self
            .schema
            .iter()
            .position(|prop| prop.name == old_name)
            .ok_or_else(|| StorageError::column_not_found(old_name.to_string()))?;

        self.schema[index].name = new_name.to_string();

        self.name_indexer.clear();
        for (idx, schema) in self.schema.iter_mut().enumerate() {
            schema.prop_id = idx as i32;
            self.name_indexer.register(schema.name.clone())?;
        }
        let _ = self
            .column_store
            .rename_column(old_name, new_name.to_string());
        if let Some(zm) = self.zone_maps.remove(old_name) {
            self.zone_maps.insert(new_name.to_string(), zm);
        }

        Ok(())
    }

    pub fn insert(
        &mut self,
        values: &[(String, Value)],
        create_ts: Timestamp,
    ) -> StorageResult<u32> {
        let offset = if let Some(free_idx) = self.free_list.pop() {
            let row_idx = (free_idx - 1) as usize;
            self.column_store.clear_row_version_chains(row_idx);
            if row_idx < self.row_create_ts.len() {
                self.row_create_ts[row_idx] = create_ts;
                self.row_delete_ts[row_idx] = None;
            } else {
                self.row_create_ts.resize(row_idx + 1, 0);
                self.row_delete_ts.resize(row_idx + 1, None);
                self.row_create_ts[row_idx] = create_ts;
                self.row_delete_ts[row_idx] = None;
            }
            if self.column_store.row_count() <= row_idx {
                self.column_store.resize(row_idx + 1);
            }
            self.tombstones_manager.remove(free_idx);
            free_idx
        } else {
            let row_idx = self.row_create_ts.len();
            let row_offset = prop_index_to_offset(row_idx);
            self.row_create_ts.push(create_ts);
            self.row_delete_ts.push(None);
            self.row_count += 1;
            if self.column_store.row_count() <= row_idx {
                self.column_store.resize(row_idx + 1);
            }
            row_offset
        };

        let row_idx = prop_offset_to_index(offset).unwrap();
        let schema_snapshot: Vec<(String, bool)> = self
            .schema
            .iter()
            .map(|s| (s.name.clone(), s.nullable))
            .collect();
        for (col_name, _) in &schema_snapshot {
            let val = values.iter().find(|(k, _)| k == col_name).map(|(_, v)| v);
            let _ = self
                .column_store
                .set_property_versioned(row_idx, col_name, val, create_ts);
        }
        self.refresh_zone_map_for_row(row_idx);

        let indexed: Vec<(String, Option<Value>)> = values
            .iter()
            .map(|(k, v)| (k.clone(), Some(v.clone())))
            .collect();
        self.value_index.index_record(&indexed, offset);

        Ok(offset)
    }

    pub fn insert_with_edge_id(
        &mut self,
        edge_id: EdgeId,
        values: &[(String, Value)],
        create_ts: Timestamp,
    ) -> StorageResult<u32> {
        let offset = self.insert(values, create_ts)?;
        if offset != 0 {
            self.edge_prop_map.insert(edge_id, offset);
        }
        Ok(offset)
    }

    pub fn get_offset_by_edge_id(&self, edge_id: EdgeId) -> Option<u32> {
        self.edge_prop_map.get(&edge_id).copied()
    }

    pub fn get_by_edge_id(
        &self,
        edge_id: EdgeId,
        query_ts: Option<Timestamp>,
    ) -> Option<Vec<(String, Option<Value>)>> {
        let offset = *self.edge_prop_map.get(&edge_id)?;
        self.get(offset, query_ts)
    }

    pub fn read_properties_by_edge_id(&self, edge_id: EdgeId) -> Option<Vec<(String, Value)>> {
        let offset = *self.edge_prop_map.get(&edge_id)?;
        self.read_properties(offset)
    }

    pub fn mark_deleted_by_edge_id(&mut self, edge_id: EdgeId, ts: Timestamp) -> StorageResult<()> {
        if let Some(&offset) = self.edge_prop_map.get(&edge_id) {
            self.mark_deleted(offset, ts)?;
        }
        Ok(())
    }

    pub fn delete_by_edge_id(&mut self, edge_id: EdgeId) {
        if let Some(offset) = self.edge_prop_map.remove(&edge_id) {
            self.delete(offset);
        }
    }

    pub fn revert_deletion_by_edge_id(&mut self, edge_id: EdgeId) {
        if let Some(&offset) = self.edge_prop_map.get(&edge_id) {
            self.revert_deletion(offset);
        }
    }

    pub fn update(
        &mut self,
        offset: u32,
        values: &[(String, Value)],
        ts: Timestamp,
    ) -> StorageResult<()> {
        let merged_values = self.get_for_update(offset, values)?;
        let merged: Vec<(String, Option<Value>)> = merged_values
            .into_iter()
            .map(|(name, value)| (name, Some(value)))
            .collect();
        self.write_versioned_row(offset, &merged, ts)
    }

    fn get_for_update(
        &self,
        offset: u32,
        updates: &[(String, Value)],
    ) -> StorageResult<Vec<(String, Value)>> {
        let mut result = Vec::new();

        if let Some(current_props) = self.get(offset, None) {
            for (name, opt_value) in current_props {
                if let Some((_, new_val)) = updates.iter().find(|(k, _)| k == &name) {
                    result.push((name, new_val.clone()));
                } else if let Some(old_val) = opt_value {
                    result.push((name, old_val));
                }
            }

            for (name, val) in updates {
                if !result.iter().any(|(n, _)| n == name) {
                    result.push((name.clone(), val.clone()));
                }
            }
        } else {
            result = updates.to_vec();
        }

        Ok(result)
    }

    pub fn get(
        &self,
        offset: u32,
        query_ts: Option<Timestamp>,
    ) -> Option<Vec<(String, Option<Value>)>> {
        let row_idx = prop_offset_to_index(offset)?;
        if row_idx >= self.row_create_ts.len() {
            return None;
        }
        if !self.is_row_visible(row_idx, offset, query_ts) {
            return None;
        }
        let ts = query_ts.unwrap_or(Timestamp::MAX);
        Some(self.column_store.get_at_ts(row_idx, ts))
    }

    pub(crate) fn is_row_visible(
        &self,
        row_idx: usize,
        offset: u32,
        query_ts: Option<Timestamp>,
    ) -> bool {
        let create_ts = match self.row_create_ts.get(row_idx) {
            Some(&c) => c,
            None => return false,
        };
        if create_ts == 0 {
            return false;
        }
        let ts = query_ts.unwrap_or(Timestamp::MAX);
        if ts < create_ts {
            return false;
        }
        // Check tombstone (row-level deletion)
        if let Some(Some(delete_ts)) = self.row_delete_ts.get(row_idx) {
            if ts >= *delete_ts {
                return false;
            }
        }
        // Also check manager for cases where row_delete_ts not yet synced
        // (production manager check mirrors row_delete_ts)
        let _ = offset;
        true
    }

    pub(crate) fn is_tombstoned_at(&self, offset: u32, ts: Timestamp) -> bool {
        if let Some(row_idx) = prop_offset_to_index(offset) {
            if let Some(Some(delete_ts)) = self.row_delete_ts.get(row_idx) {
                return ts >= *delete_ts;
            }
        }
        false
    }

    pub fn set_property(
        &mut self,
        offset: u32,
        name: &str,
        value: Option<Value>,
        ts: Timestamp,
    ) -> StorageResult<()> {
        let row_idx =
            prop_offset_to_index(offset).ok_or_else(|| StorageError::invalid_offset(offset))?;
        if row_idx >= self.row_create_ts.len() {
            return Err(StorageError::invalid_offset(offset));
        }
        if self.row_create_ts[row_idx] == 0 {
            return Err(StorageError::invalid_offset(offset));
        }
        if !self.has_property(name) {
            return Err(StorageError::column_not_found(name.to_string()));
        }
        self.check_write_conflict(row_idx, offset, ts)?;
        // Per-column back-in-time check: if the column already has a newer version, reject.
        if let Some(col) = self.column_store.get_column(name) {
            if let Some(&start) = col.row_start_ts_vec().get(row_idx) {
                if start != 0 && start > ts {
                    return Err(StorageError::write_write_conflict(format!(
                        "property row at offset {} already has a newer version of '{}' at ts={}, attempted write at ts={}",
                        offset, name, start, ts
                    )));
                }
            }
        }
        if let Some(old_props) = self.get(offset, None) {
            self.value_index.remove_record(&old_props, offset);
        }
        self.column_store
            .set_property_versioned(row_idx, name, value.as_ref(), ts)?;
        self.fold_oldest_versions(row_idx);
        self.refresh_zone_map_for_row(row_idx);
        if let Some(new_props) = self.get(offset, None) {
            self.value_index.index_record(&new_props, offset);
        }
        Ok(())
    }

    pub fn set_property_by_id(
        &mut self,
        offset: u32,
        prop_id: PropertyId,
        value: Option<Value>,
        ts: Timestamp,
    ) -> StorageResult<()> {
        let col_idx = prop_id.as_usize();
        if col_idx >= self.schema.len() {
            return Err(StorageError::column_not_found(format!(
                "prop_id={}",
                prop_id
            )));
        }
        let name = self.schema[col_idx].name.clone();
        self.set_property(offset, &name, value, ts)
    }

    pub fn row_count(&self) -> usize {
        self.row_count
    }

    pub fn has_property(&self, name: &str) -> bool {
        self.name_indexer.contains(name)
    }

    pub fn get_property_id(&self, name: &str) -> Option<crate::types::PropertyId> {
        self.name_indexer.get_id(name)
    }

    pub fn find_by_property(&self, name: &str, value: &Value) -> Vec<u32> {
        self.value_index.lookup(name, Some(value))
    }

    pub fn find_by_property_null(&self, name: &str) -> Vec<u32> {
        self.value_index.lookup(name, None)
    }

    pub fn used_memory_size(&self) -> usize {
        let mut total = std::mem::size_of::<Self>();
        total += self.row_create_ts.capacity() * std::mem::size_of::<Timestamp>();
        total += self.row_delete_ts.capacity() * std::mem::size_of::<Option<Timestamp>>();
        total += self.free_list.capacity() * std::mem::size_of::<u32>();
        total += self.value_index.entry_count()
            * (std::mem::size_of::<Vec<u8>>() + std::mem::size_of::<HashSet<u32>>());
        total += self.edge_prop_map.capacity()
            * (std::mem::size_of::<EdgeId>() + std::mem::size_of::<u32>());
        total += self.column_store.memory_size();
        total += self
            .zone_maps
            .values()
            .map(|chunks| chunks.len() * std::mem::size_of::<ColumnStats>())
            .sum::<usize>();
        total += self.zone_maps.len() * std::mem::size_of::<String>();
        total
    }

    pub fn compaction_stats(&self) -> PropertyCompactionStats {
        let tombstone_count = self.tombstones_manager.len();
        let live_records = self.row_create_ts.len() - self.free_list.len();
        let mut reclaimable_bytes = 0usize;
        for idx in 0..self.row_create_ts.len() {
            if self.row_delete_ts.get(idx).and_then(|v| *v).is_some() {
                // Estimate: per-column payload roughly
                reclaimable_bytes += 32 * self.schema.len();
            }
        }

        PropertyCompactionStats {
            tombstone_count,
            total_records: self.row_create_ts.len(),
            live_records,
            free_list_size: self.free_list.len(),
            reclaimable_bytes,
        }
    }

    /// Check if schema is suitable for fast path operations:
    /// all types are fixed-size (no String, no Date)
    pub fn is_schema_fixed_size(&self) -> bool {
        self.schema.iter().all(|s| {
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

    pub(crate) fn ensure_row_meta(&mut self, n: usize) {
        if self.row_create_ts.len() < n {
            self.row_create_ts.resize(n, 0);
            self.row_delete_ts.resize(n, None);
        }
    }

    pub(crate) fn clear_row_version_chains(&mut self, row_idx: usize) {
        self.column_store.clear_row_version_chains(row_idx);
    }

    pub(crate) fn fold_oldest_versions(&mut self, row_idx: usize) {
        let cap = self.version_chain_cap;
        if cap == 0 {
            return;
        }
        let horizon = self.retention_horizon;
        let needs_fold = self
            .column_store
            .columns()
            .iter()
            .any(|col| col.version_chain_len(row_idx) > cap);
        if !needs_fold {
            return;
        }
        // Delegate folding to each column that exceeds cap and whose oldest
        // entries are before retention horizon.
        for col in self.column_store.columns_mut() {
            col.fold_oldest(row_idx, cap, horizon);
        }
    }
}

impl PropertyTable {
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
}

impl Default for PropertyTable {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod olap_phase1_tests {
    use super::*;
    use graphdb_core::{DataType, Value};

    #[test]
    fn test_columnar_insert_and_projected_read() {
        let mut table = PropertyTable::new();
        table
            .add_property("weight".to_string(), DataType::Double, false)
            .unwrap();
        table
            .add_property("since".to_string(), DataType::Int, true)
            .unwrap();
        table
            .add_property("name".to_string(), DataType::String, true)
            .unwrap();

        let offset = table
            .insert(
                &[
                    ("weight".to_string(), Value::Double(1.5)),
                    ("since".to_string(), Value::Int(2020)),
                    ("name".to_string(), Value::string("alice")),
                ],
                100,
            )
            .unwrap();

        let projected = table
            .get_projected(offset, &["weight".to_string(), "name".to_string()], None)
            .unwrap();
        assert_eq!(projected.len(), 2);
        assert!(projected
            .iter()
            .any(|(n, v)| n == "weight" && v == &Some(Value::Double(1.5))));
        assert!(projected
            .iter()
            .any(|(n, v)| n == "name" && v == &Some(Value::string("alice"))));
        assert!(!projected.iter().any(|(n, _)| n == "since"));

        let offsets = vec![offset];
        let batch = table.get_projected_batch(&offsets, &["weight".to_string()], None);
        assert_eq!(batch.len(), 1);
        assert!(batch[0].is_some());
        let batch_row = batch[0].as_ref().unwrap();
        assert_eq!(batch_row[0].0, "weight");
        assert_eq!(batch_row[0].1, Some(Value::Double(1.5)));
    }

    #[test]
    fn test_zone_maps_prune() {
        let mut table = PropertyTable::new();
        table
            .add_property("age".to_string(), DataType::Int, false)
            .unwrap();
        for i in 0..5 {
            table
                .insert(
                    &[("age".to_string(), Value::Int((i + 1) * 10))],
                    100 + i as u64,
                )
                .unwrap();
        }
        table.rebuild_zone_maps();
        let zm = table.zone_map_for_column("age").unwrap();
        assert!(!zm.is_empty());
        let stats = table.compute_column_stats(0).unwrap();
        assert_eq!(stats.min_value, Some(Value::Int(10)));
        assert_eq!(stats.max_value, Some(Value::Int(50)));

        let mask = table
            .prune_chunks_by_range("age", Some(&Value::Int(40)), None, true, true)
            .unwrap();
        assert!(mask.iter().any(|&keep| keep));

        let mask2 = table
            .prune_chunks_by_range("age", Some(&Value::Int(100)), None, false, true)
            .unwrap();
        assert_eq!(mask2, vec![false]);
    }

    #[test]
    fn test_column_encoding() {
        let mut table = PropertyTable::new();
        table
            .add_property("status".to_string(), DataType::Int, false)
            .unwrap();
        for i in 0..20 {
            table
                .insert(&[("status".to_string(), Value::Int(i % 3))], 100)
                .unwrap();
        }
        let res = table.apply_column_encoding("status", EncodingType::Rle);
        assert!(res.is_ok());
    }

    #[test]
    fn test_dump_load_preserves_columnar_and_zone_maps() {
        let mut table = PropertyTable::new();
        table
            .add_property("weight".to_string(), DataType::Double, false)
            .unwrap();
        let offset = table
            .insert(&[("weight".to_string(), Value::Double(2.5))], 100)
            .unwrap();
        table.rebuild_zone_maps();
        let data = table.dump();
        let mut loaded = PropertyTable::new();
        loaded.load(&data).unwrap();
        assert_eq!(
            loaded.get(offset, None).unwrap()[0].1,
            Some(Value::Double(2.5))
        );
        assert!(loaded.zone_map_for_column("weight").is_some());
        let proj = loaded
            .get_projected(offset, &["weight".to_string()], None)
            .unwrap();
        assert_eq!(proj[0].1, Some(Value::Double(2.5)));
    }

    #[test]
    fn test_legacy_version_is_rejected() {
        let mut table = PropertyTable::new();
        table
            .add_property("x".to_string(), DataType::Int, false)
            .unwrap();
        let _offset = table
            .insert(&[("x".to_string(), Value::Int(42))], 10)
            .unwrap();
        let data = table.dump();

        const CHECKSUM_POS: usize = crate::persistence::HEADER_SIZE;
        const VERSION_POS: usize = CHECKSUM_POS + 4;
        let mut legacy = data.clone();
        legacy[VERSION_POS] = 4;
        let computed = crc32fast::hash(&legacy[CHECKSUM_POS + 4..]);
        legacy[CHECKSUM_POS..CHECKSUM_POS + 4].copy_from_slice(&computed.to_le_bytes());
        let mut loaded = PropertyTable::new();
        let err = loaded.load(&legacy).unwrap_err();
        assert!(err.to_string().contains("Unsupported PropertyTable version"));

        let mut current = PropertyTable::new();
        current.load(&data).unwrap();
    }
}
