//! Statistics collection module
//!
//! Collects tag/edge statistics for a space from the storage layer and
//! writes them into the [`StatisticsManager`] with composite
//! `(schema_version, data_epoch)` provenance.
//!
//! When the storage engine exposes a [`ColumnStatsReader`] snapshot for a
//! property, the min/max envelope from the snapshot replaces the sampled
//! envelope (fixing head-bias in the sample window).  NDV is still derived
//! from sampling because the zone-map path does not track exact distinct
//! counts cheaply.
//!
//! Sampling uses a rotating offset window seeded by `data_epoch` so that
//! successive collection passes cover different regions of the table without
//! requiring a global random source.

use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use parking_lot::RwLock;

use crate::core::types::EdgeDirection;
use crate::core::vertex_edge_path::Vertex;
use crate::storage::{QueryStorage, ScanOptions};

use super::{EdgeTypeStatistics, PropertyStatistics, StatisticsManager, TagStatistics};

/// Result of a collection pass for one space.
#[derive(Debug, Clone, Default)]
pub struct CollectedSummary {
    /// Number of tags with statistics collected.
    pub tags: usize,
    /// Number of edge types with statistics collected.
    pub edge_types: usize,
    /// Number of properties with distinct-value statistics collected.
    pub properties: usize,
    /// Whether the result was served from the cached (version-matched) data.
    pub cached: bool,
}

impl CollectedSummary {
    fn collected_with_props(tags: usize, edge_types: usize, properties: usize) -> Self {
        Self {
            tags,
            edge_types,
            properties,
            cached: false,
        }
    }

    fn cached() -> Self {
        Self::default().with_cached()
    }

    fn with_cached(mut self) -> Self {
        self.cached = true;
        self
    }
}

/// Collects statistical information from storage into the [`StatisticsManager`].
///
/// Scope: exact vertex/edge counts, sampled average degrees, per-property
/// NDV plus min/max envelopes.  When the storage engine provides a
/// [`ColumnStatsReader`] snapshot the envelope is taken from the snapshot
/// (exact, unbiased); otherwise the sampled envelope is used as a fallback.
pub struct StatisticsCollector;

impl StatisticsCollector {
    /// Deterministic rotation seed derived from `data_epoch` and a string key
    /// (tag/edge type name).  The seed is used to offset the sample window so
    /// that consecutive collection passes cover different rows.
    fn rotation_seed(data_epoch: u64, key: &str) -> u64 {
        let mut hasher = DefaultHasher::new();
        data_epoch.hash(&mut hasher);
        key.hash(&mut hasher);
        hasher.finish()
    }

    /// Deterministic offset into a total of `n` rows, given `sample_size`
    /// and a rotation seed.
    fn rotating_offset(n: usize, sample_size: usize, seed: u64) -> usize {
        if n <= sample_size {
            return 0;
        }
        let span = n - sample_size;
        (seed as usize) % (span + 1)
    }

    /// Collect (or serve cached) statistics for one space and write them to
    /// the manager.
    ///
    /// Cache hits require **both** `schema_version` (DDL generation) and
    /// `data_epoch` (MVCC write timestamp) to match the values recorded from
    /// the previous collection, so any committed write or DDL change triggers
    /// a refresh.
    #[allow(clippy::type_complexity)]
    pub fn collect_space(
        manager: &StatisticsManager,
        storage: &Arc<RwLock<dyn QueryStorage>>,
        space: &str,
        schema_version: u64,
        data_epoch: u64,
        sample_limit: usize,
    ) -> Result<CollectedSummary, String> {
        if manager.space_stamp(space) == Some((schema_version, data_epoch)) {
            return Ok(CollectedSummary::cached());
        }

        let storage = storage.read();

        let tags = Self::collect_tags(
            manager,
            &*storage,
            space,
            schema_version,
            data_epoch,
            sample_limit,
        )?;
        let edge_types = Self::collect_edge_types(
            manager,
            &*storage,
            space,
            schema_version,
            data_epoch,
            sample_limit,
        )?;
        let properties =
            Self::collect_property_stats(manager, &*storage, space, data_epoch, sample_limit)
                .unwrap_or(0);

        manager.set_space_stamp(space, schema_version, data_epoch);
        Ok(CollectedSummary::collected_with_props(
            tags, edge_types, properties,
        ))
    }

    fn collect_tags(
        manager: &StatisticsManager,
        storage: &dyn QueryStorage,
        space: &str,
        schema_version: u64,
        _data_epoch: u64,
        sample_limit: usize,
    ) -> Result<usize, String> {
        let tag_infos = storage
            .list_tags(space)
            .map_err(|e| format!("Failed to list tags for space '{}': {}", space, e))?;
        for tag_info in &tag_infos {
            manager.register_tag_id(tag_info.tag_id as i32, tag_info.tag_name.clone());
        }

        let mut collected = 0usize;
        for tag_info in &tag_infos {
            let vertex_count = storage
                .count_vertices_by_tag(space, &tag_info.tag_name)
                .map_err(|e| {
                    format!(
                        "Failed to count vertices for tag '{}': {}",
                        tag_info.tag_name, e
                    )
                })?;

            let (avg_out_degree, avg_in_degree) =
                Self::sample_tag_degrees(storage, space, &tag_info.tag_name, sample_limit)?;

            let mut stats = TagStatistics::new(tag_info.tag_name.clone());
            stats.vertex_count = vertex_count;
            stats.avg_out_degree = avg_out_degree;
            stats.avg_in_degree = avg_in_degree;
            manager.update_tag_stats(space, stats.with_version(space.to_string(), schema_version));
            collected += 1;
        }
        Ok(collected)
    }

    /// Estimate average in/out degree from a sample of vertices.
    fn sample_tag_degrees(
        storage: &dyn QueryStorage,
        space: &str,
        tag_name: &str,
        sample_limit: usize,
    ) -> Result<(f64, f64), String> {
        let vertices = Self::sample_vertices_by_tag(storage, space, tag_name, sample_limit, 0);
        let sample_len = vertices.len();
        if sample_len == 0 {
            return Ok((0.0, 0.0));
        }

        let mut out_total: u64 = 0;
        let mut in_total: u64 = 0;
        for vertex in &vertices {
            let out = storage
                .get_node_edges(space, &vertex.vid, EdgeDirection::Out)
                .map_err(|e| {
                    format!(
                        "Failed to read out edges for vertex '{}': {}",
                        vertex.vid, e
                    )
                })?
                .len() as u64;
            let incoming = storage
                .get_node_edges(space, &vertex.vid, EdgeDirection::In)
                .map_err(|e| format!("Failed to read in edges for vertex '{}': {}", vertex.vid, e))?
                .len() as u64;
            out_total += out;
            in_total += incoming;
        }

        let sample_len_f = sample_len as f64;
        Ok((
            out_total as f64 / sample_len_f,
            in_total as f64 / sample_len_f,
        ))
    }

    /// Read at most `sample_limit` vertices of one tag, with an optional
    /// rotation offset for unbiased windowing across collection passes.
    ///
    /// Prefers the storage cursor path (tag restriction + limit give a true
    /// early-exit scan); engines without cursor support fall back to a full
    /// scan truncated to the window.
    fn sample_vertices_by_tag(
        storage: &dyn QueryStorage,
        space: &str,
        tag_name: &str,
        sample_limit: usize,
        offset: usize,
    ) -> Vec<Vertex> {
        if sample_limit == 0 {
            return Vec::new();
        }
        let mut options = ScanOptions::new();
        options.tag = Some(tag_name.to_string());
        options.limit = Some(sample_limit);
        options.offset = offset;
        if let Ok(mut cursor) = storage.create_vertex_cursor(space, &options) {
            let mut out: Vec<Vertex> = Vec::new();
            while out.len() < sample_limit {
                match cursor.next_batch(sample_limit - out.len()) {
                    Ok(batch) if !batch.is_empty() => out.extend(batch),
                    _ => break,
                }
            }
            return out;
        }
        storage
            .scan_vertices_by_tag(space, tag_name)
            .map(|vertices| {
                vertices
                    .into_iter()
                    .skip(offset)
                    .take(sample_limit)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Compute the rotation offset for a vertex tag.  `total` is the row
    /// count for the tag.
    fn vertex_tag_offset(total: usize, sample_limit: usize, data_epoch: u64, tag: &str) -> usize {
        let seed = Self::rotation_seed(data_epoch, tag);
        Self::rotating_offset(total, sample_limit, seed)
    }

    /// Compute the rotation offset for an edge type.  `total` is the edge
    /// count for the type.
    fn edge_type_offset(
        total: usize,
        sample_limit: usize,
        data_epoch: u64,
        edge_type: &str,
    ) -> usize {
        let seed = Self::rotation_seed(data_epoch, edge_type);
        Self::rotating_offset(total, sample_limit, seed)
    }

    fn collect_edge_types(
        manager: &StatisticsManager,
        storage: &dyn QueryStorage,
        space: &str,
        schema_version: u64,
        _data_epoch: u64,
        sample_limit: usize,
    ) -> Result<usize, String> {
        let edge_type_infos = storage
            .list_edge_types(space)
            .map_err(|e| format!("Failed to list edge types for space '{}': {}", space, e))?;

        let mut collected = 0usize;
        for info in &edge_type_infos {
            let edge_count = storage
                .count_edges_by_type(space, &info.edge_type_name)
                .map_err(|e| {
                    format!(
                        "Failed to count edges for edge type '{}': {}",
                        info.edge_type_name, e
                    )
                })?;

            let (avg_out_degree, avg_in_degree) =
                Self::sample_edge_degrees(storage, space, &info.edge_type_name, sample_limit)?;

            let mut stats = EdgeTypeStatistics::new(info.edge_type_name.clone());
            stats.edge_count = edge_count;
            stats.avg_out_degree = avg_out_degree;
            stats.avg_in_degree = avg_in_degree;
            manager.update_edge_stats(space, stats.with_version(space.to_string(), schema_version));
            collected += 1;
        }
        Ok(collected)
    }

    /// Estimate average out/in degree from a sampled page of edges.
    fn sample_edge_degrees(
        storage: &dyn QueryStorage,
        space: &str,
        edge_type: &str,
        sample_limit: usize,
    ) -> Result<(f64, f64), String> {
        let edges = storage
            .scan_edges_by_type_paginated(space, edge_type, 0, sample_limit)
            .or_else(|_| {
                storage
                    .scan_edges_by_type(space, edge_type)
                    .map(|edges| edges.into_iter().take(sample_limit).collect())
            })
            .map_err(|e| format!("Failed to scan edges for edge type '{}': {}", edge_type, e))?;

        if edges.is_empty() {
            return Ok((0.0, 0.0));
        }

        let mut out_freq: HashMap<u64, u64> = HashMap::new();
        let mut in_freq: HashMap<u64, u64> = HashMap::new();
        for edge in &edges {
            *out_freq
                .entry(edge.src().as_int64().unwrap_or(0) as u64)
                .or_insert(0) += 1;
            *in_freq
                .entry(edge.dst().as_int64().unwrap_or(0) as u64)
                .or_insert(0) += 1;
        }

        let n = edges.len() as f64;
        Ok((out_freq.len() as f64 / n, in_freq.len() as f64 / n))
    }

    /// Merge storage-level snapshot bounds into a property statistics entry,
    /// overriding the sampled envelope when the snapshot carries usable
    /// bounds.  Returns `true` when the snapshot provides information.
    fn merge_snapshot_bounds(
        stat: &mut PropertyStatistics,
        snapshot: Option<&crate::storage::stats_reader::ColumnStatsSnapshot>,
    ) -> bool {
        let Some(snap) = snapshot else {
            return false;
        };
        if !snap.has_envelope() {
            return false;
        }
        // The zone-map envelope is conservative (never shrinks after writes)
        // and covers the full column rather than a head-biased sample, so it
        // is always preferred when available.
        stat.min_value.clone_from(&snap.min_value);
        stat.max_value.clone_from(&snap.max_value);
        true
    }

    /// Collect per-property distinct-value (NDV) estimates from a sampled
    /// row window, then override the min/max envelope from storage-level
    /// column snapshots when available.
    ///
    /// Populates `PropertyStatistics` for both vertex tags and edge types,
    /// enabling column-narrow CBO (selectivity = 1/NDV) and range-predicate
    /// interpolation.  Histograms are left disabled on the sampled path;
    /// runtime execution feedback compensates for skew.
    fn collect_property_stats(
        manager: &StatisticsManager,
        storage: &dyn QueryStorage,
        space: &str,
        data_epoch: u64,
        sample_limit: usize,
    ) -> Result<usize, String> {
        // Stable distinct-count key for a value: type-tagged canonical form.
        fn ndv_key(v: &crate::core::Value) -> String {
            match v {
                crate::core::Value::Null(_) => "n".to_string(),
                crate::core::Value::Bool(b) => format!("b:{b}"),
                crate::core::Value::Int(i) => format!("i:{i}"),
                crate::core::Value::BigInt(i) => format!("l:{i}"),
                crate::core::Value::Float(f) => format!("f:{:?}", f.to_bits()),
                crate::core::Value::Double(d) => format!("d:{:?}", d.to_bits()),
                crate::core::Value::String(s) => format!("s:{s}"),
                crate::core::Value::FixedString(s) => format!("s:{s}"),
                other => format!("o:{other:?}"),
            }
        }

        let mut collected = 0usize;

        // ── Vertex tag properties ──
        let tag_infos = storage.list_tags(space).map_err(|e| {
            format!(
                "Failed to list tags for property stats in space '{}': {}",
                space, e
            )
        })?;
        for tag_info in &tag_infos {
            let tag_name = &tag_info.tag_name;
            let total = storage.count_vertices_by_tag(space, tag_name).unwrap_or(0) as usize;
            let offset = Self::vertex_tag_offset(total, sample_limit, data_epoch, tag_name);

            // Sample a window of vertices for this tag (no full materialization).
            let vertices =
                Self::sample_vertices_by_tag(storage, space, tag_name, sample_limit, offset);
            if tag_info.properties.is_empty() {
                continue;
            }

            // Build NDV per property via exact distinct count on the sample;
            // the same pass maintains the per-column min/max envelope.
            let mut distinct_per_prop: HashMap<String, std::collections::HashSet<String>> =
                HashMap::new();
            let mut stats_per_prop: HashMap<String, PropertyStatistics> = HashMap::new();
            for prop in &tag_info.properties {
                distinct_per_prop.insert(prop.name.clone(), std::collections::HashSet::new());
                stats_per_prop.insert(
                    prop.name.clone(),
                    PropertyStatistics::new(prop.name.clone(), Some(tag_name.clone())),
                );
            }
            for vertex in &vertices {
                let tag_props = vertex
                    .get_tag(tag_name)
                    .map(|t| &t.properties)
                    .unwrap_or(&vertex.properties);
                for (prop_name, bucket) in distinct_per_prop.iter_mut() {
                    if let Some(v) = tag_props
                        .get(prop_name.as_str())
                        .or_else(|| vertex.properties.get(prop_name.as_str()))
                    {
                        bucket.insert(ndv_key(v));
                        if let Some(stat) = stats_per_prop.get_mut(prop_name) {
                            stat.observe_value(v);
                        }
                    }
                }
            }
            for prop_def in &tag_info.properties {
                let sampled_distinct = distinct_per_prop
                    .get(&prop_def.name)
                    .map(|s| s.len() as u64)
                    .unwrap_or(0);

                // Try the storage snapshot first for the envelope.
                let snapshot = storage.vertex_column_stats(space, tag_name, &prop_def.name);

                // Emit the property when the snapshot carries bounds or when
                // sampling observed at least one value.
                let snap_has_bounds = snapshot.as_ref().map(|s| s.has_envelope()).unwrap_or(false);
                if sampled_distinct == 0 && !snap_has_bounds {
                    continue;
                }

                let mut stat = stats_per_prop.remove(&prop_def.name).unwrap_or_else(|| {
                    PropertyStatistics::new(prop_def.name.clone(), Some(tag_name.clone()))
                });
                stat.distinct_values = sampled_distinct.max(1);
                Self::merge_snapshot_bounds(&mut stat, snapshot.as_deref());
                manager.update_property_stats(space, stat);
                collected += 1;
            }
        }

        // ── Edge type properties ──
        let edge_infos = storage.list_edge_types(space).map_err(|e| {
            format!(
                "Failed to list edge types for property stats in space '{}': {}",
                space, e
            )
        })?;
        for edge_info in &edge_infos {
            let edge_type = &edge_info.edge_type_name;
            let total = storage.count_edges_by_type(space, edge_type).unwrap_or(0) as usize;
            let offset = Self::edge_type_offset(total, sample_limit, data_epoch, edge_type);

            let edges = storage
                .scan_edges_by_type_paginated(space, edge_type, offset, sample_limit)
                .or_else(|_| {
                    storage
                        .scan_edges_by_type(space, edge_type)
                        .map(|edges| edges.into_iter().skip(offset).take(sample_limit).collect())
                })
                .unwrap_or_default();
            if edge_info.properties.is_empty() {
                continue;
            }
            let mut distinct_per_prop: HashMap<String, std::collections::HashSet<String>> =
                HashMap::new();
            let mut stats_per_prop: HashMap<String, PropertyStatistics> = HashMap::new();
            for prop in &edge_info.properties {
                distinct_per_prop.insert(prop.name.clone(), std::collections::HashSet::new());
                stats_per_prop.insert(
                    prop.name.clone(),
                    PropertyStatistics::new(prop.name.clone(), Some(edge_type.clone())),
                );
            }
            for edge in &edges {
                for (prop_name, bucket) in distinct_per_prop.iter_mut() {
                    if let Some(v) = edge.get_property(prop_name.as_str()) {
                        bucket.insert(ndv_key(v));
                        if let Some(stat) = stats_per_prop.get_mut(prop_name) {
                            stat.observe_value(v);
                        }
                    }
                }
            }
            for prop_def in &edge_info.properties {
                let sampled_distinct = distinct_per_prop
                    .get(&prop_def.name)
                    .map(|s| s.len() as u64)
                    .unwrap_or(0);
                let snapshot = storage.edge_column_stats(space, edge_type, &prop_def.name);
                let snap_has_bounds = snapshot.as_ref().map(|s| s.has_envelope()).unwrap_or(false);
                if sampled_distinct == 0 && !snap_has_bounds {
                    continue;
                }
                let mut stat = stats_per_prop.remove(&prop_def.name).unwrap_or_else(|| {
                    PropertyStatistics::new(prop_def.name.clone(), Some(edge_type.clone()))
                });
                stat.distinct_values = sampled_distinct.max(1);
                Self::merge_snapshot_bounds(&mut stat, snapshot.as_deref());
                manager.update_property_stats(space, stat);
                collected += 1;
            }
        }

        Ok(collected)
    }
}
