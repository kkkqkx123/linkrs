use super::*;
use crate::planning::plan::logical::logical_node_enum::LogicalNodeEnum;
use crate::planning::plan::SubPlan;
use graphdb_core::types::operators::AggregateFunction;

use graphdb_core::types::operators::UnaryOperator;
use graphdb_core::Value;

fn prop(var: &str, name: &str) -> Expression {
    Expression::property(Expression::variable(var), name)
}

fn eq(left: Expression, right: Expression) -> Expression {
    Expression::binary(left, BinaryOperator::Equal, right)
}

fn logical_start_subplan() -> SubPlan {
    use crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
    use crate::planning::plan::core::nodes::control_flow::start_node::StartNode;
    use crate::planning::plan::logical::logical_nodes::access::LogicalStartNode;
    SubPlan {
        root: Some(PlanNodeEnum::Start(StartNode::new())),
        tail: None,
        logical_root: Some(LogicalNodeEnum::Start(LogicalStartNode::new())),
    }
}

#[test]
fn group_join_subtree_mirrors_logical_aggregate() {
    use crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;

    let context = Arc::new(ExpressionAnalysisContext::new());
    let sub_plan = logical_start_subplan();
    let out = build_group_join_right_subtree(
        sub_plan,
        &[Expression::variable("a")],
        AggregateFunction::Count,
        &Expression::variable("a"),
        false,
        &context,
    )
    .expect("group-join subtree should build");
    assert!(matches!(out.root, Some(PlanNodeEnum::Aggregate(_))));
    let Some(LogicalNodeEnum::Aggregate(agg)) = out.logical_root else {
        panic!("expected logical Aggregate mirror");
    };
    assert_eq!(agg.aggregation_functions, vec![AggregateFunction::Count]);
    assert!(!agg.group_key_exprs.is_empty());
}

#[test]
fn group_join_subtree_stays_physical_without_upstream_logical() {
    use crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
    use crate::planning::plan::core::nodes::control_flow::start_node::StartNode;

    let context = Arc::new(ExpressionAnalysisContext::new());
    let sub_plan = SubPlan::from_single_node(PlanNodeEnum::Start(StartNode::new()));
    let out = build_group_join_right_subtree(
        sub_plan,
        &[Expression::variable("a")],
        AggregateFunction::Count,
        &Expression::variable("a"),
        false,
        &context,
    )
    .expect("group-join subtree should build");
    assert!(matches!(out.root, Some(PlanNodeEnum::Aggregate(_))));
    assert!(out.logical_root.is_none());
}

#[test]
fn extracts_conjunctive_exists() {
    let cond = Expression::binary(
        eq(prop("t", "age"), Expression::literal(30)),
        BinaryOperator::And,
        Expression::Exists {
            body: Box::new(graphdb_core::types::expr::SubqueryBody {
                patterns: vec!["(p:person)".to_string()],
                where_clause: None,
                return_expr: None,
                id: 0,
            }),
        },
    );
    let mut specs = Vec::new();
    let residual = extract_conjunctive_exists(&cond, &mut specs);
    assert_eq!(specs.len(), 1);
    assert!(!specs[0].negated);
    assert_eq!(
        residual,
        Expression::binary(
            eq(prop("t", "age"), Expression::literal(30)),
            BinaryOperator::And,
            Expression::literal(true),
        )
    );
    assert!(!is_trivially_true(&residual));
}

#[test]
fn extracts_negated_exists() {
    let cond = Expression::unary(
        UnaryOperator::Not,
        Expression::Exists {
            body: Box::new(graphdb_core::types::expr::SubqueryBody {
                patterns: vec!["(p:person)".to_string()],
                where_clause: None,
                return_expr: None,
                id: 0,
            }),
        },
    );
    let mut specs = Vec::new();
    let residual = extract_conjunctive_exists(&cond, &mut specs);
    assert_eq!(specs.len(), 1);
    assert!(specs[0].negated);
    assert!(is_trivially_true(&residual));
}

#[test]
fn extracts_not_in_as_negated_spec() {
    // `x NOT IN { … }` parses as `NOT (x IN { … })`.
    let cond = Expression::unary(
        UnaryOperator::Not,
        Expression::in_subquery(
            Expression::variable("t"),
            graphdb_core::types::expr::SubqueryBody {
                patterns: vec!["(p:person)".to_string()],
                where_clause: None,
                return_expr: Some(Box::new(prop("p", "name"))),
                id: 0,
            },
            false,
        ),
    );
    let mut specs = Vec::new();
    let residual = extract_conjunctive_exists(&cond, &mut specs);
    assert_eq!(specs.len(), 1);
    assert!(specs[0].negated);
    assert_eq!(specs[0].left_expr, Some(Expression::variable("t")));
    assert!(is_trivially_true(&residual));
}

#[test]
fn leaves_non_conjunctive_exists_untouched() {
    // EXISTS under OR must not be extracted.
    let inner = Expression::Exists {
        body: Box::new(graphdb_core::types::expr::SubqueryBody {
            patterns: vec!["(p:person)".to_string()],
            where_clause: None,
            return_expr: None,
            id: 0,
        }),
    };
    let cond = Expression::binary(
        eq(prop("t", "age"), Expression::literal(30)),
        BinaryOperator::Or,
        inner.clone(),
    );
    let mut specs = Vec::new();
    let residual = extract_conjunctive_exists(&cond, &mut specs);
    assert!(specs.is_empty());
    assert_eq!(residual, cond);
}

#[test]
fn extracts_keys_from_equality() {
    let inner: HashSet<String> = ["p".to_string()].into_iter().collect();
    let conditions = vec![eq(prop("p", "name"), prop("t", "name"))];
    let (hash, probe, residual) = extract_keys(&conditions, &inner).expect("keys");
    assert_eq!(hash, vec![prop("t", "name")]);
    assert_eq!(probe, vec![prop("p", "name")]);
    assert!(residual.is_empty());
}

#[test]
fn keeps_inner_only_condition_as_residual() {
    let inner: HashSet<String> = ["p".to_string()].into_iter().collect();
    let conditions = vec![Expression::binary(
        prop("p", "age"),
        BinaryOperator::GreaterThan,
        Expression::literal(30),
    )];
    let (hash, probe, residual) = extract_keys(&conditions, &inner).expect("keys");
    assert!(hash.is_empty());
    assert!(probe.is_empty());
    assert_eq!(residual.len(), 1);
}

#[test]
fn keeps_outer_reference_in_correlated_residual() {
    let inner: HashSet<String> = ["p".to_string()].into_iter().collect();
    let condition = Expression::binary(
        prop("p", "age"),
        BinaryOperator::GreaterThan,
        prop("t", "age"),
    );
    let conditions = vec![condition.clone()];
    let (hash, probe, residual) = extract_keys(&conditions, &inner).expect("keys");
    assert!(hash.is_empty());
    assert!(probe.is_empty());
    assert_eq!(residual, vec![condition]);
}

#[test]
fn split_correlated_separates_inner_and_outer_residuals() {
    let inner: HashSet<String> = ["p".to_string()].into_iter().collect();
    let inner_only = Expression::binary(
        prop("p", "age"),
        BinaryOperator::GreaterThan,
        Expression::literal(30),
    );
    let correlated = Expression::binary(
        prop("p", "age"),
        BinaryOperator::GreaterThan,
        prop("t", "age"),
    );
    let (inner_residual, correlated_residual) =
        split_correlated(&[inner_only.clone(), correlated.clone()], &inner);
    assert_eq!(inner_residual, vec![inner_only]);
    assert_eq!(correlated_residual, vec![correlated]);
}

#[test]
fn in_synthesizes_equality_key() {
    let inner: HashSet<String> = ["p".to_string()].into_iter().collect();
    let conditions = vec![eq(
        Expression::variable("t"),
        Expression::property(Expression::variable("p"), "name"),
    )];
    let (hash, probe, _) = extract_keys(&conditions, &inner).expect("keys");
    assert_eq!(hash, vec![Expression::variable("t")]);
    assert_eq!(probe, vec![prop("p", "name")]);
}

#[test]
fn plans_correlated_exists_as_mark_join() {
    use graphdb_core::types::expr::SubqueryBody;

    let spec = ExistsSpec {
        body: SubqueryBody {
            patterns: vec!["(p:person)".to_string()],
            where_clause: Some(Box::new(Expression::binary(
                prop("p", "age"),
                BinaryOperator::GreaterThan,
                prop("t", "age"),
            ))),
            return_expr: None,
            id: 0,
        },
        negated: false,
        left_expr: None,
    };

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

    let outer_col_names = vec!["t".to_string(), "t.name".to_string()];
    let planned = plan_subquery(&spec, &qctx, 1, "default", &outer_col_names)
        .expect("correlated subquery should plan");

    // A simple single-table subquery with a non-equi correlated residual
    // routes to the Mark-Join form (SemiJoin with a residual condition),
    // not the per-row CorrelatedApply.
    assert!(!planned.correlated, "Mark-Join is not a CorrelatedApply");
    assert!(planned.hash_keys.is_empty());
    assert!(planned.probe_keys.is_empty());
    let condition = planned
        .mark_join_condition
        .as_ref()
        .expect("non-equi residual becomes the Mark-Join condition")
        .get_expression()
        .expect("condition registered");
    // The condition references the outer variable `t` (left side) and the
    // inner variable `p` (right side).
    let cond_vars: HashSet<String> = condition.get_variables().into_iter().collect();
    assert!(
        cond_vars.contains("t"),
        "Mark-Join condition must reference the outer variable, got vars {:?}",
        cond_vars
    );
    assert!(
        cond_vars.contains("p"),
        "Mark-Join condition must reference the inner variable, got vars {:?}",
        cond_vars
    );
}

// ── scalar aggregate Group-Join planning ────────────────────

fn test_qctx() -> Arc<QueryContext> {
    Arc::new(crate::QueryContext::new(Arc::new(
        crate::QueryRequestContext {
            session_id: None,
            user_name: None,
            space_name: None,
            query: String::new(),
            parameters: std::collections::HashMap::new(),
            ..Default::default()
        },
    )))
}

#[test]
fn plans_correlated_scalar_aggregate_as_group_join() {
    use crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
    use graphdb_core::types::expr::SubqueryBody;

    // `t.city IN { (p:person) WHERE p.city = t.city RETURN max(p.age) }`
    // at an expression position: the correlated part reduces to an equi
    // key and the right subtree is a simple single-table shape, so the
    // subquery decorrelates into a Group-Join build side (Aggregate over
    // the probe key).
    let body = SubqueryBody {
        patterns: vec!["(p:person)".to_string()],
        where_clause: Some(Box::new(eq(prop("p", "city"), prop("t", "city")))),
        return_expr: Some(Box::new(Expression::Aggregate {
            func: graphdb_core::types::operators::AggregateFunction::Max,
            args: vec![prop("p", "age")],
            distinct: false,
            filter: None,
        })),
        id: 0,
    };
    let expr = Expression::in_subquery(prop("t", "city"), body, false);

    let mut id_alloc = SubqueryIdAllocator::new();
    let (_, planned) = plan_expression_subqueries(
        expr,
        &test_qctx(),
        1,
        "default",
        &["t".to_string()],
        &mut id_alloc,
    )
    .expect("scalar aggregate subquery should plan");
    assert_eq!(planned.len(), 1);
    let planned = &planned[0];

    let gj = planned.group_join.as_ref().expect("Group-Join planned");
    assert!(!planned.correlated, "Group-Join is not a CorrelatedApply");
    assert!(planned.mark_join_condition.is_none());
    assert_eq!(gj.key_columns, 1);
    assert_eq!(
        gj.function,
        graphdb_core::types::operators::AggregateFunction::Max
    );
    assert!(!gj.distinct);
    // The outer-side hash key references the outer variable `t`.
    assert_eq!(gj.hash_keys.len(), 1);
    assert_eq!(gj.hash_keys[0].get_variables(), vec!["t".to_string()]);

    // Right subtree root = Aggregate -> Project -> pattern plan.
    let root = planned.plan.root().as_ref().expect("right subtree root");
    let PlanNodeEnum::Aggregate(aggregate) = root else {
        panic!("expected Aggregate root, got {}", root.type_name());
    };
    assert_eq!(aggregate.group_keys(), &["__gj_key_0".to_string()]);
    assert_eq!(aggregate.aggregation_functions().len(), 1);
    assert_eq!(
        aggregate.aggregation_args()[0],
        vec![prop("p", "age")],
        "aggregate argument preserved"
    );
}

#[test]
fn non_equi_scalar_aggregate_keeps_correlated_apply_fallback() {
    use crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
    use graphdb_core::types::expr::SubqueryBody;

    // Non-equi correlation cannot reduce to equi keys: the extracted-key
    // attempt is restored and the per-row CorrelatedApply fallback kept.
    let body = SubqueryBody {
        patterns: vec!["(p:person)".to_string()],
        where_clause: Some(Box::new(Expression::binary(
            prop("p", "age"),
            BinaryOperator::GreaterThan,
            prop("t", "age"),
        ))),
        return_expr: Some(Box::new(Expression::Aggregate {
            func: graphdb_core::types::operators::AggregateFunction::Count,
            args: vec![Expression::Literal(Value::string("*"))],
            distinct: false,
            filter: None,
        })),
        id: 0,
    };
    let expr = Expression::exists(body);

    let mut id_alloc = SubqueryIdAllocator::new();
    let (_, planned) = plan_expression_subqueries(
        expr,
        &test_qctx(),
        1,
        "default",
        &["t".to_string()],
        &mut id_alloc,
    )
    .expect("non-equi aggregate subquery should plan");
    assert_eq!(planned.len(), 1);
    let planned = &planned[0];
    assert!(planned.group_join.is_none(), "fallback to CorrelatedApply");
    assert!(planned.correlated, "CorrelatedApply routing flag set");
    // The correlated condition survived the restore: the right subtree is
    // Project(return) -> Filter -> CrossJoin(Argument, plan).
    let root = planned.plan.root().as_ref().expect("right subtree root");
    assert_eq!(root.type_name(), "Project", "RETURN projection on top");
    let PlanNodeEnum::Project(project) = root else {
        panic!("expected Project root");
    };
    let input = project.dependencies().first().expect("project input");
    assert_eq!(input.type_name(), "Filter", "correlated filter below");
}

#[test]
fn filtered_aggregate_return_skips_group_join() {
    use graphdb_core::types::expr::SubqueryBody;

    // An aggregate with a FILTER clause is outside the Group-Join shape:
    // the RETURN expression keeps its plain projection.
    let body = SubqueryBody {
        patterns: vec!["(p:person)".to_string()],
        where_clause: None,
        return_expr: Some(Box::new(Expression::Aggregate {
            func: graphdb_core::types::operators::AggregateFunction::Count,
            args: vec![prop("p", "age")],
            distinct: false,
            filter: Some(Box::new(Expression::binary(
                prop("p", "age"),
                BinaryOperator::GreaterThan,
                Expression::literal(30),
            ))),
        })),
        id: 0,
    };
    let expr = Expression::exists(body);

    let mut id_alloc = SubqueryIdAllocator::new();
    let (_, planned) = plan_expression_subqueries(
        expr,
        &test_qctx(),
        1,
        "default",
        &["t".to_string()],
        &mut id_alloc,
    )
    .expect("filtered aggregate subquery should plan");
    assert_eq!(planned.len(), 1);
    assert!(planned[0].group_join.is_none());
    let root = planned[0].plan.root().as_ref().expect("right subtree root");
    assert_eq!(root.type_name(), "Project", "plain RETURN projection");
}

#[test]
fn plans_complex_correlated_exists_as_correlated_apply() {
    use crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
    use graphdb_core::types::expr::SubqueryBody;

    // A multi-pattern (cross-joined) correlated subquery is not a simple
    // single-table shape, so it keeps the per-row CorrelatedApply
    // fallback instead of the Mark-Join form.
    let spec = ExistsSpec {
        body: SubqueryBody {
            patterns: vec!["(p:person)".to_string(), "(q:person)".to_string()],
            where_clause: Some(Box::new(Expression::binary(
                prop("p", "age"),
                BinaryOperator::GreaterThan,
                prop("t", "age"),
            ))),
            return_expr: None,
            id: 0,
        },
        negated: false,
        left_expr: None,
    };

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

    let outer_col_names = vec!["t".to_string(), "t.name".to_string()];
    let planned = plan_subquery(&spec, &qctx, 1, "default", &outer_col_names)
        .expect("correlated subquery should plan");

    assert!(
        planned.correlated,
        "cross-joined subquery routes to CorrelatedApply"
    );
    assert!(planned.mark_join_condition.is_none());

    // Right subtree root = Filter over CrossJoin(Argument, pattern plan).
    let root = planned
        .plan
        .root()
        .as_ref()
        .expect("right subtree has a root");
    let PlanNodeEnum::Filter(filter_node) = root else {
        panic!("expected Filter root, got {}", root.type_name());
    };
    let Some(PlanNodeEnum::CrossJoin(cross_node)) = filter_node.dependencies().first() else {
        panic!("expected CrossJoin below the correlated Filter");
    };
    let PlanNodeEnum::Argument(argument) = cross_node.left_input() else {
        panic!("expected Argument as the cross join left input");
    };
    assert_eq!(
        argument.col_names(),
        outer_col_names.as_slice(),
        "Argument col_names mirror the outer layout"
    );
    assert!(
        !matches!(cross_node.right_input(), PlanNodeEnum::Argument(_)),
        "right input of the cross join is the subquery pattern plan"
    );
}

#[test]
fn nested_correlated_exists_wraps_inner_mark_join() {
    use crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
    use graphdb_core::types::expr::SubqueryBody;

    // Outer EXISTS over `(p:person)` whose WHERE contains a nested EXISTS
    // correlated against `p` (`q.age > p.age`). The nested subquery is a
    // simple single-table shape, so it becomes a Mark-Join (SemiJoin).
    let inner_body = SubqueryBody {
        patterns: vec!["(q:person)".to_string()],
        where_clause: Some(Box::new(Expression::binary(
            prop("q", "age"),
            BinaryOperator::GreaterThan,
            prop("p", "age"),
        ))),
        return_expr: None,
        id: 0,
    };
    let outer_spec = ExistsSpec {
        body: SubqueryBody {
            patterns: vec!["(p:person)".to_string()],
            where_clause: Some(Box::new(Expression::Exists {
                body: Box::new(inner_body),
            })),
            return_expr: None,
            id: 0,
        },
        negated: false,
        left_expr: None,
    };

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

    let planned = plan_subquery(&outer_spec, &qctx, 1, "default", &["t".to_string()])
        .expect("outer EXISTS should plan");
    // The outer subquery itself references no outer variables.
    assert!(!planned.correlated);
    // The nested correlated EXISTS wraps the subquery base plan as a
    // Mark-Join (SemiJoin with a residual condition).
    assert!(
        matches!(
            planned.plan.root().as_ref(),
            Some(PlanNodeEnum::SemiJoin(_))
        ),
        "nested correlated EXISTS must be planned as a Mark-Join SemiJoin"
    );
}

#[test]
fn in_with_correlated_where_keeps_synthesized_equality_in_mark_join_condition() {
    use graphdb_core::types::expr::SubqueryBody;

    let spec = ExistsSpec {
        body: SubqueryBody {
            patterns: vec!["(p:person)".to_string()],
            where_clause: Some(Box::new(Expression::binary(
                prop("p", "age"),
                BinaryOperator::GreaterThan,
                prop("t", "age"),
            ))),
            return_expr: Some(Box::new(prop("p", "age"))),
            id: 0,
        },
        negated: false,
        left_expr: Some(prop("t", "age")),
    };

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

    let planned = plan_subquery(&spec, &qctx, 1, "default", &["t".to_string()])
        .expect("correlated IN should plan");
    // Simple single-table shape: the correlated IN routes to Mark-Join.
    assert!(!planned.correlated);
    // The Mark-Join condition must include the synthesized
    // `t.age = p.age` equality (IN semantics) alongside `p.age > t.age`.
    let condition = planned
        .mark_join_condition
        .as_ref()
        .expect("Mark-Join condition registered")
        .get_expression()
        .expect("condition resolved");
    let conjuncts = {
        let mut out = Vec::new();
        collect_and_conjuncts(&condition, &mut out);
        out
    };
    assert_eq!(conjuncts.len(), 2);
    assert!(
        conjuncts.contains(&eq(prop("t", "age"), prop("p", "age"))),
        "IN synthesized equality joins the Mark-Join condition"
    );
}

#[test]
fn literal_true_is_trivially_true() {
    assert!(is_trivially_true(&Expression::literal(true)));
    assert!(is_trivially_true(&Expression::binary(
        Expression::literal(true),
        BinaryOperator::And,
        Expression::literal(true),
    )));
    assert!(!is_trivially_true(&Expression::literal(Value::Int(1))));
}

// ── expression-level subquery collection ───────────────────

fn body() -> graphdb_core::types::expr::SubqueryBody {
    graphdb_core::types::expr::SubqueryBody {
        id: 0,
        patterns: vec!["(p:person)".to_string()],
        where_clause: None,
        return_expr: None,
    }
}

fn exists() -> Expression {
    Expression::exists(body())
}

fn in_subq(left: Expression) -> Expression {
    Expression::in_subquery(left, body(), false)
}

/// Assert that `expr` contains exactly `expected` expression-level
/// subqueries and that every body received a unique, monotonically
/// allocated id.
fn assert_collected(mut expr: Expression, expected: usize) {
    let mut alloc = SubqueryIdAllocator::new();
    let bodies = collect_expression_subqueries(&mut expr, &mut alloc);
    assert_eq!(bodies.len(), expected, "collected subqueries");
    let mut ids: Vec<u64> = bodies.iter().map(|b| b.id).collect();
    ids.sort_unstable();
    let unique: std::collections::HashSet<u64> = ids.iter().copied().collect();
    assert_eq!(unique.len(), ids.len(), "ids must be unique");
    if !ids.is_empty() {
        assert_eq!(ids[0], 0, "ids start at 0");
        for w in ids.windows(2) {
            assert_eq!(w[1], w[0] + 1, "ids are contiguous");
        }
    }
    // Bodies must carry their assigned ids in the mutated expression.
    // Traverse manually: `Expression::find_all` goes through
    // `children()`, which hides the aggregate filter.
    fn count_subqueries(expr: &Expression) -> usize {
        let own = matches!(expr, Expression::Exists { .. } | Expression::In { .. }) as usize;
        let children = match expr {
            Expression::Aggregate { args, filter, .. } => {
                let mut c: Vec<&Expression> = args.iter().collect();
                if let Some(f) = filter {
                    c.push(f.as_ref());
                }
                c
            }
            Expression::Exists { body } => {
                let mut c: Vec<&Expression> = Vec::new();
                if let Some(w) = &body.where_clause {
                    c.push(w);
                }
                if let Some(r) = &body.return_expr {
                    c.push(r);
                }
                c
            }
            _ => expr.children(),
        };
        own + children.iter().map(|c| count_subqueries(c)).sum::<usize>()
    }
    assert_eq!(
        count_subqueries(&expr),
        expected,
        "expression tree retains subqueries"
    );
}

#[test]
fn collects_subquery_at_top_level() {
    assert_collected(exists(), 1);
    assert_collected(in_subq(Expression::variable("t")), 1);
}

#[test]
fn collects_binary_left_and_right() {
    let expr = Expression::binary(
        exists(),
        BinaryOperator::Or,
        in_subq(Expression::variable("t")),
    );
    assert_collected(expr, 2);
}

#[test]
fn collects_unary_operand() {
    assert_collected(Expression::unary(UnaryOperator::Not, exists()), 1);
}

#[test]
fn collects_inside_case() {
    let expr = Expression::Case {
        test_expr: Some(Box::new(exists())),
        conditions: vec![(exists(), exists())],
        default: Some(Box::new(in_subq(Expression::variable("t")))),
    };
    assert_collected(expr, 4);
}

#[test]
fn collects_inside_list_and_map() {
    assert_collected(
        Expression::List(vec![exists(), in_subq(Expression::variable("t"))]),
        2,
    );
    assert_collected(
        Expression::Map(vec![
            ("a".into(), exists()),
            ("b".into(), in_subq(Expression::variable("t"))),
        ]),
        2,
    );
}

#[test]
fn collects_inside_type_cast_subscript_range_path() {
    assert_collected(
        Expression::cast(exists(), graphdb_core::types::DataType::Bool),
        1,
    );
    assert_collected(Expression::subscript(exists(), Expression::literal(0)), 1);
    assert_collected(
        Expression::Range {
            collection: Box::new(exists()),
            start: Some(Box::new(in_subq(Expression::variable("t")))),
            end: None,
        },
        2,
    );
    assert_collected(Expression::Path(vec![exists()]), 1);
}

#[test]
fn collects_inside_list_comprehension() {
    let expr = Expression::ListComprehension {
        variable: "x".to_string(),
        source: Box::new(exists()),
        filter: Some(Box::new(in_subq(Expression::variable("t")))),
        map: Some(Box::new(Expression::function(
            "upper".to_string(),
            vec![exists()],
        ))),
    };
    assert_collected(expr, 3);
}

#[test]
fn collects_inside_function_and_aggregate_args() {
    assert_collected(
        Expression::function(
            "f".to_string(),
            vec![exists(), in_subq(Expression::variable("t"))],
        ),
        2,
    );
    let agg = Expression::Aggregate {
        func: graphdb_core::types::operators::AggregateFunction::Count,
        args: vec![exists()],
        distinct: false,
        filter: Some(Box::new(in_subq(Expression::variable("t")))),
    };
    assert_collected(agg, 2);
}

#[test]
fn collects_inside_reduce_and_window_function() {
    assert_collected(
        Expression::reduce(
            "acc",
            exists(),
            "x",
            in_subq(Expression::variable("t")),
            Expression::variable("acc"),
        ),
        2,
    );
    let window = Expression::WindowFunction {
        name: "row_number".to_string(),
        args: vec![exists()],
        over_partition_by: vec![in_subq(Expression::variable("t"))],
        over_order_by: vec![],
        over_order_desc: vec![],
    };
    assert_collected(window, 2);
}

#[test]
fn collects_inside_label_tag_property_and_path_build() {
    assert_collected(
        Expression::LabelTagProperty {
            tag: Box::new(exists()),
            property: "name".to_string(),
        },
        1,
    );
    assert_collected(
        Expression::PathBuild(vec![exists(), in_subq(Expression::variable("t"))]),
        2,
    );
}

#[test]
fn collects_in_left_operand_of_in() {
    let expr = Expression::in_subquery(exists(), body(), false);
    assert_collected(expr, 2);
}

#[test]
fn does_not_descend_into_subquery_bodies() {
    // A subquery whose WHERE / RETURN themselves contain EXISTS / IN must
    // not be collected at expression level: those are compiled
    // recursively by the subquery planner.
    let inner = graphdb_core::types::expr::SubqueryBody {
        id: 0,
        patterns: vec!["(p:person)".to_string()],
        where_clause: Some(Box::new(Expression::binary(
            Expression::variable("p"),
            BinaryOperator::Or,
            exists(),
        ))),
        return_expr: Some(Box::new(in_subq(Expression::variable("p")))),
    };
    let mut alloc = SubqueryIdAllocator::new();
    let mut expr = Expression::exists(inner);
    let bodies = collect_expression_subqueries(&mut expr, &mut alloc);
    assert_eq!(bodies.len(), 1, "only the outer subquery is collected");
}

#[test]
fn ids_are_reassigned_on_replanning() {
    let mut first_alloc = SubqueryIdAllocator::new();
    let mut second_alloc = SubqueryIdAllocator::new();
    let mut expr_a = Expression::binary(exists(), BinaryOperator::Or, exists());
    let mut expr_b = Expression::binary(exists(), BinaryOperator::Or, exists());
    let ids_a: Vec<u64> = collect_expression_subqueries(&mut expr_a, &mut first_alloc)
        .iter()
        .map(|b| b.id)
        .collect();
    let ids_b: Vec<u64> = collect_expression_subqueries(&mut expr_b, &mut second_alloc)
        .iter()
        .map(|b| b.id)
        .collect();
    assert_eq!(ids_a, vec![0, 1]);
    assert_eq!(ids_b, vec![0, 1], "fresh planning re-assigns ids from 0");
}

#[test]
fn plan_expression_subqueries_rejects_and_accepts() {
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
    let mut alloc = SubqueryIdAllocator::new();
    let outer = vec!["t".to_string()];

    // No expression-level subquery: unchanged expression, no runners.
    let expr = Expression::binary(
        prop("t", "age"),
        BinaryOperator::GreaterThan,
        Expression::literal(30),
    );
    let (out, planned) =
        plan_expression_subqueries(expr.clone(), &qctx, 1, "default", &outer, &mut alloc)
            .expect("plain expression passes");
    assert_eq!(out, expr);
    assert!(planned.is_empty());

    // EXISTS / IN at expression level: compiled into
    // standalone sub-plans with stable ids. The rewritten expression
    // carries the ids; the runners match by id.
    for expr in [exists(), in_subq(Expression::variable("t"))] {
        let (rewritten, planned) =
            plan_expression_subqueries(expr, &qctx, 1, "default", &outer, &mut alloc)
                .expect("expression-level subquery compiles");
        assert_eq!(planned.len(), 1, "one subquery compiled");
        let body_id = match &rewritten {
            Expression::Exists { body } | Expression::In { subquery: body, .. } => body.id,
            other => panic!("expected a top-level subquery expression, got {:?}", other),
        };
        assert_eq!(planned[0].id, body_id, "runner id matches the body id");
        assert!(!planned[0].correlated, "no outer references in test body");
        assert!(planned[0].plan.root().is_some(), "sub-plan has a root");
    }

    // Ids are re-allocated per planning pass and unique within a pass.
    let (_, planned) =
        plan_expression_subqueries(exists(), &qctx, 1, "default", &outer, &mut alloc)
            .expect("re-plan succeeds");
    assert_eq!(planned.len(), 1);
}
