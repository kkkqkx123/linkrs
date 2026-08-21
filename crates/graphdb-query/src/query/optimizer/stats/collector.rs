//! Statistics collection module
//!
//! Collects tag/edge statistics for a space from the storage layer and
//! writes them into the [`StatisticsManager`] with space + schema-version
//! provenance.

use std::sync::Arc;

use parking_lot::RwLock;

use crate::core::types::EdgeDirection;
use crate::core::vertex_edge_path::Vertex;
use crate::storage::{QueryStorage, ScanOptions};

use super::{EdgeTypeStatistics, StatisticsManager, TagStatistics};

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
    #[allow(dead_code)]
    fn collected(tags: usize, edge_types: usize) -> Self {
        Self {
            tags,
            edge_types,
            properties: 0,
            cached: false,
        }
    }

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
/// v1 scope: exact vertex/edge counts plus sampled average degrees. Property
/// statistics are not collected (full scans are too expensive); runtime
/// feedback (A2) compensates for them indirectly.
pub struct StatisticsCollector;

impl StatisticsCollector {
    /// Collect (or serve cached) statistics for one space and write them to
    /// the manager.
    ///
    /// When `schema_version` matches the version recorded for `space`, the
    /// cached result is returned without touching storage.
    #[allow(clippy::type_complexity)]
    pub fn collect_space(
        manager: &StatisticsManager,
        storage: &Arc<RwLock<dyn QueryStorage>>,
        space: &str,
        schema_version: u64,
        sample_limit: usize,
    ) -> Result<CollectedSummary, String> {
        if manager.space_version(space) == Some(schema_version) {
            return Ok(CollectedSummary::cached());
        }

        let storage = storage.read();

        let tags = Self::collect_tags(manager, &*storage, space, schema_version, sample_limit)?;
        let edge_types =
            Self::collect_edge_types(manager, &*storage, space, schema_version, sample_limit)?;
        let properties =
            Self::collect_property_stats(manager, &*storage, space, sample_limit).unwrap_or(0);

        manager.set_space_version(space, schema_version);
        Ok(CollectedSummary::collected_with_props(
            tags, edge_types, properties,
        ))
    }

    fn collect_tags(
        manager: &StatisticsManager,
        storage: &dyn QueryStorage,
        space: &str,
        schema_version: u64,
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
        let vertices = Self::sample_vertices_by_tag(storage, space, tag_name, sample_limit);
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

    /// Read at most `sample_limit` vertices of one tag without materializing
    /// the whole table.
    ///
    /// Prefers the storage cursor path (tag restriction + limit give a true
    /// early-exit scan); engines without cursor support fall back to a full
    /// scan truncated to the window.
    fn sample_vertices_by_tag(
        storage: &dyn QueryStorage,
        space: &str,
        tag_name: &str,
        sample_limit: usize,
    ) -> Vec<Vertex> {
        if sample_limit == 0 {
            return Vec::new();
        }
        let mut options = ScanOptions::new();
        options.tag = Some(tag_name.to_string());
        options.limit = Some(sample_limit);
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
            .map(|vertices| vertices.into_iter().take(sample_limit).collect())
            .unwrap_or_default()
    }

    fn collect_edge_types(
        manager: &StatisticsManager,
        storage: &dyn QueryStorage,
        space: &str,
        schema_version: u64,
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

        use std::collections::HashMap;
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

    /// Collect per-property distinct-value (NDV) estimates from a sampled
    /// row window. Populates `PropertyStatistics` for both vertex tags and
    /// edge types, enabling column-narrow CBO (selectivity = 1/NDV).
    /// Histograms are left disabled on the sampled path; runtime execution
    /// feedback compensates for skew. A bucketed histogram can be added here
    /// once sampling is proven stable.
    fn collect_property_stats(
        manager: &StatisticsManager,
        storage: &dyn QueryStorage,
        space: &str,
        sample_limit: usize,
    ) -> Result<usize, String> {
        use crate::query::optimizer::stats::PropertyStatistics;
        use std::collections::HashSet;

        // Stable distinct-count key for a value: type-tagged canonical form.
        // (Debug formatting is not a stable cross-release contract.)
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
            // Sample a window of vertices for this tag (no full materialization).
            let vertices = Self::sample_vertices_by_tag(storage, space, tag_name, sample_limit);
            if vertices.is_empty() || tag_info.properties.is_empty() {
                continue;
            }
            // Build NDV per property via exact distinct count on the sample.
            let mut distinct_per_prop: std::collections::HashMap<String, HashSet<String>> =
                std::collections::HashMap::new();
            for prop in &tag_info.properties {
                distinct_per_prop.insert(prop.name.clone(), HashSet::new());
            }
            for vertex in &vertices {
                // Vertex may carry properties in its Tag or in vertex-level map.
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
                    }
                    // Missing properties do not contribute to NDV.
                }
            }
            for prop_def in &tag_info.properties {
                let distinct = distinct_per_prop
                    .get(&prop_def.name)
                    .map(|s| s.len() as u64)
                    .unwrap_or(0);
                // Skip properties with no observable values (empty tag, all-null sample).
                if distinct == 0 {
                    continue;
                }
                let mut stat =
                    PropertyStatistics::new(prop_def.name.clone(), Some(tag_name.clone()));
                stat.distinct_values = distinct.max(1);
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
            // Paginated read bounds the window; the full-scan fallback keeps
            // cursorless engines working (truncated to the same window).
            let edges = storage
                .scan_edges_by_type_paginated(space, edge_type, 0, sample_limit)
                .or_else(|_| {
                    storage
                        .scan_edges_by_type(space, edge_type)
                        .map(|edges| edges.into_iter().take(sample_limit).collect())
                })
                .unwrap_or_default();
            if edges.is_empty() || edge_info.properties.is_empty() {
                continue;
            }
            let mut distinct_per_prop: std::collections::HashMap<String, HashSet<String>> =
                std::collections::HashMap::new();
            for prop in &edge_info.properties {
                distinct_per_prop.insert(prop.name.clone(), HashSet::new());
            }
            for edge in &edges {
                for (prop_name, bucket) in distinct_per_prop.iter_mut() {
                    if let Some(v) = edge.get_property(prop_name.as_str()) {
                        bucket.insert(ndv_key(v));
                    }
                }
            }
            for prop_def in &edge_info.properties {
                let distinct = distinct_per_prop
                    .get(&prop_def.name)
                    .map(|s| s.len() as u64)
                    .unwrap_or(0);
                if distinct == 0 {
                    continue;
                }
                let mut stat =
                    PropertyStatistics::new(prop_def.name.clone(), Some(edge_type.clone()));
                stat.distinct_values = distinct.max(1);
                manager.update_property_stats(space, stat);
                collected += 1;
            }
        }

        Ok(collected)
    }
}
