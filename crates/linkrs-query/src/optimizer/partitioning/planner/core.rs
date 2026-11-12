use std::sync::Arc;

use super::super::config::{PartitioningConfig, PartitioningDecision, PartitioningLayoutInfo};
use crate::optimizer::cost::SelectivityEstimator;
use crate::optimizer::stats::feedback::selectivity::SelectivitySnapshot;
use crate::optimizer::stats::StatsView;
use crate::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode;
use crate::planning::plan::{PartitionSource, PartitionSpec, PartitionStrategy, PlanNodeEnum};

use super::anchored::decide_anchored_traversal;
use super::edge::{collect_vertex_scans, decide_edge_scan};
use super::helpers::{collect_condition_properties, split_range};
use super::multi_scan::decide_multi_scan;

/// Chooses a partition layout only for a single tagged vertex scan. More
/// complex source topologies retain the existing single-tree path until they
/// have an explicit source-domain mapping in the physical planner.
#[derive(Debug, Clone)]
pub struct PartitioningPlanner {
    config: PartitioningConfig,
    selectivity: Option<Arc<SelectivityEstimator>>,
}
impl PartitioningPlanner {
    pub fn new(config: PartitioningConfig) -> Self {
        Self {
            config,
            selectivity: None,
        }
    }

    /// Attach the shared selectivity estimator so partition decisions can
    /// weigh post-filter row counts. Without it every decision falls back to
    /// the table-total behavior it had before selectivity awareness existed.
    pub fn with_selectivity(mut self, selectivity: Arc<SelectivityEstimator>) -> Self {
        self.selectivity = Some(selectivity);
        self
    }

    pub fn config(&self) -> &PartitioningConfig {
        &self.config
    }

    /// Estimate the rows surviving the filters on an anchor chain.
    ///
    /// The chain runs root-first and ends in the anchor scan. Filters below
    /// the first graph expansion filter anchor rows, so each partition does
    /// less work; filters above an expansion apply after fan-out and do not
    /// shrink the per-partition scan, so estimation stops there. A filter is
    /// only trusted when at least one property it references has collected
    /// statistics — purely heuristic estimates keep the previous behavior
    /// instead of flipping plans on guesses.
    pub(crate) fn estimate_filtered_rows(
        &self,
        chain: &[&PlanNodeEnum],
        rows: u64,
        tag: &str,
        statistics: &StatsView,
        snapshot: &SelectivitySnapshot,
    ) -> u64 {
        let Some(selectivity) = &self.selectivity else {
            return rows;
        };
        let mut combined = 1.0f64;
        let mut trusted = false;
        for node in chain.iter().rev().skip(1) {
            match node {
                PlanNodeEnum::ExpandAll(_) => break,
                PlanNodeEnum::Filter(filter) => {
                    let Some(meta) = filter.condition().expression() else {
                        continue;
                    };
                    let mut properties = Vec::new();
                    collect_condition_properties(meta.inner(), &mut properties);
                    let backed = properties
                        .iter()
                        .any(|property| statistics.property_stats(Some(tag), property).is_some());
                    if !backed {
                        continue;
                    }
                    // Read-only estimate through the decision snapshot: the
                    // recording entry point would register conditions and
                    // refresh baselines, making identical plans decide
                    // differently depending on planning order.
                    combined *= selectivity.estimate_readonly(
                        statistics.space(),
                        meta.inner(),
                        Some(tag),
                        snapshot,
                    );
                    trusted = true;
                }
                _ => {}
            }
        }
        if !trusted {
            return rows;
        }
        (rows as f64 * combined.clamp(0.0, 1.0)).clamp(1.0, rows as f64) as u64
    }

    /// Economic gate: partition only when the parallel saving exceeds the
    /// fixed plus exchange overhead, all in row-equivalents. Returns a
    /// fallback reason when serial wins. All-zero cost params disable the
    /// comparison and leave the row-count threshold as the only gate.
    pub(crate) fn cost_gate_reason(&self, work: u64, output_rows: u64, partitions: usize) -> Option<String> {
        let fixed = self.config.fixed_overhead_rows;
        let per_partition = self.config.per_partition_rows;
        let per_mille = self.config.exchange_row_per_mille;
        if fixed == 0 && per_partition == 0 && per_mille == 0 {
            return None;
        }
        let count = partitions as u64;
        let saving = work.saturating_sub(work / count.max(1));
        let exchange = output_rows.saturating_mul(per_mille as u64) / 1000;
        let overhead = fixed
            .saturating_add(count.saturating_mul(per_partition))
            .saturating_add(exchange);
        if saving > overhead {
            None
        } else {
            Some(format!(
                "parallel cost ({overhead}) exceeds the serial saving ({saving}) \
                 for {work} rows over {partitions} partitions"
            ))
        }
    }

    /// Deterministic layout signature for the current partitioning config and
    /// data domain.  It is embedded in the [`PartitionSpec::layout_version`]
    /// so the plan cache fingerprint changes whenever the trusted vertex-id
    /// range, partition granularity, data domain, or the storage's monotonic
    /// layout version changes — forcing a replan instead of reusing a stale
    /// cached partition layout.
    ///
    /// When the storage provides a real monotonic layout version, it replaces
    /// the config-range component of the signature (the storage layout is the
    /// authoritative data-domain witness); otherwise the configured range is
    /// hashed as before.
    pub(crate) fn layout_signature_with_layout(
        &self,
        source: &PartitionSource,
        strategy: &PartitionStrategy,
        layout: &PartitioningLayoutInfo,
    ) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        if layout.layout_version != 0 {
            layout.layout_version.hash(&mut hasher);
        } else {
            match &self.config.vertex_id_range {
                Some(range) => {
                    range.start.hash(&mut hasher);
                    range.end.hash(&mut hasher);
                }
                None => {
                    hasher.write_u8(0);
                }
            }
        }
        self.config.min_rows_per_partition.hash(&mut hasher);
        self.config.max_partitions.hash(&mut hasher);
        self.config.max_workers.hash(&mut hasher);
        source.to_string().hash(&mut hasher);
        // The distribution strategy participates in the signature so a
        // strategy switch invalidates cached plans that embed the old spec.
        strategy.to_string().hash(&mut hasher);
        hasher.finish()
    }

    /// Config-only layout signature (no storage layout information).
    #[cfg(test)]
    pub(crate) fn layout_signature(&self, source: &PartitionSource) -> u64 {
        self.layout_signature_with_layout(
            source,
            &PartitionStrategy::Range,
            &PartitioningLayoutInfo::default(),
        )
    }

    /// Decide using only the static configuration (config-range fallback).
    ///
    /// Convenience entry used by tests and callers without storage layout
    /// information; the pipeline passes [`PartitioningLayoutInfo`] via
    /// [`decide_with_layout`](Self::decide_with_layout).
    pub fn decide(&self, root: &PlanNodeEnum, statistics: &StatsView) -> PartitioningDecision {
        self.decide_with_layout(root, statistics, &PartitioningLayoutInfo::default())
    }

    /// Decide using storage-provided layout information (self-proven
    /// vertex-id domain and monotonic layout version).
    pub fn decide_with_layout(
        &self,
        root: &PlanNodeEnum,
        statistics: &StatsView,
        layout: &PartitioningLayoutInfo,
    ) -> PartitioningDecision {
        if !self.config.enabled {
            return fallback("partitioning is disabled");
        }
        if self.config.max_partitions < 2 {
            return fallback("partitioning max_partitions is less than two");
        }
        // The storage self-proven domain wins; the configured range is a
        // fallback for setups that explicitly trust a caller-supplied range.
        let range = match layout
            .vertex_id_range
            .clone()
            .or_else(|| self.config.vertex_id_range.clone())
        {
            Some(range) => range,
            None => {
                return fallback(
                    "no self-proven vertex-id range is available (storage evidence or config)",
                )
            }
        };
        if range.start >= range.end {
            return fallback("vertex-id range is empty");
        }

        // One snapshot per decision: every estimate below reads it, so the
        // decision is self-consistent and never mutates shared feedback.
        let snapshot = self
            .selectivity
            .as_ref()
            .map(|estimator| estimator.snapshot())
            .unwrap_or_default();

        // Reject plans with unsupported node categories.
        if has_write_operation(root) {
            return fallback("plan contains write operations; partitioning not supported");
        }
        if has_transaction_boundary(root) {
            return fallback(
                "plan crosses a transaction boundary; partitioning not supported",
            );
        }
        if has_graph_traversal(root) {
            // E4: allow an anchored *bounded* traversal (a single ExpandAll
            // above the anchor vertex scan). The anchor is partitioned by
            // vertex-id range and each partition expands locally; only
            // recursive / path-algorithm traversals are rejected outright.
            if let Some(decision) =
                decide_anchored_traversal(self, root, statistics, &range, layout, &snapshot)
            {
                return decision;
            }
            return fallback(
                "plan contains recursive graph traversal; partitioning not supported",
            );
        }

        let mut scans = Vec::new();
        collect_vertex_scans(root, &mut scans);
        if scans.len() == 1 {
            let Some(tag) = scans[0].tag() else {
                return fallback("vertex scan has no tag statistics key");
            };
            let rows = statistics.vertex_count(tag);
            if rows == 0 {
                return fallback(format!(
                    "missing statistics for vertex tag '{tag}'; cannot estimate row count"
                ));
            }
            let threshold = self.config.min_rows_per_partition.saturating_mul(2);
            if rows < threshold {
                return fallback(format!(
                    "estimated vertex rows ({rows}) are below the partition threshold"
                ));
            }
            let mut chain = Vec::new();
            // Size partitions by the rows that actually flow through them,
            // not the table total: a filter just above the gate must not cut
            // the same partition count as the unfiltered table.
            let work = if collect_vertex_chain(root, &mut chain) {
                let filtered =
                    self.estimate_filtered_rows(&chain, rows, tag, statistics, &snapshot);
                if filtered < threshold {
                    return fallback(format!(
                        "estimated filtered rows ({filtered}) are below the partition threshold"
                    ));
                }
                filtered
            } else {
                rows
            };

            let desired = self.desired_partition_count(work);
            if let Some(reason) = self.cost_gate_reason(work, work, desired) {
                return fallback(reason);
            }
            let ranges = split_range(&range, desired);
            let source = PartitionSource::VertexId {
                tag: tag.to_string(),
            };
            let layout_version =
                self.layout_signature_with_layout(&source, &PartitionStrategy::Range, layout);
            match PartitionSpec::try_new(
                ranges,
                source,
                // Layout version is a signature of the partitioning config
                // and data domain; the storage layer may supply a real
                // monotonic version in a later phase.
                Some(layout_version),
            ) {
                Ok(spec) => PartitioningDecision {
                    partition_spec: Some(spec),
                    reason: format!(
                        "partitioned tagged vertex scan '{}' into {} ranges from trusted layout",
                        tag, desired
                    ),
                },
                Err(error) => {
                    fallback(format!("invalid configured partition layout: {error}"))
                }
            }
        } else if scans.is_empty() {
            decide_edge_scan(self, root, statistics, &range, layout)
        } else {
            decide_multi_scan(self, root, statistics, &range, layout, &snapshot)
        }
    }

    /// Number of partitions to cut for a relation with `rows` estimated rows.
    ///
    /// The baseline is `rows / min_rows_per_partition` (one partition per
    /// threshold bucket), but the result is additionally bounded by the
    /// available worker threads (`max_workers`) so we never cut more
    /// partitions than can run concurrently, and by the economic worker
    /// bound so each partition carries at least two threshold buckets.
    /// The two-bucket floor mirrors the `2 * min_rows` admission gate every
    /// decide path shares: one bucket per partition leaves no margin for
    /// exchange overhead, which is where throughput plateaus past four
    /// workers. Small relations are rejected earlier via that same gate;
    /// a zero-size relation here clamps to the minimum of 2.
    pub(crate) fn desired_partition_count(&self, rows: u64) -> usize {
        let by_rows = usize::try_from(rows / self.config.min_rows_per_partition)
            .unwrap_or(self.config.max_partitions);
        let two_buckets = self.config.min_rows_per_partition.saturating_mul(2).max(1);
        let by_economics = usize::try_from(rows / two_buckets)
            .unwrap_or(self.config.max_partitions)
            .max(2);
        let by_workers = self.config.max_workers;
        by_rows
            .clamp(2, self.config.max_partitions)
            .min(by_economics)
            .min(by_workers.max(2))
    }
}

    /// Returns true when the plan tree contains any write operation node.
pub(crate) fn has_write_operation(node: &PlanNodeEnum) -> bool {
        matches!(
            node,
            PlanNodeEnum::CopyFrom(_)
                | PlanNodeEnum::CopyTo(_)
                | PlanNodeEnum::InsertVertices(_)
                | PlanNodeEnum::InsertEdges(_)
                | PlanNodeEnum::DeleteVertices(_)
                | PlanNodeEnum::DeleteEdges(_)
                | PlanNodeEnum::DeleteIndex(_)
                | PlanNodeEnum::PipeDeleteVertices(_)
                | PlanNodeEnum::PipeDeleteEdges(_)
                | PlanNodeEnum::Update(_)
        ) || node.children().iter().any(|c| has_write_operation(c))
    }

    /// Returns true when the plan tree contains a transaction-control node.
pub(crate) fn has_transaction_boundary(node: &PlanNodeEnum) -> bool {
        matches!(
            node,
            PlanNodeEnum::BeginTransaction(_) | PlanNodeEnum::Commit(_) | PlanNodeEnum::Rollback(_)
        ) || node
            .children()
            .iter()
            .any(|c| has_transaction_boundary(c))
    }

    /// Returns true when the plan tree contains a recursive graph traversal
    /// or path-algorithm node (single-hop `Expand`/`BiExpand` are
    /// morsel-parallel and no longer block partitioning; only variable-length
    /// `ExpandAll`/`Traverse` and path algorithms are considered graph
    /// traversals for partition blocking).
pub(crate) fn has_graph_traversal(node: &PlanNodeEnum) -> bool {
        matches!(
            node,
            PlanNodeEnum::ExpandAll(_)
                | PlanNodeEnum::Traverse(_)
                | PlanNodeEnum::AppendVertices(_)
                | PlanNodeEnum::BiTraverse(_)
                | PlanNodeEnum::Loop(_)
                | PlanNodeEnum::RecursiveCte(_)
                | PlanNodeEnum::MultiShortestPath(_)
                | PlanNodeEnum::BFSShortest(_)
                | PlanNodeEnum::AllPaths(_)
                | PlanNodeEnum::ShortestPath(_)
        ) || node.children().iter().any(|c| has_graph_traversal(c))
    }

pub(crate) fn fallback(reason: impl Into<String>) -> PartitioningDecision {
        PartitioningDecision {
            partition_spec: None,
            reason: reason.into(),
        }
    }

    /// Walk a linear chain that must end in a tagged vertex scan.
pub(crate) fn collect_vertex_chain<'a>(node: &'a PlanNodeEnum, chain: &mut Vec<&'a PlanNodeEnum>) -> bool {
        chain.push(node);
        match node {
            PlanNodeEnum::ScanVertices(_) => true,
            PlanNodeEnum::Filter(filter) => collect_vertex_chain(filter.input(), chain),
            PlanNodeEnum::Project(project) => collect_vertex_chain(project.input(), chain),
            PlanNodeEnum::Limit(limit) => collect_vertex_chain(limit.input(), chain),
            PlanNodeEnum::Sort(sort) => collect_vertex_chain(sort.input(), chain),
            PlanNodeEnum::Aggregate(agg) => collect_vertex_chain(agg.input(), chain),
            PlanNodeEnum::TopN(topn) => collect_vertex_chain(topn.input(), chain),
            PlanNodeEnum::Dedup(dedup) => collect_vertex_chain(dedup.input(), chain),
            PlanNodeEnum::Window(window) => collect_vertex_chain(window.input(), chain),
            _ => false,
        }
    }
