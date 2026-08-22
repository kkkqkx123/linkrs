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

use crate::core::types::Timestamp;
use crate::core::{
    data_type_from_info, DataType, DateValue, StorageError, StorageResult, TypeCodecError,
    TypeInfo, Value,
};
use crate::storage::column_stats::{compute_stats, ColumnStats};
use crate::storage::encoding::EncodingType;
use crate::storage::mvcc::TieredTombstoneManager;
use crate::storage::naming::NameIndexer;
use crate::storage::persistence::{read_header, read_u32_le, read_u64_le, section, write_header};
use crate::storage::types::PropertyId;
use crate::storage::vertex::column_store::ColumnStore;

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

    fn serialize_row(&self, values: &[(String, Value)]) -> StorageResult<Vec<u8>> {
        let mut buffer = Vec::new();

        for schema in &self.schema {
            let value = values
                .iter()
                .find(|(k, _)| k == &schema.name)
                .map(|(_, v)| v.clone());

            self.serialize_value(&mut buffer, value.as_ref(), schema)?;
        }

        Ok(buffer)
    }

    fn serialize_row_with_nulls(
        &self,
        values: &[(String, Option<Value>)],
    ) -> StorageResult<Vec<u8>> {
        let mut buffer = Vec::new();

        for schema in &self.schema {
            let value = values
                .iter()
                .find(|(k, _)| k == &schema.name)
                .and_then(|(_, v)| v.clone());

            self.serialize_value(&mut buffer, value.as_ref(), schema)?;
        }

        Ok(buffer)
    }

    fn serialize_value(
        &self,
        buffer: &mut Vec<u8>,
        value: Option<&Value>,
        schema: &PropertySchema,
    ) -> StorageResult<()> {
        match value {
            None => {
                buffer.push(0); // null marker
            }
            Some(val) => {
                buffer.push(1); // not null marker
                match &schema.data_type {
                    DataType::Bool => {
                        if let Value::Bool(b) = val {
                            buffer.push(if *b { 1 } else { 0 });
                        }
                    }
                    DataType::SmallInt => {
                        if let Value::SmallInt(i) = val {
                            buffer.extend_from_slice(&i.to_le_bytes());
                        }
                    }
                    DataType::Int => {
                        if let Value::Int(i) = val {
                            buffer.extend_from_slice(&i.to_le_bytes());
                        }
                    }
                    DataType::BigInt => {
                        if let Value::BigInt(i) = val {
                            buffer.extend_from_slice(&i.to_le_bytes());
                        }
                    }
                    DataType::Float => {
                        if let Value::Float(f) = val {
                            buffer.extend_from_slice(&f.to_le_bytes());
                        }
                    }
                    DataType::Double => {
                        if let Value::Double(d) = val {
                            buffer.extend_from_slice(&d.to_le_bytes());
                        }
                    }
                    DataType::String => {
                        if let Value::String(s) = val {
                            let s_bytes = s.as_bytes();
                            encode_varint(s_bytes.len() as u32, buffer);
                            buffer.extend_from_slice(s_bytes);
                        }
                    }
                    DataType::Date => {
                        if let Value::Date(d) = val {
                            buffer.extend_from_slice(&d.year.to_le_bytes());
                            buffer.extend_from_slice(&d.month.to_le_bytes());
                            buffer.extend_from_slice(&d.day.to_le_bytes());
                        }
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    }

    fn deserialize_row(&self, record: &[u8]) -> StorageResult<Vec<(String, Option<Value>)>> {
        let mut cursor = Cursor::new(record);
        let mut result = Vec::new();

        for schema in &self.schema {
            let mut null_marker = [0u8; 1];
            if cursor.read_exact(&mut null_marker).is_err() {
                result.push((schema.name.clone(), None));
                continue;
            }

            if null_marker[0] == 0 {
                result.push((schema.name.clone(), None));
            } else {
                let value = self.deserialize_value(&mut cursor, &schema.data_type)?;
                result.push((schema.name.clone(), value));
            }
        }

        Ok(result)
    }

    fn deserialize_value(
        &self,
        cursor: &mut Cursor<&[u8]>,
        data_type: &DataType,
    ) -> StorageResult<Option<Value>> {
        match data_type {
            DataType::Bool => {
                let mut b = [0u8; 1];
                cursor.read_exact(&mut b)?;
                Ok(Some(Value::Bool(b[0] != 0)))
            }
            DataType::SmallInt => {
                let mut buf = [0u8; 2];
                cursor.read_exact(&mut buf)?;
                Ok(Some(Value::SmallInt(i16::from_le_bytes(buf))))
            }
            DataType::Int => {
                let mut buf = [0u8; 4];
                cursor.read_exact(&mut buf)?;
                Ok(Some(Value::Int(i32::from_le_bytes(buf))))
            }
            DataType::BigInt => {
                let mut buf = [0u8; 8];
                cursor.read_exact(&mut buf)?;
                Ok(Some(Value::BigInt(i64::from_le_bytes(buf))))
            }
            DataType::Float => {
                let mut buf = [0u8; 4];
                cursor.read_exact(&mut buf)?;
                Ok(Some(Value::Float(f32::from_le_bytes(buf))))
            }
            DataType::Double => {
                let mut buf = [0u8; 8];
                cursor.read_exact(&mut buf)?;
                Ok(Some(Value::Double(f64::from_le_bytes(buf))))
            }
            DataType::String => {
                let len = decode_varint(cursor)? as usize;
                let mut str_buf = vec![0u8; len];
                cursor.read_exact(&mut str_buf)?;
                Ok(Some(Value::string(String::from_utf8_lossy(&str_buf))))
            }
            DataType::Date => {
                let mut buf = [0u8; 10];
                cursor.read_exact(&mut buf[..4])?;
                let year = i32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
                cursor.read_exact(&mut buf[..4])?;
                let month = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
                cursor.read_exact(&mut buf[..4])?;
                let day = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
                Ok(Some(Value::Date(DateValue { year, month, day })))
            }
            _ => Ok(None),
        }
    }

    /// Ensure `chain_records` has one entry per record row.
    fn ensure_chain_len(&mut self) {
        self.chain_records.resize(self.records.len(), Vec::new());
    }

    /// Refresh zone map for the chunk containing `row_idx`.
    /// Recomputes `ColumnStats` for that chunk for every column, using the
    /// columnar store's current values (MVCC current view). Zone maps are
    /// best-effort and fully rebuilt on `rebuild_zone_maps` or flush.
    fn refresh_zone_map_for_row(&mut self, row_idx: usize) {
        let chunk_id = row_idx / ZONE_MAP_CHUNK_SIZE;
        let chunk_start = chunk_id * ZONE_MAP_CHUNK_SIZE;
        let chunk_end = (chunk_start + ZONE_MAP_CHUNK_SIZE).min(self.records.len());
        if chunk_start >= chunk_end {
            return;
        }
        // Collect live rows in chunk.
        let mut live_rows: Vec<usize> = Vec::new();
        for idx in chunk_start..chunk_end {
            if self.records.get(idx).and_then(|r| r.as_ref()).is_some() {
                // Consider only rows not tombstoned at current max timestamp view;
                // for zone map we use current values (not historical).
                if self.records[idx]
                    .as_ref()
                    .is_some_and(|rec| rec.delete_ts.is_none())
                {
                    live_rows.push(idx);
                }
            }
        }
        // For each column, compute stats for this chunk.
        let col_names: Vec<String> = self.schema.iter().map(|s| s.name.clone()).collect();
        for col_name in col_names {
            let col = match self.column_store.get_column(&col_name) {
                Some(c) => c,
                None => continue,
            };
            // Gather values for live rows in chunk.
            let values: Vec<Option<Value>> = live_rows.iter().map(|&r| col.get(r)).collect();
            let raw_size = values.len() as u64
                * crate::storage::vertex::column_store::element_size(&col.data_type).max(1) as u64;
            let stats = compute_stats(&values, col.encoding_type(), raw_size, raw_size);
            let entry = self.zone_maps.entry(col_name.clone()).or_default();
            if entry.len() <= chunk_id {
                entry.resize(chunk_id + 1, ColumnStats::new(EncodingType::None, 0, 0));
            }
            entry[chunk_id] = stats;
        }
    }

    /// Rebuild all zone maps from scratch (used after bulk load).
    pub fn rebuild_zone_maps(&mut self) {
        self.zone_maps.clear();
        let total_chunks = self.records.len().div_ceil(ZONE_MAP_CHUNK_SIZE);
        if total_chunks == 0 {
            return;
        }
        for chunk_id in 0..total_chunks {
            let row_idx = chunk_id * ZONE_MAP_CHUNK_SIZE;
            self.refresh_zone_map_for_row(row_idx);
        }
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

    /// Shared slow path for in-place versioned writes: reject conflicting
    /// writes, supersede the current record into the before-image chain, and
    /// install a new record built from `values` at the same offset. Keeps the
    /// value index, columnar store, and zone maps consistent with the new
    /// version.
    fn write_versioned_row(
        &mut self,
        offset: u32,
        values: &[(String, Option<Value>)],
        ts: Timestamp,
    ) -> StorageResult<()> {
        let row_idx =
            prop_offset_to_index(offset).ok_or_else(|| StorageError::invalid_offset(offset))?;
        if row_idx >= self.records.len() {
            return Err(StorageError::invalid_offset(offset));
        }

        // Storage-layer write-write conflict detection: reject a write whose
        // timestamp would overlap a newer existing version or a tombstoned
        // row, before any side effect on indexes or records.
        self.check_write_conflict(row_idx, offset, ts)?;

        // Remove old values from the index before overwriting the record.
        if let Some(old_props) = self.get(offset, None) {
            self.value_index.remove_record(&old_props, offset);
        }

        let new_record = self.serialize_row_with_nulls(values)?;

        // MVCC: supersede the current version. The old row becomes a
        // before-image (visible on `[create_ts, ts)`) and the new row takes
        // over from `ts` onward, preserving historical snapshots.
        self.supersede_current(row_idx, offset, ts);

        let new_record_obj = PropertyRecord::new(new_record, ts);
        self.used_data_bytes += new_record_obj.data.len();
        self.records[row_idx] = Some(new_record_obj);

        // Columnar sync: version every column named in `values`.
        for (name, value) in values {
            if self.has_property(name) {
                let _ = self
                    .column_store
                    .set_property_versioned(row_idx, name, value.as_ref(), ts);
            }
        }
        self.refresh_zone_map_for_row(row_idx);

        // Re-index with new values
        self.value_index.index_record(values, offset);

        Ok(())
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

    /// Resolve the property row visible at `query_ts` (snapshot read).
    ///
    /// Cheap inspection that mirrors [`PropertyTable::get`]'s visibility
    /// rules without paying for deserialization.
    fn resolve_version(
        &self,
        row_idx: usize,
        query_ts: Option<Timestamp>,
    ) -> Option<&PropertyRecord> {
        let record = match query_ts {
            None => {
                let rec = self.records[row_idx].as_ref()?;
                if rec.delete_ts.is_some() {
                    return None;
                }
                rec
            }
            Some(ts) => {
                if let Some(rec) = self.records[row_idx].as_ref() {
                    if rec.is_visible_at(ts) {
                        return Some(rec);
                    }
                }
                self.chain_records
                    .get(row_idx)?
                    .iter()
                    .find(|record| record.is_visible_at(ts))?
            }
        };
        Some(record)
    }

    /// Serialize a single value into a byte buffer at a given offset.
    /// Used for direct byte manipulation in set_property.
    fn serialize_value_at_offset(
        &self,
        buffer: &mut [u8],
        value: Option<&Value>,
        col_idx: usize,
    ) -> StorageResult<()> {
        let byte_off = self
            .column_byte_offsets
            .get(col_idx)
            .ok_or_else(|| StorageError::column_not_found(format!("col_idx={}", col_idx)))?;

        let dt = &self.schema[col_idx].data_type;
        let val_size = Self::data_type_byte_size(dt).ok_or_else(|| {
            StorageError::not_supported(
                "Variable-size types not supported for direct update".to_string(),
            )
        })?;

        match value {
            None => {
                buffer[*byte_off] = 0; // null marker
                                       // Zero out value bytes (safety, but not strictly required)
                for i in 0..val_size {
                    buffer[*byte_off + 1 + i] = 0;
                }
            }
            Some(val) => {
                buffer[*byte_off] = 1; // not null marker
                let target = &mut buffer[*byte_off + 1..*byte_off + 1 + val_size];
                match dt {
                    DataType::Bool => {
                        if let Value::Bool(b) = val {
                            target[0] = if *b { 1 } else { 0 };
                        }
                    }
                    DataType::SmallInt => {
                        if let Value::SmallInt(i) = val {
                            target.copy_from_slice(&i.to_le_bytes());
                        }
                    }
                    DataType::Int => {
                        if let Value::Int(i) = val {
                            target.copy_from_slice(&i.to_le_bytes());
                        }
                    }
                    DataType::BigInt => {
                        if let Value::BigInt(i) = val {
                            target.copy_from_slice(&i.to_le_bytes());
                        }
                    }
                    DataType::Float => {
                        if let Value::Float(f) = val {
                            target.copy_from_slice(&f.to_le_bytes());
                        }
                    }
                    DataType::Double => {
                        if let Value::Double(d) = val {
                            target.copy_from_slice(&d.to_le_bytes());
                        }
                    }
                    _ => {
                        return Err(StorageError::not_supported(format!(
                            "Unexpected fixed-size type: {:?}",
                            dt
                        )));
                    }
                }
            }
        }
        Ok(())
    }

    /// Supersede the current version of a row in favor of a newer one.
    ///
    /// Marks the current record as tombstoned at `ts` and pushes it into the
    /// before-image chain (visible on `[create_ts, ts)`), mirroring the
    /// vertex `Column::set_versioned` guard: a before-image is only useful
    /// when the current version genuinely predates the write
    /// (`create_ts < ts`). Same-timestamp re-writes (rollback / WAL redo that
    /// reuses the transaction timestamp) and already-deleted rows produce no
    /// observable intermediate state, so they skip the chain entry.
    fn supersede_current(&mut self, row_idx: usize, offset: u32, ts: Timestamp) {
        let should_version = self.records[row_idx]
            .as_ref()
            .is_some_and(|r| r.delete_ts.is_none() && r.create_ts < ts);

        if let Some(record) = &mut self.records[row_idx] {
            if record.delete_ts.is_none() {
                record.delete_ts = Some(ts);
                self.tombstones_manager.add_tombstone(offset, ts);
            }
        }

        if should_version {
            if let Some(record) = self.records[row_idx].as_ref() {
                if self.chain_records.len() <= row_idx {
                    self.chain_records.resize(row_idx + 1, Vec::new());
                }
                self.chain_records[row_idx].push(record.clone());
                // Bound the chain length: fold the oldest before-images once
                // the cap is exceeded so memory stays bounded.
                self.fold_oldest_versions(row_idx);
            }
        }
    }

    /// Bound the before-image chain length for `row_idx` by folding the oldest
    /// entries when the chain exceeds `version_chain_cap`.
    ///
    /// Folding merges the two oldest before-images: the older entry's data is
    /// kept as the representative and its visibility interval `[create_ts,
    /// delete_ts)` is extended to cover the second entry's interval, which is
    /// then dropped. This preserves the original oldest value and the newest
    /// current value while coarsening intermediate history, so the most recent
    /// updates remain exact.
    ///
    /// A cap of `0` disables the bound (unbounded history).
    fn fold_oldest_versions(&mut self, row_idx: usize) {
        let cap = self.version_chain_cap;
        if cap == 0 {
            return;
        }
        let horizon = self.retention_horizon;
        let chain = &mut self.chain_records[row_idx];
        while chain.len() > cap {
            if chain.len() < 2 {
                break;
            }
            // Never fold an entry that an active snapshot may still observe:
            // its visibility interval must end before the retention horizon.
            let can_fold = chain[1]
                .delete_ts
                .is_none_or(|delete_ts| delete_ts <= horizon);
            if !can_fold {
                break;
            }
            // Merge the two oldest entries into one: keep the older data,
            // extend its interval to cover the younger entry.
            let second = chain.remove(1);
            if let Some(end) = second.delete_ts {
                chain[0].delete_ts = Some(end);
            }
            self.used_data_bytes = self.used_data_bytes.saturating_sub(second.data.len());
        }
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

    /// Fast path: update a single property value via direct byte manipulation.
    /// Only applicable for fixed-size schemas where byte offsets are known.
    /// Skips full deserialize → merge → serialize cycle.
    fn set_property_fixed_size(
        &mut self,
        row_idx: usize,
        offset: u32,
        col_idx: usize,
        value: Option<Value>,
        ts: Timestamp,
    ) -> StorageResult<()> {
        // Storage-layer write-write conflict detection (direct callers such as
        // `set_property_by_id` bypass `set_property`'s check).
        self.check_write_conflict(row_idx, offset, ts)?;

        let Some(record) = self.records[row_idx].as_ref() else {
            return Err(StorageError::invalid_offset(offset));
        };

        // Clone the old data and overwrite the target property's bytes
        let mut new_data = record.data.clone();
        self.serialize_value_at_offset(&mut new_data, value.as_ref(), col_idx)?;

        // MVCC: supersede the current version, keeping the prior row as a
        // before-image for snapshot reads.
        self.supersede_current(row_idx, offset, ts);

        // Replace with new record (same position, new data + timestamp)
        let new_record_obj = PropertyRecord::new(new_data, ts);
        self.used_data_bytes += new_record_obj.data.len();
        self.records[row_idx] = Some(new_record_obj);

        // Columnar sync (for direct callers like set_property_by_id).
        if let Some(schema) = self.schema.get(col_idx) {
            let col_name = schema.name.clone();
            let _ =
                self.column_store
                    .set_property_versioned(row_idx, &col_name, value.as_ref(), ts);
            self.refresh_zone_map_for_row(row_idx);
        }

        Ok(())
    }

    /// Reject a write whose timestamp would contradict the row's current
    /// version. This is the storage-layer write-write conflict detection:
    ///
    /// - Writing at `ts` strictly **before** the current version's creation
    ///   time would clobber a newer version without preserving it as history
    ///   (a "back-in-time" write overlapping an existing interval).
    /// - Writing at `ts` strictly **after** the row was marked deleted writes
    ///   to a tombstoned entity.
    ///
    /// Same-timestamp re-writes (rollback / WAL redo that reuse the original
    /// transaction timestamp) and strictly forward writes (the normal
    /// time-travel version chain) pass through unchanged, preserving the
    /// distinction between "concurrent transaction conflict" and "historical
    /// version write".
    fn check_write_conflict(
        &self,
        row_idx: usize,
        offset: u32,
        ts: Timestamp,
    ) -> StorageResult<()> {
        let Some(record) = self.records[row_idx].as_ref() else {
            return Ok(());
        };
        if let Some(del_ts) = record.delete_ts {
            if ts > del_ts {
                return Err(StorageError::write_write_conflict(format!(
                    "property row at offset {} deleted at ts={}, attempted write at ts={}",
                    offset, del_ts, ts
                )));
            }
        } else if record.create_ts > ts {
            return Err(StorageError::write_write_conflict(format!(
                "property row at offset {} already has a newer version at ts={}, attempted write at ts={}",
                offset, record.create_ts, ts
            )));
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

    /// Mark a property record as deleted for MVCC tracking
    pub fn mark_deleted(&mut self, offset: u32, delete_ts: Timestamp) -> StorageResult<()> {
        let row_idx =
            prop_offset_to_index(offset).ok_or_else(|| StorageError::invalid_offset(offset))?;
        if row_idx >= self.records.len() {
            return Ok(()); // Already deleted or doesn't exist
        }

        // Check deletion state and get old props BEFORE mutable borrow
        let can_delete = self.records[row_idx]
            .as_ref()
            .is_some_and(|r| r.delete_ts.is_none());

        if can_delete {
            // Remove from index before marking as deleted
            if let Some(props) = self.get(offset, None) {
                self.value_index.remove_record(&props, offset);
            }

            if let Some(record) = &mut self.records[row_idx] {
                record.delete_ts = Some(delete_ts);
                self.tombstones_manager.add_tombstone(offset, delete_ts);
            }
            // Columnar: zone map needs refresh; column values stay for time-travel
            // but are excluded from live zone stats.
            self.refresh_zone_map_for_row(row_idx);
            Ok(())
        } else if self.records[row_idx].is_some() {
            Err(StorageError::invalid_operation(
                "record already marked deleted",
            ))
        } else {
            Ok(()) // Idempotent: already deleted
        }
    }

    /// Revert a [`PropertyTable::mark_deleted`]: clear the deletion mark and
    /// drop the tombstone entry so the record is visible again. Used by the
    /// edge delete undo path to restore properties alongside the adjacency.
    ///
    /// Returns true if a deletion mark was actually cleared.
    pub fn revert_deletion(&mut self, offset: u32) -> bool {
        let row_idx = match prop_offset_to_index(offset) {
            Some(idx) => idx,
            None => return false,
        };
        let Some(record) = self.records[row_idx].as_mut() else {
            return false;
        };
        if record.delete_ts.is_none() {
            return false;
        }
        record.delete_ts = None;
        self.tombstones_manager.remove(offset);
        if let Some(props) = self.get(offset, None) {
            self.value_index.index_record(&props, offset);
        }
        self.refresh_zone_map_for_row(row_idx);
        true
    }

    /// Check whether the record at `offset` is currently marked deleted.
    pub fn is_deleted(&self, offset: u32) -> bool {
        let Some(row_idx) = prop_offset_to_index(offset) else {
            return false;
        };
        self.records
            .get(row_idx)
            .and_then(|r| r.as_ref())
            .is_some_and(|r| r.delete_ts.is_some())
    }

    /// Garbage collect before-image chain entries no longer visible to any
    /// active snapshot at `min_active_snapshot_ts`. Returns the number of
    /// entries removed.
    ///
    /// An entry is obsolete when its whole visibility interval `[create_ts,
    /// delete_ts)` precedes the oldest active snapshot, i.e. `delete_ts <=
    /// min_active_snapshot_ts`. The current record is never reclaimed here
    /// (it still owns the row); fully-deleted rows are reclaimed through
    /// [`PropertyTable::gc_tombstones`].
    pub fn gc_versions(&mut self, min_active_snapshot_ts: Timestamp) -> usize {
        // An unbounded horizon (`MAX`) means nothing pins history — but it is
        // not a real timestamp: treating it as one would reclaim every
        // before-image, including history arbitrary-ts time-travel reads may
        // still request. Without a bound (no active snapshot and no retention
        // floor) nothing is reclaimable.
        if min_active_snapshot_ts == Timestamp::MAX {
            return 0;
        }
        let mut removed = 0usize;
        let mut reclaimed_bytes = 0usize;
        for chain in &mut self.chain_records {
            let before_len = chain.len();
            let before_bytes: usize = chain.iter().map(|e| e.data.len()).sum();
            chain.retain(|entry| entry.delete_ts.is_none_or(|d| d > min_active_snapshot_ts));
            let after_bytes: usize = chain.iter().map(|e| e.data.len()).sum();
            removed += before_len - chain.len();
            reclaimed_bytes += before_bytes - after_bytes;
        }
        self.used_data_bytes = self.used_data_bytes.saturating_sub(reclaimed_bytes);
        // Columnar GC: reclaim per-cell version chains as well (side effect only;
        // return value stays row-chain-centric for backward compatibility with
        // existing tests that assert exact counts).
        let _ = self.column_store.gc_versions(min_active_snapshot_ts);
        // Rebuild zone maps for chunks that may have had history reclaimed.
        self.rebuild_zone_maps();
        removed
    }

    /// Garbage collect tombstones older than min_active_snapshot_ts
    pub fn gc_tombstones(&mut self, min_active_snapshot_ts: Timestamp) -> u32 {
        // Incremental batch GC first, then a full GC pass to clean remaining.
        let batch_size = 10_000usize;
        self.tombstones_manager
            .gc_batch(min_active_snapshot_ts, batch_size);
        self.tombstones_manager.gc(min_active_snapshot_ts);

        // Remove records that are fully tombstoned and older than min_active_snapshot_ts
        let mut reclaimed = 0u32;
        let mut indices_to_clear = Vec::new();

        for (idx, record_opt) in self.records.iter().enumerate() {
            if let Some(record) = record_opt {
                if let Some(delete_ts) = record.delete_ts {
                    if delete_ts < min_active_snapshot_ts {
                        let offset = prop_index_to_offset(idx);
                        indices_to_clear.push((idx, offset));
                        reclaimed += 1;
                    }
                }
            }
        }

        for (idx, offset) in &indices_to_clear {
            // Remove from index if still indexed
            if let Some(ref record) = self.records[*idx] {
                let props = deserialize_row_raw(&self.schema, &record.data);
                self.value_index.remove_record(&props, *offset);
            }
        }

        let has_cleared = !indices_to_clear.is_empty();
        for (idx, offset) in indices_to_clear {
            if let Some(record) = &self.records[idx] {
                self.used_data_bytes = self.used_data_bytes.saturating_sub(record.data.len());
            }
            // The row's full version history (before-images included) is no
            // longer visible to any active snapshot.
            if let Some(chain) = self.chain_records.get_mut(idx) {
                for entry in chain.drain(..) {
                    self.used_data_bytes = self.used_data_bytes.saturating_sub(entry.data.len());
                }
            }
            self.records[idx] = None;
            self.free_list.push(offset);
            // Columnar: keep column values for time-travel but exclude from zone maps.
            // Zone maps are refreshed below.
        }

        if has_cleared {
            self.rebuild_zone_maps();
        }

        reclaimed
    }

    /// Legacy delete method for backward compatibility (physical delete)
    pub fn delete(&mut self, offset: u32) -> bool {
        let row_idx = match prop_offset_to_index(offset) {
            Some(idx) => idx,
            None => return false,
        };
        if row_idx >= self.records.len() {
            return false;
        }

        // Remove from index before deleting
        if let Some(props) = self.get(offset, None) {
            self.value_index.remove_record(&props, offset);
        }

        if let Some(record) = &self.records[row_idx] {
            self.used_data_bytes = self.used_data_bytes.saturating_sub(record.data.len());
        }
        // The row is removed wholesale; its version history dies with it.
        if let Some(chain) = self.chain_records.get_mut(row_idx) {
            for entry in chain.drain(..) {
                self.used_data_bytes = self.used_data_bytes.saturating_sub(entry.data.len());
            }
        }
        self.records[row_idx] = None;
        self.free_list.push(offset);
        // Columnar: keep column slot but mark zone map dirty.
        self.refresh_zone_map_for_row(row_idx);
        true
    }

    pub fn row_count(&self) -> usize {
        self.row_count
    }

    pub fn has_property(&self, name: &str) -> bool {
        self.name_indexer.contains(name)
    }

    /// Get PropertyId by name
    pub fn get_property_id(&self, name: &str) -> Option<crate::storage::types::PropertyId> {
        self.name_indexer.get_id(name)
    }

    pub fn column_values(&self, col_idx: usize) -> Vec<Option<Value>> {
        if col_idx >= self.schema.len() {
            return Vec::new();
        }
        let col_name = self.schema[col_idx].name.clone();
        // Prefer columnar store (zero-copy, OLAP path) when available.
        if let Some(col) = self.column_store.get_column(&col_name) {
            let mut values = Vec::with_capacity(self.records.len());
            for row_idx in 0..self.records.len() {
                // Use current view (None = latest) for stats; respects live rows.
                if self.records[row_idx].is_none()
                    || self.records[row_idx]
                        .as_ref()
                        .is_some_and(|r| r.delete_ts.is_some())
                {
                    values.push(None);
                } else {
                    values.push(col.get(row_idx));
                }
            }
            if !values.is_empty() {
                return values;
            }
        }
        // The columnar store is rebuilt on load and dual-written on write,
        // so a missing column here means an internal invariant violation.
        debug_assert!(
            false,
            "column_values: columnar store missing column '{col_name}'"
        );
        Vec::new()
    }

    pub fn compute_column_stats(
        &self,
        col_idx: usize,
    ) -> Option<crate::storage::column_stats::ColumnStats> {
        if col_idx >= self.schema.len() {
            return None;
        }
        let schema = &self.schema[col_idx];
        // Prefer per-column zone map aggregation if available.
        if let Some(zm) = self.zone_maps.get(&schema.name) {
            if !zm.is_empty() {
                // Aggregate zone maps into global stats (min = min(mins), max = max(maxes), etc.)
                let mut agg = ColumnStats::new(EncodingType::None, 0, 0);
                let mut all_values: Vec<Option<Value>> = Vec::new();
                for zs in zm {
                    if let Some(ref v) = zs.min_value {
                        all_values.push(Some(v.clone()));
                    }
                    if let Some(ref v) = zs.max_value {
                        all_values.push(Some(v.clone()));
                    }
                    agg.null_count += zs.null_count;
                    agg.compressed_size += zs.compressed_size;
                    agg.raw_size += zs.raw_size;
                }
                // Recompute global min/max/distinct from chunk stats where possible,
                // else fallback to full column scan.
                if !all_values.is_empty() {
                    agg.min_value = all_values.iter().filter_map(|v| v.as_ref()).min().cloned();
                    agg.max_value = all_values.iter().filter_map(|v| v.as_ref()).max().cloned();
                    // distinct is sum of chunk distincts capped; precise requires scan.
                }
                // If zone maps are incomplete, fall through to full scan.
                if agg.raw_size > 0 {
                    return Some(agg);
                }
            }
        }
        let values = self.column_values(col_idx);
        let raw_size = values.len() as u64
            * crate::storage::vertex::column_store::element_size(&schema.data_type).max(1) as u64;
        // Use column's actual encoding if columnar store has it.
        let enc = self
            .column_store
            .get_column(&schema.name)
            .map(|c| c.encoding_type())
            .unwrap_or(EncodingType::None);
        Some(crate::storage::column_stats::compute_stats(
            &values, enc, raw_size, raw_size,
        ))
    }

    /// Column pruning: read only `projection` columns for one row at `query_ts`.
    /// Returns `None` if the row does not exist or is not visible at `query_ts`.
    pub fn get_projected(
        &self,
        offset: u32,
        projection: &[String],
        query_ts: Option<Timestamp>,
    ) -> Option<Vec<(String, Option<Value>)>> {
        let row_idx = prop_offset_to_index(offset)?;
        if row_idx >= self.records.len() {
            return None;
        }
        let ts = query_ts.unwrap_or(Timestamp::MAX);
        // Check row visibility (current record vs chain).
        let visible = match query_ts {
            None => self.records[row_idx]
                .as_ref()
                .is_some_and(|r| r.delete_ts.is_none()),
            Some(t) => {
                if let Some(rec) = self.records[row_idx].as_ref() {
                    if rec.is_visible_at(t) {
                        true
                    } else {
                        self.chain_records
                            .get(row_idx)
                            .is_some_and(|chain| chain.iter().any(|r| r.is_visible_at(t)))
                    }
                } else {
                    false
                }
            }
        };
        if !visible {
            return None;
        }
        if projection.is_empty() {
            return self.get(offset, query_ts);
        }
        // Columnar path: read only requested columns via ColumnStore (MVCC-aware).
        let mut out = Vec::with_capacity(projection.len());
        for col_name in projection {
            if let Some(col) = self.column_store.get_column(col_name) {
                let val = if query_ts.is_some() {
                    col.get_at_ts(row_idx, ts)
                } else {
                    col.get(row_idx)
                };
                out.push((col_name.clone(), val));
            } else {
                // Column not in column store (legacy); fallback to row decode.
                if let Some(row) = self.get(offset, query_ts) {
                    if let Some((_, v)) = row.into_iter().find(|(n, _)| n == col_name) {
                        out.push((col_name.clone(), v));
                    } else {
                        out.push((col_name.clone(), None));
                    }
                } else {
                    out.push((col_name.clone(), None));
                }
            }
        }
        Some(out)
    }

    /// Batch column pruning: read `projection` columns for many offsets.
    /// Output order matches input order; missing rows yield `None`.
    pub fn get_projected_batch(
        &self,
        offsets: &[u32],
        projection: &[String],
        query_ts: Option<Timestamp>,
    ) -> Vec<ProjectedRow> {
        let ts = query_ts.unwrap_or(Timestamp::MAX);
        // Group by chunk for zone-map pruning opportunity.
        let mut out = Vec::with_capacity(offsets.len());
        for &off in offsets {
            out.push(self.get_projected(off, projection, query_ts));
        }
        // Prefetch hint for columnar path.
        let row_indices: Vec<usize> = offsets
            .iter()
            .filter_map(|o| prop_offset_to_index(*o))
            .collect();
        if !projection.is_empty() && !row_indices.is_empty() {
            // Warm zone-map access for batch.
            let _ = self
                .column_store
                .get_projected_batch_at_ts(&row_indices, projection, ts);
        }
        out
    }

    /// Zone-map predicate pruning: given a column and value range, return a
    /// bitmask per chunk indicating whether the chunk may contain matching rows.
    /// `None` bounds are unbounded. Chunks with no overlap can be skipped.
    pub fn prune_chunks_by_range(
        &self,
        column: &str,
        lower: Option<&Value>,
        upper: Option<&Value>,
        include_lower: bool,
        include_upper: bool,
    ) -> Option<Vec<bool>> {
        let zones = self.zone_maps.get(column)?;
        let mut mask = Vec::with_capacity(zones.len());
        for stats in zones {
            let mut keep = true;
            if let Some(lo) = lower {
                if let Some(ref max) = stats.max_value {
                    let cmp = max.cmp(lo);
                    if cmp == std::cmp::Ordering::Less
                        || (cmp == std::cmp::Ordering::Equal && !include_upper && max == lo)
                    {
                        // Actually need to compare max < lower or max == lower when not inclusive?
                        // Simplified: if max < lower, chunk cannot contain value >= lower.
                        if max < lo {
                            keep = false;
                        } else if !include_lower && max == lo {
                            // max == lower but lower exclusive: still need to check min?
                            // For range pruning we conservatively keep.
                        }
                    }
                    if !keep {
                        // Check lower bound against max.
                        if max < lo || (!include_lower && max == lo) {
                            keep = false;
                        }
                    }
                }
            }
            if keep {
                if let Some(hi) = upper {
                    if let Some(ref min) = stats.min_value {
                        if min > hi || (!include_upper && min == hi) {
                            keep = false;
                        }
                    }
                }
            }
            mask.push(keep);
        }
        Some(mask)
    }

    /// Return zone maps for a column (for ShowStats / optimizer).
    pub fn zone_map_for_column(&self, column: &str) -> Option<&[ColumnStats]> {
        self.zone_maps.get(column).map(|v| v.as_slice())
    }

    /// All zone maps (for persistence / diagnostics).
    pub fn all_zone_maps(&self) -> &HashMap<String, Vec<ColumnStats>> {
        &self.zone_maps
    }

    /// Apply per-column compression encoding (ALP / bitpacking / dict / etc.)
    /// Delegates to `ColumnStore`. OLAP scans benefit from reduced IO.
    pub fn apply_column_encoding(
        &mut self,
        col_name: &str,
        encoding: EncodingType,
    ) -> StorageResult<()> {
        self.column_store
            .apply_encoding_to_column(col_name, encoding, 4096)
    }

    pub fn dump(&self) -> Vec<u8> {
        let mut result = Vec::new();

        write_header(&mut result, section::PROPERTY_TABLE);

        let checksum_pos = result.len();
        result.extend_from_slice(&[0u8; 4]);

        // Version marker (development uses a single on-disk layout).
        result.push(PROPERTY_TABLE_VERSION);

        result.extend_from_slice(&(self.schema.len() as u32).to_le_bytes());
        for prop in &self.schema {
            let name_bytes = prop.name.as_bytes();
            result.extend_from_slice(&(name_bytes.len() as u32).to_le_bytes());
            result.extend_from_slice(name_bytes);
            result.extend_from_slice(&prop.prop_id.to_le_bytes());
            result.push(prop.data_type.as_u8());
            // Parameterized types (List/Map/Set/Struct/Array) carry a
            // postcard-encoded TypeInfo block right after the code byte. Plain
            // codes have no block, keeping the old format byte-compatible for
            // scalar types.
            if prop.data_type.as_u8() >= 64
                || matches!(
                    prop.data_type,
                    DataType::List(_) | DataType::Map(_) | DataType::Set(_)
                )
            {
                let info = match &prop.data_type {
                    DataType::List(e) => TypeInfo::List(e.clone()),
                    DataType::Map(v) => TypeInfo::Map(v.clone()),
                    DataType::Set(e) => TypeInfo::Set(e.clone()),
                    DataType::Struct(s) => TypeInfo::Struct(s.as_ref().clone()),
                    DataType::Array(a) => TypeInfo::Array(a.as_ref().clone()),
                    _ => unreachable!("parameterized data type without TypeInfo"),
                };
                // Infallible for schema-valid input: only an allocation
                // overflow could error, which would abort the process anyway.
                let bytes = postcard::to_allocvec(&info)
                    .expect("TypeInfo encoding cannot fail for schema-valid input");
                result.extend_from_slice(&bytes);
            }
            result.push(if prop.nullable { 1 } else { 0 });
            result.push(prop.encoding_type.to_u8());
        }

        result.extend_from_slice(&(self.records.len() as u32).to_le_bytes());

        // Store each PropertyRecord with timestamps
        for record_opt in &self.records {
            match record_opt {
                Some(record) => {
                    result.push(1); // marker: has data
                    result.extend_from_slice(&record.create_ts.to_le_bytes());
                    if let Some(del_ts) = record.delete_ts {
                        result.push(1); // marker: has delete_ts
                        result.extend_from_slice(&del_ts.to_le_bytes());
                    } else {
                        result.push(0); // marker: no delete_ts
                    }
                    result.extend_from_slice(&(record.data.len() as u32).to_le_bytes());
                    result.extend_from_slice(&record.data);
                }
                None => {
                    result.push(0); // marker: deleted
                }
            }
        }

        // Store per-row before-image version chains (oldest first), matching
        // the record encoding: marker / create_ts / delete_ts marker / data.
        for chain in &self.chain_records {
            result.extend_from_slice(&(chain.len() as u32).to_le_bytes());
            for record in chain {
                result.extend_from_slice(&record.create_ts.to_le_bytes());
                if let Some(del_ts) = record.delete_ts {
                    result.push(1); // marker: has delete_ts
                    result.extend_from_slice(&del_ts.to_le_bytes());
                } else {
                    result.push(0); // marker: no delete_ts
                }
                result.extend_from_slice(&(record.data.len() as u32).to_le_bytes());
                result.extend_from_slice(&record.data);
            }
        }

        // Store tiered tombstones for garbage collection tracking
        result.extend_from_slice(&(self.tombstones_manager.len() as u32).to_le_bytes());

        // Serialize hot layer
        for _idx in 0..self.tombstones_manager.hot_len() {
            // Note: We serialize tombstones in order, hot then cold
            // This is for persistence; reconstruction happens during load
        }

        // Store free list with Varint encoding
        result.extend_from_slice(&(self.free_list.len() as u32).to_le_bytes());
        for &off in &self.free_list {
            encode_varint(off, &mut result);
        }

        // ── zone maps (v4) ──
        // Persist per-column zone maps (chunk stats) for predicate pruning.
        // Columnar data itself is rebuilt from row records on load (dual-write
        // in-memory column store); persisting it separately would duplicate the
        // payload. Zone maps are small and save recompute on restart.
        result.extend_from_slice(&(self.zone_maps.len() as u32).to_le_bytes());
        for (col_name, chunks) in &self.zone_maps {
            let name_bytes = col_name.as_bytes();
            result.extend_from_slice(&(name_bytes.len() as u32).to_le_bytes());
            result.extend_from_slice(name_bytes);
            result.extend_from_slice(&(chunks.len() as u32).to_le_bytes());
            for stats in chunks {
                let mut meta_buf = Vec::new();
                let _ = stats.serialize_meta(&mut meta_buf);
                result.extend_from_slice(&(meta_buf.len() as u32).to_le_bytes());
                result.extend_from_slice(&meta_buf);
            }
        }

        let checksum = crc32fast::hash(&result[checksum_pos + 4..]);
        result[checksum_pos..checksum_pos + 4].copy_from_slice(&checksum.to_le_bytes());

        result
    }

    pub fn load(&mut self, data: &[u8]) -> StorageResult<()> {
        if data.is_empty() {
            return Ok(());
        }

        let mut cursor = data;
        let (_version, section_id) = read_header(&mut cursor)?;
        if section_id != section::PROPERTY_TABLE {
            return Err(StorageError::deserialize_error(format!(
                "invalid section_id for PropertyTable: expected 0x{:04X}, got 0x{:04X}",
                section::PROPERTY_TABLE,
                section_id
            )));
        }

        if cursor.len() < 4 {
            return Err(StorageError::deserialize_error(
                "PropertyTable data too short for checksum",
            ));
        }
        let stored_checksum = u32::from_le_bytes(cursor[..4].try_into().map_err(|_| {
            StorageError::deserialize_error("failed to read PropertyTable checksum")
        })?);
        let payload = &cursor[4..];
        let computed_checksum = crc32fast::hash(payload);
        if stored_checksum != computed_checksum {
            return Err(StorageError::deserialize_error(format!(
                "PropertyTable checksum mismatch: stored {:#x}, computed {:#x}",
                stored_checksum, computed_checksum
            )));
        }

        let data = payload;
        let mut offset = 0usize;

        // Read and validate the version. Development builds keep a single
        // on-disk layout; version bumps only start after a release.
        let version = data.get(offset).copied().ok_or_else(|| {
            StorageError::deserialize_error("PropertyTable data missing version byte")
        })?;
        offset += 1;

        if version != PROPERTY_TABLE_VERSION {
            if version < PROPERTY_TABLE_VERSION {
                return Err(StorageError::deserialize_error(format!(
                    "PropertyTable data uses legacy layout version {version}, which is no \
                     longer supported; re-import the data to upgrade to version {PROPERTY_TABLE_VERSION}"
                )));
            }
            return Err(StorageError::deserialize_error(format!(
                "Unsupported PropertyTable version: expected {PROPERTY_TABLE_VERSION}, got {version}"
            )));
        }

        let schema_len = read_u32_le(data, &mut offset)? as usize;

        self.schema.clear();
        self.name_indexer.clear();
        self.column_byte_offsets.clear();

        for _ in 0..schema_len {
            let name_len = read_u32_le(data, &mut offset)? as usize;
            if offset + name_len > data.len() {
                return Err(StorageError::deserialize_error("unexpected end of data"));
            }
            let name = String::from_utf8_lossy(&data[offset..offset + name_len]).to_string();
            offset += name_len;

            let prop_id_bytes: [u8; 4] = data[offset..offset + 4]
                .try_into()
                .map_err(|_| StorageError::deserialize_error("failed to read prop_id"))?;
            let prop_id = i32::from_le_bytes(prop_id_bytes);
            offset += 4;
            let data_type = match DataType::from_u8(data[offset]) {
                Ok(dt) => {
                    offset += 1;
                    dt
                }
                Err(TypeCodecError::ParameterizedTypeCode(code)) => {
                    // Known parameterized type: read the postcard TypeInfo
                    // block that follows the code byte.
                    let (info, rest) =
                        postcard::take_from_bytes(&data[offset + 1..]).map_err(|e| {
                            StorageError::deserialize_error(format!(
                                "failed to decode TypeInfo for code {code}: {e}"
                            ))
                        })?;
                    let consumed = (data.len() - rest.len()) - (offset + 1);
                    offset += 1 + consumed;
                    data_type_from_info(code, &info).ok_or_else(|| {
                        StorageError::deserialize_error(format!(
                            "TypeInfo mismatch for parameterized code {code}"
                        ))
                    })?
                }
                Err(e) => {
                    return Err(StorageError::deserialize_error(format!(
                        "failed to decode data type: {}",
                        e
                    )))
                }
            };
            if offset + 2 > data.len() {
                return Err(StorageError::deserialize_error(
                    "unexpected end of data after parameterized type block",
                ));
            }
            let nullable = data[offset] == 1;
            offset += 1;

            let encoding_type = EncodingType::from_u8(data[offset]);
            offset += 1;

            let prop_schema = PropertySchema::new(name.clone(), prop_id, data_type)
                .nullable(nullable)
                .with_encoding(encoding_type);
            self.name_indexer.register(name.clone())?;
            self.schema.push(prop_schema);
        }

        // Recompute column byte offsets after schema is loaded
        self.recompute_column_byte_offsets();

        // Load PropertyRecords with MVCC support
        let records_len = read_u32_le(data, &mut offset)? as usize;
        self.records.clear();
        self.row_count = 0;
        self.used_data_bytes = 0;

        for _ in 0..records_len {
            if offset >= data.len() {
                return Err(StorageError::deserialize_error("unexpected end of data"));
            }
            let marker = data[offset];
            offset += 1;

            if marker == 1 {
                let create_ts = read_u64_le(data, &mut offset)?;
                let has_delete_ts = data[offset];
                offset += 1;
                let delete_ts = if has_delete_ts == 1 {
                    Some(read_u64_le(data, &mut offset)?)
                } else {
                    None
                };
                let data_len = read_u32_le(data, &mut offset)? as usize;
                if offset + data_len > data.len() {
                    return Err(StorageError::deserialize_error("unexpected end of data"));
                }
                let record_data = data[offset..offset + data_len].to_vec();
                offset += data_len;

                self.used_data_bytes += record_data.len();
                let record = PropertyRecord {
                    data: record_data,
                    create_ts,
                    delete_ts,
                };
                self.records.push(Some(record));
                self.row_count += 1;
            } else {
                self.records.push(None);
            }
        }

        // Load before-image version chains, oldest first.
        self.chain_records.clear();
        for _ in 0..self.records.len() {
            let chain_len = read_u32_le(data, &mut offset)? as usize;
            let mut chain = Vec::with_capacity(chain_len);
            for _ in 0..chain_len {
                let create_ts = read_u64_le(data, &mut offset)?;
                let has_delete_ts = data[offset];
                offset += 1;
                let delete_ts = if has_delete_ts == 1 {
                    let d = read_u64_le(data, &mut offset)?;
                    Some(d)
                } else {
                    None
                };
                let data_len = read_u32_le(data, &mut offset)? as usize;
                if offset + data_len > data.len() {
                    return Err(StorageError::deserialize_error("unexpected end of data"));
                }
                let record_data = data[offset..offset + data_len].to_vec();
                offset += data_len;
                chain.push(PropertyRecord {
                    data: record_data,
                    create_ts,
                    delete_ts,
                });
            }
            self.chain_records.push(chain);
        }
        self.ensure_chain_len();

        // Load tiered tombstones for GC tracking
        // The persisted tombstone payload is not stored (rebuilt from record delete_ts below).
        // We read and discard the count for compatibility with older files that may have
        // written placeholder bytes.
        let _tombstones_len = read_u32_le(data, &mut offset)? as usize;
        self.tombstones_manager = TieredTombstoneManager::new(10_000);

        // Rebuild tiered tombstone manager from record timestamps
        for (idx, record_opt) in self.records.iter().enumerate() {
            if let Some(record) = record_opt {
                if let Some(delete_ts) = record.delete_ts {
                    let prop_offset = prop_index_to_offset(idx);
                    self.tombstones_manager
                        .add_tombstone(prop_offset, delete_ts);
                }
            }
        }

        // Load free list with Varint decoding
        let free_list_len = read_u32_le(data, &mut offset)? as usize;
        self.free_list.clear();
        for _ in 0..free_list_len {
            let mut cursor = Cursor::new(&data[offset..]);
            let off = decode_varint(&mut cursor)?;
            offset += cursor.position() as usize;
            self.free_list.push(off);
        }

        // Rebuild property value index from loaded records
        self.value_index.rebuild(&self.schema, &self.records);

        // ── load zone maps and rebuild columnar store ──
        self.column_store = ColumnStore::new();
        for prop in &self.schema {
            self.column_store
                .add_column(prop.name.clone(), prop.data_type.clone(), prop.nullable);
        }
        // Ensure column store has enough rows.
        if !self.records.is_empty() {
            self.column_store.resize(self.records.len());
        }
        // Rebuild columnar store from row records (dual-write source of truth).
        for (row_idx, record_opt) in self.records.iter().enumerate() {
            if let Some(rec) = record_opt {
                if rec.delete_ts.is_some() {
                    continue;
                }
                if let Ok(props) = self.deserialize_row(&rec.data) {
                    for (name, opt_val) in props {
                        let _ = self.column_store.set_property_versioned(
                            row_idx,
                            &name,
                            opt_val.as_ref(),
                            rec.create_ts,
                        );
                    }
                }
            }
        }
        // Rebuild version chains for historical rows from chain_records.
        for (row_idx, chain) in self.chain_records.iter().enumerate() {
            for rec in chain {
                if let Ok(props) = self.deserialize_row(&rec.data) {
                    for (name, opt_val) in props {
                        // Historical versions: visible on [create_ts, delete_ts)
                        // ColumnStore version chain is per-cell, so we push
                        // each historical value as a before-image.
                        let end_ts = rec.delete_ts.unwrap_or(Timestamp::MAX);
                        // Only push if genuinely historical (create < delete).
                        if rec.create_ts < end_ts {
                            // Simulate versioned write: set current to historical
                            // then overwrite with next version in next iteration.
                            // For simplicity, ensure row meta allows history:
                            let _ = self.column_store.set_property_versioned(
                                row_idx,
                                &name,
                                opt_val.as_ref(),
                                rec.create_ts,
                            );
                        }
                    }
                }
            }
        }

        self.zone_maps.clear();
        if offset < data.len() {
            // Zone maps
            if offset + 4 <= data.len() {
                if let Ok(zm_len) = read_u32_le(data, &mut offset) {
                    for _ in 0..zm_len as usize {
                        if offset + 4 > data.len() {
                            break;
                        }
                        let name_len = match read_u32_le(data, &mut offset) {
                            Ok(v) => v as usize,
                            Err(_) => break,
                        };
                        if offset + name_len > data.len() {
                            break;
                        }
                        let name =
                            String::from_utf8_lossy(&data[offset..offset + name_len]).to_string();
                        offset += name_len;
                        if offset + 4 > data.len() {
                            break;
                        }
                        let chunk_count = match read_u32_le(data, &mut offset) {
                            Ok(v) => v as usize,
                            Err(_) => break,
                        };
                        let mut chunks = Vec::with_capacity(chunk_count);
                        for _ in 0..chunk_count {
                            if offset + 4 > data.len() {
                                break;
                            }
                            let meta_len = match read_u32_le(data, &mut offset) {
                                Ok(v) => v as usize,
                                Err(_) => break,
                            };
                            if offset + meta_len > data.len() {
                                break;
                            }
                            let mut cur = &data[offset..offset + meta_len];
                            if let Ok(stats) = ColumnStats::deserialize_meta(&mut cur) {
                                chunks.push(stats);
                            }
                            offset += meta_len;
                        }
                        self.zone_maps.insert(name, chunks);
                    }
                }
            }
            // If zone maps were empty (e.g., fresh v4 file with no data), rebuild.
            if self.zone_maps.is_empty() && !self.records.is_empty() {
                self.rebuild_zone_maps();
            }
        } else {
            // Fresh file with no zone-map section: rebuild from row records.
            self.rebuild_zone_maps();
        }

        Ok(())
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

    /// Reclaim property slots whose rows are physically dead: tombstoned,
    /// no longer referenced by any live edge (`offset ∉ valid_offsets`), and
    /// deleted at or before the retention bound. Cleared slots return to the
    /// free list for reuse by future inserts.
    ///
    /// Live rows never move: offsets are stable, so external references
    /// (CSR `prop_offset` pointers) stay valid without any relocation
    /// mapping. An unbounded retention bound ([`Timestamp::MAX`]) is not a
    /// real timestamp — nothing is reclaimable, preserving time-travel
    /// history.
    ///
    /// Returns the number of reclaimed rows.
    pub fn reclaim_slots(
        &mut self,
        valid_offsets: &HashSet<u32>,
        retention_bound: Timestamp,
    ) -> usize {
        if retention_bound == Timestamp::MAX {
            return 0;
        }
        let mut reclaimed = 0usize;
        for idx in 0..self.records.len() {
            let offset = prop_index_to_offset(idx);
            if valid_offsets.contains(&offset) {
                continue;
            }
            let Some(record) = self.records[idx].as_ref() else {
                continue;
            };
            // Live rows (no deletion mark) are never reclaimed here: they may
            // still be referenced by edges outside the collected set of valid
            // offsets.
            let Some(delete_ts) = record.delete_ts else {
                continue;
            };
            if delete_ts > retention_bound {
                continue;
            }

            let props = deserialize_row_raw(&self.schema, &record.data);
            self.value_index.remove_record(&props, offset);
            self.tombstones_manager.remove(offset);
            self.used_data_bytes = self.used_data_bytes.saturating_sub(record.data.len());
            // The row's full version history dies with its slot; this is safe
            // because `delete_ts <= retention_bound` means no active snapshot
            // can observe any version of the row.
            if let Some(chain) = self.chain_records.get_mut(idx) {
                for entry in chain.drain(..) {
                    self.used_data_bytes = self.used_data_bytes.saturating_sub(entry.data.len());
                }
            }
            self.records[idx] = None;
            self.free_list.push(offset);
            self.row_count = self.row_count.saturating_sub(1);
            reclaimed += 1;
        }

        if reclaimed > 0 {
            // Zone maps must exclude the cleared rows; the columnar store
            // keeps cell values until the slot is reused (same policy as
            // physical delete).
            self.rebuild_zone_maps();
        }
        reclaimed
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

    /// Get the byte size of a fixed-size data type in the serialized row format.
    /// Returns None for variable-size types (String, Date, etc.).
    fn data_type_byte_size(dt: &DataType) -> Option<usize> {
        match dt {
            DataType::Bool => Some(1),
            DataType::SmallInt => Some(2),
            DataType::Int => Some(4),
            DataType::BigInt => Some(8),
            DataType::Float => Some(4),
            DataType::Double => Some(8),
            _ => None, // Variable-size types
        }
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

    /// Prefetch a single property offset into CPU cache
    /// This is a no-op on most systems but signals intent for cache optimization
    #[inline]
    pub fn prefetch(&self, offset: u32) {
        if let Some(row_idx) = prop_offset_to_index(offset) {
            if row_idx < self.records.len() {
                if let Some(record) = &self.records[row_idx] {
                    // Prefetch the data location to L1/L2 cache
                    #[allow(unsafe_code)]
                    unsafe {
                        let addr = record.data.as_ptr();
                        // Use a volatile read to ensure prefetch happens
                        std::ptr::read_volatile(addr);
                    }
                }
            }
        }
    }

    /// Prefetch multiple property offsets in batch
    /// Improves cache locality for bulk operations
    pub fn prefetch_batch(&self, offsets: &[u32]) {
        for offset in offsets {
            self.prefetch(*offset);
        }
    }

    /// Fast path deserialization for fixed-size schemas
    /// Skips null checks and type dispatching for 2-3x speedup
    pub fn get_fast(
        &self,
        offset: u32,
        query_ts: Option<Timestamp>,
    ) -> Option<Vec<(String, Option<Value>)>> {
        if !self.is_schema_fixed_size() {
            return self.get(offset, query_ts);
        }

        let row_idx = prop_offset_to_index(offset)?;
        if row_idx >= self.records.len() {
            return None;
        }

        let record = self.resolve_version(row_idx, query_ts)?;

        let record_data = &record.data;

        // Fast path: directly deserialize without null checks
        let mut cursor = Cursor::new(record_data);
        let mut result = Vec::with_capacity(self.schema.len());

        for schema in &self.schema {
            // The row format still contains a per-column null marker; it must be
            // consumed before the value bytes.
            let mut null_marker = [0u8; 1];
            if cursor.read_exact(&mut null_marker).is_err() {
                return None;
            }
            if null_marker[0] == 0 {
                result.push((schema.name.clone(), None));
                continue;
            }
            match &schema.data_type {
                DataType::Bool => {
                    let mut b = [0u8; 1];
                    if cursor.read_exact(&mut b).is_err() {
                        return None;
                    }
                    result.push((schema.name.clone(), Some(Value::Bool(b[0] != 0))));
                }
                DataType::SmallInt => {
                    let mut buf = [0u8; 2];
                    if cursor.read_exact(&mut buf).is_err() {
                        return None;
                    }
                    result.push((
                        schema.name.clone(),
                        Some(Value::SmallInt(i16::from_le_bytes(buf))),
                    ));
                }
                DataType::Int => {
                    let mut buf = [0u8; 4];
                    if cursor.read_exact(&mut buf).is_err() {
                        return None;
                    }
                    result.push((
                        schema.name.clone(),
                        Some(Value::Int(i32::from_le_bytes(buf))),
                    ));
                }
                DataType::BigInt => {
                    let mut buf = [0u8; 8];
                    if cursor.read_exact(&mut buf).is_err() {
                        return None;
                    }
                    result.push((
                        schema.name.clone(),
                        Some(Value::BigInt(i64::from_le_bytes(buf))),
                    ));
                }
                DataType::Float => {
                    let mut buf = [0u8; 4];
                    if cursor.read_exact(&mut buf).is_err() {
                        return None;
                    }
                    result.push((
                        schema.name.clone(),
                        Some(Value::Float(f32::from_le_bytes(buf))),
                    ));
                }
                DataType::Double => {
                    let mut buf = [0u8; 8];
                    if cursor.read_exact(&mut buf).is_err() {
                        return None;
                    }
                    result.push((
                        schema.name.clone(),
                        Some(Value::Double(f64::from_le_bytes(buf))),
                    ));
                }
                _ => {
                    // Should not reach here due to is_schema_fixed_size check
                    return None;
                }
            }
        }

        Some(result)
    }

    /// Batch retrieval of properties, sorted by offset for cache locality
    /// Returns results in original order via the provided iterator
    #[allow(clippy::type_complexity)]
    pub fn get_batch<'a, I>(
        &'a self,
        offsets: I,
        query_ts: Option<Timestamp>,
    ) -> Vec<Option<Vec<(String, Option<Value>)>>>
    where
        I: IntoIterator<Item = &'a u32>,
    {
        let offsets: Vec<_> = offsets.into_iter().collect();
        let mut indexed: Vec<_> = offsets
            .iter()
            .enumerate()
            .map(|(idx, offset)| (idx, **offset))
            .collect();

        // Sort by offset to improve cache locality
        indexed.sort_by_key(|(_, offset)| *offset);

        // Prefetch all offsets
        for (_, offset) in &indexed {
            self.prefetch(*offset);
        }

        // Retrieve in sorted order
        let sorted_results: Vec<_> = indexed
            .iter()
            .map(|(_, offset)| {
                self.get_fast(*offset, query_ts)
                    .or_else(|| self.get(*offset, query_ts))
            })
            .collect();

        // Restore original order
        let mut results = vec![None; offsets.len()];
        for (orig_idx, sorted_result) in indexed.iter().zip(sorted_results) {
            results[orig_idx.0] = sorted_result;
        }

        results
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
#[path = "property_table_tests.rs"]
mod tests;

#[cfg(test)]
mod olap_phase1_tests {
    use super::*;
    use crate::core::{DataType, Value};

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
                    &[("age".to_string(), Value::Int((i as i32 + 1) * 10))],
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
                .insert(&[("status".to_string(), Value::Int((i % 3) as i32))], 100)
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
        const CHECKSUM_POS: usize = crate::storage::persistence::HEADER_SIZE;
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
