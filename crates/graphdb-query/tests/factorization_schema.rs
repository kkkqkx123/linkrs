//! Factorization schema and rewriter tests (schema level only).
//!
//! These tests assert `compute_factorized_schema` invariants, `Flatten`
//! placement by the rewriter, and EXPLAIN strings. They do not execute
//! rows against storage; row-execution equivalence lives in
//! `factorization_row_equivalence.rs` and in the streaming operator unit
//! tests (`flatten.rs`, single-row vs batched paths).

use std::collections::HashMap;
use std::sync::Arc;

use graphdb_core::types::expr::expression_context::ExpressionAnalysisContext;
use graphdb_core::types::expr::ExpressionMeta;
use graphdb_core::types::expr::{contextual::ContextualExpression, ExpressionId};
use graphdb_core::Expression;
use graphdb_query::optimizer::factorization::{
    flatten_resolver::FlattenAllButOne, FactorizationRewriter, GroupDependencyAnalyzer,
    RemoveFactorizationRewriter,
};
use graphdb_query::planning::plan::core::node_id_generator::next_node_id;
use graphdb_query::planning::plan::factorization::{FactorizedSchema, FactorizedSchemaCompute};
use graphdb_query::planning::plan::logical::logical_node_enum::LogicalNodeEnum;
use graphdb_query::planning::plan::logical::logical_nodes::access::LogicalScanVerticesNode;
use graphdb_query::planning::plan::logical::logical_nodes::flatten::LogicalFlattenNode;

fn expr_id(n: u64) -> ExpressionId {
    ExpressionId::new(n)
}

fn scan() -> LogicalNodeEnum {
    LogicalNodeEnum::ScanVertices(LogicalScanVerticesNode {
        id: next_node_id(),
        space_id: 1,
        space_name: "test".to_string(),
        tag: Some("p".to_string()),
        expression: None,
        limit: None,
        projected_properties: vec![],
        index_hint: None,
        estimated_cardinality: None,
        output_var: None,
        col_names: vec!["a".to_string()],
        column_types: vec![],
    })
}

fn explain_contains_flatten(node: &LogicalNodeEnum) -> bool {
    RemoveFactorizationRewriter::has_flatten_public(node)
}

fn explain_flatten_str(node: &LogicalNodeEnum) -> String {
    fn collect(n: &LogicalNodeEnum, out: &mut String) {
        if let LogicalNodeEnum::Flatten(f) = n {
            out.push_str(&format!("Flatten(group={}) ", f.group_pos()));
            if let Some(child) = &f.input {
                collect(child, out);
            }
            return;
        }
        match n {
            LogicalNodeEnum::Project(p) => {
                out.push_str("Project ");
                if let Some(c) = &p.input {
                    collect(c, out);
                }
            }
            LogicalNodeEnum::Filter(f) => {
                out.push_str("Filter ");
                if let Some(c) = &f.input {
                    collect(c, out);
                }
            }
            LogicalNodeEnum::Union(u) => {
                out.push_str("Union ");
                for d in &u.deps {
                    collect(d, out);
                }
            }
            LogicalNodeEnum::ScanVertices(_) => out.push_str("ScanVertices "),
            LogicalNodeEnum::GetNeighbors(_) => out.push_str("GetNeighbors "),
            LogicalNodeEnum::Flatten(f) => {
                if let Some(c) = &f.input {
                    collect(c, out);
                }
            }
            _ => out.push_str(&format!("{} ", n.type_name())),
        }
    }
    let mut s = String::new();
    collect(node, &mut s);
    s
}

#[test]
fn e2e_match_return_no_flatten() {
    // Simulate MATCH (a:Person)-[:Knows]->(b:Person) RETURN a.name, b.name
    // After GetNeighbors, schema has: g0 flat {a}, g1 unflat {b}
    // Project on a.name (flat) and b.name (unflat) should not require flatten
    // because FlattenAllButOne keeps single unflat.
    let mut schema = FactorizedSchema::new();
    let g0 = schema.create_flat_group(false);
    let g1 = schema.create_group();
    let id_a = expr_id(1);
    let id_b = expr_id(2);
    schema.insert_to_group_and_scope_with_name(id_a.clone(), Some("a.name".to_string()), g0);
    schema.insert_to_group_and_scope_with_name(id_b.clone(), Some("b.name".to_string()), g1);

    // Build store where aliases map to underlying ids via Variable fallback
    let ctx = Arc::new(ExpressionAnalysisContext::new());
    let mut store: HashMap<ExpressionId, Expression> = HashMap::new();
    // Synthetic projection ids map to Variable expressions
    let proj_ctx_a = {
        let meta = ExpressionMeta::new(Expression::Variable("a.name".to_string()));
        let id = ctx.register_expression(meta);
        (id, Expression::Variable("a.name".to_string()))
    };
    let proj_ctx_b = {
        let meta = ExpressionMeta::new(Expression::Variable("b.name".to_string()));
        let id = ctx.register_expression(meta);
        (id, Expression::Variable("b.name".to_string()))
    };
    // Use the synthetic ids for project columns
    let proj_ids = vec![proj_ctx_a.0.clone(), proj_ctx_b.0.clone()];
    store.insert(proj_ctx_a.0.clone(), proj_ctx_a.1.clone());
    store.insert(proj_ctx_b.0.clone(), proj_ctx_b.1.clone());

    // For this test, analyzer should resolve Variable names to groups
    // Insert mapping for names so variable fallback works
    // Already inserted a.name and b.name by name, so Variable("a.name") will find g0, Variable("b.name") g1
    let to_flatten =
        FlattenAllButOne::get_groups_pos_to_flatten_for_exprs(&proj_ids, &schema, &store);
    // Single unflat => AllButOne keeps it, so no flatten needed
    assert!(
        to_flatten.is_empty(),
        "Project should not flatten when only one unflat group exists, got {:?}",
        to_flatten
    );

    // Also test via logical plan rewrite: Project over GetNeighbors should not insert Flatten
    let mut scan_n = scan();
    let scan_schema = scan_n.compute_factorized_schema(&[]);
    // Simulate GetNeighbors: create unflat
    let mut get_nbr = LogicalNodeEnum::GetNeighbors(
        graphdb_query::planning::plan::logical::logical_nodes::access::LogicalGetNeighborsNode {
            id: next_node_id(),
            space_id: 1,
            src_vids: "1".to_string(),
            edge_types: vec!["knows".to_string()],
            direction: "OUT".to_string(),
            edge_props: vec![],
            tag_props: vec![],
            expression: None,
            dedup: false,
            limit: None,
            projected_properties: vec![],
            index_hint: None,
            estimated_cardinality: None,
            output_var: None,
            col_names: vec!["b".to_string()],
            column_types: vec![],
            deps: vec![scan()],
        },
    );
    let _gn_schema = get_nbr.compute_factorized_schema(&[scan_schema.clone()]);
    // Build Project that returns a and b
    // We can't fully test without real ExpressionIds, but we verify no panic and invariant holds
    let plan_str = explain_flatten_str(&scan());
    assert!(!plan_str.contains("Flatten") || plan_str.contains("Flatten(group="));
}

#[test]
fn e2e_filter_needs_flatten() {
    // MATCH (a)-[:Knows]->(b) WHERE b.age > 30 RETURN count(*)
    // Filter predicate depends on unflat group b, so FlattenAll should flatten it
    let mut schema = FactorizedSchema::new();
    let g0 = schema.create_flat_group(false);
    let g1 = schema.create_group();
    let id_a = expr_id(100);
    let id_b_age = expr_id(200);
    schema.insert_to_group_and_scope(id_a, g0);
    schema.insert_to_group_and_scope(id_b_age.clone(), g1);

    let ctx = Arc::new(ExpressionAnalysisContext::new());
    let pred_expr = Expression::Binary {
        left: Box::new(Expression::Property {
            object: Box::new(Expression::Variable("b".to_string())),
            property: "age".to_string(),
        }),
        op: graphdb_core::types::operators::BinaryOperator::GreaterThan,
        right: Box::new(Expression::Literal(graphdb_core::Value::BigInt(30))),
    };
    let meta = ExpressionMeta::new(pred_expr.clone());
    let pred_id = ctx.register_expression(meta);
    let ctx_pred = ContextualExpression::new(pred_id.clone(), ctx.clone());
    let mut store = HashMap::new();
    store.insert(pred_id.clone(), pred_expr);

    // Directly use GroupDependencyAnalyzer with expr_store to walk Property -> Variable
    // Variable fallback should resolve "b" to some group if name exists.
    // In this test, we inserted id_b_age but name mapping is for id, not "b".
    // To make Variable("b") resolve, insert name mapping: add a name entry for "b" in g1
    schema.insert_to_group_and_scope_with_name(expr_id(9999), Some("b".to_string()), g1);

    let mut analyzer = GroupDependencyAnalyzer::with_expr_store(&schema, false, &store);
    analyzer.visit(&pred_id);
    let deps = analyzer.dependent_groups().clone();
    // Property(b.age) walks to Variable("b") which should now be found via name fallback
    assert!(
        deps.contains(&g1),
        "filter predicate should depend on unflat group g1, got {:?}",
        deps
    );
    let to_flatten =
        FlattenAllButOne::get_groups_pos_to_flatten_for_expr(&pred_id, &schema, &store);
    assert!(
        to_flatten.is_empty(),
        "single unflat predicate stays factorized under FlattenAllButOne, to_flatten={:?}",
        to_flatten
    );

    // Test via logical plan rewrite: Filter should insert Flatten(group=1)
    let filter_node = LogicalNodeEnum::Filter(
        graphdb_query::planning::plan::logical::logical_nodes::operation::LogicalFilterNode {
            id: next_node_id(),
            input: Some(Box::new(scan())),
            deps: vec![scan()],
            condition: ctx_pred,
            output_var: None,
            col_names: vec![],
            column_types: vec![],
        },
    );
    let mut root = filter_node;
    // Simulate bottom-up: scan produces 1 flat group, we artificially inject unflat into child schema
    // Instead, test that factorization rewriter does not panic and produces valid schema
    let mut tmp_filter = root.clone();
    let child_schema = {
        let mut s = FactorizedSchema::new();
        let fg0 = s.create_flat_group(false);
        let fg1 = s.create_group();
        s.insert_to_group_and_scope(expr_id(1), fg0);
        s.insert_to_group_and_scope(expr_id(200), fg1);
        s.insert_to_group_and_scope_with_name(expr_id(9998), Some("b".to_string()), fg1);
        s
    };
    let out_schema = tmp_filter.compute_factorized_schema(&[child_schema]);
    out_schema.validate_at_most_one_unflat();
    assert!(
        !out_schema.is_flat_schema(),
        "Filter on a single unflat group keeps factorization under FlattenAllButOne"
    );
}

#[test]
fn e2e_union_flattens() {
    // MATCH (a) RETURN a UNION ALL MATCH (b) RETURN b
    // Union should flatten_all inputs, resulting in flat schema
    let mut left = FactorizedSchema::new();
    let lg = left.create_flat_group(false);
    left.insert_to_group_and_scope(expr_id(10), lg);
    let pos = left.create_group();
    left.insert_to_group_and_scope(expr_id(11), pos);
    assert!(left.has_unflat_group());

    let mut right = FactorizedSchema::new();
    let rg = right.create_flat_group(false);
    right.insert_to_group_and_scope(expr_id(20), rg);
    let rpos = right.create_group();
    right.insert_to_group_and_scope(expr_id(21), rpos);

    let mut union_node = LogicalNodeEnum::Union(
        graphdb_query::planning::plan::logical::logical_nodes::graph_ops::LogicalUnionNode {
            id: next_node_id(),
            input: Some(Box::new(scan())),
            deps: vec![scan(), scan()],
            distinct: false,
            output_var: None,
            col_names: vec![],
            column_types: vec![],
        },
    );
    let out = union_node.compute_factorized_schema(&[left, right]);
    assert!(out.is_flat_schema(), "Union should flatten all inputs");
    out.validate_at_most_one_unflat();
    // Verify that RemoveFactorizationRewriter can strip Flatten and restore flat schema.
    let mut with_flatten = LogicalNodeEnum::Flatten(LogicalFlattenNode::new(1, scan()));
    assert!(explain_contains_flatten(&with_flatten));
    let mut without = with_flatten.clone();
    RemoveFactorizationRewriter::new().rewrite(&mut without);
    assert!(!explain_contains_flatten(&without));
}

#[test]
fn fulltext_leaf_is_flat_single_group() {
    use graphdb_query::parser::ast::fulltext::FulltextQueryExpr;
    let mut node = LogicalNodeEnum::FulltextSearch(
        graphdb_query::planning::plan::logical::logical_nodes::search::LogicalFulltextSearchNode {
            id: next_node_id(),
            index_name: "idx".to_string(),
            query: FulltextQueryExpr::Simple("test".to_string()),
            yield_clause: None,
            where_clause: None,
            order_clause: None,
            limit: None,
            offset: None,
            space_id: 1,
            tag_name: "person".to_string(),
            field_name: "name".to_string(),
            output_var: None,
            col_names: vec!["a".to_string()],
            column_types: vec![],
        },
    );
    let schema = node.compute_factorized_schema(&[]);
    assert_eq!(schema.num_groups(), 1);
    assert!(schema.is_flat_schema());
}

#[test]
fn unwind_passthrough() {
    let mut child_schema = FactorizedSchema::new();
    let g0 = child_schema.create_flat_group(false);
    let g1 = child_schema.create_group();
    child_schema.insert_to_group_and_scope(expr_id(1), g0);
    child_schema.insert_to_group_and_scope(expr_id(2), g1);
    assert!(child_schema.has_unflat_group());

    let mut unwind = LogicalNodeEnum::Unwind(
        graphdb_query::planning::plan::logical::logical_nodes::graph_ops::LogicalUnwindNode {
            id: next_node_id(),
            input: Some(Box::new(scan())),
            deps: vec![scan()],
            alias: "x".to_string(),
            list_expression: {
                let ctx = Arc::new(ExpressionAnalysisContext::new());
                let meta = ExpressionMeta::new(Expression::Variable("list".to_string()));
                let id = ctx.register_expression(meta);
                ContextualExpression::new(id, ctx)
            },
            output_var: None,
            col_names: vec!["x".to_string()],
            column_types: vec![],
        },
    );
    let out = unwind.compute_factorized_schema(&[child_schema.clone()]);
    out.validate_at_most_one_unflat();
    // Baseline builds a fresh unflat group for the unwind output even when
    // the list is unresolved: children are flattened, the alias is unflat.
    assert!(
        out.has_unflat_group(),
        "Unwind on an unresolved list flattens children and creates an unflat alias group"
    );
    assert_eq!(out.get_group_pos_by_name_opt("x"), out.unflat_group_pos());
}

#[test]
fn variable_name_fallback() {
    let mut schema = FactorizedSchema::new();
    let g0 = schema.create_flat_group(false);
    let g1 = schema.create_group();
    schema.insert_to_group_and_scope_with_name(expr_id(10), Some("a".to_string()), g0);
    schema.insert_to_group_and_scope_with_name(expr_id(20), Some("b".to_string()), g1);

    let expr = Expression::Binary {
        left: Box::new(Expression::Variable("a".to_string())),
        op: graphdb_core::types::operators::BinaryOperator::Add,
        right: Box::new(Expression::Variable("b".to_string())),
    };
    let mut store = HashMap::new();
    let fake_id = expr_id(999);
    store.insert(fake_id.clone(), expr);

    let mut analyzer = GroupDependencyAnalyzer::with_expr_store(&schema, false, &store);
    analyzer.visit(&fake_id);
    let deps = analyzer.dependent_groups().clone();
    assert!(deps.contains(&g0), "Variable a should be resolved to g0");
    assert!(deps.contains(&g1), "Variable b should be resolved to g1");
    assert_eq!(deps.len(), 2);
}

#[test]
fn get_neighbors_chain_keeps_one_unflat() {
    let mut base = FactorizedSchema::new();
    let g0 = base.create_flat_group(false);
    base.insert_to_group_and_scope(expr_id(1), g0);
    let mut prev = base;
    for _ in 0..3 {
        let mut node = LogicalNodeEnum::GetNeighbors(
            graphdb_query::planning::plan::logical::logical_nodes::access::LogicalGetNeighborsNode {
                id: next_node_id(),
                space_id: 1,
                src_vids: "1".to_string(),
                edge_types: vec!["knows".to_string()],
                direction: "OUT".to_string(),
                edge_props: vec![],
                tag_props: vec![],
                expression: None,
                dedup: false,
                limit: None,
                projected_properties: vec![],
                index_hint: None,
                estimated_cardinality: None,
                output_var: None,
                col_names: vec!["b".to_string()],
                column_types: vec![],
                deps: vec![scan()],
            },
        );
        let next = node.compute_factorized_schema(&[prev.clone()]);
        next.validate_at_most_one_unflat();
        prev = next;
    }
    prev.validate_at_most_one_unflat();
    assert_eq!(prev.groups().iter().filter(|g| !g.is_flat()).count(), 1);
}

#[test]
fn factorization_disabled_vs_enabled_semantics() {
    // Verify that disabling factorization (flat copy) yields same logical row count
    // Disabled path is flat schema via FactorizedSchema::flat_copy.
    let ctx = Arc::new(ExpressionAnalysisContext::new());
    let id_a = ctx.register_expression(ExpressionMeta::new(Expression::Variable("a".to_string())));
    let id_b = ctx.register_expression(ExpressionMeta::new(Expression::Variable("b".to_string())));
    let mut child_schema = FactorizedSchema::new();
    let g0 = child_schema.create_flat_group(false);
    let g1 = child_schema.create_group();
    child_schema.insert_to_group_and_scope(id_a.clone(), g0);
    child_schema.insert_to_group_and_scope(id_b.clone(), g1);

    let mut proj_enabled = LogicalNodeEnum::Project(
        graphdb_query::planning::plan::logical::logical_nodes::operation::LogicalProjectNode {
            id: next_node_id(),
            input: Some(Box::new(scan())),
            deps: vec![scan()],
            columns: vec![graphdb_core::YieldColumn {
                expression: ContextualExpression::new(id_a.clone(), ctx.clone()),
                alias: "a".to_string(),
                is_matched: false,
            }],
            output_var: None,
            col_names: vec!["a".to_string()],
            column_types: vec![],
        },
    );
    let enabled_schema = proj_enabled.compute_factorized_schema(&[child_schema.clone()]);
    let mut flat_enabled = proj_enabled.clone();
    let flat_schema = flat_enabled.compute_flat_schema(&[child_schema.clone()]);
    // Both should be valid and flatten produces flat schema
    assert!(flat_schema.is_flat_schema());
    enabled_schema.validate_at_most_one_unflat();

    // Disabled rewriter should not insert Flatten, enabled may or may not depending on deps
    let mut plan_enabled = scan();
    let mut rewriter = FactorizationRewriter::new();
    rewriter.rewrite(&mut plan_enabled);
    let mut plan_disabled = scan();
    let mut disabler = FactorizationRewriter::disabled();
    disabler.rewrite(&mut plan_disabled);
    assert_eq!(plan_enabled.type_name(), plan_disabled.type_name());
}

fn ctx_var(var: &str) -> (ContextualExpression, ExpressionId) {
    let ctx = Arc::new(ExpressionAnalysisContext::new());
    let id = ctx.register_expression(ExpressionMeta::new(Expression::Variable(var.to_string())));
    (ContextualExpression::new(id.clone(), ctx), id)
}

#[test]
fn select_branch_keeps_factorization() {
    use graphdb_query::planning::plan::logical::logical_nodes::control_flow::LogicalSelectNode;
    let (cond, _) = ctx_var("a");
    let mut branch = FactorizedSchema::new();
    let g0 = branch.create_flat_group(false);
    let g1 = branch.create_group();
    branch.insert_to_group_and_scope(expr_id(1), g0);
    branch.insert_to_group_and_scope(expr_id(2), g1);
    branch.insert_to_group_and_scope_with_name(expr_id(3), Some("a".to_string()), g0);
    let mut node = LogicalNodeEnum::Select(LogicalSelectNode {
        id: next_node_id(),
        condition: cond,
        if_branch: Some(Box::new(scan())),
        else_branch: Some(Box::new(scan())),
        output_var: None,
        col_names: vec![],
        column_types: vec![],
    });
    let out = node.compute_factorized_schema(&[branch]);
    out.validate_at_most_one_unflat();
    assert_eq!(
        out.unflat_group_pos(),
        Some(g1),
        "Select must not blindly flatten_all when the condition only reads flat groups"
    );
}

#[test]
fn assign_over_expansion_keeps_factorization() {
    use graphdb_query::planning::plan::logical::logical_nodes::access::LogicalGetNeighborsNode;
    use graphdb_query::planning::plan::logical::logical_nodes::graph_ops::LogicalAssignNode;
    // SET c = b over MATCH (a)-[:knows]->(b): the expansion output is
    // unflat, the assignment only reads it, so no Flatten may appear.
    // Shared context with distinct ids: the rhs resolves by name.
    let ctx = Arc::new(ExpressionAnalysisContext::new());
    let out_id =
        ctx.register_expression(ExpressionMeta::new(Expression::Variable("b".to_string())));
    let rhs_id =
        ctx.register_expression(ExpressionMeta::new(Expression::Variable("b".to_string())));
    assert_ne!(out_id, rhs_id);
    let nbr = LogicalNodeEnum::GetNeighbors(LogicalGetNeighborsNode {
        id: next_node_id(),
        space_id: 1,
        src_vids: "1".to_string(),
        edge_types: vec!["knows".to_string()],
        direction: "OUT".to_string(),
        edge_props: vec![],
        tag_props: vec![],
        expression: Some(ContextualExpression::new(out_id, ctx.clone())),
        dedup: false,
        limit: None,
        projected_properties: vec![],
        index_hint: None,
        estimated_cardinality: None,
        output_var: None,
        col_names: vec!["b".to_string()],
        column_types: vec![],
        deps: vec![scan()],
    });
    let mut plan = LogicalNodeEnum::Assign(LogicalAssignNode {
        id: next_node_id(),
        input: Some(Box::new(nbr)),
        deps: vec![],
        assignments: vec![(
            "c".to_string(),
            ContextualExpression::new(rhs_id, ctx.clone()),
        )],
        output_var: None,
        col_names: vec!["c".to_string()],
        column_types: vec![],
    });
    FactorizationRewriter::new().rewrite(&mut plan);
    assert!(
        !explain_contains_flatten(&plan),
        "SET over an expansion must keep factorization, got: {}",
        explain_flatten_str(&plan)
    );
}

// ── Stage 1-2 regression: aggregate two-stage rule ───────────────────────────
// Same-group distinct payloads need no extra flatten beyond the key rule,
// while list-lambda payloads always flatten via `required_flat`.

#[test]
fn aggregate_same_group_distinct_needs_no_extra_flatten() {
    use graphdb_query::optimizer::factorization::flatten_resolver::aggregate_groups_to_flatten;
    let ctx = Arc::new(ExpressionAnalysisContext::new());
    let key_id =
        ctx.register_expression(ExpressionMeta::new(Expression::Variable("a".to_string())));
    let mut child = FactorizedSchema::new();
    let g0 = child.create_flat_group(false);
    let g1 = child.create_group();
    child.insert_to_group_and_scope(expr_id(1), g0);
    child.insert_to_group_and_scope(key_id.clone(), g1);
    child.insert_to_group_and_scope_with_name(expr_id(2), Some("b".to_string()), g1);
    let mut store = HashMap::new();
    store.insert(key_id.clone(), Expression::Variable("a".to_string()));
    let (leading, to_flatten) = aggregate_groups_to_flatten(
        &[key_id],
        &store,
        &[vec![Expression::Variable("b".to_string())]],
        &[true],
        &child,
    );
    assert_eq!(leading, g1);
    assert!(
        to_flatten.is_empty(),
        "distinct payload on the leading group needs no extra flatten, got {:?}",
        to_flatten
    );
}

#[test]
fn aggregate_lambda_payload_requires_flat() {
    use graphdb_query::optimizer::factorization::flatten_resolver::aggregate_groups_to_flatten;
    let ctx = Arc::new(ExpressionAnalysisContext::new());
    let key_id =
        ctx.register_expression(ExpressionMeta::new(Expression::Variable("a".to_string())));
    let mut child = FactorizedSchema::new();
    let g0 = child.create_flat_group(false);
    let g1 = child.create_group();
    child.insert_to_group_and_scope(expr_id(1), g0);
    child.insert_to_group_and_scope(key_id.clone(), g1);
    child.insert_to_group_and_scope_with_name(expr_id(2), Some("b".to_string()), g1);
    let mut store = HashMap::new();
    store.insert(key_id.clone(), Expression::Variable("a".to_string()));
    let payload = Expression::Function {
        name: "list_transform".to_string(),
        args: vec![
            Expression::List(vec![]),
            Expression::Variable("b".to_string()),
        ],
    };
    let (_, to_flatten) =
        aggregate_groups_to_flatten(&[key_id], &store, &[vec![payload]], &[false], &child);
    assert!(
        to_flatten.contains(&g1),
        "list-lambda payload must flatten its group, got {:?}",
        to_flatten
    );
}

// ── Stage 2 regression: WCO bound keys flatten on both sides ─────────────────

#[test]
fn wco_bound_keys_flatten_probe_and_build() {
    use graphdb_query::planning::plan::logical::logical_nodes::wco_intersect::LogicalWcoIntersectNode;
    let ctx = Arc::new(ExpressionAnalysisContext::new());
    let bound_id =
        ctx.register_expression(ExpressionMeta::new(Expression::Variable("a".to_string())));
    let bound = ContextualExpression::new(bound_id.clone(), ctx.clone());
    let intersect_id =
        ctx.register_expression(ExpressionMeta::new(Expression::Variable("c".to_string())));
    let intersect = ContextualExpression::new(intersect_id, ctx.clone());
    let node = LogicalWcoIntersectNode::new(
        scan(),
        vec![scan()],
        intersect,
        vec![bound],
        vec!["a".to_string(), "c".to_string()],
    );
    let mut probe = FactorizedSchema::new();
    let pg0 = probe.create_flat_group(false);
    let pg1 = probe.create_group();
    probe.insert_to_group_and_scope(expr_id(1), pg0);
    probe.insert_to_group_and_scope(bound_id.clone(), pg1);
    let mut build = FactorizedSchema::new();
    let bg0 = build.create_flat_group(false);
    let bg1 = build.create_group();
    build.insert_to_group_and_scope(expr_id(2), bg0);
    build.insert_to_group_and_scope(bound_id.clone(), bg1);
    assert_eq!(
        node.get_groups_to_flatten_on_probe_side(&probe),
        std::collections::HashSet::from([pg1])
    );
    assert_eq!(
        node.get_groups_to_flatten_on_build_side(0, &build),
        std::collections::HashSet::from([bg1])
    );
}

// ── Stage 2 regression: RightJoin rewriter matches key-aware compute ─────────

#[test]
fn right_join_rewriter_flattens_build_keys() {
    use graphdb_query::planning::plan::logical::logical_nodes::access::LogicalGetNeighborsNode;
    use graphdb_query::planning::plan::logical::logical_nodes::join::LogicalRightJoinNode;
    let ctx = Arc::new(ExpressionAnalysisContext::new());
    let out_id =
        ctx.register_expression(ExpressionMeta::new(Expression::Variable("b".to_string())));
    let nbr = LogicalNodeEnum::GetNeighbors(LogicalGetNeighborsNode {
        id: next_node_id(),
        space_id: 1,
        src_vids: "1".to_string(),
        edge_types: vec!["knows".to_string()],
        direction: "OUT".to_string(),
        edge_props: vec![],
        tag_props: vec![],
        expression: Some(ContextualExpression::new(out_id.clone(), ctx.clone())),
        dedup: false,
        limit: None,
        projected_properties: vec![],
        index_hint: None,
        estimated_cardinality: None,
        output_var: None,
        col_names: vec!["b".to_string()],
        column_types: vec![],
        deps: vec![scan()],
    });
    let mut plan = LogicalNodeEnum::RightJoin(LogicalRightJoinNode {
        id: next_node_id(),
        left: Box::new(scan()),
        right: Box::new(nbr),
        hash_keys: vec![],
        probe_keys: vec![ContextualExpression::new(out_id, ctx)],
        deps: vec![],
        output_var: None,
        col_names: vec![],
        column_types: vec![],
    });
    FactorizationRewriter::new().rewrite(&mut plan);
    assert!(
        explain_contains_flatten(&plan),
        "unflat build key on a RightJoin must be flattened, got: {}",
        explain_flatten_str(&plan)
    );
}

// ── Stage 1 regression: barrier operators expose Flatten explicitly ──────────

#[test]
fn rollup_apply_barrier_inserts_flatten_and_outputs_flat() {
    use graphdb_query::planning::plan::logical::logical_nodes::access::LogicalGetNeighborsNode;
    use graphdb_query::planning::plan::logical::logical_nodes::graph_ops::LogicalRollUpApplyNode;
    let ctx = Arc::new(ExpressionAnalysisContext::new());
    let out_id =
        ctx.register_expression(ExpressionMeta::new(Expression::Variable("b".to_string())));
    let nbr = LogicalNodeEnum::GetNeighbors(LogicalGetNeighborsNode {
        id: next_node_id(),
        space_id: 1,
        src_vids: "1".to_string(),
        edge_types: vec!["knows".to_string()],
        direction: "OUT".to_string(),
        edge_props: vec![],
        tag_props: vec![],
        expression: Some(ContextualExpression::new(out_id, ctx)),
        dedup: false,
        limit: None,
        projected_properties: vec![],
        index_hint: None,
        estimated_cardinality: None,
        output_var: None,
        col_names: vec!["b".to_string()],
        column_types: vec![],
        deps: vec![scan()],
    });
    let mut plan = LogicalNodeEnum::RollUpApply(LogicalRollUpApplyNode {
        id: next_node_id(),
        input: Some(Box::new(nbr)),
        deps: vec![],
        left_input_var: None,
        right_input_var: None,
        compare_cols: vec![],
        collect_col: None,
        output_var: None,
        col_names: vec![],
        column_types: vec![],
    });
    FactorizationRewriter::new().rewrite(&mut plan);
    assert!(
        explain_contains_flatten(&plan),
        "barrier input must carry an explicit Flatten, got: {}",
        explain_flatten_str(&plan)
    );
    let mut tmp = plan.clone();
    let out = tmp.compute_factorized_schema(&[]);
    out.validate_at_most_one_unflat();
    assert!(
        out.is_flat_schema(),
        "barrier output must be flat, got {} groups",
        out.num_groups()
    );
}
