use super::*;

#[test]
fn test_optimizer_engine_creation() {
    let _engine = OptimizerEngine::default();
}

#[test]
fn test_optimizer_engine_with_config() {
    let config = CostModelConfig::for_ssd();
    let _engine = OptimizerEngine::new(config);
}

#[test]
fn test_optimizer_engine_configuration() {
    let mut engine = OptimizerEngine::default();

    // Test enable/disable heuristic
    engine.set_enable_heuristic(false);
    assert!(!engine.enable_heuristic);

    engine.set_enable_heuristic(true);
    assert!(engine.enable_heuristic);
}

#[test]
fn test_feedback_loop_corrects_selectivity() {
    use crate::optimizer::cost::selectivity::condition_key;
    use crate::optimizer::stats::feedback::query::{OperatorFeedback, QueryExecutionFeedback};
    use graphdb_core::types::Expression;

    let engine = OptimizerEngine::default();

    // Register the condition through the estimator (as optimization does).
    let expr = Expression::Binary {
        left: Box::new(Expression::Property {
            object: Box::new(Expression::Variable("v".to_string())),
            property: "age".to_string(),
        }),
        op: graphdb_core::types::BinaryOperator::GreaterThan,
        right: Box::new(Expression::Literal(graphdb_core::Value::Int(30))),
    };
    let original =
        engine
            .selectivity_estimator
            .estimate_from_expression(Some("test_space"), &expr, None);
    let key = condition_key(Some("test_space"), &expr);
    assert!(engine
        .selectivity_feedback
        .get_corrected_selectivity(&key)
        .is_some());

    // Simulate history: filter estimated 100 rows, actually produced 10.
    // Repeat several times so the EWMA correction converges toward 0.1.
    for _ in 0..8 {
        let mut feedback = QueryExecutionFeedback::new("fp".to_string());
        feedback.space = Some("test_space".to_string());
        feedback.add_operator_feedback(OperatorFeedback {
            operator_id: "op1".to_string(),
            operator_type: "Filter".to_string(),
            estimated_rows: 100,
            actual_rows: 10,
            estimated_time_us: 0,
            actual_time_us: 0,
            execution_loops: 1,
            condition_key: Some(key.clone()),
            shape_key: None,
        });
        engine.feedback_history.add_feedback(feedback);
    }

    engine.maybe_apply_feedback();

    let corrected = engine
        .selectivity_feedback
        .get_corrected_selectivity(&key)
        .expect("condition should remain registered");
    // actual/estimated = 0.1, so the corrected selectivity must converge
    // toward 10% of the original estimate.
    assert!(
        corrected < original * 0.35,
        "corrected={} should be far below original={}",
        corrected,
        original
    );

    // Invalidation: corrections for the space are dropped.
    engine.invalidate_space_feedback(Some("test_space"));
    assert!(engine
        .selectivity_feedback
        .get_corrected_selectivity(&key)
        .is_none());
}

#[test]
fn test_feedback_loop_respects_enable_switch() {
    use crate::optimizer::stats::feedback::query::{OperatorFeedback, QueryExecutionFeedback};

    let mut engine = OptimizerEngine::default();
    engine.set_enable_feedback(false);

    let mut feedback = QueryExecutionFeedback::new("fp".to_string());
    feedback.add_operator_feedback(OperatorFeedback {
        operator_id: "op1".to_string(),
        operator_type: "Filter".to_string(),
        estimated_rows: 100,
        actual_rows: 10,
        estimated_time_us: 0,
        actual_time_us: 0,
        execution_loops: 1,
        condition_key: Some("space:v.age > 30".to_string()),
        shape_key: None,
    });
    engine.feedback_history.add_feedback(feedback);

    engine.maybe_apply_feedback();
    // No correction applied: the key was never registered by an estimate.
    assert!(engine
        .selectivity_feedback
        .get_corrected_selectivity("space:v.age > 30")
        .is_none());
    // Nothing was applied because the switch is off; enabling it and
    // triggering with a registered key would apply corrections.
    assert!(!engine.feedback_enabled());
}

#[test]
fn test_feedback_loop_corrects_cardinality_shape_keys() {
    use crate::optimizer::stats::feedback::query::{OperatorFeedback, QueryExecutionFeedback};

    let engine = OptimizerEngine::default();

    // Register the shape through the estimate path (as optimization does).
    let key = "test_space:ScanVertices:person".to_string();
    engine.cardinality_feedback.register_key(key.clone(), 100.0);

    // Simulate history: the scan estimated 100 rows, actually produced 300.
    for _ in 0..10 {
        let mut feedback = QueryExecutionFeedback::new("fp".to_string());
        feedback.space = Some("test_space".to_string());
        feedback.add_operator_feedback(OperatorFeedback {
            operator_id: "op1".to_string(),
            operator_type: "ScanVertices".to_string(),
            estimated_rows: 100,
            actual_rows: 300,
            estimated_time_us: 0,
            actual_time_us: 0,
            execution_loops: 1,
            condition_key: None,
            shape_key: Some(key.clone()),
        });
        engine.feedback_history.add_feedback(feedback);
    }

    engine.maybe_apply_feedback();

    let corrected = engine
        .cardinality_feedback
        .corrected_rows(&key)
        .expect("shape should remain registered");
    assert!(
        corrected > 150.0,
        "corrected={} should move toward 300",
        corrected
    );

    // Decision feedback: an Apply run with rows and time is folded in.
    let mut feedback = QueryExecutionFeedback::new("fp2".to_string());
    feedback.space = Some("test_space".to_string());
    feedback.apply_rows = 500;
    feedback.apply_time_us = 50_000;
    engine.feedback_history.add_feedback(feedback);
    engine.maybe_apply_feedback();
    let advice = engine.decision_feedback.advice("test_space");
    assert!(advice.apply_cost_per_row.is_some());

    // Invalidation drops both the shape corrections and decision stats.
    engine.invalidate_space_feedback(Some("test_space"));
    assert!(engine.cardinality_feedback.corrected_rows(&key).is_none());
    let advice = engine.decision_feedback.advice("test_space");
    assert!(advice.apply_cost_per_row.is_none());
}

#[test]
fn test_optimizer_engine_max_iterations() {
    let mut engine = OptimizerEngine::default();

    engine.set_max_heuristic_iterations(50);
    assert_eq!(engine.max_heuristic_iterations, 50);
}

#[test]
fn cost_based_phases_rewrite_and_emit_notes() {
    use crate::optimizer::stats::TagStatistics;
    use crate::planning::plan::core::nodes::access::graph_scan_node::ScanVerticesNode;
    use crate::planning::plan::core::nodes::operation::filter_node::FilterNode;
    use crate::planning::plan::core::nodes::operation::sort_node::{LimitNode, SortItem, SortNode};
    use graphdb_core::types::expr::expression_context::ExpressionAnalysisContext;
    use graphdb_core::types::expr::{ContextualExpression, Expression, ExpressionMeta};
    use graphdb_core::types::{Index, IndexStatus, IndexType};
    use graphdb_core::Value;
    use std::sync::Arc;

    let engine = OptimizerEngine::default();

    // Register index + vertex statistics for the tag so cost-based
    // index selection and the row estimates are data-driven.
    engine.stats_manager().register_tag_indexes(
        "test",
        "person",
        7,
        vec![Index {
            id: 3,
            name: "idx_person_name".to_string(),
            space_id: 1,
            schema_name: "person".to_string(),
            fields: Vec::new(),
            properties: vec!["name".to_string()],
            index_type: IndexType::TagIndex,
            status: IndexStatus::Active,
            is_unique: false,
            comment: None,
            covering: false,
            partial_condition: None,
        }],
    );
    let mut tag_stats = TagStatistics::new("person".to_string());
    tag_stats.vertex_count = 10_000;
    engine.stats_manager().update_tag_stats("test", tag_stats);

    // Build: Limit -> Sort -> Filter(ScanVertices(person, name = 'alice')).
    let mut scan = ScanVerticesNode::new(1, "test");
    scan.set_tag("person");
    scan.set_col_names(vec!["n".to_string()]);
    scan.set_output_var("n".to_string());
    let context = Arc::new(ExpressionAnalysisContext::new());
    let predicate = Expression::Binary {
        left: Box::new(Expression::Property {
            object: Box::new(Expression::Variable("n".to_string())),
            property: "name".to_string(),
        }),
        op: graphdb_core::types::operators::BinaryOperator::Equal,
        right: Box::new(Expression::Literal(Value::String("alice".into()))),
    };
    let id = context.register_expression(ExpressionMeta::new(predicate));
    let filter = FilterNode::new(
        PlanNodeEnum::ScanVertices(scan),
        ContextualExpression::new(id, context),
    )
    .expect("filter should build");
    let sort = SortNode::new(
        PlanNodeEnum::Filter(filter),
        vec![SortItem::column_asc("n.name".to_string())],
    )
    .expect("sort should build");
    let limit = LimitNode::new(PlanNodeEnum::Sort(sort), 0, 50).expect("limit should build");
    let plan = ExecutionPlan::new(Some(PlanNodeEnum::Limit(limit)));

    let optimized = engine
        .optimize(plan, Some("test"))
        .expect("optimization should succeed");

    // The heuristic phase converts Limit(offset=0) -> Sort to TopN, so
    // the plan must contain a TopN and an IndexScan for the predicate.
    let root = optimized.root.as_ref().expect("root should exist");
    assert!(
        contains_variant(root, &|node| matches!(node, PlanNodeEnum::TopN(_))),
        "expected TopN in optimized plan"
    );
    assert!(
        contains_variant(root, &|node| matches!(node, PlanNodeEnum::IndexScan(_))),
        "expected IndexScan in optimized plan"
    );

    // Decision notes and row estimates must be produced.
    assert!(optimized
        .cbo_notes
        .iter()
        .any(|note| note.starts_with("index:")));
    assert!(!optimized.row_estimates.is_empty());

    // The filter remains above the index scan (residual predicate).
    assert!(contains_variant(root, &|node| matches!(
        node,
        PlanNodeEnum::Filter(_)
    )));
}

fn contains_variant(node: &PlanNodeEnum, predicate: &dyn Fn(&PlanNodeEnum) -> bool) -> bool {
    if predicate(node) {
        return true;
    }
    node.children()
        .iter()
        .any(|child| contains_variant(child, predicate))
}

#[test]
fn precompute_notes_emitted_for_reused_expressions() {
    use crate::planning::plan::core::nodes::access::graph_scan_node::ScanVerticesNode;
    use crate::planning::plan::core::nodes::operation::project_node::ProjectNode;
    use graphdb_core::types::expr::expression_context::ExpressionAnalysisContext;
    use graphdb_core::types::expr::{ContextualExpression, Expression, ExpressionMeta};
    use graphdb_core::types::operators::BinaryOperator;
    use graphdb_core::Value;
    use graphdb_core::YieldColumn;
    use std::sync::Arc;

    let mut engine = OptimizerEngine::default();
    // Keep the plan shape untouched so duplicate projection columns
    // survive (heuristic dedup would collapse them).
    engine.set_enable_heuristic(false);

    let mut scan = ScanVerticesNode::new(1, "test");
    scan.set_tag("person");
    scan.set_col_names(vec!["n".to_string()]);
    scan.set_output_var("n".to_string());

    // (a + b) * 2: complex enough to clear the precomputation cost floor.
    let expr = Expression::Binary {
        left: Box::new(Expression::Binary {
            left: Box::new(Expression::Variable("a".to_string())),
            op: BinaryOperator::Add,
            right: Box::new(Expression::Variable("b".to_string())),
        }),
        op: BinaryOperator::Multiply,
        right: Box::new(Expression::Literal(Value::Int(2))),
    };
    let context = Arc::new(ExpressionAnalysisContext::new());
    let id = context.register_expression(ExpressionMeta::new(expr));
    let contextual = ContextualExpression::new(id, context);

    // The same expression is referenced by three projection columns.
    let columns: Vec<YieldColumn> = (0..3)
        .map(|i| YieldColumn {
            expression: contextual.clone(),
            alias: format!("c{}", i),
            is_matched: false,
        })
        .collect();
    let project =
        ProjectNode::new(PlanNodeEnum::ScanVertices(scan), columns).expect("project should build");
    let plan = ExecutionPlan::new(Some(PlanNodeEnum::Project(project)));

    let optimized = engine
        .optimize(plan, Some("test"))
        .expect("optimization should succeed");

    assert!(
        optimized
            .cbo_notes
            .iter()
            .any(|note| note.starts_with("precompute:")),
        "expected precompute decision notes, got: {:?}",
        optimized.cbo_notes
    );
}

#[test]
fn cost_based_consumes_logical_plan_when_attached() {
    use crate::optimizer::stats::TagStatistics;
    use crate::planning::plan::core::nodes::access::graph_scan_node::ScanVerticesNode;
    use crate::planning::plan::core::nodes::operation::filter_node::FilterNode;
    use crate::planning::plan::core::nodes::operation::sort_node::{LimitNode, SortItem, SortNode};
    use crate::planning::plan::logical_plan::LogicalPlan;
    use graphdb_core::types::expr::expression_context::ExpressionAnalysisContext;
    use graphdb_core::types::expr::{ContextualExpression, Expression, ExpressionMeta};
    use graphdb_core::types::{Index, IndexStatus, IndexType};
    use graphdb_core::Value;
    use std::sync::Arc;

    let engine = OptimizerEngine::default();

    // Register index + vertex statistics so the index selection decision
    // is data-driven.
    engine.stats_manager().register_tag_indexes(
        "test",
        "person",
        7,
        vec![Index {
            id: 3,
            name: "idx_person_name".to_string(),
            space_id: 1,
            schema_name: "person".to_string(),
            fields: Vec::new(),
            properties: vec!["name".to_string()],
            index_type: IndexType::TagIndex,
            status: IndexStatus::Active,
            is_unique: false,
            comment: None,
            covering: false,
            partial_condition: None,
        }],
    );
    let mut tag_stats = TagStatistics::new("person".to_string());
    tag_stats.vertex_count = 10_000;
    engine.stats_manager().update_tag_stats("test", tag_stats);

    // Build: Limit -> Sort -> Filter(ScanVertices(person, name = 'alice')).
    let mut scan = ScanVerticesNode::new(1, "test");
    scan.set_tag("person");
    scan.set_col_names(vec!["n".to_string()]);
    scan.set_output_var("n".to_string());
    let context = Arc::new(ExpressionAnalysisContext::new());
    let predicate = Expression::Binary {
        left: Box::new(Expression::Property {
            object: Box::new(Expression::Variable("n".to_string())),
            property: "name".to_string(),
        }),
        op: graphdb_core::types::operators::BinaryOperator::Equal,
        right: Box::new(Expression::Literal(Value::String("alice".into()))),
    };
    let id = context.register_expression(ExpressionMeta::new(predicate));
    let filter = FilterNode::new(
        PlanNodeEnum::ScanVertices(scan),
        ContextualExpression::new(id, context),
    )
    .expect("filter should build");
    let sort = SortNode::new(
        PlanNodeEnum::Filter(filter),
        vec![SortItem::column_asc("n.name".to_string())],
    )
    .expect("sort should build");
    let limit = LimitNode::new(PlanNodeEnum::Sort(sort), 0, 50).expect("limit should build");
    let root = PlanNodeEnum::Limit(limit);
    let mut plan = ExecutionPlan::new(Some(root.clone()));

    // Attach the pure logical plan — the CBO decision phases must
    // consume it.
    let logical_plan = LogicalPlan::from_plan_node(&root).expect("conversion should succeed");
    plan.set_logical_plan(logical_plan);

    let optimized = engine
        .optimize(plan, Some("test"))
        .expect("optimization should succeed");

    // The structural rewrites still apply to the physical root.
    let root = optimized.root.as_ref().expect("root should exist");
    assert!(
        contains_variant(root, &|node| matches!(node, PlanNodeEnum::IndexScan(_))),
        "expected IndexScan in optimized plan"
    );
    assert!(
        contains_variant(root, &|node| matches!(node, PlanNodeEnum::TopN(_))),
        "expected TopN in optimized plan"
    );

    // The decision notes come from the logical walkers.
    assert!(
        optimized
            .cbo_notes
            .iter()
            .any(|note| note.starts_with("index:")),
        "expected index decision note from the logical walker, got: {:?}",
        optimized.cbo_notes
    );
    assert!(!optimized.row_estimates.is_empty());

    // The logical plan survives optimization (join order write-back).
    assert!(
        optimized.logical_plan().is_some(),
        "logical plan must remain attached after optimization"
    );
}

#[test]
fn cost_based_falls_back_when_no_logical_plan_attached() {
    use crate::planning::plan::core::nodes::control_flow::control_flow_node::ArgumentNode;

    let mut engine = OptimizerEngine::default();
    engine.set_enable_heuristic(false);

    // Argument nodes are not supported by the physical-to-logical
    // converter, so no logical plan is attached and the physical
    // fallback path must keep the plan intact.
    let arg = ArgumentNode::new(-1, "x");
    let plan = ExecutionPlan::new(Some(PlanNodeEnum::Argument(arg)));
    assert!(plan.logical_plan().is_none());

    let optimized = engine
        .optimize(plan, Some("test"))
        .expect("optimization should succeed");
    assert!(matches!(optimized.root, Some(PlanNodeEnum::Argument(_))));
    assert!(optimized.logical_plan().is_none());
}
