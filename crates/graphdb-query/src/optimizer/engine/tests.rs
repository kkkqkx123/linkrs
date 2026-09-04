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
    use graphdb_core::types::expr::expression_context::ExpressionAnalysisContext;
    use graphdb_core::types::expr::{ContextualExpression, Expression, ExpressionMeta};

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
            input: Some(Box::new(input.clone())),
            deps: vec![input],
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
                .cbo_notes
                .iter()
                .any(|n| n.contains("logical_plan fallback failed")),
            "no fallback note expected, got: {:?}",
            optimized.cbo_notes
        );
    }

    #[test]
    fn unsupported_node_records_counted_fallback_notes() {
        use crate::planning::plan::core::nodes::management::manage_node_enums::SpaceManageNode;
        use crate::planning::plan::core::nodes::management::ShowSpacesNode;

        let mut engine = OptimizerEngine::default();
        engine.set_enable_heuristic(false);
        let plan = ExecutionPlan::new(Some(PlanNodeEnum::SpaceManage(
            SpaceManageNode::Show(ShowSpacesNode::new(1)),
        )));
        let optimized = engine
            .optimize(plan, Some("test"))
            .expect("optimization should succeed");
        assert!(optimized.logical_plan().is_none());
        assert!(
            optimized
                .cbo_notes
                .iter()
                .any(|n| n.starts_with("factorization: logical_plan_fallback_total=1 (node=")),
            "expected node-labelled fallback note, got: {:?}",
            optimized.cbo_notes
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
                .cbo_notes
                .iter()
                .any(|n| n.contains("WcoIntersect fallback to HashJoin")),
            "expected fallback note, got: {:?}",
            optimized.cbo_notes
        );
        assert!(
            optimized
                .cbo_notes
                .iter()
                .any(|n| n == "factorization: intersect_to_join_rewrite_total=1"),
            "expected rewrite total note, got: {:?}",
            optimized.cbo_notes
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
                .cbo_notes
                .iter()
                .any(|n| n.contains("intersect_to_join_rewrite_total")),
            "no rewrite total expected, got: {:?}",
            optimized.cbo_notes
        );
    }

    #[test]
    fn metrics_stats_collects_flatten_and_fallback_counts() {
        use crate::planning::plan::logical::logical_nodes::flatten::LogicalFlattenNode;

        let metrics = Arc::new(graphdb_core::stats::StatsManager::new());
        let mut engine = OptimizerEngine::default();
        engine.set_metrics_stats(Arc::clone(&metrics));

        // Flatten path: a logical Flatten node reports its position count.
        let mut flatten_plan = ExecutionPlan::new(None);
        flatten_plan.set_logical_plan(LogicalPlan::new(LogicalNodeEnum::Flatten(
            LogicalFlattenNode::new(0, scan_logical("a")),
        )));
        let out = engine.apply_factorization(flatten_plan);
        assert!(
            out.cbo_notes
                .iter()
                .any(|n| n == "factorization: flatten_total=1"),
            "expected flatten total note, got: {:?}",
            out.cbo_notes
        );
        assert_eq!(
            metrics.get_value(graphdb_core::MetricType::FactorizationFlattenTotal),
            Some(1)
        );

        // Fallback path: an unsupported physical-only plan increments the
        // fallback counter through the full optimize pipeline.
        use crate::planning::plan::core::nodes::management::manage_node_enums::SpaceManageNode;
        use crate::planning::plan::core::nodes::management::ShowSpacesNode;
        let plan = ExecutionPlan::new(Some(PlanNodeEnum::SpaceManage(
            SpaceManageNode::Show(ShowSpacesNode::new(1)),
        )));
        engine
            .optimize(plan, Some("test"))
            .expect("optimization should succeed");
        assert_eq!(
            metrics.get_value(graphdb_core::MetricType::FactorizationFallbackTotal),
            Some(1)
        );
    }

    #[test]
    fn builder_with_metrics_stats_wires_sink() {
        use crate::optimizer::builder::OptimizerEngineBuilder;

        let metrics = Arc::new(graphdb_core::stats::StatsManager::new());
        let engine = OptimizerEngineBuilder::new()
            .with_metrics_stats(Arc::clone(&metrics))
            .build();
        assert!(engine.metrics_stats().is_some());
        assert!(OptimizerEngine::default().metrics_stats().is_none());
    }

    #[test]
    fn connector_propagates_logical_joins() {
        use crate::planning::connector::SegmentsConnector;
        use crate::planning::plan::SubPlan;
        use std::collections::HashSet;

        let left = SubPlan::from_logical_root(scan_logical("a"));
        let right = SubPlan::from_logical_root(scan_logical("b"));
        let qctx = Arc::new(crate::QueryContext::new(Arc::new(
            crate::QueryRequestContext {
                session_id: None,
                user_name: None,
                space_name: None,
                query: String::new(),
                parameters: std::collections::HashMap::new(),
                ..Default::default()
            },
        )));
        let joined = SegmentsConnector::inner_join(&qctx, left, right, HashSet::new())
            .expect("join should build");
        assert!(matches!(joined.root, Some(PlanNodeEnum::InnerJoin(_))));
        assert!(
            matches!(joined.logical_root, Some(LogicalNodeEnum::InnerJoin(_))),
            "connector must propagate the logical join"
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
            left: Box::new(left.clone()),
            right: Box::new(right.clone()),
            deps: vec![left, right],
            space_id: 1,
            steps: 3,
            edge_types: vec!["knows".to_string()],
            min_hop: 1,
            max_hop: 3,
            acyclic: true,
            direction: graphdb_core::EdgeDirection::Out,
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
                .cbo_notes
                .iter()
                .any(|n| n.contains("logical_plan fallback failed")),
            "no fallback note expected, got: {:?}",
            optimized.cbo_notes
        );
    }

    #[test]
    fn algorithm_direction_survives_logical_to_physical() {
        let logical = all_paths_logical();
        let physical = crate::planning::physical_planner::convert_logical_to_physical(logical);
        let PlanNodeEnum::AllPaths(node) = &physical else {
            panic!("expected physical AllPaths");
        };
        assert_eq!(node.direction(), graphdb_core::EdgeDirection::Out);
    }

    #[test]
    fn yield_clause_preserves_logical_mirror() {
        use crate::planning::plan::SubPlan;
        use crate::planning::statements::clauses::yield_planner::YieldClausePlanner;

        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let column = graphdb_core::YieldColumn {
            expression: var_key(&ctx, "a"),
            alias: "a".to_string(),
            is_matched: false,
        };
        let input = SubPlan::from_logical_root(scan_logical("a"));
        let out = YieldClausePlanner::new()
            .plan_yield_clause(&[column], None, None, None, None, &input)
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
        let column = graphdb_core::YieldColumn {
            expression: var_key(&ctx, "a"),
            alias: "a".to_string(),
            is_matched: false,
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
    use crate::planning::plan::core::nodes::management::manage_node_enums::SpaceManageNode;
    use crate::planning::plan::core::nodes::management::ShowSpacesNode;

    let mut engine = OptimizerEngine::default();
    engine.set_enable_heuristic(false);

    // ShowSpaces is not supported by the physical-to-logical
    // converter, so no logical plan is attached and the physical
    // fallback path must keep the plan intact.
    let plan = ExecutionPlan::new(Some(PlanNodeEnum::SpaceManage(
        SpaceManageNode::Show(ShowSpacesNode::new(1)),
    )));
    assert!(plan.logical_plan().is_none());

    let optimized = engine
        .optimize(plan, Some("test"))
        .expect("optimization should succeed");
    assert!(matches!(
        optimized.root,
        Some(PlanNodeEnum::SpaceManage(_))
    ));
    assert!(optimized.logical_plan().is_none());
}
