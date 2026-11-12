use super::*;
use crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
use crate::planning::ExecutionPlan;

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
fn test_engine_cost_profile_reaches_calculator() {
    // The server resolves the storage medium to a cost preset at startup;
    // the engine must hand that exact preset to the calculator the
    // cost-based rewriters consume.
    let engine = OptimizerEngine::new(CostModelConfig::for_hdd());
    assert_eq!(engine.cost_calculator().config().random_page_cost, 4.0);
    let engine = OptimizerEngine::new(CostModelConfig::for_in_memory());
    assert_eq!(engine.cost_calculator().config().random_page_cost, 0.1);
    let engine = OptimizerEngine::new(CostModelConfig::for_ssd());
    assert_eq!(engine.cost_calculator().config().random_page_cost, 1.1);
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
    use linkrs_core::types::Expression;

    let engine = OptimizerEngine::default();

    // Register the condition through the estimator (as optimization does).
    let expr = Expression::Binary {
        left: Box::new(Expression::Property {
            object: Box::new(Expression::Variable("v".to_string())),
            property: "age".to_string(),
        }),
        op: linkrs_core::types::BinaryOperator::GreaterThan,
        right: Box::new(Expression::Literal(linkrs_core::Value::Int(30))),
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

    // Simulate history: filter estimated 1000 rows over a measured 3000
    // input rows, actually produced 100 (observed selectivity is a tenth
    // of the estimate). Repeat several times so the measured correction
    // converges toward 0.1.
    for _ in 0..8 {
        let mut feedback = QueryExecutionFeedback::new("fp".to_string());
        feedback.space = Some("test_space".to_string());
        feedback.add_operator_feedback(OperatorFeedback {
            operator_id: "op1".to_string(),
            operator_type: "Filter".to_string(),
            estimated_rows: 1000,
            actual_rows: 100,
            actual_input_rows: Some(3000),
            partition_count: 1,
            exhausted: true,
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
        actual_input_rows: None,
        partition_count: 1,
        exhausted: true,
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
            actual_input_rows: None,
            partition_count: 1,
            exhausted: true,
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
    use linkrs_core::types::expr::expression_context::ExpressionAnalysisContext;
    use linkrs_core::types::expr::{ContextualExpression, Expression, ExpressionMeta};
    use linkrs_core::types::{Index, IndexStatus, IndexType};
    use linkrs_core::Value;
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
        op: linkrs_core::types::operators::BinaryOperator::Equal,
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

    // The cost path is the sole Sort-to-TopN decision maker. The Sort+Limit
    // fusion is now cost-based (the previous `min_limit` floor that suppressed
    // TopN for limit < 100 has been removed), so a TopN may appear when it is
    // cheaper than a full sort. This test focuses on the index-selection
    // rewrite rather than the TopN outcome.
    let root = optimized.root.as_ref().expect("root should exist");
    assert!(
        contains_variant(root, &|node| matches!(node, PlanNodeEnum::IndexScan(_))),
        "expected IndexScan in optimized plan"
    );

    // Decision notes and row estimates must be produced.
    assert!(optimized
        .diagnostics
        .notes
        .iter()
        .any(|note| note.starts_with("index:")));

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

mod factorization_fallback_tests {
    use super::*;
    use crate::planning::plan::core::nodes::access::graph_scan_node::GetEdgesNode;
    use crate::planning::plan::core::nodes::control_flow::start_node::StartNode;
    use crate::planning::plan::core::nodes::join::wco_intersect_node::WcoIntersectNode;
    use crate::planning::plan::logical::logical_nodes::access::LogicalScanVerticesNode;
    use crate::planning::plan::logical::logical_nodes::operation::LogicalLimitNode;
    use crate::planning::plan::logical::logical_nodes::wco_intersect::LogicalWcoIntersectNode;
    use crate::planning::plan::logical::LogicalNodeEnum;
    use crate::planning::plan::logical_plan::LogicalPlan;
    use linkrs_core::types::expr::expression_context::ExpressionAnalysisContext;
    use linkrs_core::types::expr::{ContextualExpression, Expression, ExpressionMeta};

    fn var_key(ctx: &Arc<ExpressionAnalysisContext>, name: &str) -> ContextualExpression {
        let id =
            ctx.register_expression(ExpressionMeta::new(Expression::Variable(name.to_string())));
        ContextualExpression::new(id, Arc::clone(ctx))
    }

    fn scan_logical(var: &str) -> LogicalNodeEnum {
        LogicalNodeEnum::ScanVertices(LogicalScanVerticesNode {
            id: crate::planning::plan::core::node_id_generator::next_node_id(),
            space_id: 1,
            space_name: "test".to_string(),
            tag: None,
            expression: None,
            limit: None,
            projected_properties: vec![],
            index_hint: None,
            estimated_cardinality: None,
            output_var: Some(var.to_string()),
            col_names: vec![var.to_string()],
            column_types: vec![],
        })
    }

    fn limit_logical(input: LogicalNodeEnum, count: i64) -> LogicalNodeEnum {
        LogicalNodeEnum::Limit(LogicalLimitNode {
            id: crate::planning::plan::core::node_id_generator::next_node_id(),
            input: Some(Box::new(input)),
            offset: 0,
            count,
            output_var: None,
            col_names: vec![],
            column_types: vec![],
        })
    }

    fn wco_logical(
        ctx: &Arc<ExpressionAnalysisContext>,
        probe: LogicalNodeEnum,
        builds: Vec<LogicalNodeEnum>,
    ) -> LogicalNodeEnum {
        LogicalNodeEnum::WcoIntersect(LogicalWcoIntersectNode::new(
            probe,
            builds,
            var_key(ctx, "c"),
            vec![var_key(ctx, "a")],
            vec!["a".to_string(), "c".to_string()],
        ))
    }

    fn physical_wco() -> PlanNodeEnum {
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let probe = PlanNodeEnum::Start(StartNode::new());
        let build = PlanNodeEnum::Start(StartNode::new());
        PlanNodeEnum::WcoIntersect(
            WcoIntersectNode::new(
                probe,
                vec![build],
                var_key(&ctx, "c"),
                vec![var_key(&ctx, "a")],
            )
            .expect("wco node should build"),
        )
    }

    #[test]
    fn reverse_conversion_covers_wco_intersect() {
        let physical = physical_wco();
        let logical = crate::planning::plan::logical::conversion::convert_plan(&physical)
            .expect("WcoIntersect should reverse-convert");
        let LogicalNodeEnum::WcoIntersect(wco) = &logical else {
            panic!("expected logical WcoIntersect, got {}", logical.type_name());
        };
        assert_eq!(wco.num_builds(), 1);
        assert_eq!(wco.intersect_key().as_variable().as_deref(), Some("c"));
        assert_eq!(wco.col_names(), &["c".to_string()]);
    }

    #[test]
    fn reverse_conversion_covers_get_edges() {
        let physical = PlanNodeEnum::GetEdges(GetEdgesNode::new(1, "src", "edge", "rank", "dst"));
        let logical = crate::planning::plan::logical::conversion::convert_plan(&physical)
            .expect("GetEdges should reverse-convert");
        let LogicalNodeEnum::GetEdges(get) = &logical else {
            panic!("expected logical GetEdges, got {}", logical.type_name());
        };
        assert_eq!(get.space_id, 1);
        assert_eq!(get.src, "src");
        assert_eq!(get.edge_type, "edge");
        assert_eq!(get.rank, "rank");
        assert_eq!(get.dst, "dst");
    }

    #[test]
    fn physical_only_wco_plan_gains_logical_without_fallback_notes() {
        let engine = OptimizerEngine::default();
        let plan = ExecutionPlan::new(Some(physical_wco()));
        assert!(plan.logical_plan().is_none());
        let optimized = engine
            .optimize(plan, None)
            .expect("optimization should succeed");
        assert!(
            optimized.logical_plan().is_some(),
            "WcoIntersect plan must carry a logical plan after bridging"
        );
        assert!(
            !optimized
                .diagnostics
                .notes
                .iter()
                .any(|n| n.contains("logical_plan fallback failed")),
            "no fallback note expected, got: {:?}",
            optimized.diagnostics.notes
        );
    }

    #[test]
    fn unsupported_node_records_counted_fallback_notes() {
        use crate::planning::plan::core::nodes::management::manage_node_enums::SpaceManageNode;
        use crate::planning::plan::core::nodes::management::ShowSpacesNode;

        let mut engine = OptimizerEngine::default();
        engine.set_enable_heuristic(false);
        let plan = ExecutionPlan::new(Some(PlanNodeEnum::SpaceManage(SpaceManageNode::Show(
            ShowSpacesNode::new(1),
        ))));
        let optimized = engine
            .optimize(plan, Some("test"))
            .expect("optimization should succeed");
        assert!(optimized.logical_plan().is_none());
        assert!(
            optimized
                .diagnostics
                .notes
                .iter()
                .any(|n| n.starts_with("factorization: logical_plan_fallback_total=1 (node=")),
            "expected node-labelled fallback note, got: {:?}",
            optimized.diagnostics.notes
        );
    }

    #[test]
    fn rewrite_replaces_wco_with_join_when_hash_cheaper() {
        let engine = OptimizerEngine::default();
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        // Limit(0) drives the probe estimate to zero rows, where the hash
        // join cost model wins over the intersect cost model.
        let probe = limit_logical(scan_logical("a"), 0);
        let root = wco_logical(&ctx, probe, vec![scan_logical("e1")]);
        let mut plan = ExecutionPlan::new(None);
        plan.set_logical_plan(LogicalPlan::new(root));
        let optimized = engine.apply_intersect_to_join_rewrite(plan, None);
        let logical = optimized
            .logical_plan()
            .expect("logical plan must survive the rewrite");
        assert!(
            matches!(logical.root, LogicalNodeEnum::InnerJoin(_)),
            "expected InnerJoin after rewrite, got {}",
            logical.root.type_name()
        );
        assert!(
            optimized
                .diagnostics
                .notes
                .iter()
                .any(|n| n.contains("WcoIntersect fallback to HashJoin")),
            "expected fallback note, got: {:?}",
            optimized.diagnostics.notes
        );
        assert!(
            optimized
                .diagnostics
                .notes
                .iter()
                .any(|n| n == "factorization: intersect_to_join_rewrite_total=1"),
            "expected rewrite total note, got: {:?}",
            optimized.diagnostics.notes
        );
        assert!(
            OptimizerEngine::validate_factorized_invariant(&logical.root),
            "rewritten tree must hold at most one unflat group"
        );
    }

    #[test]
    fn rewrite_keeps_wco_when_intersect_cheaper() {
        let engine = OptimizerEngine::default();
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let root = wco_logical(&ctx, scan_logical("a"), vec![scan_logical("e1")]);
        let mut plan = ExecutionPlan::new(None);
        plan.set_logical_plan(LogicalPlan::new(root));
        let optimized = engine.apply_intersect_to_join_rewrite(plan, None);
        let logical = optimized.logical_plan().expect("logical plan must survive");
        assert!(
            matches!(logical.root, LogicalNodeEnum::WcoIntersect(_)),
            "expected WcoIntersect to be kept, got {}",
            logical.root.type_name()
        );
        assert!(
            !optimized
                .diagnostics
                .notes
                .iter()
                .any(|n| n.contains("intersect_to_join_rewrite_total")),
            "no rewrite total expected, got: {:?}",
            optimized.diagnostics.notes
        );
    }

    #[test]
    fn metrics_stats_collects_flatten_and_fallback_counts() {
        use crate::planning::plan::logical::logical_nodes::flatten::LogicalFlattenNode;

        let metrics = Arc::new(linkrs_metrics::StatsManager::new());
        let mut engine = OptimizerEngine::default();
        engine.set_metrics_stats(Arc::clone(&metrics));

        // Flatten path: a logical Flatten node reports its position count.
        let mut flatten_plan = ExecutionPlan::new(None);
        flatten_plan.set_logical_plan(LogicalPlan::new(LogicalNodeEnum::Flatten(
            LogicalFlattenNode::new(0, scan_logical("a")),
        )));
        let out = engine.apply_factorization(flatten_plan).unwrap();
        assert!(
            out.diagnostics
                .notes
                .iter()
                .any(|n| n == "factorization: flatten_total=1"),
            "expected flatten total note, got: {:?}",
            out.diagnostics.notes
        );
        assert_eq!(
            metrics.get_value(linkrs_metrics::MetricType::FactorizationFlattenTotal),
            Some(1)
        );

        // Fallback path: an unsupported physical-only plan increments the
        // fallback counter through the full optimize pipeline.
        use crate::planning::plan::core::nodes::management::manage_node_enums::SpaceManageNode;
        use crate::planning::plan::core::nodes::management::ShowSpacesNode;
        let plan = ExecutionPlan::new(Some(PlanNodeEnum::SpaceManage(SpaceManageNode::Show(
            ShowSpacesNode::new(1),
        ))));
        engine
            .optimize(plan, Some("test"))
            .expect("optimization should succeed");
        assert_eq!(
            metrics.get_value(linkrs_metrics::MetricType::FactorizationFallbackTotal),
            Some(1)
        );
    }

    #[test]
    fn builder_with_metrics_stats_wires_sink() {
        use crate::optimizer::builder::OptimizerEngineBuilder;

        let metrics = Arc::new(linkrs_metrics::StatsManager::new());
        let engine = OptimizerEngineBuilder::new()
            .with_metrics_stats(Arc::clone(&metrics))
            .build();
        assert!(engine.metrics_stats().is_some());
        assert!(OptimizerEngine::default().metrics_stats().is_none());
    }

    #[test]
    fn combiner_propagates_logical_joins() {
        use crate::planning::plan::SubPlan;
        use crate::planning::statements::plan_combiner;

        let left = SubPlan::from_logical_root(scan_logical("a"));
        let right = SubPlan::from_logical_root(scan_logical("b"));
        let joined = plan_combiner::cross_join_plans(left, right).expect("join should build");
        assert!(matches!(joined.root, Some(PlanNodeEnum::CrossJoin(_))));
        assert!(
            matches!(joined.logical_root, Some(LogicalNodeEnum::CrossJoin(_))),
            "combiner must propagate the logical join"
        );
    }

    fn all_paths_logical() -> LogicalNodeEnum {
        use crate::planning::plan::core::node_id_generator::next_node_id;
        use crate::planning::plan::logical::logical_nodes::access::LogicalStartNode;
        use crate::planning::plan::logical::logical_nodes::algorithm::LogicalAllPathsNode;

        let left = LogicalNodeEnum::Start(LogicalStartNode::new());
        let right = LogicalNodeEnum::Start(LogicalStartNode::new());
        LogicalNodeEnum::AllPaths(LogicalAllPathsNode {
            id: next_node_id(),
            left: Box::new(left),
            right: Box::new(right),
            space_id: 1,
            steps: 3,
            edge_types: vec!["knows".to_string()],
            min_hop: 1,
            max_hop: 3,
            path_uniqueness:
                crate::planning::plan::core::nodes::traversal::path_algorithms::PathUniqueness::VertexUnique,
            direction: linkrs_core::EdgeDirection::Out,
            has_step_limit: true,
            limit: -1,
            offset: 0,
            filter: None,
            start_vertex_ids: vec![],
            end_vertex_ids: vec![],
            output_var: None,
            col_names: vec!["path".to_string()],
            column_types: vec![],
        })
    }

    #[test]
    fn native_all_paths_plan_optimizes_without_fallback() {
        let mut engine = OptimizerEngine::default();
        engine.set_enable_heuristic(false);
        let logical = all_paths_logical();
        let physical =
            crate::planning::physical_planner::convert_logical_to_physical(logical.clone());
        assert!(
            matches!(physical, PlanNodeEnum::AllPaths(_)),
            "forward conversion must preserve the algorithm node"
        );
        let mut plan = ExecutionPlan::new(Some(physical));
        plan.set_logical_plan(LogicalPlan::new(logical));
        let optimized = engine
            .optimize(plan, None)
            .expect("optimization should succeed");
        assert!(
            matches!(optimized.root, Some(PlanNodeEnum::AllPaths(_))),
            "algorithm root must survive physical mapping"
        );
        assert!(
            optimized.logical_plan().is_some(),
            "native logical plan must survive optimization"
        );
        assert!(
            !optimized
                .diagnostics
                .notes
                .iter()
                .any(|n| n.contains("logical_plan fallback failed")),
            "no fallback note expected, got: {:?}",
            optimized.diagnostics.notes
        );
    }

    #[test]
    fn algorithm_direction_survives_logical_to_physical() {
        let logical = all_paths_logical();
        let physical = crate::planning::physical_planner::convert_logical_to_physical(logical);
        let PlanNodeEnum::AllPaths(node) = &physical else {
            panic!("expected physical AllPaths");
        };
        assert_eq!(node.direction(), linkrs_core::EdgeDirection::Out);
    }

    #[test]
    fn yield_clause_preserves_logical_mirror() {
        use crate::planning::plan::SubPlan;
        use crate::planning::statements::clauses::yield_planner::YieldClausePlanner;

        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let column = linkrs_core::YieldColumn {
            expression: var_key(&ctx, "a"),
            alias: "a".to_string(),
        };
        let input = SubPlan::from_logical_root(scan_logical("a"));
        let out = YieldClausePlanner::new()
            .plan_yield_clause(&[column], None, None, None, None, false, &input)
            .expect("yield planning should succeed");
        assert!(matches!(out.root, Some(PlanNodeEnum::Project(_))));
        assert!(
            matches!(out.logical_root, Some(LogicalNodeEnum::Project(_))),
            "yield must wrap the upstream logical tree"
        );
    }

    #[test]
    fn with_clause_preserves_logical_mirror() {
        use crate::binder::validation::{WithClauseContext, YieldClauseContext};
        use crate::planning::plan::SubPlan;
        use crate::planning::statements::clauses::with_clause_planner::WithClausePlanner;
        use std::collections::HashMap;

        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let column = linkrs_core::YieldColumn {
            expression: var_key(&ctx, "a"),
            alias: "a".to_string(),
        };
        let with_ctx = WithClauseContext {
            yield_clause: YieldClauseContext {
                yield_columns: vec![column],
                aliases_available: HashMap::new(),
                aliases_generated: HashMap::new(),
                distinct: false,
                has_agg: false,
                group_keys: vec![],
                group_items: vec![],
                need_gen_project: false,
                agg_output_column_names: vec![],
                proj_output_column_names: vec![],
                filter_condition: None,
                skip: None,
                limit: None,
            },
            aliases_available: HashMap::new(),
            aliases_generated: HashMap::new(),
            where_clause: None,
            pagination: None,
            order_by: None,
            distinct: false,
        };
        let input = SubPlan::from_logical_root(scan_logical("a"));
        let out = WithClausePlanner::new()
            .plan_with_clause(&with_ctx, &input)
            .expect("with planning should succeed");
        assert!(matches!(out.root, Some(PlanNodeEnum::Project(_))));
        assert!(
            matches!(out.logical_root, Some(LogicalNodeEnum::Project(_))),
            "with must wrap the upstream logical tree"
        );
    }
}

#[test]
fn precompute_notes_emitted_for_reused_expressions() {
    use crate::planning::plan::core::nodes::access::graph_scan_node::ScanVerticesNode;
    use crate::planning::plan::core::nodes::operation::project_node::ProjectNode;
    use linkrs_core::types::expr::expression_context::ExpressionAnalysisContext;
    use linkrs_core::types::expr::{ContextualExpression, Expression, ExpressionMeta};
    use linkrs_core::types::operators::BinaryOperator;
    use linkrs_core::Value;
    use linkrs_core::YieldColumn;
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
            .diagnostics
            .notes
            .iter()
            .any(|note| note.starts_with("precompute:")),
        "expected precompute decision notes, got: {:?}",
        optimized.diagnostics.notes
    );
}

#[test]
fn cost_based_consumes_logical_plan_when_attached() {
    use crate::optimizer::stats::TagStatistics;
    use crate::planning::plan::core::nodes::access::graph_scan_node::ScanVerticesNode;
    use crate::planning::plan::core::nodes::operation::filter_node::FilterNode;
    use crate::planning::plan::core::nodes::operation::sort_node::{LimitNode, SortItem, SortNode};
    use crate::planning::plan::logical_plan::LogicalPlan;
    use linkrs_core::types::expr::expression_context::ExpressionAnalysisContext;
    use linkrs_core::types::expr::{ContextualExpression, Expression, ExpressionMeta};
    use linkrs_core::types::{Index, IndexStatus, IndexType};
    use linkrs_core::Value;
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
        op: linkrs_core::types::operators::BinaryOperator::Equal,
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

    // The structural rewrites still apply to the physical root. The Sort+Limit
    // fusion is now cost-based (the previous `min_limit` floor that suppressed
    // TopN for limit < 100 has been removed), so a TopN may appear when it is
    // cheaper than a full sort; only the index rewrite is asserted here.
    let root = optimized.root.as_ref().expect("root should exist");
    assert!(
        contains_variant(root, &|node| matches!(node, PlanNodeEnum::IndexScan(_))),
        "expected IndexScan in optimized plan"
    );

    // The decision notes come from the logical walkers.
    assert!(
        optimized
            .diagnostics
            .notes
            .iter()
            .any(|note| note.starts_with("index:")),
        "expected index decision note from the logical walker, got: {:?}",
        optimized.diagnostics.notes
    );

    // The logical plan survives optimization (join order write-back).
    assert!(
        optimized.logical_plan().is_some(),
        "logical plan must remain attached after optimization"
    );
}

#[test]
fn cost_based_falls_back_when_no_logical_plan_attached() {
    use crate::planning::plan::core::nodes::management::manage_node_enums::SpaceManageNode;
    use crate::planning::plan::core::nodes::management::ShowSpacesNode;

    let mut engine = OptimizerEngine::default();
    engine.set_enable_heuristic(false);

    // ShowSpaces is not supported by the physical-to-logical
    // converter, so no logical plan is attached and the physical
    // fallback path must keep the plan intact.
    let plan = ExecutionPlan::new(Some(PlanNodeEnum::SpaceManage(SpaceManageNode::Show(
        ShowSpacesNode::new(1),
    ))));
    assert!(plan.logical_plan().is_none());

    let optimized = engine
        .optimize(plan, Some("test"))
        .expect("optimization should succeed");
    assert!(matches!(optimized.root, Some(PlanNodeEnum::SpaceManage(_))));
    assert!(optimized.logical_plan().is_none());
}

#[test]
fn hinted_logical_plan_skips_join_order_review() {
    use crate::optimizer::stats::StatsView;
    use crate::optimizer::stats::TagStatistics;
    use crate::planning::plan::core::node_id_generator::next_node_id;
    use crate::planning::plan::logical::logical_nodes::access::LogicalScanVerticesNode;
    use crate::planning::plan::logical::logical_nodes::join::LogicalInnerJoinNode;
    use crate::planning::plan::logical::LogicalNodeEnum as LogicalEnum;
    use crate::planning::plan::logical_plan::LogicalPlan;

    fn logical_scan(tag: &str) -> LogicalEnum {
        LogicalEnum::ScanVertices(LogicalScanVerticesNode {
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
    }

    fn join_chain() -> LogicalEnum {
        let left = logical_scan("big");
        let right = logical_scan("small");
        LogicalEnum::InnerJoin(LogicalInnerJoinNode {
            id: next_node_id(),
            left: Box::new(left),
            right: Box::new(right),
            hash_keys: Vec::new(),
            probe_keys: Vec::new(),
            recommended_algorithm: None,
            output_var: None,
            col_names: Vec::new(),
            column_types: Vec::new(),
        })
    }

    fn left_leaf_tag(root: &LogicalEnum) -> Option<String> {
        match root {
            LogicalEnum::InnerJoin(n) => match n.left_input() {
                LogicalEnum::ScanVertices(s) => s.tag.clone(),
                _ => None,
            },
            _ => None,
        }
    }

    let stats_manager = StatisticsManager::new();
    for (tag, rows) in [("big", 1000u64), ("small", 10u64)] {
        let mut tag_stats = TagStatistics::new(tag.to_string());
        tag_stats.vertex_count = rows;
        stats_manager.update_tag_stats("test", tag_stats);
    }
    let stats_view = StatsView::new(&stats_manager, Some("test"));
    let engine = OptimizerEngine::default();

    // Unhinted twin: the reviewer reorders small-first and says so.
    let mut plain = ExecutionPlan::new(None);
    plain.set_logical_plan(LogicalPlan::new(join_chain()));
    engine.apply_join_order_logical(&stats_view, &mut plain, &engine.cost_calculator_for(None));
    assert_eq!(
        left_leaf_tag(plain.logical_plan().expect("logical").root()).as_deref(),
        Some("small")
    );
    assert!(
        plain
            .diagnostics
            .notes
            .iter()
            .any(|note| note.contains("(reviewer)")),
        "expected reviewer note, got: {:?}",
        plain.diagnostics.notes
    );

    // Hinted twin: planning order kept, reviewer stays out.
    let mut hinted = ExecutionPlan::new(None);
    let mut logical = LogicalPlan::new(join_chain());
    logical.join_order_hinted = true;
    hinted.set_logical_plan(logical);
    engine.apply_join_order_logical(&stats_view, &mut hinted, &engine.cost_calculator_for(None));
    assert_eq!(
        left_leaf_tag(hinted.logical_plan().expect("logical").root()).as_deref(),
        Some("big")
    );
    assert!(
        hinted
            .diagnostics
            .notes
            .iter()
            .any(|note| note.contains("user hint")),
        "expected hint note, got: {:?}",
        hinted.diagnostics.notes
    );
    assert!(
        !hinted
            .diagnostics
            .notes
            .iter()
            .any(|note| note.contains("(reviewer)")),
        "reviewer must not touch hinted plans: {:?}",
        hinted.diagnostics.notes
    );
}

#[test]
fn stats_backed_logical_plan_skips_join_order_review() {
    use crate::optimizer::stats::StatsView;
    use crate::optimizer::stats::TagStatistics;
    use crate::planning::plan::core::node_id_generator::next_node_id;
    use crate::planning::plan::logical::logical_nodes::access::LogicalScanVerticesNode;
    use crate::planning::plan::logical::logical_nodes::join::LogicalInnerJoinNode;
    use crate::planning::plan::logical::LogicalNodeEnum as LogicalEnum;
    use crate::planning::plan::logical_plan::LogicalPlan;

    fn logical_scan(tag: &str) -> LogicalEnum {
        LogicalEnum::ScanVertices(LogicalScanVerticesNode {
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
    }

    let stats_manager = StatisticsManager::new();
    for (tag, rows) in [("big", 1000u64), ("small", 10u64)] {
        let mut tag_stats = TagStatistics::new(tag.to_string());
        tag_stats.vertex_count = rows;
        stats_manager.update_tag_stats("test", tag_stats);
    }
    let stats_view = StatsView::new(&stats_manager, Some("test"));
    let engine = OptimizerEngine::default();

    let mut backed = ExecutionPlan::new(None);
    let mut logical = LogicalPlan::new(LogicalEnum::InnerJoin(LogicalInnerJoinNode {
        id: next_node_id(),
        left: Box::new(logical_scan("big")),
        right: Box::new(logical_scan("small")),
        hash_keys: Vec::new(),
        probe_keys: Vec::new(),
        recommended_algorithm: None,
        output_var: None,
        col_names: Vec::new(),
        column_types: Vec::new(),
    }));
    logical.stats_backed = true;
    backed.set_logical_plan(logical);
    engine.apply_join_order_logical(&stats_view, &mut backed, &engine.cost_calculator_for(None));
    assert!(
        backed
            .diagnostics
            .notes
            .iter()
            .any(|note| note.contains("statistics-backed")),
        "expected stats-backed note, got: {:?}",
        backed.diagnostics.notes
    );
    assert!(
        !backed
            .diagnostics
            .notes
            .iter()
            .any(|note| note.contains("(reviewer)")),
        "reviewer must not touch stats-backed plans: {:?}",
        backed.diagnostics.notes
    );
}

#[test]
fn test_space_calculator_falls_back_to_base() {
    let engine = OptimizerEngine::new(CostModelConfig::for_ssd());
    assert_eq!(
        engine
            .cost_calculator_for(Some("unknown_space"))
            .config()
            .random_page_cost,
        1.1
    );
    assert_eq!(
        engine.cost_calculator_for(None).config().random_page_cost,
        1.1
    );
}

#[test]
fn test_space_override_reaches_space_calculator() {
    let engine = OptimizerEngine::new(CostModelConfig::for_ssd());
    let mut overrides = std::collections::HashMap::new();
    overrides.insert("cold".into(), CostModelConfig::for_hdd());
    engine.set_space_cost_configs(overrides);
    assert_eq!(
        engine
            .cost_calculator_for(Some("cold"))
            .config()
            .random_page_cost,
        4.0
    );
    assert_eq!(
        engine
            .cost_calculator_for(Some("hot"))
            .config()
            .random_page_cost,
        1.1
    );
}

#[test]
fn test_cost_config_change_bumps_epoch_and_clears_spaces() {
    let mut engine = OptimizerEngine::new(CostModelConfig::for_ssd());
    assert_eq!(engine.cost_epoch(), 0);
    let mut overrides = std::collections::HashMap::new();
    overrides.insert("cold".into(), CostModelConfig::for_hdd());
    engine.set_space_cost_configs(overrides);
    assert_eq!(engine.cost_epoch(), 1);
    assert_eq!(
        engine
            .cost_calculator_for(Some("cold"))
            .config()
            .random_page_cost,
        4.0
    );
    engine.set_cost_config(CostModelConfig::for_in_memory());
    assert_eq!(engine.cost_epoch(), 2);
    assert_eq!(
        engine
            .cost_calculator_for(Some("cold"))
            .config()
            .random_page_cost,
        4.0
    );
    assert_eq!(
        engine.cost_calculator_for(None).config().random_page_cost,
        0.1
    );
}

#[test]
fn test_feedback_fold_consumes_each_execution_once() {
    use crate::optimizer::stats::feedback::query::{OperatorFeedback, QueryExecutionFeedback};

    let engine = OptimizerEngine::default();
    let key = "test_space:ScanVertices:person".to_string();
    engine.cardinality_feedback.register_key(key.clone(), 100.0);

    // One execution aggregated across two partitions arrives as a single
    // per-operator sample.
    let mut feedback = QueryExecutionFeedback::new("fp".to_string());
    feedback.space = Some("test_space".to_string());
    feedback.estimated_rows = 0;
    feedback.actual_rows = 300;
    feedback.add_operator_feedback(OperatorFeedback {
        operator_id: "op1".to_string(),
        operator_type: "ScanVertices".to_string(),
        estimated_rows: 100,
        actual_rows: 300,
        actual_input_rows: None,
        partition_count: 2,
        exhausted: true,
        estimated_time_us: 0,
        actual_time_us: 0,
        execution_loops: 1,
        condition_key: None,
        shape_key: Some(key.clone()),
    });
    engine.feedback_history.add_feedback(feedback);

    engine.maybe_apply_feedback();
    let once = engine
        .cardinality_feedback
        .corrected_rows(&key)
        .expect("shape should remain registered");
    assert!(once > 100.0, "single 3x sample must move up: {once}");

    // Re-optimizing without new executions must not walk the same sample
    // again: the correction is exactly the one-fold value.
    engine.maybe_apply_feedback();
    let twice = engine
        .cardinality_feedback
        .corrected_rows(&key)
        .expect("shape should remain registered");
    assert!(
        (once - twice).abs() < 1e-9,
        "refold must be a no-op: {once} vs {twice}"
    );
}

#[test]
fn skip_gate_covers_hint_and_stats_combinations() {
    use crate::planning::plan::logical::logical_nodes::access::LogicalStartNode;
    use crate::planning::plan::logical::LogicalNodeEnum as LogicalEnum;
    use crate::planning::plan::logical_plan::LogicalPlan;

    fn plan_with(hinted: bool, backed: bool) -> LogicalPlan {
        let mut plan = LogicalPlan::new(LogicalEnum::Start(LogicalStartNode::new()));
        plan.join_order_hinted = hinted;
        plan.stats_backed = backed;
        plan
    }

    assert!(!plan_with(false, false).should_skip_join_order_review());
    assert!(plan_with(true, false).should_skip_join_order_review());
    assert!(plan_with(false, true).should_skip_join_order_review());
    assert!(plan_with(true, true).should_skip_join_order_review());
}

#[test]
fn stat_free_fallback_gate_matches_heuristic_rule() {
    use crate::optimizer::cost_based::subquery_unnesting::SubqueryUnnestingOptimizer;
    use crate::planning::plan::core::nodes::access::graph_scan_node::ScanVerticesNode;

    let scan = PlanNodeEnum::ScanVertices(ScanVerticesNode::new(1, "test"));
    assert!(SubqueryUnnestingOptimizer::is_stat_free_fallback_eligible(
        &scan
    ));
    let agg = PlanNodeEnum::Aggregate(
        crate::planning::plan::core::nodes::graph_operations::aggregate_node::AggregateNode::new(
            scan.clone(),
            Vec::new(),
            Vec::new(),
        )
        .expect("aggregate builds"),
    );
    assert!(!SubqueryUnnestingOptimizer::is_stat_free_fallback_eligible(
        &agg
    ));
}

mod fast_shape_tests {
    use super::*;
    use crate::optimizer::heuristic::rule_enum::RuleRegistry;
    use crate::optimizer::stats::TagStatistics;
    use crate::planning::plan::core::nodes::access::graph_scan_node::{
        ScanEdgesNode, ScanVerticesNode,
    };
    use crate::planning::plan::core::nodes::access::index_scan::{IndexScanNode, ScanType};
    use crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
    use crate::planning::plan::core::nodes::graph_operations::graph_operations_node::UnionNode;
    use crate::planning::plan::core::nodes::join::join_node::InnerJoinNode;
    use crate::planning::plan::core::nodes::operation::filter_node::FilterNode;
    use crate::planning::plan::core::nodes::operation::project_node::ProjectNode;
    use crate::planning::plan::core::nodes::operation::sort_node::{LimitNode, SortItem, SortNode};
    use crate::planning::plan::core::nodes::traversal::traversal_node::ExpandAllNode;
    use crate::planning::plan::ExecutionPlan;
    use linkrs_core::types::expr::expression_context::ExpressionAnalysisContext;
    use linkrs_core::types::expr::{ContextualExpression, Expression, ExpressionMeta};
    use linkrs_core::Value;
    use std::sync::Arc;

    /// Rule names the fast batch keeps. The no-fire test below asserts the
    /// full path never fires anything outside this set on gated shapes.
    const FAST_RULES: &[&str] = &[
        "PushFilterDownExpandAllRule",
        "PushVFilterDownScanVerticesRule",
        "PushFilterDownScanVerticesRule",
        "PushFilterDownScanEdgesRule",
        "EliminateRedundantTagFilterRule",
        "PushProjectDownScanVerticesRule",
        "PushProjectDownScanEdgesRule",
        "EnrichScanSlotsWithFilterPropsRule",
        "ExpandPushdownAnnotateRule",
        "ScanIdentityAnnotateRule",
        "ScanEdgeIdentityAnnotateRule",
        "FoldConstantsRule",
        "EliminateFilterRule",
        "RemoveNoopProjectRule",
        "CombineFilterRule",
        "CollapseProjectRule",
        "CollapseConsecutiveProjectRule",
    ];

    fn person_scan(var: &str) -> PlanNodeEnum {
        let mut scan = ScanVerticesNode::new(1, "test");
        scan.set_tag("person");
        scan.set_col_names(vec![var.to_string()]);
        scan.set_output_var(var.to_string());
        PlanNodeEnum::ScanVertices(scan)
    }

    fn prop_eq_filter(input: PlanNodeEnum, var: &str, prop: &str, value: Value) -> PlanNodeEnum {
        let context = Arc::new(ExpressionAnalysisContext::new());
        let predicate = Expression::Binary {
            left: Box::new(Expression::Property {
                object: Box::new(Expression::Variable(var.to_string())),
                property: prop.to_string(),
            }),
            op: linkrs_core::types::operators::BinaryOperator::Equal,
            right: Box::new(Expression::Literal(value)),
        };
        let id = context.register_expression(ExpressionMeta::new(predicate));
        let filter = FilterNode::new(input, ContextualExpression::new(id, context))
            .expect("filter should build");
        PlanNodeEnum::Filter(filter)
    }

    fn project_var(input: PlanNodeEnum, var: &str) -> PlanNodeEnum {
        use linkrs_core::YieldColumn;
        let context = Arc::new(ExpressionAnalysisContext::new());
        let expr_meta = ExpressionMeta::new(Expression::Variable(var.to_string()));
        let id = context.register_expression(expr_meta);
        let columns = vec![YieldColumn {
            expression: ContextualExpression::new(id, context),
            alias: var.to_string(),
        }];
        let project = ProjectNode::new(input, columns).expect("project should build");
        PlanNodeEnum::Project(project)
    }

    fn one_hop_chain() -> PlanNodeEnum {
        use crate::planning::plan::core::nodes::base::plan_node_traits::{
            MultipleInputNode, PlanNode,
        };
        let mut expand = ExpandAllNode::new(1, vec!["follows".to_string()], "OUT");
        expand.set_step_limit(1);
        expand.set_id_only(true);
        expand.set_col_names(vec!["a".to_string(), "e".to_string(), "b".to_string()]);
        expand.add_input(person_scan("a"));
        PlanNodeEnum::ExpandAll(expand)
    }

    #[test]
    fn fast_shape_gate_accepts_point_chains() {
        assert!(OptimizerEngine::is_fast_shape(&person_scan("n")));
        let filtered = prop_eq_filter(person_scan("n"), "n", "name", Value::String("a".into()));
        assert!(OptimizerEngine::is_fast_shape(&filtered));
        assert!(OptimizerEngine::is_fast_shape(&project_var(filtered, "n")));
    }

    #[test]
    fn fast_shape_gate_accepts_one_hop_and_edge_scan() {
        assert!(OptimizerEngine::is_fast_shape(&one_hop_chain()));
        let scan = ScanEdgesNode::new(1, "follows");
        assert!(OptimizerEngine::is_fast_shape(&PlanNodeEnum::ScanEdges(
            scan
        )));
        let index = IndexScanNode::new(
            1,
            7,
            3,
            "idx".to_string(),
            "person".to_string(),
            ScanType::Unique,
        );
        assert!(OptimizerEngine::is_fast_shape(&PlanNodeEnum::IndexScan(
            index
        )));
    }

    #[test]
    fn fast_shape_gate_rejects_complex_shapes() {
        // Sort+Limit chain.
        let sort = SortNode::new(
            person_scan("n"),
            vec![SortItem::column_asc("n.name".to_string())],
        )
        .expect("sort should build");
        let limit = LimitNode::new(PlanNodeEnum::Sort(sort), 0, 50).expect("limit should build");
        assert!(!OptimizerEngine::is_fast_shape(&PlanNodeEnum::Limit(limit)));

        // Union of two scans.
        let union =
            UnionNode::new(person_scan("a"), person_scan("b"), false).expect("union should build");
        assert!(!OptimizerEngine::is_fast_shape(&PlanNodeEnum::Union(union)));

        // Two-hop chain.
        {
            use crate::planning::plan::core::nodes::base::plan_node_traits::{
                MultipleInputNode, PlanNode,
            };
            let mut hop1 = ExpandAllNode::new(1, vec!["follows".to_string()], "OUT");
            hop1.set_step_limit(1);
            hop1.set_id_only(true);
            hop1.add_input(person_scan("a"));
            let mut hop2 = ExpandAllNode::new(2, vec!["follows".to_string()], "OUT");
            hop2.set_step_limit(1);
            hop2.set_id_only(true);
            hop2.add_input(PlanNodeEnum::ExpandAll(hop1));
            assert!(!OptimizerEngine::is_fast_shape(&PlanNodeEnum::ExpandAll(
                hop2
            )));
        }

        // Join of two scans.
        let join = InnerJoinNode::new(person_scan("a"), person_scan("b"), Vec::new(), Vec::new())
            .expect("join should build");
        assert!(!OptimizerEngine::is_fast_shape(&PlanNodeEnum::InnerJoin(
            join
        )));
    }

    #[test]
    fn fast_shape_registry_matches_keep_list() {
        let registry = RuleRegistry::fast_shape();
        let names: Vec<String> = registry
            .iter()
            .map(|rule| rule.name().to_string())
            .collect();
        assert_eq!(names.len(), FAST_RULES.len());
        for expected in FAST_RULES {
            assert!(
                names.iter().any(|name| name == expected),
                "fast registry must contain {expected}"
            );
        }
    }

    fn engine_with_person_stats() -> OptimizerEngine {
        use crate::optimizer::stats::TagStatistics;
        let engine = OptimizerEngine::default();
        let mut tag_stats = TagStatistics::new("person".to_string());
        tag_stats.vertex_count = 10_000;
        engine.stats_manager().update_tag_stats("test", tag_stats);
        engine
    }

    /// Normalize counter-driven identifiers (plan node ids, expression
    /// ids) so two optimizations of the same input can be compared
    /// structurally. Both paths consume the global counters in the same
    /// relative order, but absolute values depend on cross-test counter
    /// state; the inputs are clones, so any remaining digit difference
    /// outside these two anchored patterns would be a real divergence.
    fn normalize_counters(rendered: &str) -> String {
        let masked = mask_digits_after(rendered, "ExpressionId(");
        mask_digits_after(&masked, "id: ")
    }

    /// Replace the digit run following every word-boundary `marker` with
    /// `#`. The boundary keeps fields like `identity_only` or `valid`
    /// untouched: only a standalone `id` (or the exact `ExpressionId(`
    /// prefix) masks.
    fn mask_digits_after(rendered: &str, marker: &str) -> String {
        let mut masked = String::with_capacity(rendered.len());
        let mut rest = rendered;
        while let Some(position) = rest.find(marker) {
            let (head, tail) = rest.split_at(position);
            let boundary_ok = head
                .chars()
                .last()
                .is_none_or(|previous| !previous.is_alphanumeric() && previous != '_');
            // For `ExpressionId(` the marker carries its own boundary.
            let anchored = marker.ends_with('(') || boundary_ok;
            if !anchored {
                masked.push_str(head);
                masked.push_str(&tail[..marker.len()]);
                rest = &tail[marker.len()..];
                continue;
            }
            masked.push_str(head);
            masked.push_str(marker);
            rest = &tail[marker.len()..];
            let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            if digits.is_empty() {
                continue;
            }
            masked.push('#');
            rest = &rest[digits.len()..];
        }
        masked.push_str(rest);
        masked
    }

    fn full_vs_fast(root: PlanNodeEnum) {
        use crate::optimizer::partitioning::PartitioningLayoutInfo;
        let engine = engine_with_person_stats();
        let layout = PartitioningLayoutInfo::default();
        let full = engine
            .optimize_full_shape(
                ExecutionPlan::new(Some(root.clone())),
                Some("test"),
                &layout,
                &mut None,
                false,
            )
            .expect("full optimization should succeed");
        let fast = engine
            .optimize_fast_shape(
                ExecutionPlan::new(Some(root)),
                Some("test"),
                &layout,
                &mut None,
                false,
            )
            .expect("fast optimization should succeed");
        assert_eq!(
            normalize_counters(&format!("{:?}", full.root)),
            normalize_counters(&format!("{:?}", fast.root)),
            "fast path must produce the identical plan tree"
        );
        let mut fast_notes = fast.diagnostics.notes.clone();
        assert!(
            fast_notes
                .first()
                .is_some_and(|note| note.starts_with("fast_shape:")),
            "fast path must record its short-circuit decision first"
        );
        fast_notes.remove(0);
        assert_eq!(
            full.diagnostics.notes, fast_notes,
            "fast path must preserve every other diagnostic note"
        );
    }

    #[test]
    fn fast_path_matches_full_path_on_point_chains() {
        // Property filter over a tagged scan.
        full_vs_fast(prop_eq_filter(
            person_scan("n"),
            "n",
            "name",
            Value::String("alice".into()),
        ));
        // Projected point lookup.
        full_vs_fast(project_var(
            prop_eq_filter(person_scan("n"), "n", "name", Value::String("a".into())),
            "n",
        ));
        // Bare scan.
        full_vs_fast(person_scan("n"));
    }

    #[test]
    fn fast_path_matches_full_path_on_compound_filters() {
        // AND-combined filters exercise the merge rules kept in the fast set.
        let context = Arc::new(ExpressionAnalysisContext::new());
        let mut tag_predicate = |prop: &str, value: Value| {
            let predicate = Expression::Binary {
                left: Box::new(Expression::Property {
                    object: Box::new(Expression::Variable("n".to_string())),
                    property: prop.to_string(),
                }),
                op: linkrs_core::types::operators::BinaryOperator::Equal,
                right: Box::new(Expression::Literal(value)),
            };
            let id = context.register_expression(ExpressionMeta::new(predicate));
            ContextualExpression::new(id, context.clone())
        };
        let first = FilterNode::new(
            person_scan("n"),
            tag_predicate("name", Value::String("a".into())),
        )
        .expect("filter should build");
        let second = FilterNode::new(
            PlanNodeEnum::Filter(first),
            tag_predicate("value", Value::Int(1)),
        )
        .expect("filter should build");
        full_vs_fast(project_var(PlanNodeEnum::Filter(second), "n"));

        // Constant predicate exercises folding in both paths.
        let truthy = FilterNode::new(
            person_scan("n"),
            ContextualExpression::new(
                context.register_expression(ExpressionMeta::new(Expression::Literal(Value::Bool(
                    true,
                )))),
                context.clone(),
            ),
        )
        .expect("filter should build");
        full_vs_fast(PlanNodeEnum::Filter(truthy));
    }

    #[test]
    fn fast_path_matches_full_path_on_one_hop() {
        full_vs_fast(project_var(one_hop_chain(), "b"));
        full_vs_fast(one_hop_chain());
    }

    #[test]
    fn partitioned_plan_carries_partition_note() {
        use crate::optimizer::partitioning::PartitioningConfig;
        let mut engine = engine_with_person_stats();
        engine.set_partitioning_config(PartitioningConfig {
            enabled: true,
            min_rows_per_partition: 1_000,
            max_partitions: 4,
            vertex_id_range: Some(0i64..10_000),
            max_workers: 4,
            max_buffered_chunks: 10,
            fixed_overhead_rows: 0,
            per_partition_rows: 0,
            exchange_row_per_mille: 0,
        });
        let plan = engine
            .optimize(
                ExecutionPlan::new(Some(person_scan("n"))),
                Some("test"),
            )
            .expect("optimization should succeed");
        assert!(
            plan.partition_spec().is_some(),
            "ten thousand rows must partition, notes: {:?}",
            plan.diagnostics.notes
        );
        assert!(
            plan.diagnostics
                .notes
                .iter()
                .any(|note| note.starts_with("partitioning: ")),
            "partitioned plan must surface the decision, notes: {:?}",
            plan.diagnostics.notes
        );
    }

    #[test]
    fn fast_path_matches_full_path_on_edge_scans() {
        let bare = PlanNodeEnum::ScanEdges(ScanEdgesNode::new(1, "follows"));
        full_vs_fast(bare);
        let filtered = prop_eq_filter(
            PlanNodeEnum::ScanEdges(ScanEdgesNode::new(1, "follows")),
            "e",
            "weight",
            Value::Double(1.5),
        );
        full_vs_fast(filtered);
        full_vs_fast(project_var(
            prop_eq_filter(person_scan("n"), "n", "value", Value::Double(1.0)),
            "n",
        ));
    }

    #[test]
    fn fast_path_runs_single_chain_pass() {
        use crate::optimizer::partitioning::PartitioningLayoutInfo;
        let engine = engine_with_person_stats();
        let layout = PartitioningLayoutInfo::default();
        let chain = prop_eq_filter(person_scan("n"), "n", "name", Value::String("a".into()));
        let _ = engine
            .optimize_fast_shape(
                ExecutionPlan::new(Some(chain)),
                Some("test"),
                &layout,
                &mut None,
                false,
            )
            .expect("fast optimization should succeed");
        // Five batches carry fast-shape rules; a second chain pass would
        // append five more entries without applying anything.
        assert_eq!(engine.last_batch_statistics().len(), 5);
    }

    #[test]
    fn full_path_fires_no_omitted_rules_on_gated_shapes() {
        use crate::optimizer::partitioning::PartitioningLayoutInfo;
        let engine = engine_with_person_stats();
        let layout = PartitioningLayoutInfo::default();
        let chains = vec![
            prop_eq_filter(person_scan("n"), "n", "name", Value::String("a".into())),
            project_var(
                prop_eq_filter(person_scan("n"), "n", "name", Value::String("a".into())),
                "n",
            ),
            one_hop_chain(),
        ];
        for chain in chains {
            assert!(OptimizerEngine::is_fast_shape(&chain));
            engine
                .optimize_full_shape(
                    ExecutionPlan::new(Some(chain)),
                    Some("test"),
                    &layout,
                    &mut None,
                    false,
                )
                .expect("full optimization should succeed");
            for (batch, stats) in engine.last_batch_statistics() {
                for fired in stats.rule_hit_counts.keys() {
                    assert!(
                        FAST_RULES.contains(&fired.as_str()),
                        "full path fired {fired} in {batch:?} on a gated shape"
                    );
                }
            }
        }
    }
}
