//! Core EdgeTable operations: CRUD, properties, queries, and compaction.
//!
//! Provides fundamental edge table functionality including insertion, deletion,
//! querying, property management, and basic maintenance operations.

use super::segment::{CsrSegment, DeletionInfo, SegmentVersion};
use super::mvcc::MVCCManager;
use super::stats::DeletionStats;
use super::super::{CsrVariant, EdgeSchema, EdgeStrategy, Nbr, EdgeRecord, CsrBase, MutableCsrTrait};
use crate::core::types::{EdgeId, CompactConfig, LabelId, VertexId, Timestamp};
use crate::core::{DataType, StorageError, StorageResult, Value};
use crate::storage::types::{PropertyId, StoragePropertyDef};
use crate::storage::edge::PropertyTable;
use crate::storage::schema::{LabelVersionHistory, SchemaObjectType, ChangeDetails, PropertyChange};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone)]
pub struct EdgeTableConfig {
    pub initial_vertex_capacity: usize,
    pub initial_edge_capacity: usize,
    pub max_segments_per_direction: usize,
    /// Write backpressure: max size of mutable CSR (in bytes) before triggering freeze.
    /// Set to 0 to disable. Typical value: 100MB (100 * 1024 * 1024).
    pub max_mutable_csr_bytes: usize,

    /// Segment merge threshold: trigger auto-merge when segment count reaches this value.
    /// Default: 50 segments per direction before merging oldest segments.
    /// Set to 0 to disable auto-merge.
    pub segment_merge_threshold: usize,

    /// Merge behavior: how many segments to keep after merging.
    /// When merging is triggered and segment count exceeds threshold,
    /// keep only the N newest segments (others are merged).
    /// Default: 5 (keeps 5 newest, merges the rest).
    pub merge_keep_newest: usize,
}

impl Default for EdgeTableConfig {
    fn default() -> Self {
        Self {
            initial_vertex_capacity: 4096,
            initial_edge_capacity: 4096,
            max_segments_per_direction: 100,
            // Default: 100MB per direction
            max_mutable_csr_bytes: 100 * 1024 * 1024,
            // Auto-merge when segment count reaches 50 per direction
            segment_merge_threshold: 50,
            // Keep only 5 newest segments, merge the rest (oldest 45 become 1)
            merge_keep_newest: 5,
        }
    }
}

/// Parameters for update_edge_property_by_offset operation
pub struct UpdateEdgePropertyByOffsetParams {
    pub src: u32,
    pub dst: u32,
    pub rank: i64,
    pub prop_id: u16,
    pub value: Value,
    pub ts: Timestamp,
}

pub struct EdgeTableCore {
    pub label: LabelId,
    pub label_name: String,
    pub src_label: LabelId,
    pub dst_label: LabelId,
    pub schema: EdgeSchema,
    pub out_csr: CsrVariant,
    pub in_csr: CsrVariant,
    pub out_segments: Vec<CsrSegment>,
    pub in_segments: Vec<CsrSegment>,
    /// Segment index for fast time-based lookup: (create_ts_min, segment_idx in out_segments)
    /// Sorted by create_ts_min, enables binary search to skip irrelevant segments
    pub out_segment_index: Vec<(Timestamp, usize)>,
    /// Segment index for in_segments: (create_ts_min, segment_idx in in_segments)
    pub in_segment_index: Vec<(Timestamp, usize)>,
    pub mvcc: MVCCManager,
    pub properties: PropertyTable,
    pub is_open: bool,
    pub next_edge_id: EdgeId,
    pub config: EdgeTableConfig,
    pub stats_manager: Option<std::sync::Arc<crate::core::stats::StatsManager>>,
    /// Version history tracking for schema changes
    pub version_history: Arc<Mutex<LabelVersionHistory>>,
    /// Cache for property name → schema index mapping to avoid O(n) linear lookups.
    /// Invalidated whenever schema changes.
    pub property_index_cache: HashMap<String, usize>,
}

impl EdgeTableCore {
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

        let version_history = Arc::new(Mutex::new(
            LabelVersionHistory::new(label_id, label_name.clone(), SchemaObjectType::Edge)
        ));

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
            out_segments: Vec::new(),
            in_segments: Vec::new(),
            out_segment_index: Vec::new(),
            in_segment_index: Vec::new(),
            mvcc: MVCCManager::new(),
            properties,
            is_open: true,
            next_edge_id: EdgeId(0),
            config,
            stats_manager: None,
            version_history,
            property_index_cache,
        })
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

    pub fn set_stats_manager(&mut self, stats: std::sync::Arc<crate::core::stats::StatsManager>) {
        self.stats_manager = Some(stats);
    }


    fn base_get_edge(
        &self,
        segments: &[CsrSegment],
        src: u32,
        dst: VertexId,
        ts: Timestamp,
    ) -> Option<Nbr> {
        // Scan segments in reverse (newest first), with early termination optimizations
        for segment in segments.iter().rev() {
            // Skip segments that were created after the query timestamp
            if segment.create_ts_min > ts {
                continue;
            }

            // Fast path: skip segments where all deletions happened before or at query_ts
            // This means the segment is effectively not relevant for this query
            if segment.deletion_info.all_deleted_before(ts) {
                continue;
            }

            // Binary search for the specific edge in this segment
            let positioned_edges = segment.csr.edges_of_with_position(src);
            for (position, edge) in positioned_edges {
                if edge.neighbor == dst && edge.timestamp <= ts {
                    let edge_id = segment.recover_edge_id(edge, position);
                    if !self.mvcc.is_tombstoned(edge_id, ts) {
                        return Some(Nbr::new(
                            edge.neighbor,
                            edge_id,
                            edge.prop_offset,
                            edge.timestamp,
                        ));
                    }
                }
            }
        }

        None
    }

    fn base_edges_of(&self, segments: &[CsrSegment], src: u32, ts: Timestamp) -> Vec<Nbr> {
        let mut edges = Vec::new();

        for segment in segments.iter().rev() {
            if segment.create_ts_min > ts {
                continue;
            }

            // Skip segments where all edges have been deleted before the query timestamp
            if segment.deletion_info.all_deleted_before(ts) {
                continue;
            }

            for (position, edge) in segment.csr.edges_of_with_position(src) {
                if edge.timestamp <= ts {
                    let edge_id = segment.recover_edge_id(edge, position);
                    if !self.mvcc.is_tombstoned(edge_id, ts) {
                        edges.push(Nbr::new(
                            edge.neighbor,
                            edge_id,
                            edge.prop_offset,
                            edge.timestamp,
                        ));
                    }
                }
            }
        }

        edges
    }

    fn merged_edges_of(
        &self,
        delta: &CsrVariant,
        segments: &[CsrSegment],
        src: u32,
        ts: Timestamp,
    ) -> Vec<Nbr> {
        let mut seen = HashSet::new();
        let mut result = Vec::new();

        for nbr in delta.edges_of(src, ts) {
            if !self.mvcc.is_tombstoned(nbr.edge_id, ts) && seen.insert(nbr.edge_id) {
                result.push(nbr);
            }
        }

        for nbr in self.base_edges_of(segments, src, ts) {
            if seen.insert(nbr.edge_id) {
                result.push(nbr);
            }
        }

        result
    }

    fn merged_get_edge(
        &self,
        delta: &CsrVariant,
        segments: &[CsrSegment],
        src: u32,
        dst: VertexId,
        ts: Timestamp,
    ) -> Option<Nbr> {
        if let Some(nbr) = delta.get_edge(src, dst, ts) {
            if !self.mvcc.is_tombstoned(nbr.edge_id, ts) {
                return Some(nbr);
            }
        }

        self.base_get_edge(segments, src, dst, ts)
    }

    fn edge_record_from_nbr(&self, src: u32, nbr: Nbr) -> EdgeRecord {
        let (dst_vid, rank) = Self::decode_edge_endpoint(nbr.neighbor);
        EdgeRecord {
            src_vid: VertexId::from_int64(src as i64),
            dst_vid,
            rank,
            properties: self.properties_for_offset(nbr.prop_offset),
        }
    }

    fn properties_for_offset(&self, prop_offset: u32) -> Vec<(String, Value)> {
        if prop_offset == 0 {
            return Vec::new();
        }

        self.properties
            .get(prop_offset, None)
            .map(|props| {
                props
                    .into_iter()
                    .filter_map(|(k, v)| v.map(|v| (k, v)))
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn validate_segment_integrity(&self) -> usize {
        let mut valid_count = 0;

        for segment in &self.out_segments {
            if segment.version.validate(segment) {
                valid_count += 1;
            }
        }

        for segment in &self.in_segments {
            if segment.version.validate(segment) {
                valid_count += 1;
            }
        }

        valid_count
    }

    pub fn segment_versions(&self) -> Vec<(usize, u32, u32)> {
        let mut versions = Vec::new();

        for (idx, seg) in self.out_segments.iter().enumerate() {
            versions.push((idx, seg.version.version, seg.version.checksum));
        }

        for (idx, seg) in self.in_segments.iter().enumerate() {
            versions.push((idx + 1000, seg.version.version, seg.version.checksum));
        }

        versions
    }

    pub fn update_segment_checksums(&mut self) {
        for segment in &mut self.out_segments {
            segment.version.checksum = SegmentVersion::compute_checksum(segment);
            segment.version.increment();
        }

        for segment in &mut self.in_segments {
            segment.version.checksum = SegmentVersion::compute_checksum(segment);
            segment.version.increment();
        }
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

        if self.has_edge(src, dst, rank, ts) {
            if prop_offset > 0 {
                self.properties.delete(prop_offset);
            }
            return Err(StorageError::edge_already_exists(format!(
                "{} -> {}@{}",
                src, dst, rank
            )));
        }

        let dst_key = Self::edge_endpoint_key(dst, rank);
        let src_key = Self::edge_endpoint_key(src, rank);

        let edge_id = self.next_edge_id.fetch_add();
        if !self
            .out_csr
            .insert_edge(src, dst_key, edge_id, prop_offset, ts)
        {
            if prop_offset > 0 {
                self.properties.delete(prop_offset);
            }
            return Err(StorageError::edge_already_exists(format!(
                "{} -> {}@{}",
                src, dst, rank
            )));
        }

        if !self
            .in_csr
            .insert_edge(dst, src_key, edge_id, prop_offset, ts)
        {
            self.out_csr.delete_edge(src, edge_id, ts);
            if prop_offset > 0 {
                self.properties.delete(prop_offset);
            }
            return Err(StorageError::edge_already_exists(format!(
                "{} -> {}@{}",
                dst, src, rank
            )));
        }

        // Check write backpressure after successful insertion
        self.check_and_apply_write_backpressure(ts);

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

        if let Some(nbr) = self.out_csr.get_edge(src, dst_key, ts) {
            let edge_id = nbr.edge_id;

            self.out_csr.delete_edge(src, edge_id, ts);
            self.in_csr.delete_edge_by_dst(dst, src_key, ts);

            return Ok(true);
        }

        if let Some(nbr) = self.base_get_edge(&self.out_segments, src, dst_key, ts) {
            let edge_id = nbr.edge_id;
            self.mvcc.pending_segment_deletions.insert(edge_id, ts);
            self.mvcc.tombstones.insert(edge_id, ts);
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
        if self.out_csr.get_edge(src, dst_key, ts).is_some() {
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

    pub fn get_edge(&self, src: u32, dst: u32, rank: i64, ts: Timestamp) -> Option<EdgeRecord> {
        if !self.is_open {
            return None;
        }

        let dst_key = Self::edge_endpoint_key(dst, rank);
        let nbr = self.merged_get_edge(&self.out_csr, &self.out_segments, src, dst_key, ts)?;
        let properties = self.properties_for_offset(nbr.prop_offset);

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

        let nbrs = self.merged_edges_of(&self.out_csr, &self.out_segments, src, ts);

        // Optimization: prefetch all properties first to improve cache locality
        let prop_offsets: Vec<_> = nbrs.iter().map(|nbr| nbr.prop_offset).collect();
        if !prop_offsets.is_empty() {
            self.properties.prefetch_batch(&prop_offsets);
        }

        nbrs.into_iter()
            .map(|nbr| {
                let (dst_vid, rank) = Self::decode_edge_endpoint(nbr.neighbor);
                // Try fast path first, fall back to regular get if not fixed-size
                let properties = self
                    .properties
                    .get_fast(nbr.prop_offset, None)
                    .or_else(|| self.properties.get(nbr.prop_offset, None))
                    .map(|props| {
                        props
                            .into_iter()
                            .filter_map(|(k, v)| v.map(|v| (k, v)))
                            .collect()
                    })
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

        let nbrs = self.merged_edges_of(&self.in_csr, &self.in_segments, dst, ts);

        // Optimization: prefetch all properties first to improve cache locality
        let prop_offsets: Vec<_> = nbrs.iter().map(|nbr| nbr.prop_offset).collect();
        if !prop_offsets.is_empty() {
            self.properties.prefetch_batch(&prop_offsets);
        }

        nbrs.into_iter()
            .map(|nbr| {
                let (src_vid, rank) = Self::decode_edge_endpoint(nbr.neighbor);
                // Try fast path first, fall back to regular get if not fixed-size
                let properties = self
                    .properties
                    .get_fast(nbr.prop_offset, None)
                    .or_else(|| self.properties.get(nbr.prop_offset, None))
                    .map(|props| {
                        props
                            .into_iter()
                            .filter_map(|(k, v)| v.map(|v| (k, v)))
                            .collect()
                    })
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

    pub fn has_edge(&self, src: u32, dst: u32, rank: i64, ts: Timestamp) -> bool {
        if !self.is_open {
            return false;
        }
        let dst_key = Self::edge_endpoint_key(dst, rank);
        self.merged_get_edge(&self.out_csr, &self.out_segments, src, dst_key, ts)
            .is_some()
    }

    pub fn edge_count(&self) -> u64 {
        self.out_csr.edge_count()
            + self
                .out_segments
                .iter()
                .map(|segment| {
                    segment
                        .csr
                        .iter()
                        .filter(|(_, edge)| !self.mvcc.is_tombstoned(edge.edge_id, u32::MAX))
                        .count() as u64
                })
                .sum::<u64>()
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

    /// Scan edges with pagination support.
    /// Returns at most `page_size` edges starting from `offset`.
    /// Returns (edges, has_more) where has_more indicates if there are more edges beyond this page.
    pub fn scan_paginated(&self, ts: Timestamp, offset: usize, page_size: usize) -> (Vec<EdgeRecord>, bool) {
        if !self.is_open {
            return (Vec::new(), false);
        }

        let mut edges = Vec::new();
        let mut skip_count = 0;
        let mut total_count = 0;

        for edge in self.iter(ts) {
            total_count += 1;
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
            .add_property(name.clone(), data_type.clone(), nullable);

        let prop_def = StoragePropertyDef::new(name.clone(), data_type.clone());
        let new_idx = self.schema.properties.len();
        self.schema
            .properties
            .push(prop_def);
        self.property_index_cache.insert(name.clone(), new_idx);
        // Increment schema version when property is added
        self.schema.increment_version();

        // Record schema change
        let change = PropertyChange::new(
            self.schema.schema_version,
            SchemaObjectType::Edge,
            self.label,
            self.label_name.clone(),
            ChangeDetails::PropertyAdded {
                name,
                data_type,
                nullable,
                default_value: None,
            },
        );
        self.version_history.lock().unwrap().add_change(change);

        Ok(())
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
        for (prop_name, idx) in &mut self.property_index_cache {
            if *idx > index {
                *idx -= 1;
            }
        }
        // Increment schema version when property is removed
        self.schema.increment_version();

        // Record schema change
        let change = PropertyChange::new(
            self.schema.schema_version,
            SchemaObjectType::Edge,
            self.label,
            self.label_name.clone(),
            ChangeDetails::PropertyRemoved {
                name: removed_prop.name,
                data_type: removed_prop.data_type,
            },
        );
        self.version_history.lock().unwrap().add_change(change);

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
        // Increment schema version when property is renamed
        self.schema.increment_version();

        // Record schema change
        let change = PropertyChange::new(
            self.schema.schema_version,
            SchemaObjectType::Edge,
            self.label,
            self.label_name.clone(),
            ChangeDetails::PropertyRenamed {
                old_name: old_name.to_string(),
                new_name: new_name.to_string(),
            },
        );
        self.version_history.lock().unwrap().add_change(change);

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
        if let Some(nbr) = self.merged_get_edge(&self.out_csr, &self.out_segments, src, dst_key, ts)
        {
            self.properties
                .set_property(nbr.prop_offset, prop_name, Some(value.clone()), ts)?;
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
        if let Some(nbr) = self.merged_get_edge(
            &self.out_csr,
            &self.out_segments,
            params.src,
            dst_key,
            params.ts,
        ) {
            self.properties.set_property_by_id(nbr.prop_offset, PropertyId(params.prop_id), Some(params.value.clone()), params.ts)?;

            let src_key = Self::edge_endpoint_key(params.src, params.rank);
            if let Some(ie_nbr) = self.merged_get_edge(
                &self.in_csr,
                &self.in_segments,
                params.dst,
                src_key,
                params.ts,
            ) {
                if nbr.prop_offset != ie_nbr.prop_offset {
                    return Err(StorageError::data_corruption(
                        format!(
                            "property offset mismatch: out_csr={}, in_csr={} at edge ({}, {})",
                            nbr.prop_offset, ie_nbr.prop_offset, params.src, params.dst
                        ),
                    ));
                }
            }
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

    pub fn label_name(&self) -> &str {
        &self.label_name
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

    /// Set schema with explicit version number (used for undo operations)
    pub fn set_schema_with_version(&mut self, mut schema: EdgeSchema, new_version: u64) {
        schema.schema_version = new_version;
        // Rebuild property index cache
        self.property_index_cache.clear();
        for (idx, prop) in schema.properties.iter().enumerate() {
            self.property_index_cache.insert(prop.name.clone(), idx);
        }
        self.schema = schema;
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
        total += self
            .out_segments
            .iter()
            .map(|segment| segment.csr.used_memory_size())
            .sum::<usize>();
        total += self
            .in_segments
            .iter()
            .map(|segment| segment.csr.used_memory_size())
            .sum::<usize>();
        total += self.mvcc.tombstones.len() * std::mem::size_of::<(EdgeId, Timestamp)>();
        total += self.properties.used_memory_size();

        // Account for property_index_cache
        total += self.property_index_cache.len()
            * (std::mem::size_of::<String>() + std::mem::size_of::<usize>());

        total
    }

    /// Get mutable CSR memory usage (out_csr + in_csr)
    pub fn mutable_csr_memory_size(&self) -> usize {
        self.out_csr.used_memory_size() + self.in_csr.used_memory_size()
    }

    /// Check write backpressure and trigger freeze if necessary.
    /// Returns true if a freeze was triggered.
    pub fn check_and_apply_write_backpressure(&mut self, current_ts: Timestamp) -> bool {
        if self.config.max_mutable_csr_bytes == 0 {
            return false; // Backpressure disabled
        }

        let mutable_size = self.mutable_csr_memory_size();

        // Record current metrics
        if let Some(stats) = &self.stats_manager {
            stats.record_mutable_csr_backpressure(mutable_size as u64, mutable_size as u64);
        }

        if mutable_size > self.config.max_mutable_csr_bytes {
            // Trigger freeze
            let _frozen = self.freeze_csr_only(current_ts);

            // Record freeze event
            if let Some(stats) = &self.stats_manager {
                stats.record_mutable_csr_freeze();
            }

            return true;
        }

        false
    }
}

pub struct EdgeTableScanIterator<'a> {
    _table: &'a EdgeTableCore,
    records: std::vec::IntoIter<EdgeRecord>,
    /// Maximum number of records to return (None = unlimited)
    max_records: Option<usize>,
    /// Current record count
    current_count: usize,
}

impl<'a> EdgeTableScanIterator<'a> {
    pub fn new(table: &'a EdgeTableCore, ts: Timestamp) -> Self {
        Self::with_limit(table, ts, None)
    }

    /// Create a scan iterator with a maximum record limit
    pub fn with_limit(table: &'a EdgeTableCore, ts: Timestamp, max_records: Option<usize>) -> Self {
        let mut seen = HashSet::new();
        let mut records = Vec::new();

        for (src_vid, nbr) in table.out_csr.iter(ts) {
            if !table.mvcc.is_tombstoned(nbr.edge_id, ts) && seen.insert(nbr.edge_id) {
                records
                    .push(table.edge_record_from_nbr(src_vid.as_int64().unwrap_or(0) as u32, nbr));

                if let Some(max) = max_records {
                    if records.len() >= max {
                        break;
                    }
                }
            }
        }

        if max_records.is_none() || records.len() < max_records.unwrap() {
            for segment in table.out_segments.iter().rev() {
                if segment.create_ts_min > ts {
                    continue;
                }

                for (src_vid, edge) in segment.csr.iter() {
                    if edge.timestamp <= ts
                        && !table.mvcc.is_tombstoned(edge.edge_id, ts)
                        && seen.insert(edge.edge_id)
                    {
                        records.push(table.edge_record_from_nbr(
                            src_vid.as_int64().unwrap_or(0) as u32,
                            Nbr::new(
                                edge.neighbor,
                                edge.edge_id,
                                edge.prop_offset,
                                edge.timestamp,
                            ),
                        ));

                        if let Some(max) = max_records {
                            if records.len() >= max {
                                break;
                            }
                        }
                    }
                }

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

    /// Check if iterator has more records to fetch
    pub fn has_more(&self) -> bool {
        if let Some(max) = self.max_records {
            self.current_count < max
        } else {
            true
        }
    }
}

impl<'a> Iterator for EdgeTableScanIterator<'a> {
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

#[cfg(test)]
#[path = "core_tests.rs"]
mod tests;



