//! Core EdgeStore operations: CRUD, properties, queries, and compaction.
//!
//! Node-group sharded edge table: one sharded CSR per direction plus
//! centralized row-level timestamps. There are no frozen segments, no
//! merges, and no cross-segment deduplication.

use super::super::{CsrBase, CsrShardSet, EdgeRecord, EdgeSchema, MutableCsrTrait, Nbr};
use super::mvcc::MVCCManager;
use super::schema_add_column::PendingAddColumn;
use super::staging::EdgeStagingBatch;
use crate::edge::property_schema::PropertySchema;
use crate::edge::{CsrWithProperties, VertexFragmentation};
use crate::index::edge_index_manager::EdgePropertyIndex;
use crate::schema::{ChangeDetails, LabelVersionHistory, PropertyChange, SchemaObjectType};
use crate::types::PropertyId;
use graphdb_core::types::{EdgeId, LabelId, Timestamp, VertexId};
use graphdb_core::{DataType, StorageError, StorageResult, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

pub use super::config::{AutoMaintenanceConfig, EdgeTableConfig, UpdateEdgePropertyByOffsetParams};
pub use super::iterator::EdgeTableScanIterator;

/// Node-group sharded edge store: one sharded CSR per direction with MVCC
/// row timestamps.
pub struct EdgeStore {
    pub label: LabelId,
    pub label_name: String,
    pub src_label: LabelId,
    pub dst_label: LabelId,
    pub schema: EdgeSchema,
    pub out_csr: CsrShardSet,
    pub in_csr: CsrShardSet,
    pub mvcc: MVCCManager,
    pub properties: CsrWithProperties,
    /// Whether property columns changed since the last checkpoint.
    pub properties_dirty: bool,
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
    /// In-flight staged add-column change. Memory-only: a crash before
    /// publishing is equivalent to aborting, because reload rebuilds the
    /// property store from the published schema.
    pub(crate) pending_add_column: Option<PendingAddColumn>,
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

        let mut out_csr = CsrShardSet::new(
            schema.oe_strategy,
            config.node_group_bits,
            config.overflow_chunk_edges,
        )?;
        let mut in_csr = CsrShardSet::new(
            schema.ie_strategy,
            config.node_group_bits,
            config.overflow_chunk_edges,
        )?;
        // Pre-create groups covering the configured initial row space so
        // small tables start with their full address range addressable.
        let initial_groups = config
            .initial_vertex_capacity
            .div_ceil(out_csr.group_size())
            .max(1);
        if schema.oe_strategy != super::super::EdgeStrategy::None {
            out_csr.resize_groups(initial_groups)?;
        }
        if schema.ie_strategy != super::super::EdgeStrategy::None {
            in_csr.resize_groups(initial_groups)?;
        }

        let prop_schemas: Vec<PropertySchema> = schema
            .properties
            .iter()
            .enumerate()
            .map(|(i, p)| {
                PropertySchema::new(p.name.clone(), i as i32, p.data_type.clone())
                    .nullable(p.nullable)
                    .with_default_value(p.default_value.clone())
            })
            .collect();
        let properties = CsrWithProperties::new(prop_schemas);

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
            properties_dirty: false,
            is_open: true,
            next_edge_id: EdgeId(0),
            config,
            stats_manager: None,
            version_history,
            property_index_cache,
            property_index: None,
            maintenance_serial: 0,
            last_gc_min_snapshot_ts: 0,
            pending_add_column: None,
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

    /// Single row-location entry for point lookups: topology lookup via the
    /// CSR plus the authoritative MVCC visibility check. Adjacency, existence
    /// and record reads must funnel through here rather than reading CSR
    /// timestamps directly.
    fn merged_get_edge(
        &self,
        csr: &CsrShardSet,
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

    fn merged_edges_of(&self, csr: &CsrShardSet, src: u32, ts: Timestamp) -> Vec<Nbr> {
        csr.edges_of(src, ts)
            .into_iter()
            .filter(|nbr| self.mvcc.is_edge_visible(nbr.edge_id, ts))
            .collect()
    }

    fn merged_edges_of_with_gate(
        &self,
        csr: &CsrShardSet,
        src: u32,
        ts: Timestamp,
        gate: &crate::mvcc_visibility::PendingGate<'_>,
    ) -> Vec<Nbr> {
        csr.edges_of(src, ts)
            .into_iter()
            .filter(|nbr| self.mvcc.is_edge_visible_with_gate(nbr.edge_id, ts, gate))
            .collect()
    }

    pub fn merged_out_nbrs_with_gate(
        &self,
        src: u32,
        ts: Timestamp,
        gate: &crate::mvcc_visibility::PendingGate<'_>,
    ) -> Vec<Nbr> {
        self.merged_edges_of_with_gate(&self.out_csr, src, ts, gate)
    }

    pub fn merged_in_nbrs_with_gate(
        &self,
        dst: u32,
        ts: Timestamp,
        gate: &crate::mvcc_visibility::PendingGate<'_>,
    ) -> Vec<Nbr> {
        self.merged_edges_of_with_gate(&self.in_csr, dst, ts, gate)
    }

    pub fn out_edges_with_gate(
        &self,
        src: u32,
        ts: Timestamp,
        gate: &crate::mvcc_visibility::PendingGate<'_>,
    ) -> Vec<EdgeRecord> {
        if !self.is_open {
            return Vec::new();
        }
        self.merged_out_nbrs_with_gate(src, ts, gate)
            .into_iter()
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

    pub fn in_edges_with_gate(
        &self,
        dst: u32,
        ts: Timestamp,
        gate: &crate::mvcc_visibility::PendingGate<'_>,
    ) -> Vec<EdgeRecord> {
        if !self.is_open {
            return Vec::new();
        }
        self.merged_in_nbrs_with_gate(dst, ts, gate)
            .into_iter()
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

    pub(crate) fn edge_record_from_nbr(
        &self,
        src: u32,
        nbr: Nbr,
        query_ts: Timestamp,
    ) -> EdgeRecord {
        self.edge_record_from_nbr_projected(src, nbr, query_ts, None)
    }

    pub(crate) fn edge_record_from_nbr_projected(
        &self,
        src: u32,
        nbr: Nbr,
        query_ts: Timestamp,
        projection: Option<&[String]>,
    ) -> EdgeRecord {
        let dst_vid = VertexId::from_int64(nbr.endpoint as i64);
        let rank = nbr.rank;
        let properties = self.properties_for_edge_projected(nbr.edge_id, query_ts, projection);
        EdgeRecord {
            src_vid: VertexId::from_int64(src as i64),
            dst_vid,
            rank,
            properties,
        }
    }

    fn properties_for_edge(&self, edge_id: EdgeId, query_ts: Timestamp) -> Vec<(String, Value)> {
        self.properties_for_edge_projected(edge_id, query_ts, None)
    }

    /// Topology-first property read: MVCC authority decides visibility,
    /// then only the projected columns are decoded. `None` decodes all
    /// columns, `Some(&[])` decodes none.
    fn properties_for_edge_projected(
        &self,
        edge_id: EdgeId,
        query_ts: Timestamp,
        projection: Option<&[String]>,
    ) -> Vec<(String, Value)> {
        // MVCCManager is the single visibility authority. The CSR row
        // timestamps are physical replicas kept in sync on the write path;
        // they must not decide query visibility here.
        if !self.mvcc.is_edge_visible(edge_id, query_ts) {
            return Vec::new();
        }
        // Snapshot read through the property version chain so an old reader
        // observes the before-image, not the latest write.
        self.properties
            .get_projected_by_edge_id(edge_id, query_ts, projection)
            .map(|rows| {
                rows.into_iter()
                    .filter_map(|(name, value)| value.map(|v| (name, v)))
                    .collect()
            })
            .unwrap_or_default()
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

        if self.schema.oe_strategy == super::super::EdgeStrategy::None {
            return Err(StorageError::invalid_operation(
                "Cannot insert edge: out-edge strategy is None".to_string(),
            ));
        }

        // Single-entry staging commit: the batch owns the whole multi-step
        // write, so failure handling lives in one place below instead of in
        // per-step compensation branches here.
        let mut batch = EdgeStagingBatch::new();
        batch.stage_insert(src, dst, rank, property_values, ts);
        self.commit_staging_batch(batch).map(|_| ())
    }

    /// Create an empty staging batch for one atomic group of edge writes.
    pub fn staging_batch() -> EdgeStagingBatch {
        EdgeStagingBatch::new()
    }

    /// Commit one staging batch atomically.
    ///
    /// Entries are validated and moved into the committed topology, property
    /// rows, and visibility records in order. When any entry fails, only the
    /// entries this batch already applied are rolled back; committed data
    /// from other batches is never touched. Dropping a batch without
    /// committing discards it with no residue.
    ///
    /// Returns the number of applied entries (inserts plus deletes).
    pub fn commit_staging_batch(&mut self, mut batch: EdgeStagingBatch) -> StorageResult<usize> {
        if !self.is_open {
            return Err(StorageError::storage_not_open());
        }
        if batch.insert_count() > 0 && self.schema.oe_strategy == super::super::EdgeStrategy::None {
            return Err(StorageError::invalid_operation(
                "Cannot insert edge: out-edge strategy is None".to_string(),
            ));
        }
        let max_ts = batch.max_timestamp();
        let inserts = batch.take_inserts();
        let deletes = batch.take_deletes();
        if inserts.is_empty() && deletes.is_empty() {
            return Ok(0);
        }

        let mut applied_inserts: Vec<(u32, u32, i64, EdgeId, Timestamp)> =
            Vec::with_capacity(inserts.len());
        for ins in &inserts {
            match self.apply_staged_insert(
                ins.src,
                ins.dst,
                ins.rank,
                &ins.properties,
                ins.create_ts,
            ) {
                Ok(edge_id) => {
                    applied_inserts.push((ins.src, ins.dst, ins.rank, edge_id, ins.create_ts));
                }
                Err(e) => {
                    for (src, dst, rank, edge_id, ts) in applied_inserts {
                        self.erase_applied_insert(src, dst, rank, edge_id, ts);
                    }
                    return Err(e);
                }
            }
        }

        let mut applied_deletes: Vec<(u32, u32, i64, EdgeId, Timestamp)> =
            Vec::with_capacity(deletes.len());
        for del in &deletes {
            match self.apply_staged_delete(del.src, del.dst, del.rank, del.delete_ts) {
                Ok(Some(edge_id)) => {
                    applied_deletes.push((del.src, del.dst, del.rank, edge_id, del.delete_ts));
                }
                Ok(None) => {}
                Err(e) => {
                    for (src, dst, _rank, edge_id, ts) in applied_deletes {
                        self.revert_applied_delete(src, dst, edge_id, ts);
                    }
                    for (src, dst, rank, edge_id, ts) in applied_inserts {
                        self.erase_applied_insert(src, dst, rank, edge_id, ts);
                    }
                    return Err(e);
                }
            }
        }

        let applied = applied_inserts.len() + applied_deletes.len();
        if applied > 0 {
            if let Some(ts) = max_ts {
                self.check_and_apply_write_backpressure(ts);
            }
            self.maybe_run_auto_maintenance();
            for (_, _, _, edge_id, _) in applied_inserts.iter().chain(applied_deletes.iter()) {
                self.debug_assert_copies_consistent(*edge_id);
            }
        }
        Ok(applied)
    }

    fn convert_property_values(
        &self,
        property_values: &[(String, Value)],
    ) -> StorageResult<Vec<(String, Value)>> {
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
        Ok(converted_values)
    }

    /// Move one staged insert into the committed structures.
    ///
    /// Self-contained: a failure cleans up only this entry, so the batch
    /// rollback above only handles entries that fully applied.
    fn apply_staged_insert(
        &mut self,
        src: u32,
        dst: u32,
        rank: i64,
        property_values: &[(String, Value)],
        ts: Timestamp,
    ) -> StorageResult<EdgeId> {
        let converted_values = self.convert_property_values(property_values)?;
        let edge_id = self.next_edge_id.fetch_add();

        // Pre-flight duplicate check before touching any shared state so a
        // failed insert leaves no partial record behind.
        if self.has_edge(src, dst, rank, ts) {
            return Err(StorageError::edge_already_exists(format!(
                "{} -> {}@{}",
                src, dst, rank
            )));
        }

        self.mvcc.record_creation(edge_id, ts);

        if let Err(e) = self
            .properties
            .insert_for_edge(edge_id, &converted_values, ts)
        {
            self.mvcc.remove_edge_timestamps(edge_id);
            return Err(e);
        }

        let dst_key = Self::edge_endpoint_key(dst, rank);
        let src_key = Self::edge_endpoint_key(src, rank);
        if let Err(e) = self.out_csr.insert_edge(src, dst_key, edge_id, ts) {
            if let Some(row) = self.properties.remove_edge_mapping(edge_id) {
                self.properties.release_row(row);
            }
            self.mvcc.remove_edge_timestamps(edge_id);
            self.mark_properties_dirty();
            self.debug_assert_copies_consistent(edge_id);
            return Err(e);
        }

        if let Err(e) = self.in_csr.insert_edge(dst, src_key, edge_id, ts) {
            // Roll back the out-direction insertion physically so no
            // tombstone residue remains; fall back to logical deletion if
            // the entry cannot be located.
            if !self.out_csr.remove_edge(src, edge_id) {
                let _ = self.out_csr.delete_edge(src, edge_id, ts);
            }
            if let Some(row) = self.properties.remove_edge_mapping(edge_id) {
                self.properties.release_row(row);
            }
            let _ = self.properties.mark_deleted(edge_id, ts);
            self.mvcc.remove_edge_timestamps(edge_id);
            self.mark_properties_dirty();
            self.debug_assert_copies_consistent(edge_id);
            return Err(e);
        }

        if let Some(ref mut index) = self.property_index {
            for (prop_name, prop_value) in &converted_values {
                let _ = index.insert(prop_name, prop_value, src, dst, rank, self.label, ts);
            }
        }

        self.mark_properties_dirty();
        self.debug_assert_copies_consistent(edge_id);
        Ok(edge_id)
    }

    /// Move one staged delete into the committed structures.
    ///
    /// Returns the deleted edge id, or `None` when no edge matched.
    fn apply_staged_delete(
        &mut self,
        src: u32,
        dst: u32,
        rank: i64,
        ts: Timestamp,
    ) -> StorageResult<Option<EdgeId>> {
        let dst_key = Self::edge_endpoint_key(dst, rank);
        let src_key = Self::edge_endpoint_key(src, rank);

        let edge_properties = if self.property_index.is_some() {
            self.get_edge(src, dst, rank, ts).map(|e| e.properties)
        } else {
            None
        };

        if let Some(nbr) = self.out_csr.get_edge(src, dst_key, ts) {
            let edge_id = nbr.edge_id;

            if !self.out_csr.delete_edge(src, edge_id, ts)? {
                return Ok(None);
            }
            if !self.in_csr.delete_edge_by_dst(dst, src_key, ts) {
                // Roll back the out-direction deletion to keep both sides
                // consistent.
                self.out_csr.revert_delete_by_edge_id(src, edge_id, ts);
                return Ok(None);
            }

            self.mvcc.record_edge_deletion(edge_id, ts);
            let _ = self.properties.mark_deleted(edge_id, ts);
            self.update_property_index_on_delete(&edge_properties, src, dst, rank, ts);
            self.mark_properties_dirty();
            self.debug_assert_copies_consistent(edge_id);
            return Ok(Some(edge_id));
        }

        Ok(None)
    }

    /// Erase one batch-applied insert during batch rollback.
    ///
    /// Physical removal across all copies; tolerates absence so rollback
    /// stays total even under partial application.
    fn erase_applied_insert(
        &mut self,
        src: u32,
        dst: u32,
        rank: i64,
        edge_id: EdgeId,
        ts: Timestamp,
    ) {
        let properties = self
            .properties
            .read_properties_by_edge_id(edge_id)
            .unwrap_or_default();
        self.out_csr.remove_edge(src, edge_id);
        self.in_csr.remove_edge(dst, edge_id);
        if let Some(row) = self.properties.remove_edge_mapping(edge_id) {
            self.properties.release_row(row);
        }
        self.mvcc.remove_edge_timestamps(edge_id);
        self.mvcc.remove_deletion(edge_id);
        if let Some(ref mut index) = self.property_index {
            for (prop_name, prop_value) in &properties {
                let _ = index.delete(prop_name, prop_value, src, dst, rank, ts);
            }
        }
        self.mark_properties_dirty();
        self.debug_assert_copies_consistent(edge_id);
    }

    /// Revert one batch-applied delete during batch rollback.
    fn revert_applied_delete(&mut self, src: u32, dst: u32, edge_id: EdgeId, ts: Timestamp) {
        self.out_csr.revert_delete_by_edge_id(src, edge_id, ts);
        self.in_csr.revert_delete_by_edge_id(dst, edge_id, ts);
        self.mvcc.remove_deletion(edge_id);
        if let Some(ts_info) = self.mvcc.edge_timestamps.get_mut(&edge_id) {
            ts_info.delete_ts = Timestamp::MAX;
        }
        let _ = self.properties.revert_deletion_for_edge(edge_id);
        self.mark_properties_dirty();
        self.debug_assert_copies_consistent(edge_id);
    }

    /// Per-vertex fragmentation view combining both directions.
    ///
    /// Backs the per-vertex collection trigger with one observation entry
    /// per row; `cutoff` decides the reclaimable count.
    pub fn vertex_fragmentation(&self, vid: u32, cutoff: Timestamp) -> VertexFragmentation {
        let (out_live, out_dead, out_cap) = self.out_csr.vertex_census(vid);
        let (in_live, in_dead, in_cap) = self.in_csr.vertex_census(vid);
        VertexFragmentation {
            vertex: vid,
            live_edges: out_live + in_live,
            dead_entries: out_dead + in_dead,
            capacity: out_cap + in_cap,
            reclaimable: self.out_csr.reclaimable_count(vid, cutoff)
                + self.in_csr.reclaimable_count(vid, cutoff),
        }
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

        // Single-entry staging commit: the batch owns the two-direction
        // write, so the out/in rollback lives in one place.
        let mut batch = EdgeStagingBatch::new();
        batch.stage_delete(src, dst, rank, ts);
        Ok(self.commit_staging_batch(batch)? > 0)
    }

    /// Physically erase an edge inserted by an uncommitted transaction.
    ///
    /// Insert-undo path: unlike a user delete (logical deletion through
    /// `delete_edge`), aborting an insert must leave no trace in any of the
    /// three copies — otherwise the aborted edge would stay visible inside
    /// its snapshot window and leave a permanent tombstone. Every step
    /// tolerates absence, so replaying the undo (abort re-drive, WAL
    /// recovery re-application) is idempotent.
    pub fn erase_edge(&mut self, src: u32, dst: u32, rank: i64, ts: Timestamp) -> bool {
        let Some(edge_id) = self.edge_id_of(src, dst, rank, ts) else {
            return false;
        };
        let properties = self
            .properties
            .read_properties_by_edge_id(edge_id)
            .unwrap_or_default();
        self.out_csr.remove_edge(src, edge_id);
        self.in_csr.remove_edge(dst, edge_id);
        if let Some(row) = self.properties.remove_edge_mapping(edge_id) {
            self.properties.release_row(row);
        }
        self.mvcc.remove_edge_timestamps(edge_id);
        self.mvcc.remove_deletion(edge_id);
        if let Some(ref mut index) = self.property_index {
            for (prop_name, prop_value) in &properties {
                let _ = index.delete(prop_name, prop_value, src, dst, rank, ts);
            }
        }
        self.debug_assert_copies_consistent(edge_id);
        true
    }

    /// Debug-only cross-copy consistency check for one edge.
    ///
    /// Release builds skip the whole body (zero overhead): every arm is a
    /// `debug_assert`. Tombstone presence must agree with the authoritative
    /// `edge_timestamps` deletion stamp in both directions, and a property
    /// row mapping must never outlive its authority entry (orphan row).
    /// Called on insert success, delete success and delete-rollback success.
    fn debug_assert_copies_consistent(&self, edge_id: EdgeId) {
        debug_assert!(
            !self.mvcc.tombstones.contains_key(&edge_id)
                || self
                    .mvcc
                    .edge_timestamps
                    .get(&edge_id)
                    .is_some_and(|ts| ts.delete_ts != Timestamp::MAX),
            "tombstone without authoritative deletion stamp"
        );
        debug_assert!(
            !self
                .mvcc
                .edge_timestamps
                .get(&edge_id)
                .is_some_and(|ts| ts.delete_ts != Timestamp::MAX)
                || self.mvcc.tombstones.contains_key(&edge_id),
            "authoritative deletion without tombstone entry"
        );
        debug_assert!(
            self.properties.get_row_for_edge(edge_id).is_none()
                || self.mvcc.edge_timestamps.contains_key(&edge_id),
            "property row mapping without authority entry"
        );
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
            self.mark_properties_dirty();
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
                // Restore the property-index entries the delete path
                // removed, mirroring the slow path below.
                let restored = self.properties_for_edge(nbr.edge_id, ts);
                if let Some(ref mut index) = self.property_index {
                    for (prop_name, prop_value) in restored {
                        let _ =
                            index.insert(&prop_name, &prop_value, src, dst, rank, self.label, ts);
                    }
                }
                self.mark_properties_dirty();
                self.debug_assert_copies_consistent(nbr.edge_id);
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
        self.mark_properties_dirty();
        self.debug_assert_copies_consistent(edge_id);
        Ok(true)
    }

    /// Resolve the edge id for `(src, dst, rank)` without decoding properties.
    ///
    /// Operation-layer point lookups use it to recheck the fetched record
    /// through the pending-aware gate
    /// (`MVCCManager::is_edge_visible_with_gate`).
    pub fn edge_id_of(&self, src: u32, dst: u32, rank: i64, ts: Timestamp) -> Option<EdgeId> {
        if !self.is_open {
            return None;
        }
        let dst_key = Self::edge_endpoint_key(dst, rank);
        self.out_csr
            .get_edge(src, dst_key, ts)
            .map(|nbr| nbr.edge_id)
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

    pub fn get_edge_with_gate(
        &self,
        src: u32,
        dst: u32,
        rank: i64,
        ts: Timestamp,
        gate: &crate::mvcc_visibility::PendingGate<'_>,
    ) -> Option<EdgeRecord> {
        if !self.is_open {
            return None;
        }
        let dst_key = Self::edge_endpoint_key(dst, rank);
        let nbr = self.out_csr.get_edge(src, dst_key, ts)?;
        if !self.mvcc.is_edge_visible_with_gate(nbr.edge_id, ts, gate) {
            return None;
        }
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

    pub fn scan_with_gate(
        &self,
        ts: Timestamp,
        gate: &crate::mvcc_visibility::PendingGate<'_>,
    ) -> Vec<EdgeRecord> {
        if !self.is_open {
            return Vec::new();
        }
        let mut records = Vec::new();
        for (src_vid, nbr) in self.out_csr.iter(ts) {
            if !self.mvcc.is_edge_visible_with_gate(nbr.edge_id, ts, gate) {
                continue;
            }
            records.push(self.edge_record_from_nbr(
                src_vid.as_int64().unwrap_or(0) as u32,
                nbr,
                ts,
            ));
        }
        records
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
    pub(crate) fn record_schema_change(&mut self, details: ChangeDetails) -> StorageResult<()> {
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
        // Single code path: the immediate add is prepare + fill + publish of
        // the staged state machine, so no second column-construction
        // implementation exists.
        self.prepare_add_property(name.clone(), data_type.clone(), nullable, None)?;
        if let Err(error) = self.fill_pending_add_property() {
            let _ = self.abort_pending_add_property();
            return Err(error);
        }
        self.publish_pending_add_property()
    }

    /// Select and persist encodings for every property column.
    ///
    /// Explicit maintenance operation shared with the checkpoint path: hot
    /// columns stay unencoded between runs by design so everyday writes never
    /// pay re-encoding. Returns the number of columns that received an
    /// encoding and marks properties dirty when at least one did.
    pub fn encode_property_columns(&mut self) -> usize {
        let encoded = self.properties.auto_encode_properties();
        if encoded > 0 {
            self.mark_properties_dirty();
        }
        encoded
    }

    /// Recompute persisted per-column statistics from current contents.
    pub fn refresh_property_stats(&mut self) {
        self.properties.refresh_column_stats();
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
        self.mark_properties_dirty();

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
        self.mark_properties_dirty();

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
            self.mark_properties_dirty();
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
            self.mark_properties_dirty();

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
        self.used_memory_size()
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

        // Whole-table waste is an observation metric from the live
        // structures, never estimated from empty counters.
        let mut total_capacity = 0usize;
        let mut total_wasted = 0usize;
        for csr in [&self.out_csr, &self.in_csr] {
            if let Some(stats) = csr.fragmentation_stats() {
                total_capacity += stats.total_capacity;
                total_wasted += stats.wasted_capacity;
            }
        }
        if total_capacity > 0 && total_wasted as f32 / total_capacity as f32 > 0.5 {
            log::debug!(
                "EdgeTable[{}] high waste: {}/{} slots",
                self.label,
                total_wasted,
                total_capacity
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

    /// Write-path fast path: bound comes from the per-table pin cache, with
    /// no global watermark capture. Background passes use the watermark
    /// variant below.
    pub fn maybe_run_auto_maintenance(&mut self) -> usize {
        let bound = self.mvcc.min_active_snapshot_ts;
        self.run_auto_maintenance_pass(bound)
    }

    fn run_auto_maintenance_pass(&mut self, bound: Timestamp) -> usize {
        let cfg = self.config.auto_maintenance;
        if cfg.tombstone_gc_threshold == 0 {
            return 0;
        }
        // Serial must advance on every call; gating on a counter that only
        // advances when work was found turns the cooldown into a per-write
        // full scan.
        self.maintenance_serial = self.maintenance_serial.saturating_add(1);
        let cooldown_due =
            cfg.gc_min_serial > 0 && self.maintenance_serial.is_multiple_of(cfg.gc_min_serial);
        let mut maintenance_ran = 0;

        if self.mvcc.total_tombstone_count() > cfg.tombstone_gc_threshold
            && bound < Timestamp::MAX
            && (bound != self.last_gc_min_snapshot_ts || cooldown_due)
        {
            let cleaned = self.mvcc.gc_tombstones(bound);
            self.last_gc_min_snapshot_ts = bound;
            if cleaned > 0 {
                maintenance_ran += 1;
            }
        }

        // Reclaim edge-property before-images that no active snapshot can
        // observe. Runs on the same watermark as tombstone GC so version
        // chains cannot grow without bound while snapshots are short-lived.
        if bound != Timestamp::MAX && (cooldown_due || cfg.gc_min_serial == 0) {
            let removed = self.properties.gc_property_versions(bound);
            if removed > 0 {
                self.mark_properties_dirty();
                maintenance_ran += 1;
            }
        }

        if cfg.property_compact_ratio > 0.0
            && bound != Timestamp::MAX
            && (cooldown_due || cfg.gc_min_serial == 0)
        {
            let prop_stats = self.properties.compaction_stats();
            if prop_stats.fragmentation_ratio() >= cfg.property_compact_ratio as f64 {
                self.compact_properties(bound);
                maintenance_ran += 1;
            }
        }

        // Incremental row reclaim on the write path: only rows holding
        // entries eligible at this cutoff are visited, and each pass stops
        // after a bounded row count so small writes never cause large
        // rebuilds. Skipped entirely while no tombstone exists.
        if self.run_vertex_reclaim_pass(bound) {
            maintenance_ran += 1;
        }

        maintenance_ran
    }

    /// Background variant: bound comes from one per-pass watermark capture
    /// shared across all tables.
    pub fn maybe_run_auto_maintenance_with_watermarks(
        &mut self,
        watermarks: &graphdb_transaction::MvccWatermarks,
        margin: Timestamp,
    ) -> usize {
        let bound = watermarks.safe_gc_timestamp_with_margin(margin);
        self.run_auto_maintenance_pass(bound)
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
        let path = path.as_ref();
        let crate::compression::CompressionType::Zstd { level } = compression;
        let page_size = crate::compression::DEFAULT_PAGE_SIZE;
        self.flush_incremental(path, page_size, level)
    }

    pub fn load<P: AsRef<std::path::Path>>(&mut self, path: P) -> StorageResult<()> {
        self.load_incremental(path.as_ref())
    }

    /// Fail-closed cross-copy audit used by [`EdgeStore::load`].
    ///
    /// Returns `(orphan property mappings, orphan CSR rows, tombstone /
    /// authority mismatches)`. Tombstones are rebuilt from the authority
    /// table on load, so a nonzero mismatch count signals file corruption or
    /// a write-path regression; callers reject the load.
    pub fn loaded_copy_mismatches(&self) -> (usize, usize, usize) {
        let orphan_mappings = self
            .properties
            .edge_ids()
            .filter(|edge_id| !self.mvcc.edge_timestamps.contains_key(edge_id))
            .count();
        let mut orphan_csr_rows = 0;
        for (_, nbr) in self.out_csr.iter_all().chain(self.in_csr.iter_all()) {
            if !self.mvcc.edge_timestamps.contains_key(&nbr.edge_id) {
                orphan_csr_rows += 1;
            }
        }
        let tombstone_mismatches = self
            .mvcc
            .tombstones
            .iter()
            .filter(|(edge_id, delete_ts)| {
                self.mvcc
                    .edge_timestamps
                    .get(edge_id)
                    .map_or(true, |ts| ts.delete_ts != **delete_ts)
            })
            .count();
        (orphan_mappings, orphan_csr_rows, tombstone_mismatches)
    }
}

#[cfg(test)]
#[path = "core_tests.rs"]
mod tests;
