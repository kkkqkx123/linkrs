//! Core EdgeStore operations: CRUD, properties, queries, and compaction.
//!
//! Single-segment edge table: one mutable CSR per direction plus centralized
//! row-level timestamps. There are no frozen segments, no merges, and no
//! cross-segment deduplication.

use super::super::{CsrBase, CsrVariant, EdgeRecord, EdgeSchema, MutableCsrTrait, Nbr};
use super::mvcc::MVCCManager;
use crate::edge::property_schema::PropertySchema;
use crate::edge::CsrWithProperties;
use crate::index::edge_index_manager::EdgePropertyIndex;
use crate::schema::{ChangeDetails, LabelVersionHistory, PropertyChange, SchemaObjectType};
use crate::types::{PropertyId, StoragePropertyDef};
use graphdb_core::types::{EdgeId, LabelId, Timestamp, VertexId};
use graphdb_core::{DataType, StorageError, StorageResult, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

pub use super::config::{AutoMaintenanceConfig, EdgeTableConfig, UpdateEdgePropertyByOffsetParams};
pub use super::iterator::EdgeTableScanIterator;

/// Single-segment edge store: one CSR per direction with MVCC row timestamps.
pub struct EdgeStore {
    pub label: LabelId,
    pub label_name: String,
    pub src_label: LabelId,
    pub dst_label: LabelId,
    pub schema: EdgeSchema,
    pub out_csr: CsrVariant,
    pub in_csr: CsrVariant,
    pub mvcc: MVCCManager,
    pub properties: CsrWithProperties,
    pub is_open: bool,
    pub next_edge_id: EdgeId,
    pub config: EdgeTableConfig,
    pub stats_manager: Option<std::sync::Arc<graphdb_metrics::StatsManager>>,
    /// Version history tracking for schema changes
    pub version_history: Arc<Mutex<LabelVersionHistory>>,
    /// Cache for property name → schema index mapping to avoid O(n) linear lookups.
    /// Invalidated whenever schema changes.
    pub property_index_cache: HashMap<String, usize>,

    /// Edge property index for efficient property-based filtering.
    /// When set, insert/delete operations automatically maintain the index.
    pub property_index: Option<EdgePropertyIndex>,

    /// Serial counter for automatic maintenance: incremented on every
    /// maintenance run so tombstone GC can be rate-limited.
    pub maintenance_serial: u64,
    /// Snapshot timestamp used by the last automatic GC run. Used to avoid
    /// re-running GC when `min_active_snapshot_ts` has not advanced.
    pub last_gc_min_snapshot_ts: Timestamp,
}

impl std::fmt::Debug for EdgeStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EdgeStore")
            .field("label", &self.label)
            .field("label_name", &self.label_name)
            .field("out_csr", &self.out_csr)
            .field("in_csr", &self.in_csr)
            .field("is_open", &self.is_open)
            .field("next_edge_id", &self.next_edge_id)
            .field("config", &self.config)
            .finish()
    }
}

impl EdgeStore {
    pub fn new(schema: EdgeSchema) -> StorageResult<Self> {
        Self::with_config(schema, EdgeTableConfig::default())
    }

    pub fn with_config(schema: EdgeSchema, config: EdgeTableConfig) -> StorageResult<Self> {
        schema.validate()?;

        if config.overflow_chunk_edges == 0 {
            return Err(StorageError::invalid_operation(
                "overflow_chunk_edges must be greater than zero",
            ));
        }

        let out_csr = CsrVariant::from_strategy_with_overflow(
            schema.oe_strategy,
            config.initial_vertex_capacity,
            config.initial_edge_capacity,
            config.overflow_chunk_edges,
        )?;
        let in_csr = CsrVariant::from_strategy_with_overflow(
            schema.ie_strategy,
            config.initial_vertex_capacity,
            config.initial_edge_capacity,
            config.overflow_chunk_edges,
        )?;

        let prop_schemas: Vec<PropertySchema> = schema
            .properties
            .iter()
            .enumerate()
            .map(|(i, p)| {
                PropertySchema::new(p.name.clone(), i as i32, p.data_type.clone())
                    .nullable(p.nullable)
            })
            .collect();
        let properties = CsrWithProperties::new(config.initial_vertex_capacity, prop_schemas);

        let label_id = schema.label_id;
        let label_name = schema.label_name.clone();

        let version_history = Arc::new(Mutex::new(LabelVersionHistory::new(
            label_id,
            label_name.clone(),
            SchemaObjectType::Edge,
        )));

        let mut property_index_cache = HashMap::new();
        for (idx, prop) in schema.properties.iter().enumerate() {
            property_index_cache.insert(prop.name.clone(), idx);
        }

        Ok(Self {
            label: label_id,
            label_name,
            src_label: schema.src_label,
            dst_label: schema.dst_label,
            schema,
            out_csr,
            in_csr,
            mvcc: MVCCManager::new(),
            properties,
            is_open: true,
            next_edge_id: EdgeId(0),
            config,
            stats_manager: None,
            version_history,
            property_index_cache,
            property_index: None,
            maintenance_serial: 0,
            last_gc_min_snapshot_ts: 0,
        })
    }

    pub(crate) fn edge_endpoint_key(endpoint: u32, rank: i64) -> VertexId {
        let mut data = Vec::with_capacity(16);
        data.extend_from_slice(&(endpoint as i64).to_be_bytes());
        data.extend_from_slice(&rank.to_be_bytes());
        VertexId::from_bytes(data)
    }

    pub(crate) fn decode_edge_endpoint(key: VertexId) -> (VertexId, i64) {
        let bytes = key.as_bytes();
        if bytes.len() != 16 {
            log::warn!(
                "decode_edge_endpoint: unexpected key length {}, expected 16",
                bytes.len()
            );
        }
        let mut buf = [0u8; 16];
        let copy_len = bytes.len().min(16);
        buf[..copy_len].copy_from_slice(&bytes[..copy_len]);
        let mut endpoint_bytes = [0u8; 8];
        endpoint_bytes.copy_from_slice(&buf[..8]);
        let mut rank_bytes = [0u8; 8];
        rank_bytes.copy_from_slice(&buf[8..16]);

        (
            VertexId::from_int64(i64::from_be_bytes(endpoint_bytes)),
            i64::from_be_bytes(rank_bytes),
        )
    }

    pub fn set_stats_manager(&mut self, stats: std::sync::Arc<graphdb_metrics::StatsManager>) {
        self.stats_manager = Some(stats);
    }

    fn merged_get_edge(
        &self,
        csr: &CsrVariant,
        src: u32,
        dst: VertexId,
        ts: Timestamp,
    ) -> Option<Nbr> {
        let nbr = csr.get_edge(src, dst, ts)?;
        if self.mvcc.is_edge_visible(nbr.edge_id, ts) {
            Some(nbr)
        } else {
            None
        }
    }

    fn merged_edges_of(&self, csr: &CsrVariant, src: u32, ts: Timestamp) -> Vec<Nbr> {
        csr.edges_of(src, ts)
            .into_iter()
            .filter(|nbr| self.mvcc.is_edge_visible(nbr.edge_id, ts))
            .collect()
    }

    pub(crate) fn edge_record_from_nbr(
        &self,
        src: u32,
        nbr: Nbr,
        query_ts: Timestamp,
    ) -> EdgeRecord {
        let dst_vid = VertexId::from_int64(nbr.endpoint as i64);
        let rank = nbr.rank;
        let properties = self.properties_for_edge(nbr.edge_id, query_ts);
        EdgeRecord {
            src_vid: VertexId::from_int64(src as i64),
            dst_vid,
            rank,
            properties,
        }
    }

    fn properties_for_edge(&self, edge_id: EdgeId, query_ts: Timestamp) -> Vec<(String, Value)> {
        if let Some(props) = self.properties.get_by_edge_id(edge_id, query_ts) {
            return props
                .into_iter()
                .filter_map(|(k, v)| v.map(|v| (k, v)))
                .collect();
        }
        Vec::new()
    }

    pub fn insert_edge(
        &mut self,
        src: u32,
        dst: u32,
        rank: i64,
        property_values: &[(String, Value)],
        ts: Timestamp,
    ) -> StorageResult<()> {
        if !self.is_open {
            return Err(StorageError::storage_not_open());
        }

        if self.schema.oe_strategy == super::super::EdgeStrategy::Multiple {
            // Multiple-edge strategy always stores out edges; no extra check.
        } else if self.schema.oe_strategy == super::super::EdgeStrategy::None {
            return Err(StorageError::invalid_operation(
                "Cannot insert edge: out-edge strategy is None".to_string(),
            ));
        }

        let mut converted_values: Vec<(String, Value)> = Vec::with_capacity(property_values.len());
        for (name, value) in property_values {
            let prop_idx = self
                .property_index_cache
                .get(name)
                .ok_or_else(|| StorageError::column_not_found(name.clone()))?;
            let prop_def = &self.schema.properties[*prop_idx];

            if value.data_type() != prop_def.data_type {
                let converted = value.try_cast_to(&prop_def.data_type)?;
                converted_values.push((name.clone(), converted));
            } else {
                converted_values.push((name.clone(), value.clone()));
            }
        }

        let edge_id = self.next_edge_id.fetch_add();

        // Pre-flight duplicate check before touching any shared state so a
        // failed insert leaves no partial MVCC/CSR record behind.
        if self.has_edge(src, dst, rank, ts) {
            return Err(StorageError::edge_already_exists(format!(
                "{} -> {}@{}",
                src, dst, rank
            )));
        }

        // Record edge creation in the centralized MVCC store.
        self.mvcc.record_creation(edge_id, ts);

        // Insert property rows and the out-direction CSR entry. Each fallible
        // step rolls back everything it already touched on failure so a failed
        // insert leaves no half-visible edge behind.
        if !converted_values.is_empty() {
            if let Err(e) = self
                .properties
                .insert_for_edge(edge_id, &converted_values, ts)
            {
                self.mvcc.remove_edge_timestamps(edge_id);
                return Err(e);
            }
        }

        let dst_key = Self::edge_endpoint_key(dst, rank);
        let src_key = Self::edge_endpoint_key(src, rank);
        if let Err(e) = self.out_csr.insert_edge(src, dst_key, edge_id, ts) {
            self.properties.remove_edge_mapping(edge_id);
            self.mvcc.remove_edge_timestamps(edge_id);
            return Err(e);
        }

        if let Err(e) = self.in_csr.insert_edge(dst, src_key, edge_id, ts) {
            // Roll back the out-direction insertion physically so no
            // tombstone residue remains; fall back to logical deletion if
            // the entry cannot be located (e.g. strategy mismatch).
            if !self.out_csr.remove_edge(src, edge_id) {
                let _ = self.out_csr.delete_edge(src, edge_id, ts);
            }
            self.properties.remove_edge_mapping(edge_id);
            let _ = self.properties.mark_deleted(edge_id, ts);
            // Remove the centralized MVCC creation record so the failed edge
            // does not survive as a phantom entry in timestamp lookups.
            self.mvcc.remove_edge_timestamps(edge_id);
            return Err(e);
        }

        // Update property index if enabled
        if let Some(ref mut index) = self.property_index {
            for (prop_name, prop_value) in &converted_values {
                let _ = index.insert(prop_name, prop_value, src, dst, rank, self.label, ts);
            }
        }

        // Check write backpressure after successful insertion
        self.check_and_apply_write_backpressure(ts);
        self.maybe_run_auto_maintenance();

        Ok(())
    }

    pub fn delete_edge(
        &mut self,
        src: u32,
        dst: u32,
        rank: i64,
        ts: Timestamp,
    ) -> StorageResult<bool> {
        if !self.is_open {
            return Err(StorageError::storage_not_open());
        }

        let dst_key = Self::edge_endpoint_key(dst, rank);
        let src_key = Self::edge_endpoint_key(src, rank);

        // Look up edge properties before deletion for index maintenance
        let edge_properties = if self.property_index.is_some() {
            self.get_edge(src, dst, rank, ts).map(|e| e.properties)
        } else {
            None
        };

        if let Some(nbr) = self.out_csr.get_edge(src, dst_key, ts) {
            let edge_id = nbr.edge_id;

            if !self.out_csr.delete_edge(src, edge_id, ts)? {
                // Defensive: the out side could not be deleted.
                return Ok(false);
            }
            if !self.in_csr.delete_edge_by_dst(dst, src_key, ts) {
                // Roll back the out-direction deletion to keep both sides
                // consistent.
                self.out_csr.revert_delete_by_edge_id(src, edge_id, ts);
                return Ok(false);
            }

            // Record deletion in the centralized MVCC store.
            self.mvcc.record_edge_deletion(edge_id, ts);

            // Mark the property record deleted once both sides are gone so
            // the row is reclaimable by compact_properties.
            let _ = self.properties.mark_deleted(edge_id, ts);
            self.update_property_index_on_delete(&edge_properties, src, dst, rank, ts);
            self.maybe_run_auto_maintenance();
            return Ok(true);
        }

        Ok(false)
    }

    fn update_property_index_on_delete(
        &mut self,
        properties: &Option<Vec<(String, Value)>>,
        src: u32,
        dst: u32,
        rank: i64,
        ts: Timestamp,
    ) {
        if let Some(ref mut index) = self.property_index {
            if let Some(ref props) = properties {
                for (prop_name, prop_value) in props {
                    let _ = index.delete(prop_name, prop_value, src, dst, rank, ts);
                }
            }
        }
    }

    pub fn delete_edge_by_offset(
        &mut self,
        src: u32,
        dst: u32,
        rank: i64,
        oe_offset: i32,
        ie_offset: i32,
        ts: Timestamp,
    ) -> StorageResult<bool> {
        if !self.is_open {
            return Err(StorageError::storage_not_open());
        }
        let dst_key = Self::edge_endpoint_key(dst, rank);
        if let Some(nbr) = self.out_csr.get_edge(src, dst_key, ts) {
            if !self.out_csr.delete_edge_by_offset(src, oe_offset, ts) {
                return Ok(false);
            }
            if !self.in_csr.delete_edge_by_offset(dst, ie_offset, ts) {
                // Roll back the out-direction deletion to keep both sides
                // consistent.
                self.out_csr.revert_delete_by_offset(src, oe_offset, ts);
                return Ok(false);
            }
            // Record deletion in the centralized MVCC store.
            self.mvcc.record_edge_deletion(nbr.edge_id, ts);
            // Mark the property record deleted once both sides are gone so
            // the row is reclaimable by compact_properties.
            let _ = self.properties.mark_deleted(nbr.edge_id, ts);
            self.maybe_run_auto_maintenance();
            return Ok(true);
        }
        Ok(false)
    }

    pub fn revert_delete_edge_by_offset(
        &mut self,
        src: u32,
        dst: u32,
        rank: i64,
        oe_offset: i32,
        ie_offset: i32,
        ts: Timestamp,
    ) -> StorageResult<bool> {
        if !self.is_open {
            return Err(StorageError::storage_not_open());
        }

        let reverted = self.out_csr.revert_delete_by_offset(src, oe_offset, ts);

        if reverted {
            self.in_csr.revert_delete_by_offset(dst, ie_offset, ts);
            // Restore the property record marked by mark_deleted so the edge
            // regains its original properties after the undo.
            let dst_key = Self::edge_endpoint_key(dst, rank);
            if let Some(nbr) = self.out_csr.get_edge(src, dst_key, ts) {
                // Restore edge visibility in the centralized MVCC store
                // and remove the tombstone so the edge is visible again.
                if let Some(ts_info) = self.mvcc.edge_timestamps.get_mut(&nbr.edge_id) {
                    ts_info.delete_ts = Timestamp::MAX;
                }
                self.mvcc.remove_deletion(nbr.edge_id);
                let _ = self.properties.revert_deletion_for_edge(nbr.edge_id);
            }
            return Ok(true);
        }

        // Fallback undo for deletions recorded without usable CSR offsets:
        // locate the edge among all physically present entries, verify this
        // undo owns the deletion, then revert both CSR sides by edge id.
        let Some(edge_id) = self
            .out_csr
            .iter_all()
            .filter_map(|(row, nbr)| {
                let row_u32 = row.as_int64().unwrap_or(-1);
                if row_u32 == src as i64 && nbr.endpoint == dst && nbr.rank == rank {
                    Some(nbr.edge_id)
                } else {
                    None
                }
            })
            .next()
        else {
            return Ok(false);
        };
        match self.mvcc.tombstones.get(&edge_id) {
            Some(delete_ts) if *delete_ts <= ts => {}
            _ => return Ok(false),
        }
        if !self.out_csr.revert_delete_by_edge_id(src, edge_id, ts) {
            return Ok(false);
        }
        self.in_csr.revert_delete_by_edge_id(dst, edge_id, ts);
        self.mvcc.remove_deletion(edge_id);
        if let Some(ts_info) = self.mvcc.edge_timestamps.get_mut(&edge_id) {
            ts_info.delete_ts = Timestamp::MAX;
        }
        let _ = self.properties.revert_deletion_for_edge(edge_id);
        let restored = self.properties_for_edge(edge_id, ts);
        if let Some(ref mut index) = self.property_index {
            for (prop_name, prop_value) in restored {
                let _ = index.insert(&prop_name, &prop_value, src, dst, rank, self.label, ts);
            }
        }
        Ok(true)
    }

    pub fn get_edge(&self, src: u32, dst: u32, rank: i64, ts: Timestamp) -> Option<EdgeRecord> {
        if !self.is_open {
            return None;
        }

        let dst_key = Self::edge_endpoint_key(dst, rank);
        let nbr = self.merged_get_edge(&self.out_csr, src, dst_key, ts)?;
        let properties = self.properties_for_edge(nbr.edge_id, ts);

        Some(EdgeRecord {
            src_vid: VertexId::from_int64(src as i64),
            dst_vid: VertexId::from_int64(dst as i64),
            rank,
            properties,
        })
    }

    pub fn out_edges(&self, src: u32, ts: Timestamp) -> Vec<EdgeRecord> {
        if !self.is_open {
            return Vec::new();
        }

        let nbrs = self.merged_out_nbrs(src, ts);

        nbrs.into_iter()
            .map(|nbr| {
                let dst_vid = VertexId::from_int64(nbr.endpoint as i64);
                let rank = nbr.rank;
                let properties = self.properties_for_edge(nbr.edge_id, ts);

                EdgeRecord {
                    src_vid: VertexId::from_int64(src as i64),
                    dst_vid,
                    rank,
                    properties,
                }
            })
            .collect()
    }

    /// Raw out-edge neighbors of `src` (MVCC-filtered, snapshot-consistent)
    /// with no property decoding.
    pub fn merged_out_nbrs(&self, src: u32, ts: Timestamp) -> Vec<Nbr> {
        self.merged_edges_of(&self.out_csr, src, ts)
    }

    pub fn in_edges(&self, dst: u32, ts: Timestamp) -> Vec<EdgeRecord> {
        if !self.is_open {
            return Vec::new();
        }

        let nbrs = self.merged_in_nbrs(dst, ts);

        nbrs.into_iter()
            .map(|nbr| {
                let src_vid = VertexId::from_int64(nbr.endpoint as i64);
                let rank = nbr.rank;
                let properties = self.properties_for_edge(nbr.edge_id, ts);

                EdgeRecord {
                    src_vid,
                    dst_vid: VertexId::from_int64(dst as i64),
                    rank,
                    properties,
                }
            })
            .collect()
    }

    /// Raw in-edge neighbors of `dst` (MVCC-filtered, snapshot-consistent)
    /// with no property decoding.
    pub fn merged_in_nbrs(&self, dst: u32, ts: Timestamp) -> Vec<Nbr> {
        self.merged_edges_of(&self.in_csr, dst, ts)
    }

    pub fn has_edge(&self, src: u32, dst: u32, rank: i64, ts: Timestamp) -> bool {
        if !self.is_open {
            return false;
        }
        let dst_key = Self::edge_endpoint_key(dst, rank);
        self.merged_get_edge(&self.out_csr, src, dst_key, ts)
            .is_some()
    }

    pub fn edge_count(&self) -> u64 {
        self.out_csr.edge_count()
    }

    pub fn delta_edge_count(&self) -> u64 {
        self.out_csr.edge_count() + self.in_csr.edge_count()
    }

    pub fn scan(&self, ts: Timestamp) -> Vec<EdgeRecord> {
        if !self.is_open {
            return Vec::new();
        }

        self.iter(ts).collect()
    }

    /// Optimizer-facing statistics snapshot for one property column
    /// (zone-map aggregated, with live row count).
    pub fn column_stats_snapshot(
        &self,
        column: &str,
    ) -> Option<crate::stats_reader::ColumnStatsSnapshot> {
        self.properties.column_stats_snapshot(column)
    }

    /// Record a schema change event
    ///
    /// Handles the common pattern of:
    /// 1. Computing next version number from history
    /// 2. Creating a PropertyChange event
    /// 3. Recording it in the version history
    fn record_schema_change(&mut self, details: ChangeDetails) -> StorageResult<()> {
        let mut history_guard = self
            .version_history
            .lock()
            .map_err(|_| StorageError::db_error("Failed to lock version_history"))?;

        let next_version = history_guard.latest_version() + 1;
        self.schema.schema_version = next_version;

        let change = PropertyChange::new(
            next_version,
            SchemaObjectType::Edge,
            self.label,
            self.label_name.clone(),
            details,
        );

        history_guard.add_change(change);

        Ok(())
    }

    pub fn add_property(
        &mut self,
        name: String,
        data_type: DataType,
        nullable: bool,
    ) -> StorageResult<()> {
        if !self.is_open {
            return Err(StorageError::storage_not_open());
        }

        if self.properties.has_property(&name) {
            return Err(StorageError::column_already_exists(name));
        }

        self.properties
            .add_property(name.clone(), data_type.clone(), nullable)?;

        let prop_def = StoragePropertyDef::new(name.clone(), data_type.clone());
        let new_idx = self.schema.properties.len();
        self.schema.properties.push(prop_def);
        self.property_index_cache.insert(name.clone(), new_idx);

        self.record_schema_change(ChangeDetails::PropertyAdded {
            name,
            data_type,
            nullable,
            default_value: None,
        })?;

        Ok(())
    }

    /// Rebuild schema change record during WAL recovery
    ///
    /// This is used during recovery when the column already exists (from SchemaManager),
    /// but we need to update version_history to reflect the schema operation in the WAL.
    /// Does NOT add the property (it already exists), but DOES record the change.
    pub fn rebuild_schema_change_from_redo(&mut self, details: ChangeDetails) -> StorageResult<()> {
        self.record_schema_change(details)
    }

    pub fn remove_property(&mut self, name: &str) -> StorageResult<()> {
        if !self.is_open {
            return Err(StorageError::storage_not_open());
        }

        let index = self
            .schema
            .properties
            .iter()
            .position(|prop| prop.name == name)
            .ok_or_else(|| StorageError::column_not_found(name.to_string()))?;

        // Get property details before removal for change recording
        let removed_prop = self.schema.properties[index].clone();

        // Remove from properties first (potentially failing operation)
        self.properties.remove_property(name)?;
        // Only modify schema if properties removal succeeded
        self.schema.properties.remove(index);
        // Update cache: remove deleted property and adjust indices
        self.property_index_cache.remove(name);
        for idx in self.property_index_cache.values_mut() {
            if *idx > index {
                *idx -= 1;
            }
        }

        self.record_schema_change(ChangeDetails::PropertyRemoved {
            name: removed_prop.name,
            data_type: removed_prop.data_type,
        })?;

        Ok(())
    }

    pub fn rename_property(&mut self, old_name: &str, new_name: &str) -> StorageResult<()> {
        if !self.is_open {
            return Err(StorageError::storage_not_open());
        }

        if self
            .schema
            .properties
            .iter()
            .any(|prop| prop.name == new_name)
        {
            return Err(StorageError::column_already_exists(new_name.to_string()));
        }

        let index = self
            .schema
            .properties
            .iter()
            .position(|prop| prop.name == old_name)
            .ok_or_else(|| StorageError::column_not_found(old_name.to_string()))?;

        // Rename in properties first (potentially failing operation)
        self.properties.rename_property(old_name, new_name)?;
        // Only modify schema if properties rename succeeded
        self.schema.properties[index].name = new_name.to_string();
        // Update cache: rename key, keep index
        if let Some(idx) = self.property_index_cache.remove(old_name) {
            self.property_index_cache.insert(new_name.to_string(), idx);
        }

        self.record_schema_change(ChangeDetails::PropertyRenamed {
            old_name: old_name.to_string(),
            new_name: new_name.to_string(),
        })?;

        Ok(())
    }

    pub fn update_edge_property(
        &mut self,
        src: u32,
        dst: u32,
        rank: i64,
        prop_name: &str,
        value: &Value,
        ts: Timestamp,
    ) -> StorageResult<bool> {
        if !self.is_open {
            return Err(StorageError::storage_not_open());
        }

        // Validate property exists via cache
        let _ = self
            .property_index_cache
            .get(prop_name)
            .ok_or_else(|| StorageError::column_not_found(prop_name.to_string()))?;

        let dst_key = Self::edge_endpoint_key(dst, rank);
        if let Some(nbr) = self.merged_get_edge(&self.out_csr, src, dst_key, ts) {
            self.properties
                .set_property_for_edge(nbr.edge_id, prop_name, Some(value.clone()), ts)
                .map_err(|_| StorageError::column_not_found(prop_name.to_string()))?;
            self.maybe_run_auto_maintenance();
            return Ok(true);
        }

        Ok(false)
    }

    pub fn update_edge_property_by_offset(
        &mut self,
        params: UpdateEdgePropertyByOffsetParams,
    ) -> StorageResult<bool> {
        if !self.is_open {
            return Err(StorageError::storage_not_open());
        }

        let dst_key = Self::edge_endpoint_key(params.dst, params.rank);
        if let Some(nbr) = self.merged_get_edge(&self.out_csr, params.src, dst_key, params.ts) {
            self.properties
                .set_property_by_id_for_edge(
                    nbr.edge_id,
                    PropertyId(params.prop_id),
                    Some(params.value.clone()),
                    params.ts,
                )
                .map_err(|_| {
                    StorageError::column_not_found(format!("prop_id={}", params.prop_id))
                })?;

            let src_key = Self::edge_endpoint_key(params.src, params.rank);
            if let Some(ie_nbr) = self.merged_get_edge(&self.in_csr, params.dst, src_key, params.ts)
            {
                if nbr.edge_id != ie_nbr.edge_id {
                    return Err(StorageError::data_corruption(format!(
                        "edge_id mismatch: out_csr={}, in_csr={} at edge ({}, {})",
                        nbr.edge_id.0, ie_nbr.edge_id.0, params.src, params.dst
                    )));
                }
            }
            self.maybe_run_auto_maintenance();
            return Ok(true);
        }

        Ok(false)
    }

    pub fn label(&self) -> LabelId {
        self.label
    }

    pub fn src_label(&self) -> LabelId {
        self.src_label
    }

    pub fn dst_label(&self) -> LabelId {
        self.dst_label
    }

    pub fn schema(&self) -> &EdgeSchema {
        &self.schema
    }

    pub(crate) fn schema_mut(&mut self) -> &mut EdgeSchema {
        &mut self.schema
    }

    pub fn set_schema(&mut self, schema: EdgeSchema) {
        // Rebuild property index cache
        self.property_index_cache.clear();
        for (idx, prop) in schema.properties.iter().enumerate() {
            self.property_index_cache.insert(prop.name.clone(), idx);
        }
        self.schema = schema;
    }

    /// Get reference to version history Arc for shared access
    pub fn version_history_ref(&self) -> Arc<Mutex<LabelVersionHistory>> {
        Arc::clone(&self.version_history)
    }

    pub fn iter(&self, ts: Timestamp) -> EdgeTableScanIterator<'_> {
        EdgeTableScanIterator::new(self, ts)
    }

    pub fn memory_size(&self) -> usize {
        let total = self.used_memory_size();
        let mutable = self.mutable_csr_memory_size();
        let out_epv = self.out_csr.edges_per_vertex();
        let in_epv = self.in_csr.edges_per_vertex();
        if out_epv > 0 || in_epv > 0 {
            log::trace!(
                "EdgeTable[{}] memory: {} bytes (mutable={}), MultiSingle edges_per_vertex (out={}, in={})",
                self.label,
                total,
                mutable,
                out_epv,
                in_epv
            );
        }
        total
    }

    pub fn used_memory_size(&self) -> usize {
        let mut total = 0;

        total += self.out_csr.used_memory_size();
        total += self.in_csr.used_memory_size();
        total += self.mvcc.total_tombstone_count() * std::mem::size_of::<(EdgeId, Timestamp)>();
        total += self.mvcc.edge_timestamps.len()
            * (std::mem::size_of::<EdgeId>() + std::mem::size_of::<super::mvcc::EdgeTimestamps>());
        total += self.properties.used_memory_size();

        // Account for property_index_cache
        total += self.property_index_cache.len()
            * (std::mem::size_of::<String>() + std::mem::size_of::<usize>());

        // Account for the edge property index (if enabled)
        if let Some(ref index) = self.property_index {
            total += index.memory_usage() as usize;
        }

        total
    }

    /// Get mutable CSR memory usage (out_csr + in_csr)
    pub fn mutable_csr_memory_size(&self) -> usize {
        self.out_csr.used_memory_size() + self.in_csr.used_memory_size()
    }

    /// Estimate memory usage based on edge count and CSR strategy.
    pub fn estimate_memory_usage(&self) -> usize {
        let out_edges = self.out_csr.edge_count() as usize;
        let in_edges = self.in_csr.edge_count() as usize;
        let out_bytes_per_edge = self.out_csr.bytes_per_edge();
        let in_bytes_per_edge = self.in_csr.bytes_per_edge();
        let estimated = out_edges * out_bytes_per_edge + in_edges * in_bytes_per_edge;

        let total_capacity = out_edges + in_edges;
        let frag_stats =
            crate::edge::FragmentationStats::new(total_capacity, out_edges.min(in_edges));
        if frag_stats.fragmentation_ratio() > 2.0 {
            log::debug!(
                "EdgeTable[{}] high fragmentation: {:.2}",
                self.label,
                frag_stats.fragmentation_ratio()
            );
        }

        estimated
    }

    /// Record mutable CSR pressure without performing maintenance on the write path.
    pub fn check_and_apply_write_backpressure(&mut self, _current_ts: Timestamp) -> bool {
        if self.config.max_mutable_csr_bytes == 0 {
            return false; // Backpressure disabled
        }

        let mutable_size = self.estimate_memory_usage();

        // Record current metrics
        if let Some(stats) = &self.stats_manager {
            stats.record_mutable_csr_backpressure(mutable_size as u64, mutable_size as u64);
        }

        if mutable_size > self.config.max_mutable_csr_bytes {
            return true;
        }

        false
    }

    pub fn needs_background_maintenance(&self) -> bool {
        self.config.max_mutable_csr_bytes > 0
            && self.estimate_memory_usage() > self.config.max_mutable_csr_bytes
    }

    /// Run automatic maintenance based on configured thresholds.
    ///
    /// Called from write paths (`insert_edge`, `delete_edge`, updates) so
    /// deleted entries and stale metadata are reclaimed without waiting for
    /// an explicit maintenance invocation:
    ///
    /// - tombstone GC when the total tombstone count exceeds the threshold
    ///   (rate-limited by `gc_min_serial` to bound write-path latency)
    /// - property compaction when the deleted-row ratio is high
    ///
    /// Returns the number of maintenance passes that actually ran.
    pub fn maybe_run_auto_maintenance(&mut self) -> usize {
        let cfg = self.config.auto_maintenance;
        if cfg.tombstone_gc_threshold == 0 {
            return 0;
        }
        let mut maintenance_ran = 0;

        // Tier 1: tombstone GC (rate-limited by serial counter).
        if self.mvcc.total_tombstone_count() > cfg.tombstone_gc_threshold {
            let bound = self.mvcc.min_active_snapshot_ts;
            if bound < Timestamp::MAX
                && (bound != self.last_gc_min_snapshot_ts
                    || (cfg.gc_min_serial > 0
                        && self.maintenance_serial.is_multiple_of(cfg.gc_min_serial)))
            {
                let cleaned = self.mvcc.gc_tombstones(bound);
                self.last_gc_min_snapshot_ts = bound;
                self.maintenance_serial = self.maintenance_serial.saturating_add(1);
                if cleaned > 0 {
                    maintenance_ran += 1;
                    log::debug!(
                        "Auto-maintenance GC: removed {} tombstones (bound={}, total={})",
                        cleaned,
                        bound,
                        self.mvcc.total_tombstone_count()
                    );
                }
            }
        }

        // Tier 2: property table compaction when the deleted-row ratio is high.
        let bound = self.mvcc.min_active_snapshot_ts;
        if cfg.property_compact_ratio > 0.0 && bound != Timestamp::MAX {
            let prop_stats = self.properties.compaction_stats();
            if prop_stats.fragmentation_ratio() >= cfg.property_compact_ratio as f64 {
                self.compact_properties(bound);
                self.maintenance_serial = self.maintenance_serial.saturating_add(1);
                maintenance_ran += 1;
            }
        }

        maintenance_ran
    }

    /// Unified watermark variant of `maybe_run_auto_maintenance`. Caller captures
    /// `MvccWatermarks` once per GC pass and shares the same safe cutoff across
    /// all table types so a prefix reclaim cannot change the cutoff for a later
    /// sub-system in the same pass.
    pub fn maybe_run_auto_maintenance_with_watermarks(
        &mut self,
        watermarks: &graphdb_transaction::MvccWatermarks,
        margin: Timestamp,
    ) -> usize {
        let cfg = self.config.auto_maintenance;
        if cfg.tombstone_gc_threshold == 0 {
            return 0;
        }
        let mut maintenance_ran = 0;
        let bound = watermarks.safe_gc_timestamp_with_margin(margin);

        if self.mvcc.total_tombstone_count() > cfg.tombstone_gc_threshold
            && bound < Timestamp::MAX
            && (bound != self.last_gc_min_snapshot_ts
                || (cfg.gc_min_serial > 0
                    && self.maintenance_serial.is_multiple_of(cfg.gc_min_serial)))
        {
            let cleaned = self.mvcc.gc_tombstones(bound);
            self.last_gc_min_snapshot_ts = bound;
            self.maintenance_serial = self.maintenance_serial.saturating_add(1);
            if cleaned > 0 {
                maintenance_ran += 1;
                log::debug!(
                    "Auto-maintenance GC (watermark): removed {} tombstones (bound={}, total={})",
                    cleaned,
                    bound,
                    self.mvcc.total_tombstone_count()
                );
            }
        }

        if cfg.property_compact_ratio > 0.0 && bound != Timestamp::MAX {
            let prop_stats = self.properties.compaction_stats();
            if prop_stats.fragmentation_ratio() >= cfg.property_compact_ratio as f64 {
                self.compact_properties(bound);
                self.maintenance_serial = self.maintenance_serial.saturating_add(1);
                maintenance_ran += 1;
            }
        }

        maintenance_ran
    }

    // ── Edge Property Index ──

    /// Enable property index with the specified pool capacity.
    /// Builds the index from existing edge data.
    pub fn enable_property_index(&mut self, pool_capacity: u64) -> StorageResult<()> {
        self.build_property_index(pool_capacity)
    }

    /// Build the property index by scanning all edges.
    pub(crate) fn build_property_index(&mut self, pool_capacity: u64) -> StorageResult<()> {
        let mut index = EdgePropertyIndex::new(pool_capacity);
        // MAX_TIMESTAMP satisfies `create_ts <= ts < delete_ts` for live
        // edges, so all non-tombstoned edges are scanned.
        let all_ts = graphdb_core::types::MAX_TIMESTAMP;

        let iter = EdgeTableScanIterator::new(self, all_ts);
        let edge_records: Vec<EdgeRecord> = iter.collect();
        for edge in &edge_records {
            let src_u32 = edge.src_vid.as_int64().unwrap_or(0) as u32;
            let dst_u32 = edge.dst_vid.as_int64().unwrap_or(0) as u32;
            for (prop_name, prop_value) in &edge.properties {
                let _ = index.insert(
                    prop_name, prop_value, src_u32, dst_u32, edge.rank, self.label, all_ts,
                );
            }
        }

        self.property_index = Some(index);
        Ok(())
    }

    /// Check if property index is enabled.
    pub fn has_property_index(&self) -> bool {
        self.property_index.is_some()
    }

    /// Drop the property index to free memory.
    pub fn disable_property_index(&mut self) {
        self.property_index = None;
    }

    /// Lookup edges by a property value range using the EdgePropertyIndex.
    /// Returns `(src, dst, rank)` tuples for matching edges.
    pub fn lookup_edges_by_property_range(
        &self,
        prop_name: &str,
        value_lower: &[u8],
        value_upper: &[u8],
    ) -> Vec<(u32, u32, i64)> {
        let Some(ref index) = self.property_index else {
            return Vec::new();
        };
        if !index.has_index(prop_name) {
            return Vec::new();
        }
        index
            .lookup(prop_name, value_lower, value_upper)
            .into_iter()
            .map(|((src, dst, rank), _record)| (src, dst, rank))
            .collect()
    }

    pub fn flush<P: AsRef<std::path::Path>>(
        &mut self,
        path: P,
        compression: crate::compression::CompressionType,
    ) -> StorageResult<()> {
        use std::fs;
        let path = path.as_ref();
        fs::create_dir_all(path)?;
        crate::compression::cleanup_shadow_files(path)?;

        let crate::compression::CompressionType::Zstd { level } = compression;
        let page_size = crate::compression::DEFAULT_PAGE_SIZE;

        let mut meta_payload = Vec::new();
        crate::persistence::write_header_to(
            &mut meta_payload,
            crate::persistence::section::EDGE_META,
        )
        .map_err(|e| StorageError::io_error(format!("Failed to write edge meta header: {}", e)))?;

        super::persistence::flush_metadata(
            &mut meta_payload,
            self.label,
            self.src_label,
            self.dst_label,
            &self.label_name,
            self.is_open,
            &self.schema,
            self.next_edge_id,
            &self.mvcc.edge_timestamps,
        )?;
        super::persistence::write_pages_to_file(
            &path.join("meta.bin"),
            &meta_payload,
            page_size,
            level,
            1,
        )?;

        let mut out_csr_payload = Vec::new();
        super::persistence::serialize_csr(
            &self.out_csr,
            crate::persistence::section::EDGE_OUT_CSR,
            &mut out_csr_payload,
        )?;
        let out_edge_count = self.out_csr.edge_count() as u32;
        super::persistence::write_pages_to_file(
            &path.join("out_csr.bin"),
            &out_csr_payload,
            page_size,
            level,
            out_edge_count,
        )?;

        let mut in_csr_payload = Vec::new();
        super::persistence::serialize_csr(
            &self.in_csr,
            crate::persistence::section::EDGE_IN_CSR,
            &mut in_csr_payload,
        )?;
        let in_edge_count = self.in_csr.edge_count() as u32;
        super::persistence::write_pages_to_file(
            &path.join("in_csr.bin"),
            &in_csr_payload,
            page_size,
            level,
            in_edge_count,
        )?;

        let mut props_payload = Vec::new();
        super::persistence::serialize_csr_properties(&mut self.properties, &mut props_payload)?;
        let edge_count = self.next_edge_id.0 as u32;
        super::persistence::write_pages_to_file(
            &path.join("properties.bin"),
            &props_payload,
            page_size,
            level,
            edge_count,
        )?;

        Ok(())
    }

    pub fn load<P: AsRef<std::path::Path>>(&mut self, path: P) -> StorageResult<()> {
        use std::io::Read;
        let path = path.as_ref();

        let meta_path = path.join("meta.bin");
        let (meta_data, _meta_rows) = super::persistence::read_pages_from_file(&meta_path)?;
        let mut meta_cursor = &meta_data[..];
        let mut header_buf = [0u8; crate::persistence::HEADER_SIZE];
        meta_cursor.read_exact(&mut header_buf)?;
        {
            let mut slice = &header_buf[..];
            let (_version, sid) = crate::persistence::read_header(&mut slice)?;
            if sid != crate::persistence::section::EDGE_META {
                return Err(StorageError::deserialize_error(format!(
                    "unexpected section id in edge meta: expected {:#06x}, got {:#06x}",
                    crate::persistence::section::EDGE_META,
                    sid
                )));
            }
        }

        let mut version_bytes = [0u8; 4];
        meta_cursor.read_exact(&mut version_bytes)?;
        let version = u32::from_le_bytes(version_bytes);
        if version != super::persistence::EDGE_META_VERSION {
            return Err(StorageError::deserialize_error(format!(
                "unsupported edge meta version: {}",
                version
            )));
        }

        let meta = super::persistence::load_metadata(&mut meta_cursor)?;

        self.label = meta.label;
        self.src_label = meta.src_label;
        self.dst_label = meta.dst_label;
        self.label_name = meta.label_name;
        self.is_open = meta.is_open;
        self.set_schema(meta.schema);
        self.next_edge_id = meta.next_edge_id;
        self.mvcc.edge_timestamps = meta.edge_timestamps;
        // The tombstone table is rebuilt from persisted deletion timestamps;
        // the GC watermark itself is runtime-only.
        self.mvcc.tombstones.clear();
        for (edge_id, ts) in self.mvcc.edge_timestamps.iter() {
            if ts.delete_ts != Timestamp::MAX {
                self.mvcc.tombstones.insert(*edge_id, ts.delete_ts);
            }
        }
        self.mvcc.min_active_snapshot_ts = Timestamp::MAX;
        self.mvcc.active_snapshots.clear();

        let out_csr_path = path.join("out_csr.bin");
        super::persistence::load_csr(&out_csr_path, &mut self.out_csr)?;

        let in_csr_path = path.join("in_csr.bin");
        super::persistence::load_csr(&in_csr_path, &mut self.in_csr)?;

        let props_path = path.join("properties.bin");
        self.properties = {
            let p = super::persistence::load_csr_properties(&props_path)?;
            let mut new_props = p;
            // Rebuild columns to match current schema if needed
            let current_schema_names: std::collections::HashSet<_> =
                self.schema.properties.iter().map(|p| &p.name).collect();
            let existing_names: std::collections::HashSet<_> = new_props
                .property_schema()
                .iter()
                .map(|s| &s.name)
                .collect();
            if current_schema_names != existing_names {
                // Schema mismatch: rebuild from schema
                let prop_schemas: Vec<crate::edge::property_schema::PropertySchema> = self
                    .schema
                    .properties
                    .iter()
                    .enumerate()
                    .map(|(i, p)| {
                        crate::edge::property_schema::PropertySchema::new(
                            p.name.clone(),
                            i as i32,
                            p.data_type.clone(),
                        )
                        .nullable(p.nullable)
                    })
                    .collect();
                new_props =
                    crate::edge::CsrWithProperties::new(new_props.vertex_capacity(), prop_schemas);
            }
            new_props
        };

        if self.next_edge_id.0 == 0 {
            let max_id = self
                .out_csr
                .iter_all()
                .map(|(_, nbr)| nbr.edge_id.0 + 1)
                .max()
                .unwrap_or(0);
            self.next_edge_id = EdgeId(max_id);
        }
        self.is_open = true;
        Ok(())
    }
}

#[cfg(test)]
#[path = "core_tests.rs"]
mod tests;
