//! Property Table for Edges
//!
//! Column-oriented MVCC storage for edge properties.
//!
//! # Design Rationale (Columnar)
//!
//! The earlier row-oriented layout stored edge properties as whole-row blobs.
//! For OLAP workloads (full scans, multi-hop aggregates, GROUP BY) this forces
//! reading all columns even when only 1-2 are needed, causing 5-10x IO waste.
//! The columnar format mirrors vertex
//! `ColumnStore` (one `Column` per property, independent compression, zero-copy
//! scans, column pruning, predicate pushdown).
//!
//! Each property column is stored independently via `ColumnStore`, supporting:
//! - ALP / bit-packing / dictionary / FSST / RLE per-column compression
//! - Zone maps (per-chunk min/max/ndv) for predicate pruning
//! - MVCC per-cell version chains (`Column::set_versioned` / `get_at_ts`) for
//!   lock-free snapshot reads, coordinated with row-level tombstones for
//!   time-travel queries
//! - Batch / vectorized scans (`get_batch`, `get_projected`, `get_column_values`)
//!
//! ## MVCC Strategy
//!
//! PropertyTable implements record-level MVCC (create_ts/delete_ts) rather than
//! relying on external versioning like VertexTable. This allows:
//! - Independent version tracking without re-scanning CSR structure
//! - Delayed garbage collection via TieredTombstoneManager
//! - Time-travel queries on edge properties
//!
//! Each property record includes create_ts and delete_ts for version tracking,
//! enabling time-travel queries and garbage collection of expired versions.
//!
//! Column-level history is additionally tracked in `ColumnStore::Column`
//! version chains, so `get_at_ts` on a single column does not deserialize the
//! whole row.
//!
//! ## Performance Optimizations
//!
//! Columnar storage enables OLAP optimizations:
//! - `get_projected` / `get_batch_projected`: column pruning (read only needed columns)
//! - `get_fast()`: Skip null checks for fixed-size schemas (2-3x speedup)
//! - `set_property_fixed_size()`: Direct byte manipulation avoids full serialize cycle
//! - `column_byte_offsets`: Precomputed for O(1) column lookup (legacy row path)
//! - `prefetch_batch()`: CPU cache locality for bulk reads
//! - `get_batch()`: Sorted access pattern for sequential cache hits
//! - Zone maps per 1024-row chunk: prune chunks via min/max before scanning

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
    data_type_from_info, DataType, DateValue, StorageError, StorageResult, TypeCodecError,
    TypeInfo, Value,
};

// Internal submodules: each holds one `impl PropertyTable` block grouped by
// responsibility. They are descendants of this module, so they can access the
// private fields and helpers declared here.
mod columnar;
mod mvcc;
mod serialization;
mod tombstone;
mod zone_map;

/// Current on-disk layout version: columnar (ColumnStore per property) +
/// zone maps + per-column encodings. The legacy row-oriented layout (v3) is
/// no longer readable; files in that format must be re-imported.
const PROPERTY_TABLE_VERSION: u8 = 4;

/// Rows per zone-map chunk. Zone maps store min/max/ndv/null_count per chunk
/// for predicate pushdown (skip chunks whose zone cannot contain the predicate).
pub const ZONE_MAP_CHUNK_SIZE: usize = 1024;

pub use super::property_schema::{
    prop_index_to_offset, prop_offset_to_index, PropertyCompactionStats, PropertyRecord,
    PropertySchema,
};

/// A single projected row: optional list of `(column_name, optional_value)` pairs.
type ProjectedRow = Option<Vec<(String, Option<Value>)>>;

/// Property value index for fast edge lookup by property value.
///
/// Maps (property_name → canonical_value_bytes → set of property offsets).
/// Enables O(1) lookups of edges by property value without scanning the
/// entire property table. Maintained incrementally during insert/update/delete.
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

    /// Insert a property value for a given offset.
    pub fn insert(&mut self, name: &str, value: Option<&Value>, offset: u32) {
        let entry = self.index.entry(name.to_string()).or_default();
        let key = encode_value_for_index(value);
        entry.entry(key).or_default().insert(offset);
    }

    /// Index all property values for a record at the given offset.
    pub fn index_record(&mut self, props: &[(String, Option<Value>)], offset: u32) {
        for (name, val) in props {
            self.insert(name, val.as_ref(), offset);
        }
    }

    /// Remove a property value for a given offset.
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

    /// Remove all indexed values for a given offset.
    pub fn remove_record(&mut self, props: &[(String, Option<Value>)], offset: u32) {
        for (name, val) in props {
            self.remove(name, val.as_ref(), offset);
        }
    }

    /// Find all property offsets that have the given property value.
    pub fn lookup(&self, name: &str, value: Option<&Value>) -> Vec<u32> {
        let key = encode_value_for_index(value);
        self.index
            .get(name)
            .and_then(|entry| entry.get(&key))
            .map(|offsets| offsets.iter().copied().collect())
            .unwrap_or_default()
    }

    /// Clear all index entries.
    pub fn clear(&mut self) {
        self.index.clear();
    }

    /// Number of distinct (property_name, value) pairs indexed.
    pub fn entry_count(&self) -> usize {
        self.index.values().map(|m| m.len()).sum()
    }

    /// Rebuild index from a list of records.
    pub fn rebuild(&mut self, schema: &[PropertySchema], records: &[Option<PropertyRecord>]) {
        self.clear();
        for (row_idx, record_opt) in records.iter().enumerate() {
            let Some(record) = record_opt else { continue };
            if record.delete_ts.is_some() {
                continue;
            }
            let offset = prop_index_to_offset(row_idx);
            let props = deserialize_row_raw(schema, &record.data);
            for (name, val) in props {
                self.insert(&name, val.as_ref(), offset);
            }
        }
    }
}

/// Encode a Value into a canonical byte key for index lookups.
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

/// Deserialize a row from raw bytes, used for index rebuilding.
fn deserialize_row_raw(schema: &[PropertySchema], data: &[u8]) -> Vec<(String, Option<Value>)> {
    let mut cursor = Cursor::new(data);
    let mut result = Vec::new();
    for schema_entry in schema {
        let mut null_marker = [0u8; 1];
        if cursor.read_exact(&mut null_marker).is_err() {
            result.push((schema_entry.name.clone(), None));
            continue;
        }
        if null_marker[0] == 0 {
            result.push((schema_entry.name.clone(), None));
        } else {
            let value = deserialize_value_from_cursor(&mut cursor, &schema_entry.data_type);
            result.push((schema_entry.name.clone(), value));
        }
    }
    result
}

fn deserialize_value_from_cursor(
    cursor: &mut Cursor<&[u8]>,
    data_type: &DataType,
) -> Option<Value> {
    match data_type {
        DataType::Bool => {
            let mut b = [0u8; 1];
            cursor.read_exact(&mut b).ok()?;
            Some(Value::Bool(b[0] != 0))
        }
        DataType::SmallInt => {
            let mut buf = [0u8; 2];
            cursor.read_exact(&mut buf).ok()?;
            Some(Value::SmallInt(i16::from_le_bytes(buf)))
        }
        DataType::Int => {
            let mut buf = [0u8; 4];
            cursor.read_exact(&mut buf).ok()?;
            Some(Value::Int(i32::from_le_bytes(buf)))
        }
        DataType::BigInt => {
            let mut buf = [0u8; 8];
            cursor.read_exact(&mut buf).ok()?;
            Some(Value::BigInt(i64::from_le_bytes(buf)))
        }
        DataType::Float => {
            let mut buf = [0u8; 4];
            cursor.read_exact(&mut buf).ok()?;
            Some(Value::Float(f32::from_le_bytes(buf)))
        }
        DataType::Double => {
            let mut buf = [0u8; 8];
            cursor.read_exact(&mut buf).ok()?;
            Some(Value::Double(f64::from_le_bytes(buf)))
        }
        DataType::String => {
            let len = decode_varint(cursor).unwrap_or(0) as usize;
            let mut str_buf = vec![0u8; len];
            cursor.read_exact(&mut str_buf).ok()?;
            Some(Value::string(String::from_utf8_lossy(&str_buf)))
        }
        DataType::Date => {
            let mut buf = [0u8; 10];
            cursor.read_exact(&mut buf[..4]).ok()?;
            let year = i32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
            cursor.read_exact(&mut buf[..4]).ok()?;
            let month = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
            cursor.read_exact(&mut buf[..4]).ok()?;
            let day = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
            Some(Value::Date(DateValue { year, month, day }))
        }
        _ => None,
    }
}

// Varint encoding for compact string lengths
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
    records: Vec<Option<PropertyRecord>>, // row_index → current (newest) PropertyRecord with timestamps
    /// Before-image version chain per row, oldest first.
    ///
    /// Each entry is an older version of the row's property data, superseded
    /// by the current record. Entries are appended in supersede order, so the
    /// first entry is the oldest surviving version. The version is visible on
    /// `[create_ts, delete_ts)`, and its `delete_ts` equals the timestamp at
    /// which the successor version took over. `get_at_ts` resolves snapshot
    /// reads by scanning the current record first, then the chain; obsolete
    /// entries are reclaimed by [`PropertyTable::gc_versions`].
    chain_records: Vec<Vec<PropertyRecord>>,
    row_count: usize,
    free_list: Vec<u32>,

    // Tiered tombstone manager for efficient deletion tracking (hot/cold layers)
    tombstones_manager: TieredTombstoneManager<u32>,

    /// Pre-computed byte offsets for each column in the serialized row format.
    /// Only meaningful for fixed-size schemas. Used for direct byte manipulation
    /// in set_property to avoid full deserialize-merge-serialize cycle.
    column_byte_offsets: Vec<usize>,

    /// Property value index for fast edge lookup by property value.
    /// Maps (property_name → encoded_value → set of offsets).
    value_index: PropertyValueIndex,

    /// Edge ID to property offset mapping.
    /// Enables topology-properties separation: CSR entries store only
    /// neighbor + edge_id, and properties are looked up by edge_id.
    edge_prop_map: HashMap<EdgeId, u32>,

    /// O(1) sum of live record payload bytes, maintained incrementally on
    /// insert/update/delete so `used_memory_size` does not scan all records.
    used_data_bytes: usize,

    /// Upper bound on the before-image version chain length per row.
    ///
    /// When a row is updated more than this many times, the oldest before-images
    /// are folded together (interval-merged) so memory stays bounded. This
    /// trades unbounded historical precision for a bounded chain: the most
    /// recent `cap` versions remain exact, older history is coarsened into a
    /// single representative interval.
    version_chain_cap: usize,

    /// Lower bound of timestamps still observable by active snapshots.
    ///
    /// Folding must not destroy versions inside `[retention_horizon, +∞)`:
    /// an entry whose visibility interval ends at or after this bound may
    /// still be observed by some active snapshot. Defaults to `Timestamp::MAX`
    /// ("nothing pinned"), which keeps folding fully aggressive; the edge
    /// store refreshes it whenever the set of active snapshots changes.
    retention_horizon: Timestamp,

    // ── OLAP: columnar store + zone maps + fine-grained concurrency ──
    /// Columnar storage for OLAP scans: one `Column` per property, independent
    /// compression (ALP / bitpacking / dictionary / FSST / RLE), column pruning,
    /// and vectorized batch reads. Dual-written with `records`; `load`
    /// rebuilds it from the persisted row data.
    column_store: ColumnStore,

    /// Per-column zone maps (one `ColumnStats` per `ZONE_MAP_CHUNK_SIZE` rows)
    /// for predicate pushdown / segment pruning, persisted with the table.
    /// Maps `column_name → Vec<chunk_stats>`.
    zone_maps: HashMap<String, Vec<ColumnStats>>,
}

/// Default upper bound on the per-row before-image version chain length.
pub const DEFAULT_VERSION_CHAIN_CAP: usize = 64;

impl Clone for PropertyTable {
    fn clone(&self) -> Self {
        Self {
            schema: self.schema.clone(),
            name_indexer: self.name_indexer.clone(),
            records: self.records.clone(),
            chain_records: self.chain_records.clone(),
            row_count: self.row_count,
            free_list: self.free_list.clone(),
            tombstones_manager: self.tombstones_manager.clone(),
            column_byte_offsets: self.column_byte_offsets.clone(),
            value_index: self.value_index.clone(),
            edge_prop_map: self.edge_prop_map.clone(),
            used_data_bytes: self.used_data_bytes,
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
            records: Vec::new(),
            chain_records: Vec::new(),
            row_count: 0,
            free_list: Vec::new(),
            tombstones_manager: TieredTombstoneManager::new(10_000),
            column_byte_offsets: Vec::new(),
            value_index: PropertyValueIndex::new(),
            edge_prop_map: HashMap::new(),
            used_data_bytes: 0,
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
            records: Vec::with_capacity(capacity),
            chain_records: Vec::with_capacity(capacity),
            row_count: 0,
            free_list: Vec::with_capacity(capacity / 10),
            tombstones_manager: TieredTombstoneManager::new(10_000),
            column_byte_offsets: Vec::new(),
            value_index: PropertyValueIndex::new(),
            edge_prop_map: HashMap::with_capacity(capacity),
            used_data_bytes: 0,
            version_chain_cap: DEFAULT_VERSION_CHAIN_CAP,
            retention_horizon: Timestamp::MAX,
            column_store: ColumnStore::with_capacity(capacity),
            zone_maps: HashMap::new(),
        }
    }

    /// Set the upper bound on the per-row before-image version chain length.
    ///
    /// A value of `0` disables the bound (unbounded history). When the chain
    /// exceeds the bound, the oldest before-images are folded together; see
    /// [`PropertyTable::fold_oldest_versions`].
    pub fn set_version_chain_cap(&mut self, cap: usize) {
        self.version_chain_cap = cap;
    }

    /// Refresh the oldest timestamp still observable by active snapshots.
    ///
    /// Called by the edge store whenever the active-snapshot set changes.
    /// Version-chain folding must preserve every entry whose visibility
    /// interval reaches into `[retention_horizon, +∞)`.
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
        self.recompute_column_byte_offsets();
        // Columnar store: one Column per property, mirrors vertex ColumnStore.
        self.column_store
            .add_column(name.clone(), data_type, nullable);
        // Invalidate zone map for new column (empty).
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
        self.recompute_column_byte_offsets();
        // Keep column store in sync: drop the column.
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
        self.recompute_column_byte_offsets();
        // Rename in column store and zone maps.
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
        let record_data = self.serialize_row(values)?;

        let record = PropertyRecord::new(record_data.clone(), create_ts);
        self.used_data_bytes += record.data.len();

        let offset = if let Some(free_idx) = self.free_list.pop() {
            let row_idx = (free_idx - 1) as usize;
            self.records[row_idx] = Some(record);
            // A reused slot starts a fresh version chain: any surviving
            // before-images were already invisible to every active snapshot.
            self.chain_records[row_idx].clear();
            // Columnar: clear per-cell version chains for reused row.
            // ColumnStore columns reuse the same row index; versioned write
            // below will push a before-image only if prior start_ts < create_ts,
            // which is correct for recycled slots (prior data was deleted).
            free_idx
        } else {
            let row_idx = self.records.len();
            let row_offset = prop_index_to_offset(row_idx);
            self.records.push(Some(record));
            self.chain_records.push(Vec::new());
            self.row_count += 1;
            row_offset
        };

        let row_idx = prop_offset_to_index(offset).unwrap();
        // Columnar dual-write: mirror values into ColumnStore with MVCC.
        // Ensure ColumnStore has enough rows.
        if self.column_store.row_count() <= row_idx {
            self.column_store.resize(row_idx + 1);
        }
        // Write each property column; missing columns are set to null.
        // Clone schema names to avoid borrow conflict with mutable column_store.
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
        // Update zone maps for affected chunk (best-effort; rebuilt on flush if needed).
        self.refresh_zone_map_for_row(row_idx);

        // Index property values for fast lookup
        let indexed: Vec<(String, Option<Value>)> = values
            .iter()
            .map(|(k, v)| (k.clone(), Some(v.clone())))
            .collect();
        self.value_index.index_record(&indexed, offset);

        Ok(offset)
    }

    /// Insert properties for an edge, mapping edge_id to the property offset.
    ///
    /// This is the primary entry point for topology-properties separation:
    /// the CSR stores only (neighbor, edge_id), and properties are looked up
    /// by edge_id via [`Self::get_offset_by_edge_id`].
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

    /// Get the property offset for an edge by its edge_id.
    ///
    /// Returns `None` if the edge has no properties or is not mapped.
    pub fn get_offset_by_edge_id(&self, edge_id: EdgeId) -> Option<u32> {
        self.edge_prop_map.get(&edge_id).copied()
    }

    /// Get properties for an edge by edge_id.
    ///
    /// Combines edge_id → prop_offset lookup with property retrieval.
    pub fn get_by_edge_id(
        &self,
        edge_id: EdgeId,
        query_ts: Option<Timestamp>,
    ) -> Option<Vec<(String, Option<Value>)>> {
        let offset = *self.edge_prop_map.get(&edge_id)?;
        self.get(offset, query_ts)
    }

    /// Get properties for an edge by edge_id, returning only non-null values.
    pub fn read_properties_by_edge_id(&self, edge_id: EdgeId) -> Option<Vec<(String, Value)>> {
        let offset = *self.edge_prop_map.get(&edge_id)?;
        self.read_properties(offset)
    }

    /// Mark properties as deleted by edge_id.
    pub fn mark_deleted_by_edge_id(&mut self, edge_id: EdgeId, ts: Timestamp) -> StorageResult<()> {
        if let Some(&offset) = self.edge_prop_map.get(&edge_id) {
            self.mark_deleted(offset, ts)?;
        }
        Ok(())
    }

    /// Delete properties by edge_id (physical removal).
    pub fn delete_by_edge_id(&mut self, edge_id: EdgeId) {
        if let Some(offset) = self.edge_prop_map.remove(&edge_id) {
            self.delete(offset);
        }
    }

    /// Revert property deletion by edge_id.
    pub fn revert_deletion_by_edge_id(&mut self, edge_id: EdgeId) {
        if let Some(&offset) = self.edge_prop_map.get(&edge_id) {
            self.revert_deletion(offset);
        }
    }

    /// Update one or more properties of a row in place, creating a new
    /// version at `ts`. The row keeps its offset: the previous record becomes
    /// a before-image in the version chain (subject to snapshot-aware
    /// folding), so external references (CSR `prop_offset` pointers) remain
    /// valid across updates.
    ///
    /// Column names not present in the schema are ignored by row
    /// serialization; use `add_property` first to extend the schema.
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

            // Add any new properties from updates that weren't in current
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
        if row_idx >= self.records.len() {
            return None;
        }

        let record = match query_ts {
            // Current version: only the newest live record is visible.
            None => {
                let rec = self.records[row_idx].as_ref()?;
                if rec.delete_ts.is_some() {
                    return None;
                }
                rec
            }
            // Time-travel query: newest record covering `query_ts` wins,
            // otherwise fall back to the before-image version chain.
            Some(ts) => {
                if let Some(rec) = self.records[row_idx].as_ref() {
                    if rec.is_visible_at(ts) {
                        return self.deserialize_row(&rec.data).ok();
                    }
                }
                let record = self
                    .chain_records
                    .get(row_idx)?
                    .iter()
                    .find(|record| record.is_visible_at(ts))?;
                return self.deserialize_row(&record.data).ok();
            }
        };

        self.deserialize_row(&record.data).ok()
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
        if row_idx >= self.records.len() {
            return Err(StorageError::invalid_offset(offset));
        }

        if !self.has_property(name) {
            return Err(StorageError::column_not_found(name.to_string()));
        }

        // Fast path: for fixed-size schemas, do direct byte manipulation
        let col_idx = self
            .schema
            .iter()
            .position(|p| p.name == name)
            .ok_or_else(|| StorageError::column_not_found(name.to_string()))?;

        if self.is_schema_fixed_size() && col_idx < self.column_byte_offsets.len() {
            // Remove old value from index before updating
            if let Some(ref props) = self.get(offset, None) {
                self.value_index.remove_record(props, offset);
            }
            let result = self.set_property_fixed_size(row_idx, offset, col_idx, value.clone(), ts);
            // Columnar sync: also version the column in ColumnStore.
            let _ = self
                .column_store
                .set_property_versioned(row_idx, name, value.as_ref(), ts);
            self.refresh_zone_map_for_row(row_idx);
            // Re-index with new value
            if let Some(new_props) = self.get(offset, None) {
                self.value_index.index_record(&new_props, offset);
            }
            return result;
        }

        // Slow path: full deserialize → merge → serialize cycle via the
        // shared in-place versioned-write helper. Untouched columns must be
        // carried over: row serialization writes NULL for absent names.
        let old_props = self.get(offset, None);
        let mut merged_values: Vec<(String, Option<Value>)> = Vec::new();
        match old_props {
            Some(props) => {
                for (n, v) in props {
                    if n == name {
                        merged_values.push((n, value.clone()));
                    } else {
                        merged_values.push((n, v));
                    }
                }
            }
            None => merged_values.push((name.to_string(), value)),
        }
        self.write_versioned_row(offset, &merged_values, ts)
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

        // Direct path: bypass set_property's linear name lookup
        let row_idx = match prop_offset_to_index(offset) {
            Some(idx) => idx,
            None => return Err(StorageError::invalid_offset(offset)),
        };
        if row_idx >= self.records.len() {
            return Err(StorageError::invalid_offset(offset));
        }

        if self.is_schema_fixed_size() && col_idx < self.column_byte_offsets.len() {
            return self.set_property_fixed_size(row_idx, offset, col_idx, value, ts);
        }

        self.set_property(offset, &self.schema[col_idx].name.clone(), value, ts)
    }

    pub fn row_count(&self) -> usize {
        self.row_count
    }

    pub fn has_property(&self, name: &str) -> bool {
        self.name_indexer.contains(name)
    }

    /// Get PropertyId by name
    pub fn get_property_id(&self, name: &str) -> Option<crate::types::PropertyId> {
        self.name_indexer.get_id(name)
    }

    /// Find property offsets by exact property value match.
    ///
    /// Returns all property offsets (row handles) whose record has the given
    /// property set to the given value. This enables fast edge property-based
    /// lookups without scanning the entire property table.
    ///
    /// Uses the in-memory `PropertyValueIndex` for O(1) lookup.
    pub fn find_by_property(&self, name: &str, value: &Value) -> Vec<u32> {
        self.value_index.lookup(name, Some(value))
    }

    /// Find property offsets where the given property is null.
    pub fn find_by_property_null(&self, name: &str) -> Vec<u32> {
        self.value_index.lookup(name, None)
    }

    pub fn used_memory_size(&self) -> usize {
        let mut total = self.used_data_bytes;
        total += self.records.len() * std::mem::size_of::<Option<PropertyRecord>>();
        // Version chain overhead: entry slots plus a small header per entry.
        total += self.chain_records.capacity() * std::mem::size_of::<Vec<PropertyRecord>>();
        for chain in &self.chain_records {
            total += chain.capacity() * std::mem::size_of::<PropertyRecord>();
        }
        total += std::mem::size_of::<Self>();
        total += self.value_index.entry_count()
            * (std::mem::size_of::<Vec<u8>>() + std::mem::size_of::<HashSet<u32>>());
        // Edge-to-property offset mapping.
        total += self.edge_prop_map.capacity()
            * (std::mem::size_of::<EdgeId>() + std::mem::size_of::<u32>());
        // Columnar store + zone maps.
        total += self.column_store.memory_size();
        total += self
            .zone_maps
            .values()
            .map(|chunks| chunks.len() * std::mem::size_of::<ColumnStats>())
            .sum::<usize>();
        total += self.zone_maps.len() * std::mem::size_of::<String>();
        total
    }

    /// Calculate compaction statistics for the property table
    pub fn compaction_stats(&self) -> PropertyCompactionStats {
        let tombstone_count = self.tombstones_manager.len();
        let live_records = self.records.iter().filter(|r| r.is_some()).count();

        // Estimate reclaimable bytes from tombstoned records
        let mut reclaimable_bytes = 0usize;
        for idx in 0..self.records.len() {
            if let Some(record) = &self.records[idx] {
                if record.delete_ts.is_some() {
                    reclaimable_bytes += record.data.len() + std::mem::size_of::<PropertyRecord>();
                }
            }
        }

        PropertyCompactionStats {
            tombstone_count,
            total_records: self.records.len(),
            live_records,
            free_list_size: self.free_list.len(),
            reclaimable_bytes,
        }
    }

    /// Get all live records (non-deleted) with their current offsets
    pub fn filter_live_records(&self) -> Vec<(u32, PropertyRecord)> {
        self.records
            .iter()
            .enumerate()
            .filter_map(|(idx, record_opt)| {
                record_opt.as_ref().map(|record| {
                    let offset = prop_index_to_offset(idx);
                    (offset, record.clone())
                })
            })
            .collect()
    }

    /// Recompute column byte offsets for fixed-size schemas.
    /// Each column occupies: 1 byte (null marker) + N bytes (value data).
    /// Called after any schema change.
    fn recompute_column_byte_offsets(&mut self) {
        self.column_byte_offsets.clear();
        if !self.is_schema_fixed_size() {
            return;
        }
        let mut offset = 0usize;
        for col in &self.schema {
            self.column_byte_offsets.push(offset);
            // null marker (1) + value size
            if let Some(sz) = Self::data_type_byte_size(&col.data_type) {
                offset += 1 + sz;
            }
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
}

impl PropertyTable {
    pub fn read_properties(&self, offset: u32) -> Option<Vec<(String, Value)>> {
        let row_idx = prop_offset_to_index(offset)?;
        if row_idx >= self.records.len() {
            return None;
        }
        let record = self.records[row_idx].as_ref()?;
        let props = self.deserialize_row(&record.data).ok()?;
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

        // Column pruning: read only weight and name, not since.
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

        // Batch projected read.
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
        // Insert enough rows to span multiple chunks (ZONE_MAP_CHUNK_SIZE = 1024)
        // Use small chunk for test by manually rebuilding with chunk size logic:
        // Insert 5 rows with ages 10,20,30,40,50
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
        // Global stats should reflect min 10, max 50.
        let stats = table.compute_column_stats(0).unwrap();
        assert_eq!(stats.min_value, Some(Value::Int(10)));
        assert_eq!(stats.max_value, Some(Value::Int(50)));

        // Predicate pruning: age >= 40 should keep chunk with max 50, prune others if chunked.
        // With only one chunk (5 rows < 1024), all chunks kept.
        let mask = table
            .prune_chunks_by_range("age", Some(&Value::Int(40)), None, true, true)
            .unwrap();
        assert!(mask.iter().any(|&keep| keep));

        // Range that excludes all: age > 100
        let mask2 = table
            .prune_chunks_by_range("age", Some(&Value::Int(100)), None, false, true)
            .unwrap();
        // With single chunk covering [10,50], max 50 < 100, so chunk pruned.
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
        // Apply RLE encoding (good for repetitive values)
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
        // Zone maps survive roundtrip
        assert!(loaded.zone_map_for_column("weight").is_some());
        // Projected read after reload
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

        // Layout: header (12 bytes) + checksum (4) + version byte. Mutate the
        // version byte to a legacy value, fix up the checksum so it does not
        // mask the version check, and assert loading fails with an explicit
        // re-import hint instead of silently best-effort reading.
        const CHECKSUM_POS: usize = crate::persistence::HEADER_SIZE;
        const VERSION_POS: usize = CHECKSUM_POS + 4;
        let mut legacy = data.clone();
        legacy[VERSION_POS] = 3;
        let computed = crc32fast::hash(&legacy[CHECKSUM_POS + 4..]);
        legacy[CHECKSUM_POS..CHECKSUM_POS + 4].copy_from_slice(&computed.to_le_bytes());
        let mut loaded = PropertyTable::new();
        let err = loaded.load(&legacy).unwrap_err();
        assert!(err.to_string().contains("no longer supported"));

        // Current-version data still loads.
        let mut current = PropertyTable::new();
        current.load(&data).unwrap();
    }
}
