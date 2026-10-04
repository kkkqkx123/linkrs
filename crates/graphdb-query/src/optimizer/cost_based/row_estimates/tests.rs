use super::*;
use crate::optimizer::cost::config::UNKNOWN_SCAN_ROWS;

use crate::optimizer::stats::StatisticsManager;
use crate::planning::plan::core::nodes::access::graph_scan_node::ScanVerticesNode;
use crate::planning::plan::core::nodes::operation::filter_node::FilterNode;
use crate::planning::plan::core::nodes::operation::sort_node::{
    LimitNode, SortItem, SortNode, TopNNode,
};
use graphdb_core::types::expr::expression_context::ExpressionAnalysisContext;
use graphdb_core::types::expr::{ContextualExpression, Expression, ExpressionMeta};
use graphdb_core::value::Value;
use std::sync::Arc;

fn setup() -> (Arc<StatisticsManager>, SelectivityEstimator) {
    let manager = Arc::new(StatisticsManager::new());
    let selectivity = SelectivityEstimator::new(manager.clone());
    (manager, selectivity)
}

#[test]
fn scan_without_stats_falls_back_to_constant() {
    let (manager, selectivity) = setup();
    let view = StatsView::new(&manager, Some("test"));
    let scan = PlanNodeEnum::ScanVertices(ScanVerticesNode::new(1, "test"));
    assert_eq!(
        estimate_node_output_rows(&scan, &view, &selectivity),
        UNKNOWN_SCAN_ROWS
    );
}

#[test]
fn filter_uses_expression_selectivity() {
    let (manager, selectivity) = setup();
    let view = StatsView::new(&manager, Some("test"));
    let mut scan = ScanVerticesNode::new(1, "test");
    scan.set_tag("person");
    let context = Arc::new(ExpressionAnalysisContext::new());
    let id =
        context.register_expression(ExpressionMeta::new(Expression::Literal(Value::Bool(true))));
    let condition = ContextualExpression::new(id, context);
    let filter =
        FilterNode::new(PlanNodeEnum::ScanVertices(scan), condition).expect("filter should build");
    let estimate = estimate_node_output_rows(&PlanNodeEnum::Filter(filter), &view, &selectivity);
    assert!(estimate >= 1);
    assert!(estimate <= UNKNOWN_SCAN_ROWS);
}

#[test]
fn limit_caps_input_rows() {
    let (manager, selectivity) = setup();
    let view = StatsView::new(&manager, Some("test"));
    let scan = PlanNodeEnum::ScanVertices(ScanVerticesNode::new(1, "test"));
    let limit = LimitNode::new(scan, 5, 10).expect("limit should build");
    let estimate = estimate_node_output_rows(&PlanNodeEnum::Limit(limit), &view, &selectivity);
    assert_eq!(estimate, 15);
}

#[test]
fn topn_caps_input_rows() {
    let (manager, selectivity) = setup();
    let view = StatsView::new(&manager, Some("test"));
    let mut tag_stats = crate::optimizer::stats::TagStatistics::new("person".to_string());
    tag_stats.vertex_count = 100;
    manager.update_tag_stats("test", tag_stats);
    let mut scan = ScanVerticesNode::new(1, "test");
    scan.set_tag("person");
    let sort = SortNode::new(
        PlanNodeEnum::ScanVertices(scan),
        vec![SortItem::column_asc("x".to_string())],
    )
    .expect("sort should build");
    let topn = TopNNode::new(
        PlanNodeEnum::Sort(sort),
        vec![SortItem::column_asc("x".to_string())],
        7,
    )
    .expect("topn should build");
    let estimate = estimate_node_output_rows(&PlanNodeEnum::TopN(topn), &view, &selectivity);
    assert_eq!(estimate, 7);
}

#[test]
fn collect_returns_estimates_for_every_node() {
    let (manager, selectivity) = setup();
    let view = StatsView::new(&manager, Some("test"));
    // Direct constructors leave the placeholder id (-1); number the nodes
    // explicitly like planner output, since estimates are keyed by node id.
    let start = crate::planning::plan::core::nodes::control_flow::start_node::StartNode::new()
        .clone_with_new_id(1);
    let sort = SortNode::new(start, vec![SortItem::column_asc("x".to_string())])
        .expect("sort should build")
        .clone_with_new_id(2);
    let plan = LimitNode::new(sort, 0, 5)
        .expect("limit should build")
        .clone_with_new_id(3);
    let estimates = collect_node_row_estimates(&plan, &view, &selectivity);
    assert_eq!(estimates.len(), 3);
    assert!(estimates.contains_key(&plan.id()));
}

#[test]
fn logical_scan_without_stats_falls_back_to_constant() {
    use crate::planning::plan::logical::logical_nodes::access::LogicalScanVerticesNode;
    use crate::planning::plan::logical::LogicalNodeEnum;

    let (manager, selectivity) = setup();
    let view = StatsView::new(&manager, Some("test"));
    let scan = LogicalNodeEnum::ScanVertices(LogicalScanVerticesNode {
        id: 1,
        space_id: 1,
        space_name: "test".to_string(),
        tag: None,
        expression: None,
        limit: None,
        projected_properties: vec![],
        index_hint: None,
        estimated_cardinality: None,
        output_var: None,
        col_names: vec![],
        column_types: vec![],
    });
    assert_eq!(
        estimate_node_output_rows_logical(&scan, &view, &selectivity),
        UNKNOWN_SCAN_ROWS
    );
}

#[test]
fn logical_aggregate_uses_group_key_selectivity() {
    use crate::planning::plan::logical::logical_nodes::access::LogicalScanVerticesNode;
    use crate::planning::plan::logical::logical_nodes::operation::LogicalAggregateNode;
    use crate::planning::plan::logical::LogicalNodeEnum;

    let (manager, selectivity) = setup();
    let view = StatsView::new(&manager, Some("test"));
    let mut tag_stats = crate::optimizer::stats::TagStatistics::new("person".to_string());
    tag_stats.vertex_count = 1_000;
    manager.update_tag_stats("test", tag_stats);
    let scan = LogicalNodeEnum::ScanVertices(LogicalScanVerticesNode {
        id: 1,
        space_id: 1,
        space_name: "test".to_string(),
        tag: Some("person".to_string()),
        expression: None,
        limit: None,
        projected_properties: vec![],
        index_hint: None,
        estimated_cardinality: None,
        output_var: None,
        col_names: vec![],
        column_types: vec![],
    });
    let ctx = std::sync::Arc::new(
        graphdb_core::types::expr::expression_context::ExpressionAnalysisContext::new(),
    );
    let expr = graphdb_core::Expression::Variable("n.age".to_string());
    let meta = graphdb_core::types::expr::ExpressionMeta::new(expr);
    let id = ctx.register_expression(meta);
    let ctx_expr = graphdb_core::types::expr::contextual::ContextualExpression::new(id, ctx);
    let aggregate = LogicalNodeEnum::Aggregate(LogicalAggregateNode {
        id: 2,
        input: Some(Box::new(scan)),
        group_key_exprs: vec![ctx_expr],
        aggregation_functions: vec![],
        aggregation_args: vec![],
        aggregation_distinct: vec![],
        aggregation_filters: vec![],
        grouping_sets: vec![],
        output_var: None,
        col_names: vec![],
        column_types: vec![],
    });
    let estimate = estimate_node_output_rows_logical(&aggregate, &view, &selectivity);
    // Single group-count source: estimate_group_count(1000, 1) = 500.
    assert_eq!(estimate, 500);
}

#[test]
fn corrected_variant_applies_learned_shape_factor() {
    use crate::optimizer::stats::feedback::cardinality::CardinalityFeedbackManager;

    let (manager, selectivity) = setup();
    let view = StatsView::new(&manager, Some("test"));
    let mut tag_stats = crate::optimizer::stats::TagStatistics::new("person".to_string());
    tag_stats.vertex_count = 100;
    manager.update_tag_stats("test", tag_stats);
    let mut scan = ScanVerticesNode::new(1, "test");
    scan.set_tag("person");
    let scan_node = PlanNodeEnum::ScanVertices(scan);

    let raw = estimate_node_output_rows(&scan_node, &view, &selectivity);
    assert_eq!(raw, 100);

    // Learn that the scan actually returns 3x the estimate.
    let cardinality = CardinalityFeedbackManager::new();
    cardinality.register_key("test:ScanVertices:person".to_string(), raw as f64);
    for _ in 0..50 {
        cardinality.update_feedback_ratio("test:ScanVertices:person", 3.0);
    }

    let corrected =
        estimate_node_output_rows_corrected(&scan_node, &view, &selectivity, &cardinality);
    assert!(
        corrected > 200 && corrected <= 1000,
        "corrected={} should move toward 300",
        corrected
    );

    // The raw variant is unaffected (plan writeback keeps raw estimates).
    assert_eq!(
        estimate_node_output_rows(&scan_node, &view, &selectivity),
        100
    );
}

#[test]
fn corrected_variant_propagates_through_pass_through_nodes() {
    use crate::optimizer::stats::feedback::cardinality::CardinalityFeedbackManager;
    use crate::planning::plan::core::nodes::operation::sort_node::LimitNode;

    let (manager, selectivity) = setup();
    let view = StatsView::new(&manager, Some("test"));
    let mut tag_stats = crate::optimizer::stats::TagStatistics::new("person".to_string());
    tag_stats.vertex_count = 100;
    manager.update_tag_stats("test", tag_stats);
    let mut scan = ScanVerticesNode::new(1, "test");
    scan.set_tag("person");
    let scan_node = PlanNodeEnum::ScanVertices(scan);
    let limit = LimitNode::new(scan_node, 0, 1000).expect("limit should build");
    let limit_node = PlanNodeEnum::Limit(limit);

    let cardinality = CardinalityFeedbackManager::new();
    cardinality.register_key("test:ScanVertices:person".to_string(), 100.0);
    for _ in 0..50 {
        cardinality.update_feedback_ratio("test:ScanVertices:person", 2.0);
    }

    // The Limit (pass-through) estimate inherits the corrected child rows.
    let corrected =
        estimate_node_output_rows_corrected(&limit_node, &view, &selectivity, &cardinality);
    assert!(
        corrected > 150,
        "corrected={} should inherit the factor",
        corrected
    );
}

#[test]
fn logical_expand_applies_stats_fanout_like_physical() {
    use crate::planning::plan::logical::logical_nodes::access::LogicalScanVerticesNode;
    use crate::planning::plan::logical::logical_nodes::traversal::LogicalExpandNode;
    use crate::planning::plan::logical::LogicalNodeEnum;
    use graphdb_core::types::EdgeDirection;

    let (manager, selectivity) = setup();
    let mut tag_stats = crate::optimizer::stats::TagStatistics::new("person".to_string());
    tag_stats.vertex_count = 100;
    manager.update_tag_stats("test", tag_stats);
    let mut edge_stats = crate::optimizer::stats::EdgeTypeStatistics::new("knows".to_string());
    edge_stats.avg_out_degree = 5.0;
    manager.update_edge_stats("test", edge_stats);
    let view = StatsView::new(&manager, Some("test"));

    let scan = LogicalNodeEnum::ScanVertices(LogicalScanVerticesNode {
        id: 1,
        space_id: 1,
        space_name: "test".to_string(),
        tag: Some("person".to_string()),
        expression: None,
        limit: None,
        projected_properties: vec![],
        index_hint: None,
        estimated_cardinality: None,
        output_var: None,
        col_names: vec![],
        column_types: vec![],
    });
    let expand = LogicalNodeEnum::Expand(LogicalExpandNode {
        id: 2,
        deps: vec![scan],
        space_id: 1,
        edge_types: vec!["knows".to_string()],
        direction: EdgeDirection::Out,
        step_limit: None,
        filter: None,
        dst_tag: None,
        output_var: None,
        col_names: vec![],
        column_types: vec![],
    });
    assert_eq!(
        estimate_node_output_rows_logical(&expand, &view, &selectivity),
        500
    );
}

#[test]
fn logical_wco_intersect_is_bounded_by_stats() {
    use crate::planning::plan::logical::logical_nodes::access::LogicalScanVerticesNode;
    use crate::planning::plan::logical::logical_nodes::wco_intersect::LogicalWcoIntersectNode;
    use crate::planning::plan::logical::LogicalNodeEnum;

    let (manager, selectivity) = setup();
    let mut tag_stats = crate::optimizer::stats::TagStatistics::new("person".to_string());
    tag_stats.vertex_count = 10_000;
    manager.update_tag_stats("test", tag_stats);
    let view = StatsView::new(&manager, Some("test"));

    let leaf = |id: i64, tag: &str| {
        LogicalNodeEnum::ScanVertices(LogicalScanVerticesNode {
            id,
            space_id: 1,
            space_name: "test".to_string(),
            tag: Some(tag.to_string()),
            expression: None,
            limit: None,
            projected_properties: vec![],
            index_hint: None,
            estimated_cardinality: None,
            output_var: None,
            col_names: vec![],
            column_types: vec![],
        })
    };
    let ctx = Arc::new(ExpressionAnalysisContext::new());
    let key = |name: &str| {
        let id =
            ctx.register_expression(ExpressionMeta::new(Expression::Variable(name.to_string())));
        ContextualExpression::new(id, ctx.clone())
    };
    let node = LogicalNodeEnum::WcoIntersect(LogicalWcoIntersectNode::new(
        leaf(1, "person"),
        vec![leaf(2, "person"), leaf(3, "person")],
        key("c"),
        vec![key("a"), key("b")],
        vec![],
    ));
    let estimate = estimate_node_output_rows_logical(&node, &view, &selectivity);
    // probe = 10000; conservative = 2000; must not exceed the builds.
    assert!(estimate >= 1);
    assert!(estimate <= 10_000);
    assert!(estimate <= 2_000);
}
