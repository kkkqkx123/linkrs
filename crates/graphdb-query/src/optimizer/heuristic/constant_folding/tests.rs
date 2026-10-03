    use crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
    use crate::planning::plan::core::nodes::control_flow::start_node::StartNode;
    use crate::planning::plan::core::nodes::operation::FilterNode;
    use crate::planning::plan::core::nodes::operation::ProjectNode;
    use graphdb_core::types::expr::analysis_utils::is_evaluable;
    use graphdb_core::types::expr::ExpressionAnalysisContext;
    use graphdb_core::types::expr::ExpressionMeta;
    use graphdb_core::types::operators::BinaryOperator;
    use graphdb_core::types::ContextualExpression;
    use graphdb_core::Value;
    use graphdb_core::YieldColumn;
    use std::sync::Arc;

    fn contextual(
        expr_ctx: &Arc<ExpressionAnalysisContext>,
        expr: Expression,
    ) -> ContextualExpression {
        let meta = ExpressionMeta::new(expr);
        let id = expr_ctx.register_expression(meta);
        ContextualExpression::new(id, expr_ctx.clone())
    }

    #[test]
    fn test_fold_rule_name() {
        let rule = FoldConstantsRule::new();
        assert_eq!(rule.name(), "FoldConstantsRule");
    }

    #[test]
    fn test_fold_binary_constant() {
        let expr = Expression::Binary {
            left: Box::new(Expression::Literal(Value::Int(1))),
            op: BinaryOperator::Add,
            right: Box::new(Expression::Literal(Value::Int(2))),
        };
        assert!(is_evaluable(&expr));
        let folded = FoldConstantsRule::fold_expression(&expr);
        assert_eq!(folded, Expression::Literal(Value::Int(3)));
    }

    #[test]
    fn test_fold_with_variable_is_not_folded() {
        let expr = Expression::Binary {
            left: Box::new(Expression::Variable("x".to_string())),
            op: BinaryOperator::Add,
            right: Box::new(Expression::Literal(Value::Int(2))),
        };
        let folded = FoldConstantsRule::fold_expression(&expr);
        assert_eq!(folded, expr, "expressions with variables must not fold");
    }

    #[test]
    fn test_fold_partial_constant_subexpression() {
        let expr = Expression::Binary {
            left: Box::new(Expression::Binary {
                left: Box::new(Expression::Literal(Value::Int(1))),
                op: BinaryOperator::Add,
                right: Box::new(Expression::Literal(Value::Int(2))),
            }),
            op: BinaryOperator::Add,
            right: Box::new(Expression::Variable("x".to_string())),
        };
        let folded = FoldConstantsRule::fold_expression(&expr);
        assert_eq!(
            folded,
            Expression::Binary {
                left: Box::new(Expression::Literal(Value::Int(3))),
                op: BinaryOperator::Add,
                right: Box::new(Expression::Variable("x".to_string())),
            },
            "constant sub-expressions fold, the variable part stays"
        );
    }

    #[test]
    fn test_fold_impure_function_is_not_folded() {
        let expr = Expression::Function {
            name: "rand".to_string(),
            args: vec![],
        };
        assert!(is_evaluable(&expr), "rand has no context dependency");
        let folded = FoldConstantsRule::fold_expression(&expr);
        assert_eq!(folded, expr, "impure functions must never fold");
    }

    #[test]
    fn test_fold_unregistered_function_is_not_folded() {
        // A function name that is not in the registry must conservatively
        // stay unfolded even though it carries no context dependency.
        let expr = Expression::Function {
            name: "future_nondeterministic_fn".to_string(),
            args: vec![FunctionArg::Positional(Expression::Literal(Value::Int(1)))],
        };
        assert!(is_evaluable(&expr), "no context dependency");
        let folded = FoldConstantsRule::fold_expression(&expr);
        assert_eq!(folded, expr, "unregistered functions must never fold");
    }

    #[test]
    fn test_fold_pure_builtin_function() {
        let expr = Expression::Function {
            name: "abs".to_string(),
            args: vec![FunctionArg::Positional(Expression::Literal(Value::Int(-5)))],
        };
        let folded = FoldConstantsRule::fold_expression(&expr);
        assert_eq!(
            folded,
            Expression::Literal(Value::Int(5)),
            "pure builtin functions fold"
        );
    }

    #[test]
    fn test_fold_aggregate_is_not_folded() {
        let expr = Expression::Aggregate {
            func: graphdb_core::AggregateFunction::Count,
            args: vec![Expression::Literal(Value::Int(1))],
            distinct: false,
            filter: None,
        };
        let folded = FoldConstantsRule::fold_expression(&expr);
        assert_eq!(folded, expr, "aggregates need a row-group context");
    }

    #[test]
    fn test_fold_filter_node_condition() {
        let expr_ctx = Arc::new(ExpressionAnalysisContext::new());
        let start = PlanNodeEnum::Start(StartNode::new());
        let condition = contextual(
            &expr_ctx,
            Expression::Binary {
                left: Box::new(Expression::Literal(Value::Int(1))),
                op: BinaryOperator::Equal,
                right: Box::new(Expression::Literal(Value::Int(1))),
            },
        );
        let filter = FilterNode::new(start, condition).expect("filter node");
        let node = PlanNodeEnum::Filter(filter);

        let rule = FoldConstantsRule::new();
        let mut ctx = RewriteContext::new();
        let result = rule.apply(&mut ctx, &node).expect("apply should succeed");
        let result = result.expect("filter with constant condition must fold");
        let new_node = result.new_nodes.first().cloned().expect("replacement node");
        match new_node {
            PlanNodeEnum::Filter(f) => {
                let folded = f
                    .condition()
                    .expression()
                    .expect("expression")
                    .inner()
                    .clone();
                assert_eq!(
                    folded,
                    Expression::Literal(Value::Bool(true)),
                    "1 = 1 folds to TRUE"
                );
            }
            other => panic!("expected Filter node, got {:?}", other),
        }
    }

    #[test]
    fn test_fold_project_node_columns() {
        let expr_ctx = Arc::new(ExpressionAnalysisContext::new());
        let start = PlanNodeEnum::Start(StartNode::new());
        let column = YieldColumn {
            expression: contextual(
                &expr_ctx,
                Expression::Binary {
                    left: Box::new(Expression::Literal(Value::Int(10))),
                    op: BinaryOperator::Divide,
                    right: Box::new(Expression::Literal(Value::Int(2))),
                },
            ),
            alias: "half".to_string(),
        };
        let project = ProjectNode::new(start, vec![column]).expect("project node");
        let node = PlanNodeEnum::Project(project);

        let rule = FoldConstantsRule::new();
        let mut ctx = RewriteContext::new();
        let result = rule.apply(&mut ctx, &node).expect("apply should succeed");
        let result = result.expect("project with constant column must fold");
        let new_node = result.new_nodes.first().cloned().expect("replacement node");
        match new_node {
            PlanNodeEnum::Project(p) => {
                let folded = p.columns()[0]
                    .expression
                    .expression()
                    .expect("expression")
                    .inner()
                    .clone();
                assert_eq!(folded, Expression::Literal(Value::Int(5)));
                assert_eq!(p.columns()[0].alias, "half", "alias is preserved");
            }
            other => panic!("expected Project node, got {:?}", other),
        }
    }

    #[test]
    fn test_fold_project_partial_subexpression() {
        // Partial folding: `p.age + (1 + 2)` keeps the variable part but
        // folds the constant sub-expression to `p.age + 3`.
        let expr_ctx = Arc::new(ExpressionAnalysisContext::new());
        let start = PlanNodeEnum::Start(StartNode::new());
        let column = YieldColumn {
            expression: contextual(
                &expr_ctx,
                Expression::Binary {
                    left: Box::new(Expression::Variable("p.age".to_string())),
                    op: BinaryOperator::Add,
                    right: Box::new(Expression::Binary {
                        left: Box::new(Expression::Literal(Value::Int(1))),
                        op: BinaryOperator::Add,
                        right: Box::new(Expression::Literal(Value::Int(2))),
                    }),
                },
            ),
            alias: "age_plus".to_string(),
        };
        let project = ProjectNode::new(start, vec![column]).expect("project node");
        let node = PlanNodeEnum::Project(project);

        let rule = FoldConstantsRule::new();
        let mut ctx = RewriteContext::new();
        let result = rule.apply(&mut ctx, &node).expect("apply should succeed");
        let result = result.expect("project with foldable sub-expression must change");
        let new_node = result.new_nodes.first().cloned().expect("replacement node");
        match new_node {
            PlanNodeEnum::Project(p) => {
                let folded = p.columns()[0]
                    .expression
                    .expression()
                    .expect("expression")
                    .inner()
                    .clone();
                assert_eq!(
                    folded,
                    Expression::Binary {
                        left: Box::new(Expression::Variable("p.age".to_string())),
                        op: BinaryOperator::Add,
                        right: Box::new(Expression::Literal(Value::Int(3))),
                    },
                    "constant sub-expression folds, the variable part stays"
                );
            }
            other => panic!("expected Project node, got {:?}", other),
        }
    }

    #[test]
    fn test_fold_assign_node_assignments() {
        use crate::planning::plan::core::nodes::graph_operations::graph_operations_node::AssignNode;

        let expr_ctx = Arc::new(ExpressionAnalysisContext::new());
        let start = PlanNodeEnum::Start(StartNode::new());
        let assignments = vec![
            (
                "c".to_string(),
                contextual(
                    &expr_ctx,
                    Expression::Binary {
                        left: Box::new(Expression::Literal(Value::Int(1))),
                        op: BinaryOperator::Add,
                        right: Box::new(Expression::Literal(Value::Int(2))),
                    },
                ),
            ),
            (
                "partial".to_string(),
                contextual(
                    &expr_ctx,
                    Expression::Binary {
                        left: Box::new(Expression::Variable("p.age".to_string())),
                        op: BinaryOperator::Subtract,
                        right: Box::new(Expression::Literal(Value::Int(4))),
                    },
                ),
            ),
        ];
        let assign = AssignNode::new(start, assignments).expect("assign node");
        let node = PlanNodeEnum::Assign(assign);

        let rule = FoldConstantsRule::new();
        let mut ctx = RewriteContext::new();
        let result = rule.apply(&mut ctx, &node).expect("apply should succeed");
        let result = result.expect("assign with constant assignment must fold");
        let new_node = result.new_nodes.first().cloned().expect("replacement node");
        match new_node {
            PlanNodeEnum::Assign(a) => {
                let (_, folded) = &a.assignments()[0];
                assert_eq!(
                    folded.expression().expect("expression").inner().clone(),
                    Expression::Literal(Value::Int(3)),
                    "constant assignment folds"
                );
                let (_, kept) = &a.assignments()[1];
                assert_eq!(
                    kept.expression().expect("expression").inner().clone(),
                    Expression::Binary {
                        left: Box::new(Expression::Variable("p.age".to_string())),
                        op: BinaryOperator::Subtract,
                        right: Box::new(Expression::Literal(Value::Int(4))),
                    },
                    "assignments with variables stay"
                );
            }
            other => panic!("expected Assign node, got {:?}", other),
        }
    }

    #[test]
    fn test_fold_sort_node_items() {
        use crate::planning::plan::core::nodes::operation::sort_node::SortNode;

        let start = PlanNodeEnum::Start(StartNode::new());
        let items = vec![
            crate::planning::plan::core::nodes::operation::sort_node::SortItem::asc(
                Expression::Binary {
                    left: Box::new(Expression::Literal(Value::Int(1))),
                    op: BinaryOperator::Add,
                    right: Box::new(Expression::Literal(Value::Int(2))),
                },
            ),
            crate::planning::plan::core::nodes::operation::sort_node::SortItem::desc(
                Expression::Variable("x".to_string()),
            ),
        ];
        let sort = SortNode::new(start, items).expect("sort node");
        let node = PlanNodeEnum::Sort(sort);

        let rule = FoldConstantsRule::new();
        let mut ctx = RewriteContext::new();
        let result = rule.apply(&mut ctx, &node).expect("apply should succeed");
        let result = result.expect("sort with constant item must fold");
        let new_node = result.new_nodes.first().cloned().expect("replacement node");
        match new_node {
            PlanNodeEnum::Sort(s) => {
                assert_eq!(
                    s.sort_items()[0].expression,
                    Expression::Literal(Value::Int(3)),
                    "constant sort expression folds"
                );
                assert_eq!(
                    s.sort_items()[1].expression,
                    Expression::Variable("x".to_string()),
                    "variable sort expression stays"
                );
            }
            other => panic!("expected Sort node, got {:?}", other),
        }
    }

    #[test]
    fn test_fold_window_node_specs() {
        use crate::planning::plan::core::nodes::graph_operations::window_node::{
            WindowFunctionSpec, WindowNode,
        };

        let start = PlanNodeEnum::Start(StartNode::new());
        let spec = WindowFunctionSpec {
            name: "rank".to_string(),
            args: vec![Expression::Literal(Value::Int(1))],
            partition_by: vec![Expression::Binary {
                left: Box::new(Expression::Literal(Value::Int(10))),
                op: BinaryOperator::Subtract,
                right: Box::new(Expression::Literal(Value::Int(2))),
            }],
            order_by: vec![Expression::Variable("x".to_string())],
            order_desc: vec![false],
        };
        let window = WindowNode::new(start, vec![spec]).expect("window node");
        let node = PlanNodeEnum::Window(window);

        let rule = FoldConstantsRule::new();
        let mut ctx = RewriteContext::new();
        let result = rule.apply(&mut ctx, &node).expect("apply should succeed");
        let result = result.expect("window with constant spec must fold");
        let new_node = result.new_nodes.first().cloned().expect("replacement node");
        match new_node {
            PlanNodeEnum::Window(w) => {
                assert_eq!(
                    w.window_functions()[0].args,
                    vec![Expression::Literal(Value::Int(1))],
                    "already-literal args stay"
                );
                assert_eq!(
                    w.window_functions()[0].partition_by,
                    vec![Expression::Literal(Value::Int(8))],
                    "constant partition expression folds"
                );
                assert_eq!(
                    w.window_functions()[0].order_by,
                    vec![Expression::Variable("x".to_string())],
                    "variable order expression stays"
                );
            }
            other => panic!("expected Window node, got {:?}", other),
        }
    }

    #[test]
    fn test_fold_aggregate_node_filters() {
        use crate::planning::plan::core::nodes::graph_operations::aggregate_node::AggregateNode;
        use graphdb_core::AggregateFunction;

        let start = PlanNodeEnum::Start(StartNode::new());
        let aggregate =
            AggregateNode::new(start, vec!["g".to_string()], vec![AggregateFunction::Count])
                .expect("aggregate node");
        let mut aggregate = aggregate.clone();
        aggregate.set_aggregation_filters(vec![Some(Expression::Binary {
            left: Box::new(Expression::Literal(Value::Int(1))),
            op: BinaryOperator::Equal,
            right: Box::new(Expression::Literal(Value::Int(1))),
        })]);
        let node = PlanNodeEnum::Aggregate(aggregate);

        let rule = FoldConstantsRule::new();
        let mut ctx = RewriteContext::new();
        let result = rule.apply(&mut ctx, &node).expect("apply should succeed");
        let result = result.expect("aggregate with constant filter must fold");
        let new_node = result.new_nodes.first().cloned().expect("replacement node");
        match new_node {
            PlanNodeEnum::Aggregate(a) => {
                assert_eq!(
                    a.aggregation_filters()[0],
                    Some(Expression::Literal(Value::Bool(true))),
                    "constant aggregate filter folds"
                );
            }
            other => panic!("expected Aggregate node, got {:?}", other),
        }
    }

    #[test]
    fn test_fold_inner_join_keys() {
        use crate::planning::plan::core::nodes::join::join_node::InnerJoinNode;

        let expr_ctx = Arc::new(ExpressionAnalysisContext::new());
        let left = PlanNodeEnum::Start(StartNode::new());
        let right = PlanNodeEnum::Start(StartNode::new());
        let hash_keys = vec![contextual(
            &expr_ctx,
            Expression::Binary {
                left: Box::new(Expression::Literal(Value::Int(1))),
                op: BinaryOperator::Add,
                right: Box::new(Expression::Literal(Value::Int(2))),
            },
        )];
        let probe_keys = vec![contextual(
            &expr_ctx,
            Expression::Variable("r.id".to_string()),
        )];
        let join = InnerJoinNode::new(left, right, hash_keys, probe_keys).expect("join node");
        let node = PlanNodeEnum::InnerJoin(join);

        let rule = FoldConstantsRule::new();
        let mut ctx = RewriteContext::new();
        let result = rule.apply(&mut ctx, &node).expect("apply should succeed");
        let result = result.expect("join with constant key must fold");
        let new_node = result.new_nodes.first().cloned().expect("replacement node");
        match new_node {
            PlanNodeEnum::InnerJoin(j) => {
                assert_eq!(
                    j.hash_keys()[0]
                        .expression()
                        .expect("expression")
                        .inner()
                        .clone(),
                    Expression::Literal(Value::Int(3)),
                    "constant join key folds"
                );
                assert_eq!(
                    j.probe_keys()[0]
                        .expression()
                        .expect("expression")
                        .inner()
                        .clone(),
                    Expression::Variable("r.id".to_string()),
                    "variable join key stays"
                );
            }
            other => panic!("expected InnerJoin node, got {:?}", other),
        }
    }

    #[test]
    fn test_fold_does_not_touch_unchanged_nodes() {
        let start = PlanNodeEnum::Start(StartNode::new());
        let items = vec![
            crate::planning::plan::core::nodes::operation::sort_node::SortItem::asc(
                Expression::Variable("x".to_string()),
            ),
        ];
        let sort = SortNode::new(start, items).expect("sort node");
        let node = PlanNodeEnum::Sort(sort);

        let rule = FoldConstantsRule::new();
        let mut ctx = RewriteContext::new();
        let result = rule.apply(&mut ctx, &node).expect("apply should succeed");
        assert!(
            result.is_none(),
            "sort with only variable expressions must not change"
        );
    }
