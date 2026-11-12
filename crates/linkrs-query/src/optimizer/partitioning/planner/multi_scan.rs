use crate::optimizer::stats::StatsView;
use crate::optimizer::stats::feedback::selectivity::SelectivitySnapshot;
use crate::planning::plan::{PartitionSource, PartitionSpec, PartitionStrategy, PlanNodeEnum};

use super::super::config::{PartitioningDecision, PartitioningLayoutInfo};
use super::anchored::{equality_join_keys_are_simple, split_independent_branches};
use super::core::{collect_vertex_chain, fallback, PartitioningPlanner};
use super::helpers::split_range;

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
pub(crate) fn decide_multi_scan(
    planner: &PartitioningPlanner,
        root: &PlanNodeEnum,
        statistics: &StatsView,
        range: &std::ops::Range<i64>,
        layout: &PartitioningLayoutInfo,
        snapshot: &SelectivitySnapshot,
    ) -> PartitioningDecision {
        let Some((left, right, kind)) = split_independent_branches(root) else {
            return fallback(
                "multi-scan plan is not a union/cross-join/equality-join of independent scan branches",
            );
        };
        let mut left_chain = Vec::new();
        let mut right_chain = Vec::new();
        if !collect_vertex_chain(left, &mut left_chain)
            || !collect_vertex_chain(right, &mut right_chain)
        {
            return fallback(
                "multi-scan branches are not linear chains ending in tagged vertex scans",
            );
        }
        let PlanNodeEnum::ScanVertices(left_scan) = left_chain[left_chain.len() - 1] else {
            return fallback("left branch must end in a tagged vertex scan");
        };
        let PlanNodeEnum::ScanVertices(right_scan) = right_chain[right_chain.len() - 1] else {
            return fallback("right branch must end in a tagged vertex scan");
        };
        let Some(left_tag) = left_scan.tag() else {
            return fallback("left vertex scan has no tag statistics key");
        };
        let Some(right_tag) = right_scan.tag() else {
            return fallback("right vertex scan has no tag statistics key");
        };

        let left_rows = statistics.vertex_count(left_tag);
        let right_rows = statistics.vertex_count(right_tag);
        if left_rows == 0 || right_rows == 0 {
            return fallback(format!(
                "missing statistics for vertex tag(s) '{left_tag}'/'{right_tag}'"
            ));
        }
        let threshold = planner.config().min_rows_per_partition.saturating_mul(2);
        let left_filtered =
            planner.estimate_filtered_rows(&left_chain, left_rows, left_tag, statistics, snapshot);
        let right_filtered =
            planner.estimate_filtered_rows(&right_chain, right_rows, right_tag, statistics, snapshot);
        // Branches run independently under one shared layout and gather
        // afterwards, so the dominant branch decides: a tiny branch must not
        // veto partitioning that pays off for the branch holding the work.
        // (Co-partitioned joins stay correct regardless of branch sizes —
        // fragment pairing only needs the shared layout, not balanced rows.)
        // There is intentionally no per-branch raw gate: without a trusted
        // filter the filtered estimate degrades to the raw count, so the
        // single dominant-branch gate below subsumes it.
        let work = left_filtered.max(right_filtered);
        if work < threshold {
            return fallback(format!(
                "estimated filtered rows ({left_filtered}/{right_filtered}) are below the partition threshold"
            ));
        }

        let desired = planner.desired_partition_count(work);
        let output_rows = left_filtered.saturating_add(right_filtered);
        if let Some(reason) = planner.cost_gate_reason(work, output_rows, desired) {
            return fallback(reason);
        }
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
        let strategy = multi_scan_join_strategy(root);

        let layout_version = planner.layout_signature_with_layout(&source, &strategy, layout);
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
            Err(error) => fallback(format!("invalid configured partition layout: {error}")),
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
        if !equality_join_keys_are_simple(join.hash_keys(), join.probe_keys()) {
            return PartitionStrategy::Range;
        }
        if keys_reference_vid(join.hash_keys()) && keys_reference_vid(join.probe_keys())
        {
            return PartitionStrategy::Range;
        }
        let key = join
            .hash_keys()
            .first()
            .and_then(|k| k.expression())
            .and_then(|meta| match meta.inner() {
                linkrs_core::types::expr::Expression::Variable(name) => Some(name.clone()),
                _ => None,
            })
            .unwrap_or_else(|| DEFAULT_KEY.to_string());
        PartitionStrategy::Hash { key }
    }

    /// Whether every key expression references the vertex-id partition key.
    fn keys_reference_vid(
        keys: &[linkrs_core::types::expr::contextual::ContextualExpression],
    ) -> bool {
        !keys.is_empty()
            && keys.iter().all(|key| {
                key.expression().is_some_and(|meta| {
                    matches!(
                        meta.inner(),
                        linkrs_core::types::expr::Expression::Variable(name)
                            if name == "vid" || name.ends_with(".vid")
                    )
                })
            })
    }
