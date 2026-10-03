use super::super::flatten::logical_output_var;
use crate::optimizer::cost::CostCalculator;
use crate::optimizer::stats::StatisticsManager;
use crate::planning::plan::core::nodes::join::join_node::InnerJoinNode;

fn make_scan(id: &str, _rows: u64) -> PlanNodeEnum {
    // Use a StartNode as a stand-in leaf for testing
    let mut node =
        crate::planning::plan::core::nodes::control_flow::start_node::StartNode::new();
    node.set_output_var(id.to_string());
    node.set_col_names(vec![id.to_string()]);
    PlanNodeEnum::Start(node)
}

fn make_hash_join(
    left: PlanNodeEnum,
    right: PlanNodeEnum,
    hk: Vec<&str>,
    pk: Vec<&str>,
) -> PlanNodeEnum {
    let ctx = std::sync::Arc::new(
        graphdb_core::types::expr::expression_context::ExpressionAnalysisContext::new(),
    );
    let hash_keys: Vec<ContextualExpression> = hk
        .iter()
        .map(|s| {
            let meta = graphdb_core::types::expr::ExpressionMeta::new(
                graphdb_core::types::expr::Expression::Variable(s.to_string()),
            );
            let id = ctx.register_expression(meta);
            graphdb_core::types::expr::contextual::ContextualExpression::new(id, ctx.clone())
        })
        .collect();
    let probe_keys: Vec<ContextualExpression> = pk
        .iter()
        .map(|s| {
            let meta = graphdb_core::types::expr::ExpressionMeta::new(
                graphdb_core::types::expr::Expression::Variable(s.to_string()),
            );
            let id = ctx.register_expression(meta);
            graphdb_core::types::expr::contextual::ContextualExpression::new(id, ctx.clone())
        })
        .collect();
    PlanNodeEnum::InnerJoin(InnerJoinNode::new(left, right, hash_keys, probe_keys).unwrap())
}

#[test]
fn test_single_table_returns_none() {
    let scan = make_scan("a", 1000);
    assert!(flatten_join_chain(&scan).is_none());
}

#[test]
fn test_two_table_chain() {
    let a = make_scan("a", 1000);
    let b = make_scan("b", 500);
    let join = make_hash_join(a, b, vec!["a.id"], vec!["b.id"]);
    let chain = flatten_join_chain(&join).expect("should flatten");
    assert_eq!(chain.leaves.len(), 2);
    assert_eq!(chain.predicates.len(), 1);
}

#[test]
fn test_three_table_chain() {
    let a = make_scan("a", 1000);
    let b = make_scan("b", 500);
    let c = make_scan("c", 2000);
    let join1 = make_hash_join(a, b, vec!["a.id"], vec!["b.id"]);
    let join2 = make_hash_join(join1, c, vec!["b.id"], vec!["c.id"]);
    let chain = flatten_join_chain(&join2).expect("should flatten");
    assert_eq!(chain.leaves.len(), 3);
    assert_eq!(chain.predicates.len(), 2);
}

#[test]
fn test_non_join_unchanged() {
    let a = make_scan("a", 1000);
    let stats = StatisticsManager::new();
    let cost_calc = CostCalculator::new(std::sync::Arc::new(stats.clone()));
    let stats_view = StatsView::new(&stats, None);
    let mut notes = Vec::new();
    let result = walk_and_optimize_joins_with_decisions(
        &a,
        &stats_view,
        &cost_calc,
        &mut notes,
        &mut None,
    );
    // StartNode is preserved (same variant)
    assert!(matches!(result, PlanNodeEnum::Start(_)));
    // Output var is preserved
    assert_eq!(result.output_var(), a.output_var());
}

#[test]
fn test_optimize_three_table() {
    let a = make_scan("a", 1000);
    let b = make_scan("b", 10);
    let c = make_scan("c", 2000);
    let join1 = make_hash_join(a, b, vec!["a.id"], vec!["b.id"]);
    let join2 = make_hash_join(join1, c, vec!["b.id"], vec!["c.id"]);

    let stats = StatisticsManager::new();
    let cost_calc = CostCalculator::new(std::sync::Arc::new(stats.clone()));
    let stats_view = StatsView::new(&stats, None);
    let mut notes = Vec::new();
    let optimized = walk_and_optimize_joins_with_decisions(
        &join2,
        &stats_view,
        &cost_calc,
        &mut notes,
        &mut None,
    );

    // The smallest table (b, 10 rows) should be first in the new tree
    assert!(matches!(optimized, PlanNodeEnum::InnerJoin(_)));
    // Verify it's still a valid join tree
    assert!(optimized.children().len() >= 2);
}

#[test]
fn test_keyed_chain_records_join_decision() {
    use crate::optimizer::stats::TagStatistics;
    use crate::planning::plan::core::nodes::access::graph_scan_node::ScanVerticesNode;

    // Informative leaves: the small table sorts first, so the reviewer
    // strictly improves the order and records the algorithm decision.
    let stats = StatisticsManager::new();
    let tag_scan = |tag: &str, rows: u64| {
        let mut tag_stats = TagStatistics::new(tag.to_string());
        tag_stats.vertex_count = rows;
        stats.update_tag_stats("test", tag_stats);
        let mut scan = ScanVerticesNode::new(1, "test");
        scan.set_tag(tag);
        PlanNodeEnum::ScanVertices(scan)
    };
    let a = tag_scan("a", 1000);
    let b = tag_scan("b", 10);
    let join = make_hash_join(a, b, vec!["scan_a.id"], vec!["scan_b.id"]);

    let cost_calc = CostCalculator::new(std::sync::Arc::new(stats.clone()));
    let stats_view = StatsView::new(&stats, Some("test"));
    let mut notes = Vec::new();
    let mut decisions = HashMap::new();
    let optimized = walk_and_optimize_joins_with_decisions(
        &join,
        &stats_view,
        &cost_calc,
        &mut notes,
        &mut Some(&mut decisions),
    );

    // The reordered tree is a join and the rebuild records exactly one
    // decision, keyed by the rebuilt root node id.
    assert!(matches!(optimized, PlanNodeEnum::InnerJoin(_)));
    assert_eq!(decisions.len(), 1);
    let algorithm = decisions
        .get(&optimized.id())
        .expect("decision for the rebuilt join node");
    assert!(
        matches!(
            algorithm,
            JoinAlgorithm::HashJoin { .. } | JoinAlgorithm::NestedLoopJoin { .. }
        ),
        "expected an executable join algorithm, got {:?}",
        algorithm
    );
}

#[test]
fn test_already_optimal_order_is_kept_without_notes() {
    use crate::optimizer::stats::TagStatistics;
    use crate::planning::plan::core::nodes::access::graph_scan_node::ScanVerticesNode;

    fn tag_scan(stats: &StatisticsManager, space: &str, tag: &str, rows: u64) -> PlanNodeEnum {
        let mut tag_stats = TagStatistics::new(tag.to_string());
        tag_stats.vertex_count = rows;
        stats.update_tag_stats(space, tag_stats);
        let mut scan = ScanVerticesNode::new(1, space);
        scan.set_tag(tag);
        PlanNodeEnum::ScanVertices(scan)
    }

    // Small table first already: the proposal matches the chain, so the
    // reviewer must not rebuild (pure churn) and must stay silent.
    let stats = StatisticsManager::new();
    let small = tag_scan(&stats, "test", "small", 10);
    let big = tag_scan(&stats, "test", "big", 1000);
    let join = make_hash_join(small, big, vec!["scan_small.id"], vec!["scan_big.id"]);

    let cost_calc = CostCalculator::new(std::sync::Arc::new(stats.clone()));
    let stats_view = StatsView::new(&stats, Some("test"));
    let mut notes = Vec::new();
    let mut decisions = HashMap::new();
    let result = walk_and_optimize_joins_with_decisions(
        &join,
        &stats_view,
        &cost_calc,
        &mut notes,
        &mut Some(&mut decisions),
    );
    assert!(matches!(result, PlanNodeEnum::InnerJoin(_)));
    assert!(notes.is_empty(), "no-op review must stay silent: {notes:?}");
    assert!(decisions.is_empty());
    // Leaf order preserved: small table still leftmost.
    match &result {
        PlanNodeEnum::InnerJoin(n) => match n.left_input() {
            PlanNodeEnum::ScanVertices(s) => {
                assert_eq!(s.tag().map(String::as_str), Some("small"))
            }
            other => panic!("expected left ScanVertices leaf, got {other:?}"),
        },
        other => panic!("expected InnerJoin, got {other:?}"),
    }
}

#[test]
fn test_tied_costs_keep_current_order() {
    // Symmetric cross join: both orders cost exactly the same, so the
    // reviewer keeps the current shape instead of flapping.
    let a = make_scan("a", 100);
    let b = make_scan("b", 100);
    let join = make_hash_join(a, b, vec![], vec![]);

    let stats = StatisticsManager::new();
    let cost_calc = CostCalculator::new(std::sync::Arc::new(stats.clone()));
    let stats_view = StatsView::new(&stats, None);
    let mut notes = Vec::new();
    let optimized = walk_and_optimize_joins_with_decisions(
        &join,
        &stats_view,
        &cost_calc,
        &mut notes,
        &mut None,
    );
    assert!(matches!(optimized, PlanNodeEnum::InnerJoin(_)));
    assert!(notes.is_empty(), "tied review must stay silent: {notes:?}");
}

#[test]
fn test_min_improvement_threshold_blocks_marginal_rewrite() {
    use crate::optimizer::cost::config::CostModelConfig;
    use crate::optimizer::stats::TagStatistics;
    use crate::planning::plan::core::nodes::access::graph_scan_node::ScanVerticesNode;

    fn tag_scan(stats: &StatisticsManager, space: &str, tag: &str, rows: u64) -> PlanNodeEnum {
        let mut tag_stats = TagStatistics::new(tag.to_string());
        tag_stats.vertex_count = rows;
        stats.update_tag_stats(space, tag_stats);
        let mut scan = ScanVerticesNode::new(1, space);
        scan.set_tag(tag);
        PlanNodeEnum::ScanVertices(scan)
    }

    let build = |stats: &StatisticsManager| {
        let a = tag_scan(stats, "test", "a", 1000);
        let b = tag_scan(stats, "test", "b", 10);
        let c = tag_scan(stats, "test", "c", 2000);
        let join1 = make_hash_join(a, b, vec!["scan_a.id"], vec!["scan_b.id"]);
        make_hash_join(join1, c, vec!["scan_b.id"], vec!["scan_c.id"])
    };

    let stats = StatisticsManager::new();
    let stats_view = StatsView::new(&stats, Some("test"));

    // Default threshold (zero): any strict improvement rewrites.
    let cost_calc = CostCalculator::new(std::sync::Arc::new(stats.clone()));
    let mut notes = Vec::new();
    let rewritten = walk_and_optimize_joins_with_decisions(
        &build(&stats),
        &stats_view,
        &cost_calc,
        &mut notes,
        &mut None,
    );
    assert_eq!(notes.len(), 1, "expected one rewrite note: {notes:?}");
    assert!(
        notes[0].contains("cost "),
        "rewrite note carries the cost ratio: {}",
        notes[0]
    );
    let _ = rewritten;

    // A demanding threshold blocks the same marginal rewrite.
    let mut config = CostModelConfig::default();
    config.strategy_thresholds.join_reorder_min_improvement = 0.5;
    let cost_calc = CostCalculator::with_config(std::sync::Arc::new(stats.clone()), config);
    let mut notes = Vec::new();
    let kept = walk_and_optimize_joins_with_decisions(
        &build(&stats),
        &stats_view,
        &cost_calc,
        &mut notes,
        &mut None,
    );
    assert!(
        notes.is_empty(),
        "blocked review must stay silent: {notes:?}"
    );
    assert!(matches!(kept, PlanNodeEnum::InnerJoin(_)));
}

#[test]
fn test_unchanged_tree_records_no_decisions() {
    let a = make_scan("a", 1000);
    let stats = StatisticsManager::new();
    let cost_calc = CostCalculator::new(std::sync::Arc::new(stats.clone()));
    let stats_view = StatsView::new(&stats, None);
    let mut notes = Vec::new();
    let mut decisions = HashMap::new();
    let result = walk_and_optimize_joins_with_decisions(
        &a,
        &stats_view,
        &cost_calc,
        &mut notes,
        &mut Some(&mut decisions),
    );
    assert!(matches!(result, PlanNodeEnum::Start(_)));
    assert!(decisions.is_empty());
}

#[test]
fn test_cross_join_never_records_hash_decision() {
    // A join without keyed predicates has no hash keys: the HashJoin
    // decision must never be recorded for it even when the cost model
    // selects the hash algorithm.
    let a = make_scan("a", 1000);
    let b = make_scan("b", 10);
    let join = make_hash_join(a, b, vec![], vec![]);

    let stats = StatisticsManager::new();
    let cost_calc = CostCalculator::new(std::sync::Arc::new(stats.clone()));
    let stats_view = StatsView::new(&stats, None);
    let mut notes = Vec::new();
    let mut decisions = HashMap::new();
    let optimized = walk_and_optimize_joins_with_decisions(
        &join,
        &stats_view,
        &cost_calc,
        &mut notes,
        &mut Some(&mut decisions),
    );
    assert!(matches!(optimized, PlanNodeEnum::InnerJoin(_)));
    assert!(
        !decisions
            .values()
            .any(|a| matches!(a, JoinAlgorithm::HashJoin { .. })),
        "keyless join must not record a HashJoin decision"
    );
}

#[test]
fn test_left_join_acts_as_boundary() {
    let a = make_scan("a", 1000);
    let b = make_scan("b", 500);
    let inner = make_hash_join(a.clone(), b, vec!["a.id"], vec!["b.id"]);
    // LeftJoin with an inner join on the left and a scan on the right
    let c = make_scan("c", 100);
    let _ctx = std::sync::Arc::new(
        graphdb_core::types::expr::expression_context::ExpressionAnalysisContext::new(),
    );
    use crate::planning::plan::core::nodes::join::join_node::LeftJoinNode;
    let left_join =
        PlanNodeEnum::LeftJoin(LeftJoinNode::new(inner, c, vec![], vec![]).unwrap());

    let stats = StatisticsManager::new();
    let cost_calc = CostCalculator::new(std::sync::Arc::new(stats.clone()));
    let stats_view = StatsView::new(&stats, None);
    let mut notes = Vec::new();
    let result = walk_and_optimize_joins_with_decisions(
        &left_join,
        &stats_view,
        &cost_calc,
        &mut notes,
        &mut None,
    );

    // The root should still be a LeftJoin
    assert!(matches!(result, PlanNodeEnum::LeftJoin(_)));
}

#[test]
fn test_large_join_uses_greedy() {
    let mut tables = Vec::new();
    for i in 0..12 {
        tables.push(make_scan(&format!("t{}", i), (i as u64 + 1) * 100));
    }
    // Build a left-deep join chain
    let mut join = make_hash_join(tables[0].clone(), tables[1].clone(), vec![], vec![]);
    for table in tables.iter().skip(2) {
        join = make_hash_join(join, table.clone(), vec![], vec![]);
    }

    let stats = StatisticsManager::new();
    let cost_calc = CostCalculator::new(std::sync::Arc::new(stats.clone()));
    let stats_view = StatsView::new(&stats, None);
    let mut notes = Vec::new();
    let result = walk_and_optimize_joins_with_decisions(
        &join,
        &stats_view,
        &cost_calc,
        &mut notes,
        &mut None,
    );

    // Should complete without panic (greedy path)
    assert!(matches!(result, PlanNodeEnum::InnerJoin(_)));
}

// ===================================================================
// Logical-plan walker tests
// ===================================================================

use crate::planning::plan::logical::logical_nodes::access::LogicalStartNode;
use crate::planning::plan::logical::logical_nodes::join::LogicalInnerJoinNode;

fn make_logical_scan(id: &str) -> LogicalNodeEnum {
    let mut node = LogicalStartNode::new();
    node.set_output_var(id.to_string());
    node.set_col_names(vec![id.to_string()]);
    LogicalNodeEnum::Start(node)
}

fn make_logical_hash_join(
    left: LogicalNodeEnum,
    right: LogicalNodeEnum,
    hk: Vec<&str>,
    pk: Vec<&str>,
) -> LogicalNodeEnum {
    let ctx = std::sync::Arc::new(
        graphdb_core::types::expr::expression_context::ExpressionAnalysisContext::new(),
    );
    let hash_keys: Vec<ContextualExpression> = hk
        .iter()
        .map(|s| {
            let meta = graphdb_core::types::expr::ExpressionMeta::new(
                graphdb_core::types::expr::Expression::Variable(s.to_string()),
            );
            let id = ctx.register_expression(meta);
            graphdb_core::types::expr::contextual::ContextualExpression::new(id, ctx.clone())
        })
        .collect();
    let probe_keys: Vec<ContextualExpression> = pk
        .iter()
        .map(|s| {
            let meta = graphdb_core::types::expr::ExpressionMeta::new(
                graphdb_core::types::expr::Expression::Variable(s.to_string()),
            );
            let id = ctx.register_expression(meta);
            graphdb_core::types::expr::contextual::ContextualExpression::new(id, ctx.clone())
        })
        .collect();
    LogicalNodeEnum::InnerJoin(LogicalInnerJoinNode {
        id: crate::planning::plan::core::node_id_generator::next_node_id(),
        left: Box::new(left),
        right: Box::new(right),
        hash_keys,
        probe_keys,
        recommended_algorithm: None,
        output_var: None,
        col_names: vec![],
        column_types: vec![],
    })
}

#[test]
fn test_logical_single_table_returns_none() {
    let scan = make_logical_scan("a");
    assert!(flatten_join_chain_logical(&scan).is_none());
}

#[test]
fn test_logical_two_table_chain() {
    let a = make_logical_scan("a");
    let b = make_logical_scan("b");
    let join = make_logical_hash_join(a, b, vec!["a.id"], vec!["b.id"]);
    let chain = flatten_join_chain_logical(&join).expect("should flatten");
    assert_eq!(chain.leaves.len(), 2);
    assert_eq!(chain.predicates.len(), 1);
}

#[test]
fn test_logical_three_table_reorder_emits_note() {
    use crate::optimizer::stats::TagStatistics;
    use crate::planning::plan::core::node_id_generator::next_node_id;
    use crate::planning::plan::logical::logical_nodes::access::LogicalScanVerticesNode;

    // Informative leaves: row counts differ, so the reviewer strictly
    // improves the order and emits its decision note.
    let stats = StatisticsManager::new();
    let tag_scan = |tag: &str, rows: u64| {
        let mut tag_stats = TagStatistics::new(tag.to_string());
        tag_stats.vertex_count = rows;
        stats.update_tag_stats("test", tag_stats);
        LogicalNodeEnum::ScanVertices(LogicalScanVerticesNode {
            id: next_node_id(),
            space_id: 1,
            space_name: "test".to_string(),
            tag: Some(tag.to_string()),
            expression: None,
            limit: None,
            projected_properties: Vec::new(),
            index_hint: None,
            estimated_cardinality: None,
            output_var: None,
            col_names: Vec::new(),
            column_types: Vec::new(),
        })
    };
    let a = tag_scan("a", 1000);
    let b = tag_scan("b", 10);
    let c = tag_scan("c", 2000);
    let join1 = make_logical_hash_join(a, b, vec!["scan_a.id"], vec!["scan_b.id"]);
    let join2 = make_logical_hash_join(join1, c, vec!["scan_b.id"], vec!["scan_c.id"]);

    let cost_calc = CostCalculator::new(std::sync::Arc::new(stats.clone()));
    let stats_view = StatsView::new(&stats, Some("test"));
    let mut notes = Vec::new();
    let optimized =
        walk_and_optimize_joins_logical(&join2, &stats_view, &cost_calc, &mut notes);

    // The reordered logical tree is a logical InnerJoin (no physical
    // InnerJoin can appear in the logical tree).
    assert!(matches!(optimized, LogicalNodeEnum::InnerJoin(_)));
    assert_eq!(notes.len(), 1);
    assert!(notes[0].starts_with("join_order:"));
    assert!(notes[0].contains("order=["));
}

#[test]
fn test_logical_non_join_unchanged() {
    let a = make_logical_scan("a");
    let stats = StatisticsManager::new();
    let cost_calc = CostCalculator::new(std::sync::Arc::new(stats.clone()));
    let stats_view = StatsView::new(&stats, None);
    let mut notes = Vec::new();
    let result = walk_and_optimize_joins_logical(&a, &stats_view, &cost_calc, &mut notes);
    assert!(matches!(result, LogicalNodeEnum::Start(_)));
    assert_eq!(logical_output_var(&result), logical_output_var(&a));
    assert!(notes.is_empty());
}

#[test]
fn test_logical_left_join_acts_as_boundary() {
    let a = make_logical_scan("a");
    let b = make_logical_scan("b");
    let inner = make_logical_hash_join(a, b, vec!["a.id"], vec!["b.id"]);
    let c = make_logical_scan("c");
    let left_join = LogicalNodeEnum::LeftJoin(
        crate::planning::plan::logical::logical_nodes::join::LogicalLeftJoinNode {
            id: crate::planning::plan::core::node_id_generator::next_node_id(),
            left: Box::new(inner),
            right: Box::new(c),
            hash_keys: vec![],
            probe_keys: vec![],
            output_var: None,
            col_names: vec![],
            column_types: vec![],
        },
    );

    let stats = StatisticsManager::new();
    let cost_calc = CostCalculator::new(std::sync::Arc::new(stats.clone()));
    let stats_view = StatsView::new(&stats, None);
    let mut notes = Vec::new();
    let result =
        walk_and_optimize_joins_logical(&left_join, &stats_view, &cost_calc, &mut notes);

    // The root stays a LeftJoin; the inner join below it may be reordered.
    assert!(matches!(result, LogicalNodeEnum::LeftJoin(_)));
}
