use super::config::{PartitioningConfig, PartitioningDecision, PartitioningLayoutInfo};
use crate::optimizer::stats::StatsView;
use crate::planning::plan::core::nodes::base::plan_node_traits::{
    MultipleInputNode, SingleInputNode,
};
use crate::planning::plan::{PartitionSource, PartitionSpec, PartitionStrategy, PlanNodeEnum};

/// Chooses a partition layout only for a single tagged vertex scan. More
/// complex source topologies retain the existing single-tree path until they
/// have an explicit source-domain mapping in the physical planner.
#[derive(Debug, Clone)]
pub struct PartitioningPlanner {
    config: PartitioningConfig,
}

impl PartitioningPlanner {
    pub fn new(config: PartitioningConfig) -> Self {
        Self { config }
    }

    pub fn config(&self) -> &PartitioningConfig {
        &self.config
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
    fn layout_signature_with_layout(
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
            return Self::fallback("partitioning is disabled");
        }
        if self.config.max_partitions < 2 {
            return Self::fallback("partitioning max_partitions is less than two");
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
                return Self::fallback(
                    "no self-proven vertex-id range is available (storage evidence or config)",
                )
            }
        };
        if range.start >= range.end {
            return Self::fallback("vertex-id range is empty");
        }

        // Reject plans with unsupported node categories.
        if Self::has_write_operation(root) {
            return Self::fallback("plan contains write operations; partitioning not supported");
        }
        if Self::has_transaction_boundary(root) {
            return Self::fallback(
                "plan crosses a transaction boundary; partitioning not supported",
            );
        }
        if Self::has_graph_traversal(root) {
            // E4: allow an anchored *bounded* traversal (a single ExpandAll
            // above the anchor vertex scan). The anchor is partitioned by
            // vertex-id range and each partition expands locally; only
            // recursive / path-algorithm traversals are rejected outright.
            if let Some(decision) = self.decide_anchored_traversal(root, statistics, &range, layout)
            {
                return decision;
            }
            return Self::fallback(
                "plan contains recursive graph traversal; partitioning not supported",
            );
        }

        let mut scans = Vec::new();
        Self::collect_vertex_scans(root, &mut scans);
        if scans.len() == 1 {
            let Some(tag) = scans[0].tag() else {
                return Self::fallback("vertex scan has no tag statistics key");
            };
            let rows = statistics.vertex_count(tag);
            if rows == 0 {
                return Self::fallback(format!(
                    "missing statistics for vertex tag '{tag}'; cannot estimate row count"
                ));
            }
            if rows < self.config.min_rows_per_partition.saturating_mul(2) {
                return Self::fallback(format!(
                    "estimated vertex rows ({rows}) are below the partition threshold"
                ));
            }

            let desired = self.desired_partition_count(rows);
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
                    Self::fallback(format!("invalid configured partition layout: {error}"))
                }
            }
        } else if scans.is_empty() {
            self.decide_edge_scan(root, statistics, &range, layout)
        } else {
            self.decide_multi_scan(root, statistics, &range, layout)
        }
    }

    /// Choose a partition layout for a plan with several independent tagged
    /// vertex scans (UNION / MINUS / INTERSECT / cross-join / equality join
    /// of partition-local scan chains). Every branch shares the same
    /// vertex-id ranges; each branch is scanned independently per partition
    /// and gathered before the global set/join operator runs.
    ///
    /// For equality joins (E1b): when both sides scan the same vertex tag,
    /// they are co-partitioned and the join runs partition-locally without
    /// a hash exchange.  When sides scan different tags, a hash exchange
    /// aligns partitions by the join key.
    fn decide_multi_scan(
        &self,
        root: &PlanNodeEnum,
        statistics: &StatsView,
        range: &std::ops::Range<i64>,
        layout: &PartitioningLayoutInfo,
    ) -> PartitioningDecision {
        let Some((left, right, kind)) = Self::split_independent_branches(root) else {
            return Self::fallback(
                "multi-scan plan is not a union/cross-join/equality-join of independent scan branches",
            );
        };
        let mut left_chain = Vec::new();
        let mut right_chain = Vec::new();
        if !Self::collect_vertex_chain(left, &mut left_chain)
            || !Self::collect_vertex_chain(right, &mut right_chain)
        {
            return Self::fallback(
                "multi-scan branches are not linear chains ending in tagged vertex scans",
            );
        }
        let PlanNodeEnum::ScanVertices(left_scan) = left_chain[left_chain.len() - 1] else {
            return Self::fallback("left branch must end in a tagged vertex scan");
        };
        let PlanNodeEnum::ScanVertices(right_scan) = right_chain[right_chain.len() - 1] else {
            return Self::fallback("right branch must end in a tagged vertex scan");
        };
        let Some(left_tag) = left_scan.tag() else {
            return Self::fallback("left vertex scan has no tag statistics key");
        };
        let Some(right_tag) = right_scan.tag() else {
            return Self::fallback("right vertex scan has no tag statistics key");
        };

        let left_rows = statistics.vertex_count(left_tag);
        let right_rows = statistics.vertex_count(right_tag);
        if left_rows == 0 || right_rows == 0 {
            return Self::fallback(format!(
                "missing statistics for vertex tag(s) '{left_tag}'/'{right_tag}'"
            ));
        }
        let threshold = self.config.min_rows_per_partition.saturating_mul(2);
        if left_rows < threshold || right_rows < threshold {
            return Self::fallback(format!(
                "estimated vertex rows ({left_rows}/{right_rows}) are below the partition threshold"
            ));
        }

        let rows = left_rows.max(right_rows);
        let desired = self.desired_partition_count(rows);
        let ranges = split_range(range, desired);
        let representative = left_tag.to_string();
        let source = PartitionSource::VertexId {
            tag: representative,
        };

        // Q4: classify the join key domain. Keys that reference the
        // vertex-id partition key are co-partitionable by id ranges
        // (Range). Any other simple variable key cannot be mapped onto the
        // id domain, so the plan declares a hash distribution contract and
        // the physical builder aligns rows via its RepartitionHash exchange.
        let strategy = Self::multi_scan_join_strategy(root);

        let layout_version = self.layout_signature_with_layout(&source, &strategy, layout);
        let build = |strategy: &PartitionStrategy| match strategy {
            PartitionStrategy::Range => {
                PartitionSpec::try_new(ranges.clone(), source.clone(), Some(layout_version)).map(
                    |spec| PartitioningDecision {
                        partition_spec: Some(spec),
                        reason: format!(
                    "partitioned {kind} '{left_tag}'/'{right_tag}' into {desired} shared ranges"
                ),
                    },
                )
            }
            PartitionStrategy::Hash { key } => PartitionSpec::try_new_hash(
                key.clone(),
                ranges.clone(),
                source.clone(),
                Some(layout_version),
            )
            .map(|spec| PartitioningDecision {
                partition_spec: Some(spec),
                reason: format!(
                    "hash-partitioned {kind} '{left_tag}'/'{right_tag}' by key '{key}' \
                         into {desired} buckets"
                ),
            }),
            PartitionStrategy::RoundRobin => {
                unreachable!("multi-scan decisions never emit round-robin layouts")
            }
        };
        match build(&strategy) {
            Ok(decision) => decision,
            Err(error) => Self::fallback(format!("invalid configured partition layout: {error}")),
        }
    }

    /// Distribution strategy for a multi-scan plan root.
    ///
    /// Equality joins whose keys all reference the vertex-id partition key
    /// keep the range co-partitioning layout; joins on any other simple
    /// variable key get a `Hash { key }` layout keyed by the first hash-key
    /// variable.
    fn multi_scan_join_strategy(root: &PlanNodeEnum) -> PartitionStrategy {
        const DEFAULT_KEY: &str = "vid";
        let PlanNodeEnum::InnerJoin(join) = root else {
            return PartitionStrategy::Range;
        };
        if !Self::equality_join_keys_are_simple(join.hash_keys(), join.probe_keys()) {
            return PartitionStrategy::Range;
        }
        if Self::keys_reference_vid(join.hash_keys()) && Self::keys_reference_vid(join.probe_keys())
        {
            return PartitionStrategy::Range;
        }
        let key = join
            .hash_keys()
            .first()
            .and_then(|k| k.expression())
            .and_then(|meta| match meta.inner() {
                graphdb_core::types::expr::Expression::Variable(name) => Some(name.clone()),
                _ => None,
            })
            .unwrap_or_else(|| DEFAULT_KEY.to_string());
        PartitionStrategy::Hash { key }
    }

    /// Whether every key expression references the vertex-id partition key.
    fn keys_reference_vid(
        keys: &[graphdb_core::types::expr::contextual::ContextualExpression],
    ) -> bool {
        !keys.is_empty()
            && keys.iter().all(|key| {
                key.expression().is_some_and(|meta| {
                    matches!(
                        meta.inner(),
                        graphdb_core::types::expr::Expression::Variable(name)
                            if name == "vid" || name.ends_with(".vid")
                    )
                })
            })
    }

    /// Choose a partition layout for an anchored bounded traversal (E4):
    /// a linear chain of `ExpandAll` hops above the anchor vertex
    /// scan. The anchor is partitioned by vertex-id range; every partition
    /// runs the bounded traversal locally over its anchor subrange and the
    /// results are gathered globally. Recursive traversals and path
    /// algorithms are not chain-walkable and fall through to rejection.
    ///
    /// Returns `None` when the plan is not an anchored bounded traversal, so
    /// the caller can fall back to the generic graph-traversal rejection.
    fn decide_anchored_traversal(
        &self,
        root: &PlanNodeEnum,
        statistics: &StatsView,
        range: &std::ops::Range<i64>,
        layout: &PartitioningLayoutInfo,
    ) -> Option<PartitioningDecision> {
        let mut chain = Vec::new();
        if !Self::collect_anchored_chain(root, &mut chain) {
            return None;
        }
        // chain is root-first, ending with the anchor scan.
        let expand_count = chain
            .iter()
            .filter(|n| matches!(n, PlanNodeEnum::ExpandAll(_)))
            .count();
        if expand_count == 0 {
            return Some(Self::fallback(
                "anchored traversal must contain at least one ExpandAll hop",
            ));
        }
        // C1: every ExpandAll hop must be de-materialized (id_only/count_only)
        // and filter-free, so each partition runs the full bounded chain over
        // its anchor subrange without changing row semantics.
        let hops_ok = chain.iter().all(|n| match n {
            PlanNodeEnum::ExpandAll(expand) => {
                expand.filter().is_none()
                    && expand.step_limit().unwrap_or(1) == 1
                    && (expand.id_only() || expand.count_only())
            }
            _ => true,
        });
        if !hops_ok {
            return Some(Self::fallback(
                "anchored traversal hops must be filter-free and de-materialized (id_only/count_only)",
            ));
        }
        let PlanNodeEnum::ScanVertices(scan) = chain[chain.len() - 1] else {
            return Some(Self::fallback(
                "anchored traversal must end in a tagged vertex scan",
            ));
        };
        let Some(tag) = scan.tag() else {
            return Some(Self::fallback(
                "anchor vertex scan has no tag statistics key",
            ));
        };
        let rows = statistics.vertex_count(tag);
        if rows == 0 {
            return Some(Self::fallback(format!(
                "missing statistics for anchor tag '{tag}'; cannot estimate row count"
            )));
        }
        if rows < self.config.min_rows_per_partition.saturating_mul(2) {
            return Some(Self::fallback(format!(
                "estimated anchor rows ({rows}) are below the partition threshold"
            )));
        }

        let desired = self.desired_partition_count(rows);
        let ranges = split_range(range, desired);
        let source = PartitionSource::VertexId {
            tag: tag.to_string(),
        };
        let layout_version =
            self.layout_signature_with_layout(&source, &PartitionStrategy::Range, layout);
        match PartitionSpec::try_new(ranges, source, Some(layout_version)) {
            Ok(spec) => Some(PartitioningDecision {
                partition_spec: Some(spec),
                reason: format!(
                    "partitioned anchored traversal by '{tag}' vertex-id ranges into {desired} partitions"
                ),
            }),
            Err(error) => Some(Self::fallback(format!(
                "invalid configured partition layout: {error}"
            ))),
        }
    }

    /// Walk a linear chain that ends in a tagged vertex scan. `Flatten` is
    /// row-preserving (it replays child rows), so it passes through like the
    /// other non-graph operators. Any other graph operator (Traverse, Loop,
    /// path algorithms, vertex-property fetches) fails the walk.
    fn collect_anchored_chain<'a>(
        node: &'a PlanNodeEnum,
        chain: &mut Vec<&'a PlanNodeEnum>,
    ) -> bool {
        chain.push(node);
        match node {
            PlanNodeEnum::ScanVertices(_) => true,
            PlanNodeEnum::Filter(filter) => Self::collect_anchored_chain(filter.input(), chain),
            PlanNodeEnum::Project(project) => Self::collect_anchored_chain(project.input(), chain),
            PlanNodeEnum::Flatten(flatten) => Self::collect_anchored_chain(flatten.input(), chain),
            PlanNodeEnum::Limit(limit) => Self::collect_anchored_chain(limit.input(), chain),
            PlanNodeEnum::Sort(sort) => Self::collect_anchored_chain(sort.input(), chain),
            PlanNodeEnum::Aggregate(agg) => Self::collect_anchored_chain(agg.input(), chain),
            PlanNodeEnum::TopN(topn) => Self::collect_anchored_chain(topn.input(), chain),
            PlanNodeEnum::Dedup(dedup) => Self::collect_anchored_chain(dedup.input(), chain),
            PlanNodeEnum::Window(window) => Self::collect_anchored_chain(window.input(), chain),
            PlanNodeEnum::ExpandAll(expand_all) => {
                if let Some(input) = expand_all.inputs().first() {
                    Self::collect_anchored_chain(input, chain)
                } else {
                    false
                }
            }
            _ => false,
        }
    }

    /// Split a binary-op root into its two branch inputs when the op is a set
    /// op, cross join, or equality join on the partition key (vertex-id).
    fn split_independent_branches(
        node: &PlanNodeEnum,
    ) -> Option<(&PlanNodeEnum, &PlanNodeEnum, &'static str)> {
        match node {
            PlanNodeEnum::Union(union) => Some((union.input(), union.union_input(), "union")),
            PlanNodeEnum::Minus(minus) => Some((minus.input(), minus.minus_input(), "minus")),
            PlanNodeEnum::Intersect(intersect) => {
                Some((intersect.input(), intersect.intersect_input(), "intersect"))
            }
            PlanNodeEnum::CrossJoin(join) => {
                Some((join.left_input(), join.right_input(), "cross join"))
            }
            PlanNodeEnum::InnerJoin(join) => {
                // E1b: allow equality join when the join key is a simple variable
                // reference (i.e. the vertex-id partition key).  Complex join keys
                // (expressions, composite keys) are rejected for now.
                if Self::equality_join_keys_are_simple(join.hash_keys(), join.probe_keys()) {
                    Some((join.left_input(), join.right_input(), "equality join"))
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// Whether a join's hash/probe keys are each a single simple variable
    /// reference (the only key shape the partitioned join path supports).
    fn equality_join_keys_are_simple(
        hash_keys: &[graphdb_core::types::expr::contextual::ContextualExpression],
        probe_keys: &[graphdb_core::types::expr::contextual::ContextualExpression],
    ) -> bool {
        hash_keys.len() == 1
            && probe_keys.len() == 1
            && hash_keys
                .first()
                .and_then(|k| k.expression())
                .is_some_and(|m| {
                    matches!(
                        m.inner(),
                        graphdb_core::types::expr::Expression::Variable(_)
                    )
                })
            && probe_keys
                .first()
                .and_then(|k| k.expression())
                .is_some_and(|m| {
                    matches!(
                        m.inner(),
                        graphdb_core::types::expr::Expression::Variable(_)
                    )
                })
    }

    /// Walk a linear chain that must end in a tagged vertex scan.
    fn collect_vertex_chain<'a>(node: &'a PlanNodeEnum, chain: &mut Vec<&'a PlanNodeEnum>) -> bool {
        chain.push(node);
        match node {
            PlanNodeEnum::ScanVertices(_) => true,
            PlanNodeEnum::Filter(filter) => Self::collect_vertex_chain(filter.input(), chain),
            PlanNodeEnum::Project(project) => Self::collect_vertex_chain(project.input(), chain),
            PlanNodeEnum::Limit(limit) => Self::collect_vertex_chain(limit.input(), chain),
            PlanNodeEnum::Sort(sort) => Self::collect_vertex_chain(sort.input(), chain),
            PlanNodeEnum::Aggregate(agg) => Self::collect_vertex_chain(agg.input(), chain),
            PlanNodeEnum::TopN(topn) => Self::collect_vertex_chain(topn.input(), chain),
            PlanNodeEnum::Dedup(dedup) => Self::collect_vertex_chain(dedup.input(), chain),
            PlanNodeEnum::Window(window) => Self::collect_vertex_chain(window.input(), chain),
            _ => false,
        }
    }

    /// Number of partitions to cut for a relation with `rows` estimated rows.
    ///
    /// E5 granularity tuning: the baseline is `rows / min_rows_per_partition`
    /// (one partition per threshold bucket), but the result is additionally
    /// bounded by the available worker threads (`max_workers`) so we never cut
    /// more partitions than can run concurrently — the actual parallelism is
    /// `min(partitions, workers)`, and surplus partitions only add exchange
    /// overhead. Small relations are rejected earlier via the `2 * min_rows`
    /// threshold; a zero-size relation here clamps to the minimum of 2.
    fn desired_partition_count(&self, rows: u64) -> usize {
        let by_rows = usize::try_from(rows / self.config.min_rows_per_partition)
            .unwrap_or(self.config.max_partitions);
        let by_workers = self.config.max_workers;
        by_rows
            .clamp(2, self.config.max_partitions)
            .min(by_workers.max(2))
    }

    /// Choose an edge-scan partition layout for a pure edge-table chain.
    ///
    /// The plan must be a linear chain ending in a `ScanEdges` node whose rows
    /// are self-sufficient (edge properties / aggregates only). Chains that
    /// fetch src/dst vertex properties (Expand, GetProp, AppendVertices) are
    /// rejected because partitioning by src-id range cannot provide the
    /// vertex-side join key.
    fn decide_edge_scan(
        &self,
        root: &PlanNodeEnum,
        statistics: &StatsView,
        range: &std::ops::Range<i64>,
        layout: &PartitioningLayoutInfo,
    ) -> PartitioningDecision {
        let mut chain = Vec::new();
        if !Self::collect_chain(root, &mut chain) {
            return Self::fallback(
                "edge scan plan is not a linear chain ending in a ScanEdges node",
            );
        }
        let PlanNodeEnum::ScanEdges(scan) = chain[chain.len() - 1] else {
            return Self::fallback(
                "edge scan plan must end in a ScanEdges node for partition selection",
            );
        };
        let Some(edge_type) = scan.edge_type() else {
            return Self::fallback("edge scan has no edge type statistics key");
        };
        let rows = statistics.edge_count(&edge_type);
        if rows == 0 {
            return Self::fallback(format!(
                "missing statistics for edge type '{edge_type}'; cannot estimate row count"
            ));
        }
        if rows < self.config.min_rows_per_partition.saturating_mul(2) {
            return Self::fallback(format!(
                "estimated edge rows ({rows}) are below the partition threshold"
            ));
        }

        let desired = self.desired_partition_count(rows);
        let ranges = split_range(range, desired);
        let source = PartitionSource::EdgeId {
            edge_type: edge_type.to_string(),
        };
        let layout_version =
            self.layout_signature_with_layout(&source, &PartitionStrategy::Range, layout);
        match PartitionSpec::try_new(ranges, source, Some(layout_version)) {
            Ok(spec) => {
                let description = spec.source().to_string();
                PartitioningDecision {
                    partition_spec: Some(spec),
                    reason: format!(
                        "partitioned edge scan {description} into {desired} src-id ranges from trusted layout"
                    ),
                }
            }
            Err(error) => Self::fallback(format!("invalid configured partition layout: {error}")),
        }
    }

    /// Walk a linear chain from `node` down to its terminal scan. Supports the
    /// same unary operators as the physical partition builder; anything else
    /// (joins, Expand, vertex lookups, set ops) returns `false`.
    fn collect_chain<'a>(node: &'a PlanNodeEnum, chain: &mut Vec<&'a PlanNodeEnum>) -> bool {
        chain.push(node);
        match node {
            PlanNodeEnum::ScanVertices(_) | PlanNodeEnum::ScanEdges(_) => true,
            PlanNodeEnum::Filter(filter) => Self::collect_chain(filter.input(), chain),
            PlanNodeEnum::Project(project) => Self::collect_chain(project.input(), chain),
            PlanNodeEnum::Limit(limit) => Self::collect_chain(limit.input(), chain),
            PlanNodeEnum::Sort(sort) => Self::collect_chain(sort.input(), chain),
            PlanNodeEnum::Aggregate(agg) => Self::collect_chain(agg.input(), chain),
            PlanNodeEnum::TopN(topn) => Self::collect_chain(topn.input(), chain),
            PlanNodeEnum::Dedup(dedup) => Self::collect_chain(dedup.input(), chain),
            PlanNodeEnum::Window(window) => Self::collect_chain(window.input(), chain),
            _ => false,
        }
    }

    fn collect_vertex_scans<'a>(
        node: &'a PlanNodeEnum,
        scans: &mut Vec<&'a crate::planning::plan::core::nodes::ScanVerticesNode>,
    ) {
        if let PlanNodeEnum::ScanVertices(scan) = node {
            scans.push(scan);
        }
        for child in node.children() {
            Self::collect_vertex_scans(child, scans);
        }
    }

    /// Returns true when the plan tree contains any write operation node.
    fn has_write_operation(node: &PlanNodeEnum) -> bool {
        matches!(
            node,
            PlanNodeEnum::CopyFrom(_)
                | PlanNodeEnum::CopyTo(_)
                | PlanNodeEnum::InsertVertices(_)
                | PlanNodeEnum::InsertEdges(_)
                | PlanNodeEnum::DeleteVertices(_)
                | PlanNodeEnum::DeleteEdges(_)
                | PlanNodeEnum::DeleteTags(_)
                | PlanNodeEnum::DeleteIndex(_)
                | PlanNodeEnum::PipeDeleteVertices(_)
                | PlanNodeEnum::PipeDeleteEdges(_)
                | PlanNodeEnum::Update(_)
                | PlanNodeEnum::UpdateVertices(_)
                | PlanNodeEnum::UpdateEdges(_)
        ) || node.children().iter().any(|c| Self::has_write_operation(c))
    }

    /// Returns true when the plan tree contains a transaction-control node.
    fn has_transaction_boundary(node: &PlanNodeEnum) -> bool {
        matches!(
            node,
            PlanNodeEnum::BeginTransaction(_) | PlanNodeEnum::Commit(_) | PlanNodeEnum::Rollback(_)
        ) || node
            .children()
            .iter()
            .any(|c| Self::has_transaction_boundary(c))
    }

    /// Returns true when the plan tree contains a recursive graph traversal
    /// or path-algorithm node (single-hop `Expand`/`BiExpand` are
    /// morsel-parallel and no longer block partitioning; only variable-length
    /// `ExpandAll`/`Traverse` and path algorithms are considered graph
    /// traversals for partition blocking).
    fn has_graph_traversal(node: &PlanNodeEnum) -> bool {
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
        ) || node.children().iter().any(|c| Self::has_graph_traversal(c))
    }

    fn fallback(reason: impl Into<String>) -> PartitioningDecision {
        PartitioningDecision {
            partition_spec: None,
            reason: reason.into(),
        }
    }
}

fn split_range(range: &std::ops::Range<i64>, partition_count: usize) -> Vec<std::ops::Range<i64>> {
    let total = range.end - range.start;
    let width = (total + partition_count as i64 - 1) / partition_count as i64;
    let mut ranges = Vec::with_capacity(partition_count);
    for index in 0..partition_count {
        let start = range.start + (index as i64) * width;
        if start >= range.end {
            break;
        }
        ranges.push(start..(start + width).min(range.end));
    }
    ranges
}
