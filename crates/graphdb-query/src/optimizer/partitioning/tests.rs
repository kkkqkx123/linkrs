use super::*;
use crate::optimizer::stats::StatisticsManager;
use crate::optimizer::stats::StatsView;
use crate::optimizer::TagStatistics;
use crate::planning::plan::core::nodes::ScanVerticesNode;
use crate::planning::plan::{PartitionSource, PartitionSpec, PartitionStrategy, PlanNodeEnum};
use std::sync::Arc;

const TEST_SPACE: &str = "test";

fn tagged_scan() -> PlanNodeEnum {
    let mut scan = ScanVerticesNode::new(1, "space");
    scan.set_tag("person");
    PlanNodeEnum::ScanVertices(scan)
}

fn view_of(stats: &StatisticsManager) -> StatsView<'_> {
    StatsView::new(stats, Some(TEST_SPACE))
}

#[test]
fn selects_only_with_trusted_range_and_sufficient_statistics() {
    let stats = StatisticsManager::new();
    let mut tag = TagStatistics::new("person".to_string());
    tag.vertex_count = 10_000;
    stats.update_tag_stats(TEST_SPACE, tag);
    let planner = PartitioningPlanner::new(PartitioningConfig {
        enabled: true,
        min_rows_per_partition: 1_000,
        max_partitions: 4,
        vertex_id_range: Some(0i64..10_000),
        max_workers: 4,
        max_buffered_chunks: 10,
    });

    let decision = planner.decide(&tagged_scan(), &view_of(&stats));
    assert_eq!(
        decision
            .partition_spec
            .as_ref()
            .map(PartitionSpec::partition_count),
        Some(4)
    );
    let spec = decision.partition_spec.expect("partitioned spec");
    assert!(
        spec.layout_version().is_some(),
        "layout version must be populated from the config/domain signature"
    );
    let signature = planner.layout_signature(spec.source());
    assert_eq!(spec.layout_version(), Some(signature));
}

#[test]
fn layout_signature_changes_when_config_changes() {
    let stats = make_stats();
    let base = make_planner();
    let reranged = PartitioningPlanner::new(PartitioningConfig {
        vertex_id_range: Some(0i64..20_000),
        ..make_planner().config().clone()
    });
    let resized = PartitioningPlanner::new(PartitioningConfig {
        max_workers: 8,
        ..make_planner().config().clone()
    });
    let plan = tagged_scan();
    let base_sig = base
        .decide(&plan, &view_of(&stats))
        .partition_spec
        .map(|spec| spec.layout_version().unwrap());
    let base_again = base
        .decide(&plan, &view_of(&stats))
        .partition_spec
        .map(|spec| spec.layout_version().unwrap());
    let reranged_sig = reranged
        .decide(&plan, &view_of(&stats))
        .partition_spec
        .map(|spec| spec.layout_version().unwrap());
    let resized_sig = resized
        .decide(&plan, &view_of(&stats))
        .partition_spec
        .map(|spec| spec.layout_version().unwrap());
    let base = base_sig.expect("layout signature");
    assert_eq!(
        base_again.expect("layout signature"),
        base,
        "signature is deterministic"
    );
    assert_ne!(
        base,
        reranged_sig.expect("layout signature"),
        "vertex-id range change must alter the signature"
    );
    assert_ne!(
        base,
        resized_sig.expect("layout signature"),
        "worker count change must alter the signature"
    );
}

#[test]
fn falls_back_without_a_trusted_range() {
    let stats = StatisticsManager::new();
    let planner = PartitioningPlanner::new(PartitioningConfig {
        enabled: true,
        max_partitions: 4,
        ..PartitioningConfig::default()
    });

    let decision = planner.decide(&tagged_scan(), &view_of(&stats));
    assert!(decision.partition_spec.is_none());
    assert!(decision.reason.contains("vertex-id range"));
}

#[test]
fn storage_self_proven_domain_enables_partitioning_without_config_range() {
    // The storage self-proven domain (PartitioningLayoutInfo) must be
    // sufficient to enable partitioning even when the config carries no
    // trusted range.
    let stats = make_stats();
    let planner = PartitioningPlanner::new(PartitioningConfig {
        enabled: true,
        min_rows_per_partition: 1_000,
        max_partitions: 4,
        max_workers: 4,
        ..PartitioningConfig::default()
    });

    let layout = PartitioningLayoutInfo {
        layout_version: 42,
        vertex_id_range: Some(0i64..10_000),
    };
    let decision = planner.decide_with_layout(&tagged_scan(), &view_of(&stats), &layout);
    let spec = decision
        .partition_spec
        .expect("storage-proven domain must enable partitioning");
    assert_eq!(spec.partition_count(), 4);
}

#[test]
fn storage_self_proven_domain_overrides_config_range() {
    let stats = make_stats();
    let planner = PartitioningPlanner::new(PartitioningConfig {
        enabled: true,
        min_rows_per_partition: 1_000,
        max_partitions: 4,
        vertex_id_range: Some(0i64..10_000),
        max_workers: 4,
        max_buffered_chunks: 10,
    });

    // A narrower proven domain must be used (not the config range).
    let layout = PartitioningLayoutInfo {
        layout_version: 7,
        vertex_id_range: Some(100i64..200),
    };
    let decision = planner.decide_with_layout(&tagged_scan(), &view_of(&stats), &layout);
    let spec = decision.partition_spec.expect("partitioned spec");
    assert_eq!(spec.ranges().first().expect("first range").start, 100);
    assert_eq!(
        spec.ranges().last().expect("last range").end,
        200,
        "ranges must be derived from the storage-proven domain"
    );
}

#[test]
fn storage_layout_version_changes_the_signature() {
    // The plan-cache fingerprint must change when the storage's monotonic
    // layout version changes, even with identical config and domain.
    let stats = make_stats();
    let planner = make_planner();
    let plan = tagged_scan();

    let v1 = planner
        .decide_with_layout(
            &plan,
            &view_of(&stats),
            &PartitioningLayoutInfo {
                layout_version: 1,
                vertex_id_range: Some(0i64..10_000),
            },
        )
        .partition_spec
        .expect("partitioned")
        .layout_version();
    let v2 = planner
        .decide_with_layout(
            &plan,
            &view_of(&stats),
            &PartitioningLayoutInfo {
                layout_version: 2,
                vertex_id_range: Some(0i64..10_000),
            },
        )
        .partition_spec
        .expect("partitioned")
        .layout_version();
    assert_ne!(
        v1, v2,
        "a storage layout change must invalidate the cached partition layout"
    );
}

#[test]
fn missing_storage_domain_falls_back_without_config_range() {
    // No storage evidence and no configured range: partitioning must stay
    // disabled (safe default) with an observable reason.
    let stats = make_stats();
    let planner = PartitioningPlanner::new(PartitioningConfig {
        enabled: true,
        min_rows_per_partition: 1_000,
        max_partitions: 4,
        max_workers: 4,
        ..PartitioningConfig::default()
    });
    let decision = planner.decide_with_layout(
        &tagged_scan(),
        &view_of(&stats),
        &PartitioningLayoutInfo::default(),
    );
    assert!(decision.partition_spec.is_none());
    assert!(decision.reason.contains("vertex-id range"));
}

fn make_planner() -> PartitioningPlanner {
    PartitioningPlanner::new(PartitioningConfig {
        enabled: true,
        min_rows_per_partition: 1_000,
        max_partitions: 4,
        vertex_id_range: Some(0i64..10_000),
        max_workers: 4,
        max_buffered_chunks: 10,
    })
}

fn make_stats() -> StatisticsManager {
    let stats = StatisticsManager::new();
    let mut tag = TagStatistics::new("person".to_string());
    tag.vertex_count = 10_000;
    stats.update_tag_stats(TEST_SPACE, tag);
    stats
}

#[test]
fn falls_back_on_missing_statistics() {
    let stats = StatisticsManager::new(); // no stats populated
    let plan = tagged_scan();
    let decision = make_planner().decide(&plan, &view_of(&stats));
    assert!(decision.partition_spec.is_none());
    assert!(decision.reason.contains("missing statistics"));
}

#[test]
fn falls_back_on_transaction_boundary() {
    use crate::planning::plan::core::nodes::control_flow::control_flow_node::BeginTransactionNode;
    let plan = PlanNodeEnum::BeginTransaction(BeginTransactionNode::new(1));
    let stats = make_stats();
    let decision = make_planner().decide(&plan, &view_of(&stats));
    assert!(decision.partition_spec.is_none());
    assert!(decision.reason.contains("transaction boundary"));
}

#[test]
fn falls_back_on_graph_traversal() {
    use crate::planning::plan::core::nodes::traversal::traversal_node::AppendVerticesNode;
    let plan = PlanNodeEnum::AppendVertices(AppendVerticesNode::new(1, "person"));
    let stats = make_stats();
    let decision = make_planner().decide(&plan, &view_of(&stats));
    assert!(decision.partition_spec.is_none());
    assert!(decision.reason.contains("graph traversal"));
}

#[test]
fn multi_scan_union_selects_partition_layout() {
    use crate::planning::plan::core::nodes::graph_operations::graph_operations_node::UnionNode;

    let stats = StatisticsManager::new();
    let mut tag = TagStatistics::new("person".to_string());
    tag.vertex_count = 10_000;
    stats.update_tag_stats(TEST_SPACE, tag);
    let mut other = TagStatistics::new("company".to_string());
    other.vertex_count = 10_000;
    stats.update_tag_stats(TEST_SPACE, other);

    let mut scan_a = ScanVerticesNode::new(1, "space");
    scan_a.set_tag("person");
    let mut scan_b = ScanVerticesNode::new(2, "space");
    scan_b.set_tag("company");
    let union = UnionNode::new(
        PlanNodeEnum::ScanVertices(scan_a),
        PlanNodeEnum::ScanVertices(scan_b),
        false,
    )
    .expect("union plan should build");
    let plan = PlanNodeEnum::Union(union);

    let decision = make_planner().decide(&plan, &view_of(&stats));
    let spec = decision
        .partition_spec
        .as_ref()
        .expect("union of two large scans should partition");
    assert_eq!(spec.partition_count(), 4);
    assert!(
        matches!(spec.source(), PartitionSource::VertexId { tag } if tag == "person"),
        "representative source is the left scan tag"
    );
}

#[test]
fn multi_scan_union_falls_back_when_branch_is_not_a_scan_chain() {
    use crate::planning::plan::core::nodes::graph_operations::graph_operations_node::UnionNode;
    use crate::planning::plan::core::nodes::join::join_node::CrossJoinNode;

    let stats = make_stats();
    let mut scan_a = ScanVerticesNode::new(1, "space");
    scan_a.set_tag("person");
    let mut scan_b = ScanVerticesNode::new(2, "space");
    scan_b.set_tag("person");
    let mut scan_c = ScanVerticesNode::new(3, "space");
    scan_c.set_tag("company");
    let cross = CrossJoinNode::new(
        PlanNodeEnum::ScanVertices(scan_b),
        PlanNodeEnum::ScanVertices(scan_c),
    )
    .expect("cross join should build");
    let union = UnionNode::new(
        PlanNodeEnum::ScanVertices(scan_a),
        PlanNodeEnum::CrossJoin(cross),
        false,
    )
    .expect("union plan should build");
    let plan = PlanNodeEnum::Union(union);

    let decision = make_planner().decide(&plan, &view_of(&stats));
    assert!(decision.partition_spec.is_none());
    assert!(decision.reason.contains("linear chains"));
}

#[test]
fn equality_join_with_empty_keys_is_rejected_for_partitioning() {
    use crate::planning::plan::core::nodes::join::join_node::InnerJoinNode;

    let stats = make_stats();
    let mut scan = ScanVerticesNode::new(1, "space");
    scan.set_tag("person");
    let join = InnerJoinNode::new(
        PlanNodeEnum::ScanVertices(scan.clone()),
        PlanNodeEnum::ScanVertices(scan),
        Vec::new(),
        Vec::new(),
    )
    .expect("join plan should build");
    let plan = PlanNodeEnum::InnerJoin(join);

    let decision = make_planner().decide(&plan, &view_of(&stats));
    assert!(decision.partition_spec.is_none());
    assert!(decision.reason.contains("not a union/cross-join"));
}

#[test]
fn equality_join_with_variable_key_selects_partition_layout() {
    use crate::planning::plan::core::nodes::join::join_node::InnerJoinNode;
    use graphdb_core::types::expr::contextual::ContextualExpression;
    use graphdb_core::types::expr::ExpressionMeta;

    let stats = make_stats();
    let mut left_scan = ScanVerticesNode::new(1, "space");
    left_scan.set_tag("person");
    let mut right_scan = ScanVerticesNode::new(2, "space");
    right_scan.set_tag("person");

    // Create join keys using proper ExpressionAnalysisContext
    let expr_ctx = Arc::new(graphdb_core::types::expr::ExpressionAnalysisContext::new());
    let left_key_expr = graphdb_core::types::Expression::variable("a.vid");
    let left_key_id = expr_ctx.register_expression(ExpressionMeta::new(left_key_expr));
    let hash_key = ContextualExpression::new(left_key_id, expr_ctx.clone());

    let right_key_expr = graphdb_core::types::Expression::variable("b.vid");
    let right_key_id = expr_ctx.register_expression(ExpressionMeta::new(right_key_expr));
    let probe_key = ContextualExpression::new(right_key_id, expr_ctx.clone());

    let join = InnerJoinNode::new(
        PlanNodeEnum::ScanVertices(left_scan),
        PlanNodeEnum::ScanVertices(right_scan),
        vec![hash_key],
        vec![probe_key],
    )
    .expect("join plan should build");
    let plan = PlanNodeEnum::InnerJoin(join);

    let decision = make_planner().decide(&plan, &view_of(&stats));
    let spec = decision
        .partition_spec
        .as_ref()
        .expect("equality join on vertex-id should partition");
    assert!(spec.partition_count() >= 2);
    assert!(decision.reason.contains("equality join"));
}

#[test]
fn hash_inner_join_with_variable_key_selects_partition_layout() {
    use crate::planning::plan::core::nodes::join::join_node::InnerJoinNode;
    use graphdb_core::types::expr::contextual::ContextualExpression;
    use graphdb_core::types::expr::ExpressionMeta;

    // Real keyed-join queries lower to a InnerJoin node; the partition
    // decision must treat it the same as the plain InnerJoin variant.
    let stats = make_stats();
    let mut left_scan = ScanVerticesNode::new(1, "space");
    left_scan.set_tag("person");
    let mut right_scan = ScanVerticesNode::new(2, "space");
    right_scan.set_tag("person");

    let expr_ctx = Arc::new(graphdb_core::types::expr::ExpressionAnalysisContext::new());
    let left_key_expr = graphdb_core::types::Expression::variable("a.vid");
    let left_key_id = expr_ctx.register_expression(ExpressionMeta::new(left_key_expr));
    let hash_key = ContextualExpression::new(left_key_id, expr_ctx.clone());

    let right_key_expr = graphdb_core::types::Expression::variable("b.vid");
    let right_key_id = expr_ctx.register_expression(ExpressionMeta::new(right_key_expr));
    let probe_key = ContextualExpression::new(right_key_id, expr_ctx.clone());

    let join = InnerJoinNode::new(
        PlanNodeEnum::ScanVertices(left_scan),
        PlanNodeEnum::ScanVertices(right_scan),
        vec![hash_key],
        vec![probe_key],
    )
    .expect("join plan should build");
    let plan = PlanNodeEnum::InnerJoin(join);

    let decision = make_planner().decide(&plan, &view_of(&stats));
    let spec = decision
        .partition_spec
        .as_ref()
        .expect("hash equality join on vertex-id should partition");
    assert!(spec.partition_count() >= 2);
    assert!(decision.reason.contains("equality join"));
}

#[test]
fn hash_inner_join_with_composite_key_is_rejected() {
    use crate::planning::plan::core::nodes::join::join_node::InnerJoinNode;
    use graphdb_core::types::expr::contextual::ContextualExpression;
    use graphdb_core::types::expr::ExpressionMeta;

    let stats = make_stats();
    let mut left_scan = ScanVerticesNode::new(1, "space");
    left_scan.set_tag("person");
    let mut right_scan = ScanVerticesNode::new(2, "space");
    right_scan.set_tag("person");

    let expr_ctx = Arc::new(graphdb_core::types::expr::ExpressionAnalysisContext::new());
    let make_key = |name: &str| {
        let expr = graphdb_core::types::Expression::variable(name);
        let id = expr_ctx.register_expression(ExpressionMeta::new(expr));
        ContextualExpression::new(id, expr_ctx.clone())
    };
    // Two keys per side: the partitioned path only accepts a single simple
    // variable key and must fall back.
    let join = InnerJoinNode::new(
        PlanNodeEnum::ScanVertices(left_scan),
        PlanNodeEnum::ScanVertices(right_scan),
        vec![make_key("a.vid"), make_key("a.value")],
        vec![make_key("b.vid"), make_key("b.value")],
    )
    .expect("join plan should build");
    let plan = PlanNodeEnum::InnerJoin(join);

    let decision = make_planner().decide(&plan, &view_of(&stats));
    assert!(decision.partition_spec.is_none());
    assert!(decision.reason.contains("not a union/cross-join"));
}

#[test]
fn non_vid_join_key_selects_hash_partition_layout() {
    use crate::planning::plan::core::nodes::join::join_node::InnerJoinNode;
    use graphdb_core::types::expr::contextual::ContextualExpression;
    use graphdb_core::types::expr::ExpressionMeta;

    // Q4: a join on a property variable cannot map onto the vertex-id
    // domain, so the plan declares a hash distribution by that key.
    let stats = make_stats();
    let mut left_scan = ScanVerticesNode::new(1, "space");
    left_scan.set_tag("person");
    let mut right_scan = ScanVerticesNode::new(2, "space");
    right_scan.set_tag("person");

    let expr_ctx = Arc::new(graphdb_core::types::expr::ExpressionAnalysisContext::new());
    let make_key = |name: &str| {
        let expr = graphdb_core::types::Expression::variable(name);
        let id = expr_ctx.register_expression(ExpressionMeta::new(expr));
        ContextualExpression::new(id, expr_ctx.clone())
    };

    let join = InnerJoinNode::new(
        PlanNodeEnum::ScanVertices(left_scan),
        PlanNodeEnum::ScanVertices(right_scan),
        vec![make_key("a.name")],
        vec![make_key("b.name")],
    )
    .expect("join plan should build");
    let plan = PlanNodeEnum::InnerJoin(join);

    let decision = make_planner().decide(&plan, &view_of(&stats));
    let spec = decision
        .partition_spec
        .as_ref()
        .expect("non-vid equality join should hash-partition");
    assert_eq!(
        spec.strategy(),
        &PartitionStrategy::Hash {
            key: "a.name".to_string()
        },
        "hash strategy keyed by the join key variable"
    );
    assert!(spec.partition_count() >= 2);
    // The scan input stays sliced into disjoint ranges so no row is
    // duplicated before the hash exchange redistributes rows.
    assert_eq!(spec.ranges().len(), spec.partition_count());
    assert!(decision.reason.contains("hash-partitioned"));
}

#[test]
fn edge_scan_selects_partition_layout() {
    use crate::optimizer::stats::EdgeTypeStatistics;
    use crate::planning::plan::core::nodes::access::graph_scan_node::ScanEdgesNode;

    let stats = StatisticsManager::new();
    let mut edge = EdgeTypeStatistics::new("follows".to_string());
    edge.edge_count = 10_000;
    stats.update_edge_stats(TEST_SPACE, edge);

    let scan = ScanEdgesNode::new(1, "follows");
    let plan = PlanNodeEnum::ScanEdges(scan);

    let decision = make_planner().decide(&plan, &view_of(&stats));
    let spec = decision
        .partition_spec
        .as_ref()
        .expect("large edge scan should partition");
    assert_eq!(spec.partition_count(), 4);
    assert!(
        matches!(spec.source(), PartitionSource::EdgeId { edge_type } if edge_type == "follows")
    );
}

#[test]
fn edge_scan_with_traversal_above_rejected() {
    use crate::optimizer::stats::EdgeTypeStatistics;
    use crate::planning::plan::core::nodes::access::graph_scan_node::ScanEdgesNode;
    use crate::planning::plan::core::nodes::base::plan_node_traits::MultipleInputNode;
    use crate::planning::plan::core::nodes::traversal::traversal_node::ExpandNode;
    use graphdb_core::EdgeDirection;

    let stats = StatisticsManager::new();
    let mut edge = EdgeTypeStatistics::new("follows".to_string());
    edge.edge_count = 10_000;
    stats.update_edge_stats(TEST_SPACE, edge);

    // Expand above an edge scan needs vertex-side data.
    let scan = PlanNodeEnum::ScanEdges(ScanEdgesNode::new(1, "follows"));
    let mut expand = ExpandNode::new(1, vec!["follows".to_string()], EdgeDirection::Out);
    expand.add_input(scan);
    let plan = PlanNodeEnum::Expand(expand);

    let decision = make_planner().decide(&plan, &view_of(&stats));
    assert!(decision.partition_spec.is_none());
    assert!(
        decision.reason.contains("graph traversal") || decision.reason.contains("linear chain"),
        "expand plans must be rejected, got: {}",
        decision.reason
    );
}

#[test]
fn partition_count_is_capped_by_available_workers() {
    // E5 granularity: never cut more partitions than can run concurrently.
    // rows/min_rows = 10, but only 2 workers -> exactly 2 partitions.
    let stats = make_stats();
    let planner = PartitioningPlanner::new(PartitioningConfig {
        enabled: true,
        min_rows_per_partition: 1_000,
        max_partitions: 8,
        vertex_id_range: Some(0i64..10_000),
        max_workers: 2,
        max_buffered_chunks: 10,
    });

    let decision = planner.decide(&tagged_scan(), &view_of(&stats));
    let spec = decision
        .partition_spec
        .as_ref()
        .expect("large scan should partition");
    assert_eq!(spec.partition_count(), 2);
}

#[test]
fn partition_count_is_capped_by_max_partitions() {
    // rows/min_rows = 20, but max_partitions = 4 -> 4 partitions.
    let stats = make_stats();
    let planner = PartitioningPlanner::new(PartitioningConfig {
        enabled: true,
        min_rows_per_partition: 500,
        max_partitions: 4,
        vertex_id_range: Some(0i64..10_000),
        max_workers: 8,
        max_buffered_chunks: 10,
    });

    let decision = planner.decide(&tagged_scan(), &view_of(&stats));
    let spec = decision
        .partition_spec
        .as_ref()
        .expect("large scan should partition");
    assert_eq!(spec.partition_count(), 4);
}

#[test]
fn anchored_traversal_selects_partition_layout() {
    use crate::planning::plan::core::nodes::base::plan_node_traits::MultipleInputNode;
    use crate::planning::plan::core::nodes::traversal::traversal_node::ExpandAllNode;

    let stats = make_stats();
    let mut scan = ScanVerticesNode::new(1, "space");
    scan.set_tag("person");
    let mut expand = ExpandAllNode::new(1, vec!["follows".to_string()], "OUT");
    expand.set_step_limit(1);
    expand.set_id_only(true);
    expand.add_input(PlanNodeEnum::ScanVertices(scan));
    let plan = PlanNodeEnum::ExpandAll(expand);

    let decision = make_planner().decide(&plan, &view_of(&stats));
    let spec = decision
        .partition_spec
        .as_ref()
        .expect("anchored traversal should partition");
    assert_eq!(spec.partition_count(), 4);
    assert!(
        matches!(spec.source(), PartitionSource::VertexId { tag } if tag == "person"),
        "anchor scan tag is the partition source"
    );
}

#[test]
fn two_hop_traversal_is_rejected_without_annotation() {
    use crate::planning::plan::core::nodes::base::plan_node_traits::MultipleInputNode;
    use crate::planning::plan::core::nodes::traversal::traversal_node::ExpandAllNode;

    let stats = make_stats();
    let mut scan = ScanVerticesNode::new(1, "space");
    scan.set_tag("person");
    let mut hop1 = ExpandAllNode::new(1, vec!["follows".to_string()], "OUT");
    hop1.add_input(PlanNodeEnum::ScanVertices(scan));
    let mut hop2 = ExpandAllNode::new(2, vec!["follows".to_string()], "OUT");
    hop2.add_input(PlanNodeEnum::ExpandAll(hop1));
    let plan = PlanNodeEnum::ExpandAll(hop2);

    let decision = make_planner().decide(&plan, &view_of(&stats));
    assert!(decision.partition_spec.is_none());
    assert!(
        decision.reason.contains("de-materialized"),
        "unannotated two-hop traversals must be rejected, got: {}",
        decision.reason
    );
}

#[test]
fn annotated_two_hop_traversal_selects_partition_layout() {
    use crate::planning::plan::core::nodes::base::plan_node_traits::{MultipleInputNode, PlanNode};
    use crate::planning::plan::core::nodes::traversal::traversal_node::ExpandAllNode;

    // C1: a fully de-materialized (id_only / count_only), filter-free
    // two-hop chain is partitionable by the anchor vertex range.
    let stats = make_stats();
    let mut scan = ScanVerticesNode::new(1, "space");
    scan.set_tag("person");
    let mut hop1 = ExpandAllNode::new(1, vec!["follows".to_string()], "OUT");
    hop1.set_step_limit(1);
    hop1.set_id_only(true);
    hop1.set_col_names(vec!["a".to_string(), "e1".to_string(), "b".to_string()]);
    hop1.add_input(PlanNodeEnum::ScanVertices(scan));
    let mut hop2 = ExpandAllNode::new(2, vec!["follows".to_string()], "OUT");
    hop2.set_step_limit(1);
    hop2.set_count_only(true);
    hop2.set_col_names(vec!["b".to_string(), "e2".to_string(), "c".to_string()]);
    hop2.add_input(PlanNodeEnum::ExpandAll(hop1));
    let plan = PlanNodeEnum::ExpandAll(hop2);

    let decision = make_planner().decide(&plan, &view_of(&stats));
    let spec = decision
        .partition_spec
        .as_ref()
        .expect("annotated two-hop traversal should partition");
    assert_eq!(spec.partition_count(), 4);
    assert!(
        matches!(spec.source(), PartitionSource::VertexId { tag } if tag == "person"),
        "anchor scan tag is the partition source"
    );
}

#[test]
fn recursive_traversal_is_rejected() {
    use crate::planning::plan::core::nodes::base::plan_node_traits::MultipleInputNode;
    use crate::planning::plan::core::nodes::traversal::traversal_node::AppendVerticesNode;

    let stats = make_stats();
    let mut scan = ScanVerticesNode::new(1, "space");
    scan.set_tag("person");
    let mut append = AppendVerticesNode::new(1, "person");
    append.add_input(PlanNodeEnum::ScanVertices(scan));
    let plan = PlanNodeEnum::AppendVertices(append);

    let decision = make_planner().decide(&plan, &view_of(&stats));
    assert!(decision.partition_spec.is_none());
    assert!(
        decision.reason.contains("recursive graph traversal"),
        "vertex-property-fetch traversals must be rejected, got: {}",
        decision.reason
    );
}
