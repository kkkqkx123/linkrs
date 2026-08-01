//! Statistics collection module
//!
//! Collects tag/edge statistics for a space from the storage layer and
//! writes them into the [`StatisticsManager`] with space + schema-version
//! provenance.

use std::sync::Arc;

use parking_lot::RwLock;

use crate::core::types::EdgeDirection;
use crate::storage::QueryStorage;

use super::{EdgeTypeStatistics, StatisticsManager, TagStatistics};

/// Result of a collection pass for one space.
#[derive(Debug, Clone, Default)]
pub struct CollectedSummary {
    /// Number of tags with statistics collected.
    pub tags: usize,
    /// Number of edge types with statistics collected.
    pub edge_types: usize,
    /// Whether the result was served from the cached (version-matched) data.
    pub cached: bool,
}

impl CollectedSummary {
    fn collected(tags: usize, edge_types: usize) -> Self {
        Self {
            tags,
            edge_types,
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

        manager.set_space_version(space, schema_version);
        Ok(CollectedSummary::collected(tags, edge_types))
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
        let vertices = storage
            .scan_vertices_by_tag(space, tag_name)
            .map_err(|e| format!("Failed to scan vertices for tag '{}': {}", tag_name, e))?;

        let sample_len = vertices.len().min(sample_limit);
        if sample_len == 0 {
            return Ok((0.0, 0.0));
        }

        let mut out_total: u64 = 0;
        let mut in_total: u64 = 0;
        for vertex in vertices.iter().take(sample_len) {
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
}
