use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use super::super::{
    CsrBase, CsrVariant, EdgeRecord, EdgeSchema, MutableCsrTrait, Nbr, PropertyTable,
};
use super::core::EdgeTableConfig;
use super::snapshot::{ExportedEdgeSnapshot, SnapshotBuilder};
use super::stats::{DeletionStats, MergeMetrics, MergeMetricsResult, MergeStats};
use super::CompactionMode;
use crate::core::types::{
    CompactConfig, EdgeId, LabelId, Timestamp, VertexId,
};
use crate::core::{DataType, StorageError, StorageResult, Value};
use crate::storage::persistence::write_header_to;
use crate::storage::schema::{
    ChangeDetails, LabelVersionHistory, PropertyChange, SchemaObjectType,
};
use crate::storage::types::{PropertyId, StoragePropertyDef};

pub struct SimpleEdgeStore {
    pub label: LabelId,
    pub label_name: String,
    pub src_label: LabelId,
    pub dst_label: LabelId,
    pub schema: EdgeSchema,
    pub out_csr: CsrVariant,
    pub in_csr: CsrVariant,
    pub properties: PropertyTable,
    pub is_open: bool,
    pub next_edge_id: EdgeId,
    pub config: EdgeTableConfig,
    pub stats_manager: Option<Arc<crate::core::stats::StatsManager>>,
    pub deleted_edges: HashSet<EdgeId>,
    pub version_history: Arc<Mutex<LabelVersionHistory>>,
    pub property_index_cache: HashMap<String, usize>,
}

impl SimpleEdgeStore {
    pub fn new(schema: EdgeSchema) -> StorageResult<Self> {
        Self::with_config(schema, EdgeTableConfig::default())
    }

    pub fn with_config(schema: EdgeSchema, config: EdgeTableConfig) -> StorageResult<Self> {
        schema.validate()?;

        let out_csr = CsrVariant::from_strategy(
            schema.oe_strategy,
            config.initial_vertex_capacity,
            config.initial_edge_capacity,
        )?;
        let in_csr = CsrVariant::from_strategy(
            schema.ie_strategy,
            config.initial_vertex_capacity,
            config.initial_edge_capacity,
        )?;

        let mut properties = PropertyTable::with_capacity(config.initial_edge_capacity);
        for prop in &schema.properties {
            properties.add_property(prop.name.clone(), prop.data_type.clone(), prop.nullable);
        }

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
            properties,
            is_open: true,
            next_edge_id: EdgeId(0),
            config,
            stats_manager: None,
            deleted_edges: HashSet::new(),
            version_history,
            property_index_cache,
        })
    }

    pub fn set_stats_manager(&mut self, stats: Arc<crate::core::stats::StatsManager>) {
        self.stats_manager = Some(stats);
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

    pub fn label_name(&self) -> &str {
        &self.label_name
    }

    pub fn schema(&self) -> &EdgeSchema {
        &self.schema
    }

    pub fn schema_mut(&mut self) -> &mut EdgeSchema {
        &mut self.schema
    }

    pub fn set_schema(&mut self, schema: EdgeSchema) {
        self.property_index_cache.clear();
        for (idx, prop) in schema.properties.iter().enumerate() {
            self.property_index_cache.insert(prop.name.clone(), idx);
        }
        self.schema = schema;
    }

    pub fn version_history_ref(&self) -> Arc<Mutex<LabelVersionHistory>> {
        Arc::clone(&self.version_history)
    }

    pub fn edge_count(&self) -> u64 {
        self.out_csr.edge_count() - self.deleted_edges.len() as u64
    }

    pub fn delta_edge_count(&self) -> u64 {
        self.out_csr.edge_count() + self.in_csr.edge_count()
    }

    pub fn memory_size(&self) -> usize {
        self.used_memory_size()
    }

    pub fn used_memory_size(&self) -> usize {
        let mut total = 0;
        total += self.out_csr.used_memory_size();
        total += self.in_csr.used_memory_size();
        total += self.deleted_edges.len() * (std::mem::size_of::<EdgeId>() + std::mem::size_of::<Timestamp>());
        total += self.properties.used_memory_size();
        total += self.property_index_cache.len()
            * (std::mem::size_of::<String>() + std::mem::size_of::<usize>());
        total
    }

    pub fn mutable_csr_memory_size(&self) -> usize {
        self.out_csr.used_memory_size() + self.in_csr.used_memory_size()
    }

    pub fn estimate_memory_usage(&self) -> usize {
        let out_edges = self.out_csr.edge_count() as usize;
        let in_edges = self.in_csr.edge_count() as usize;
        out_edges * self.out_csr.bytes_per_edge() + in_edges * self.in_csr.bytes_per_edge()
    }

    pub fn validate_segment_integrity(&self) -> usize {
        0
    }

    pub fn segment_versions(&self) -> Vec<(usize, u32)> {
        Vec::new()
    }

    pub fn update_segment_checksums(&mut self) {}

    pub fn freeze_csr_only(&mut self, _ts: Timestamp) -> usize {
        0
    }

    pub fn compact_and_freeze(&mut self, _ts: Timestamp, _config: &CompactConfig, _mode: CompactionMode) -> usize {
        0
    }

    pub fn compact_properties(&mut self, _ts: Timestamp) {}

    pub fn compact_csr_only(&mut self, _ts: Timestamp, _reserve_ratio: f32) -> usize {
        0
    }

    pub fn maybe_compact_for_flush(&mut self, _ts: Timestamp, _threshold: f32) {}

    pub fn merge_segments_lsm_tiered(&mut self, _current_ts: Timestamp) -> usize {
        0
    }

    pub fn merge_segments_adaptive(
        &mut self,
        _current_ts: Timestamp,
        _max_segment_age: Timestamp,
        _deletion_threshold: f64,
        _max_segment_size_bytes: usize,
    ) -> usize {
        0
    }

    pub fn merge_segments_with_config(
        &mut self,
        _time_threshold: Timestamp,
        _size_threshold_bytes: usize,
    ) -> MergeMetricsResult {
        MergeMetricsResult {
            metrics: MergeMetrics {
                segments_before: 0,
                segments_after: 0,
                edges_merged: 0,
                duration_ms: 0,
            },
            segments_reduced: 0,
        }
    }

    pub fn merge_segments_with_config_and_deletion_filter(
        &mut self,
        _time_threshold: Timestamp,
        _size_threshold_bytes: usize,
        _min_active_snapshot_ts: Option<Timestamp>,
    ) -> MergeMetricsResult {
        MergeMetricsResult {
            metrics: MergeMetrics {
                segments_before: 0,
                segments_after: 0,
                edges_merged: 0,
                duration_ms: 0,
            },
            segments_reduced: 0,
        }
    }

    pub fn merge_stats(&self) -> MergeStats {
        MergeStats::default()
    }

    pub fn deletion_stats(&self) -> DeletionStats {
        DeletionStats::default()
    }

    pub fn register_snapshot(&mut self, _ts: Timestamp) {}

    pub fn unregister_snapshot(&mut self, _ts: Timestamp) {}

    pub fn snapshot_handle(&mut self, _ts: Timestamp) -> SimpleSnapshotHandle<'_> {
        SimpleSnapshotHandle { _table: self }
    }

    pub fn export_snapshot(&self, ts: Timestamp) -> StorageResult<ExportedEdgeSnapshot> {
        let out_edges: Vec<(u32, Nbr)> = self
            .out_csr
            .iter(ts)
            .map(|(src, nbr)| {
                let src_u32 = src.as_int64().unwrap_or(0) as u32;
                (src_u32, nbr)
            })
            .collect();
        let in_edges: Vec<(u32, Nbr)> = self
            .in_csr
            .iter(ts)
            .map(|(src, nbr)| {
                let src_u32 = src.as_int64().unwrap_or(0) as u32;
                (src_u32, nbr)
            })
            .collect();

        let out_csr = SnapshotBuilder::build_csr(out_edges, self.out_csr.vertex_capacity())?;
        let in_csr = SnapshotBuilder::build_csr(in_edges, self.in_csr.vertex_capacity())?;

        Ok(ExportedEdgeSnapshot {
            snapshot_ts: ts,
            label: self.label,
            out_csr,
            in_csr,
            properties: self.properties.clone(),
            schema: self.schema.clone(),
        })
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

        let prop_offset = if !converted_values.is_empty() {
            self.properties.insert(&converted_values, ts)?
        } else {
            0
        };

        if self.has_edge_internal(src, dst, rank) {
            if prop_offset > 0 {
                self.properties.delete(prop_offset);
            }
            return Err(StorageError::edge_already_exists(format!(
                "{} -> {}@{}", src, dst, rank
            )));
        }

        let dst_key = Self::edge_endpoint_key(dst, rank);
        let src_key = Self::edge_endpoint_key(src, rank);
        let edge_id = self.next_edge_id.fetch_add();

        if !self.out_csr.insert_edge(src, dst_key, edge_id, prop_offset, ts) {
            if prop_offset > 0 {
                self.properties.delete(prop_offset);
            }
            return Err(StorageError::edge_already_exists(format!(
                "{} -> {}@{}", src, dst, rank
            )));
        }

        if !self.in_csr.insert_edge(dst, src_key, edge_id, prop_offset, ts) {
            self.out_csr.delete_edge(src, edge_id, ts);
            if prop_offset > 0 {
                self.properties.delete(prop_offset);
            }
            return Err(StorageError::edge_already_exists(format!(
                "{} -> {}@{}", dst, src, rank
            )));
        }

        Ok(())
    }

    pub fn delete_edge(&mut self, src: u32, dst: u32, rank: i64, ts: Timestamp) -> StorageResult<bool> {
        if !self.is_open {
            return Err(StorageError::storage_not_open());
        }
        let dst_key = Self::edge_endpoint_key(dst, rank);
        let src_key = Self::edge_endpoint_key(src, rank);
        if let Some(nbr) = self.out_csr.get_edge(src, dst_key, ts) {
            let edge_id = nbr.edge_id;
            self.out_csr.delete_edge(src, edge_id, ts);
            self.in_csr.delete_edge_by_dst(dst, src_key, ts);
            self.deleted_edges.insert(edge_id);
            return Ok(true);
        }
        Ok(false)
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
            self.deleted_edges.insert(nbr.edge_id);
            self.out_csr.delete_edge_by_offset(src, oe_offset, ts);
            self.in_csr.delete_edge_by_offset(dst, ie_offset, ts);
            return Ok(true);
        }
        Ok(false)
    }

    pub fn revert_delete_edge_by_offset(
        &mut self,
        src: u32,
        dst: u32,
        _rank: i64,
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
        }
        Ok(reverted)
    }

    fn edge_endpoint_key(endpoint: u32, rank: i64) -> VertexId {
        let mut data = Vec::with_capacity(16);
        data.extend_from_slice(&(endpoint as i64).to_be_bytes());
        data.extend_from_slice(&rank.to_be_bytes());
        VertexId::from_bytes(data)
    }

    fn decode_edge_endpoint(key: VertexId) -> (VertexId, i64) {
        let bytes = key.as_bytes();
        if bytes.len() != 16 {
            return (key, 0);
        }
        let mut endpoint_bytes = [0u8; 8];
        endpoint_bytes.copy_from_slice(&bytes[..8]);
        let mut rank_bytes = [0u8; 8];
        rank_bytes.copy_from_slice(&bytes[8..16]);
        (
            VertexId::from_int64(i64::from_be_bytes(endpoint_bytes)),
            i64::from_be_bytes(rank_bytes),
        )
    }

    fn properties_for_offset(&self, prop_offset: u32) -> Vec<(String, Value)> {
        if prop_offset == 0 {
            return Vec::new();
        }
        self.properties
            .get(prop_offset, None)
            .map(|props| props.into_iter().filter_map(|(k, v)| v.map(|v| (k, v))).collect())
            .unwrap_or_default()
    }

    fn has_edge_internal(&self, src: u32, dst: u32, rank: i64) -> bool {
        let dst_key = Self::edge_endpoint_key(dst, rank);
        self.out_csr
            .get_edge(src, dst_key, u32::MAX)
            .is_some_and(|nbr| !self.deleted_edges.contains(&nbr.edge_id))
    }

    pub fn get_edge(&self, src: u32, dst: u32, rank: i64, ts: Timestamp) -> Option<EdgeRecord> {
        if !self.is_open {
            return None;
        }
        let dst_key = Self::edge_endpoint_key(dst, rank);
        let nbr = self.out_csr.get_edge(src, dst_key, ts)?;
        if self.deleted_edges.contains(&nbr.edge_id) {
            return None;
        }
        let properties = self.properties_for_offset(nbr.prop_offset);
        Some(EdgeRecord {
            src_vid: VertexId::from_int64(src as i64),
            dst_vid: VertexId::from_int64(dst as i64),
            rank,
            properties,
        })
    }

    pub fn has_edge(&self, src: u32, dst: u32, rank: i64, ts: Timestamp) -> bool {
        if !self.is_open {
            return false;
        }
        let dst_key = Self::edge_endpoint_key(dst, rank);
        self.out_csr
            .get_edge(src, dst_key, ts)
            .is_some_and(|nbr| !self.deleted_edges.contains(&nbr.edge_id))
    }

    pub fn out_edges(&self, src: u32, ts: Timestamp) -> Vec<EdgeRecord> {
        if !self.is_open {
            return Vec::new();
        }

        let nbrs: Vec<Nbr> = self
            .out_csr
            .edges_of(src, ts)
            .iter()
            .filter(|edge| !self.deleted_edges.contains(&edge.edge_id))
            .copied()
            .collect();

        let prop_offsets: Vec<_> = nbrs.iter().map(|nbr| nbr.prop_offset).collect();
        if !prop_offsets.is_empty() {
            self.properties.prefetch_batch(&prop_offsets);
        }

        nbrs.into_iter()
            .map(|nbr| {
                let (dst_vid, rank) = Self::decode_edge_endpoint(nbr.neighbor);
                let properties = self
                    .properties
                    .get_fast(nbr.prop_offset, None)
                    .or_else(|| self.properties.get(nbr.prop_offset, None))
                    .map(|props| props.into_iter().filter_map(|(k, v)| v.map(|v| (k, v))).collect())
                    .unwrap_or_default();
                EdgeRecord {
                    src_vid: VertexId::from_int64(src as i64),
                    dst_vid,
                    rank,
                    properties,
                }
            })
            .collect()
    }

    pub fn in_edges(&self, dst: u32, ts: Timestamp) -> Vec<EdgeRecord> {
        if !self.is_open {
            return Vec::new();
        }

        let nbrs: Vec<Nbr> = self
            .in_csr
            .edges_of(dst, ts)
            .iter()
            .filter(|edge| !self.deleted_edges.contains(&edge.edge_id))
            .copied()
            .collect();

        let prop_offsets: Vec<_> = nbrs.iter().map(|nbr| nbr.prop_offset).collect();
        if !prop_offsets.is_empty() {
            self.properties.prefetch_batch(&prop_offsets);
        }

        nbrs.into_iter()
            .map(|nbr| {
                let (src_vid, rank) = Self::decode_edge_endpoint(nbr.neighbor);
                let properties = self
                    .properties
                    .get_fast(nbr.prop_offset, None)
                    .or_else(|| self.properties.get(nbr.prop_offset, None))
                    .map(|props| props.into_iter().filter_map(|(k, v)| v.map(|v| (k, v))).collect())
                    .unwrap_or_default();
                EdgeRecord {
                    src_vid,
                    dst_vid: VertexId::from_int64(dst as i64),
                    rank,
                    properties,
                }
            })
            .collect()
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
        let _ = self
            .property_index_cache
            .get(prop_name)
            .ok_or_else(|| StorageError::column_not_found(prop_name.to_string()))?;

        let dst_key = Self::edge_endpoint_key(dst, rank);
        if let Some(nbr) = self.out_csr.get_edge(src, dst_key, ts) {
            if self.deleted_edges.contains(&nbr.edge_id) {
                return Ok(false);
            }
            self.properties
                .set_property(nbr.prop_offset, prop_name, Some(value.clone()), ts)?;
            return Ok(true);
        }
        Ok(false)
    }

    pub fn update_edge_property_by_offset(
        &mut self,
        params: super::core::UpdateEdgePropertyByOffsetParams,
    ) -> StorageResult<bool> {
        if !self.is_open {
            return Err(StorageError::storage_not_open());
        }
        let dst_key = Self::edge_endpoint_key(params.dst, params.rank);
        if let Some(nbr) = self.out_csr.get_edge(params.src, dst_key, params.ts) {
            if self.deleted_edges.contains(&nbr.edge_id) {
                return Ok(false);
            }
            self.properties.set_property_by_id(
                nbr.prop_offset,
                PropertyId(params.prop_id),
                Some(params.value.clone()),
                params.ts,
            )?;
            let src_key = Self::edge_endpoint_key(params.src, params.rank);
            if let Some(ie_nbr) = self.in_csr.get_edge(params.dst, src_key, params.ts) {
                if nbr.prop_offset != ie_nbr.prop_offset {
                    return Err(StorageError::data_corruption(format!(
                        "property offset mismatch: out_csr={}, in_csr={} at edge ({}, {})",
                        nbr.prop_offset, ie_nbr.prop_offset, params.src, params.dst
                    )));
                }
            }
            return Ok(true);
        }
        Ok(false)
    }

    fn record_schema_change(&mut self, details: ChangeDetails) -> StorageResult<()> {
        let mut history_guard = self
            .version_history
            .lock()
            .map_err(|_| StorageError::db_error("Failed to lock version_history"))?;
        let next_version = history_guard.latest_version() + 1;
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

    pub fn add_property(&mut self, name: String, data_type: DataType, nullable: bool) -> StorageResult<()> {
        if !self.is_open {
            return Err(StorageError::storage_not_open());
        }
        if self.properties.has_property(&name) {
            return Err(StorageError::column_already_exists(name));
        }
        self.properties.add_property(name.clone(), data_type.clone(), nullable);
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
        let removed_prop = self.schema.properties[index].clone();
        self.properties.remove_property(name)?;
        self.schema.properties.remove(index);
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
        if self.schema.properties.iter().any(|prop| prop.name == new_name) {
            return Err(StorageError::column_already_exists(new_name.to_string()));
        }
        let index = self
            .schema
            .properties
            .iter()
            .position(|prop| prop.name == old_name)
            .ok_or_else(|| StorageError::column_not_found(old_name.to_string()))?;
        self.properties.rename_property(old_name, new_name)?;
        self.schema.properties[index].name = new_name.to_string();
        if let Some(idx) = self.property_index_cache.remove(old_name) {
            self.property_index_cache.insert(new_name.to_string(), idx);
        }
        self.record_schema_change(ChangeDetails::PropertyRenamed {
            old_name: old_name.to_string(),
            new_name: new_name.to_string(),
        })?;
        Ok(())
    }

    pub fn scan(&self, ts: Timestamp) -> Vec<EdgeRecord> {
        if !self.is_open {
            return Vec::new();
        }
        self.iter(ts).collect()
    }

    pub fn scan_paginated(&self, ts: Timestamp, offset: usize, page_size: usize) -> (Vec<EdgeRecord>, bool) {
        if !self.is_open {
            return (Vec::new(), false);
        }
        let mut edges = Vec::new();
        let mut skip_count = 0;
        for edge in self.iter(ts) {
            if skip_count < offset {
                skip_count += 1;
                continue;
            }
            if edges.len() >= page_size {
                return (edges, true);
            }
            edges.push(edge);
        }
        (edges, false)
    }

    pub fn scan_paginated_iter(&self, ts: Timestamp, offset: usize, page_size: usize) -> SimpleScanIterator<'_> {
        let mut iter = SimpleScanIterator::with_limit(self, ts, Some(page_size));
        for _ in 0..offset {
            iter.next();
        }
        iter
    }

    pub fn iter(&self, ts: Timestamp) -> SimpleScanIterator<'_> {
        SimpleScanIterator::new(self, ts)
    }

    pub fn flush<P: AsRef<std::path::Path>>(
        &self,
        path: P,
        compression: crate::storage::compression::CompressionType,
    ) -> StorageResult<()> {
        use std::fs;
        let path = path.as_ref();
        fs::create_dir_all(path)?;

        let meta_path = path.join("meta.bin");
        let mut meta_file = std::fs::File::create(&meta_path)?;
        write_header_to(&mut meta_file, crate::storage::persistence::section::EDGE_META)
            .map_err(|e| StorageError::io_error(format!("Failed to write edge meta header: {}", e)))?;
        super::persistence::flush_metadata(
            &mut meta_file,
            self.label,
            self.src_label,
            self.dst_label,
            &self.label_name,
            self.is_open,
            &self.schema,
            self.next_edge_id,
            &HashMap::new(),
            u32::MAX,
        )?;
        drop(meta_file);
        crate::storage::compression::compress_file_inplace(&meta_path, compression)?;

        let out_csr_path = path.join("out_csr.bin");
        super::persistence::flush_csr(
            &self.out_csr,
            &[],
            &out_csr_path,
            crate::storage::persistence::section::EDGE_OUT_CSR,
        )?;
        crate::storage::compression::compress_file_inplace(&out_csr_path, compression)?;

        let in_csr_path = path.join("in_csr.bin");
        super::persistence::flush_csr(
            &self.in_csr,
            &[],
            &in_csr_path,
            crate::storage::persistence::section::EDGE_IN_CSR,
        )?;
        crate::storage::compression::compress_file_inplace(&in_csr_path, compression)?;

        let props_path = path.join("properties.bin");
        super::persistence::flush_properties(&self.properties, &props_path)?;
        crate::storage::compression::compress_file_inplace(&props_path, compression)?;

        Ok(())
    }

    pub fn load<P: AsRef<std::path::Path>>(&mut self, path: P) -> StorageResult<()> {
        use std::io::Read;
        let path = path.as_ref();

        let meta_path = path.join("meta.bin");
        let meta_data = crate::storage::compression::read_decompressed(&meta_path)?;
        let mut meta_cursor = &meta_data[..];
        let mut header_buf = [0u8; crate::storage::persistence::HEADER_SIZE];
        meta_cursor.read_exact(&mut header_buf)?;
        {
            let mut slice = &header_buf[..];
            let (_version, sid) = crate::storage::persistence::read_header(&mut slice)?;
            if sid != crate::storage::persistence::section::EDGE_META {
                return Err(StorageError::deserialize_error(format!(
                    "unexpected section id in edge meta: expected {:#06x}, got {:#06x}",
                    crate::storage::persistence::section::EDGE_META, sid
                )));
            }
        }

        let mut version_bytes = [0u8; 4];
        meta_cursor.read_exact(&mut version_bytes)?;
        let version = u32::from_le_bytes(version_bytes);
        if version != 2 {
            return Err(StorageError::deserialize_error(format!("unsupported edge meta version: {}", version)));
        }

        let (label, src_label, dst_label, label_name, is_open, schema, next_edge_id, _tombstones, _min_snapshot_ts) =
            super::persistence::load_metadata(&mut meta_cursor)?;

        self.label = label;
        self.src_label = src_label;
        self.dst_label = dst_label;
        self.label_name = label_name;
        self.is_open = is_open;
        self.set_schema(schema);
        self.next_edge_id = next_edge_id;

        let out_csr_path = path.join("out_csr.bin");
        let mut empty = Vec::new();
        super::persistence::load_csr(&out_csr_path, &mut self.out_csr, &mut empty)?;

        let in_csr_path = path.join("in_csr.bin");
        let mut empty = Vec::new();
        super::persistence::load_csr(&in_csr_path, &mut self.in_csr, &mut empty)?;

        let props_path = path.join("properties.bin");
        self.properties = super::persistence::load_properties(&props_path)?;

        if self.next_edge_id.0 == 0 {
            let ts = u32::MAX;
            let max_id = self
                .out_csr
                .iter(ts)
                .map(|(_, nbr)| nbr.edge_id.0 + 1)
                .max()
                .unwrap_or(0);
            self.next_edge_id = EdgeId(max_id);
        }
        self.is_open = true;
        Ok(())
    }
}

pub struct SimpleScanIterator<'a> {
    _table: &'a SimpleEdgeStore,
    records: std::vec::IntoIter<EdgeRecord>,
    max_records: Option<usize>,
    current_count: usize,
}

impl<'a> SimpleScanIterator<'a> {
    pub fn new(table: &'a SimpleEdgeStore, ts: Timestamp) -> Self {
        Self::with_limit(table, ts, None)
    }

    pub fn with_limit(table: &'a SimpleEdgeStore, ts: Timestamp, max_records: Option<usize>) -> Self {
        let mut seen = HashSet::new();
        let mut records = Vec::new();

        for (src_vid, nbr) in table.out_csr.iter(ts) {
            if !table.deleted_edges.contains(&nbr.edge_id) && seen.insert(nbr.edge_id) {
                let src_u32 = src_vid.as_int64().unwrap_or(0) as u32;
                let (dst_vid, rank) = SimpleEdgeStore::decode_edge_endpoint(nbr.neighbor);
                let properties = table.properties_for_offset(nbr.prop_offset);
                records.push(EdgeRecord {
                    src_vid: VertexId::from_int64(src_u32 as i64),
                    dst_vid,
                    rank,
                    properties,
                });
                if let Some(max) = max_records {
                    if records.len() >= max {
                        break;
                    }
                }
            }
        }

        Self {
            _table: table,
            records: records.into_iter(),
            max_records,
            current_count: 0,
        }
    }

    pub fn has_more(&self) -> bool {
        if let Some(max) = self.max_records {
            self.current_count < max && self.records.len() > self.current_count
        } else {
            self.records.len() > self.current_count
        }
    }
}

impl<'a> Iterator for SimpleScanIterator<'a> {
    type Item = EdgeRecord;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(max) = self.max_records {
            if self.current_count >= max {
                return None;
            }
        }
        if let Some(record) = self.records.next() {
            self.current_count += 1;
            Some(record)
        } else {
            None
        }
    }
}

pub struct SimpleSnapshotHandle<'a> {
    _table: &'a SimpleEdgeStore,
}

impl<'a> SimpleSnapshotHandle<'a> {
    pub fn timestamp(&self) -> Timestamp {
        u32::MAX
    }

    pub fn export(&self) -> StorageResult<ExportedEdgeSnapshot> {
        Err(StorageError::invalid_operation(
            "Snapshots are not supported for non-time-travel edge types".to_string(),
        ))
    }

    pub fn release(self) {}
}
