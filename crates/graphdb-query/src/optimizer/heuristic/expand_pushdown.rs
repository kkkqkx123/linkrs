//! Expand pushdown annotation batch.
//!
//! Annotates `ExpandAll` nodes in an expansion chain with `id_only` /
//! `count_only` flags so the streaming executor can skip materializing full
//! `Value::Vertex(Box)` / `Value::Edge(Box)` rows between hops.
//!
//! - **id_only**: the hop's destination variable is only used as the next
//!   hop's seed (or not referenced downstream at all).  The executor emits
//!   `Value::VertexId` / `Value::Null` instead of calling `get_vertex`.
//! - **count_only**: the hop is the chain tail feeding a count-only aggregate
//!   (no GROUP BY, all COUNT) through pure `Project` pass-throughs.  The
//!   executor returns a per-chunk edge count, and the aggregate is rewritten
//!   to `SUM(_expand_count)` by the arena builder.
//!
//! The rule is a whole-plan pass: it matches any node but only acts at the
//! plan root (detected via [`RewriteContext::current_node_id`] == 0), walking
//! the tree top-down to collect ancestor context.  The batch runs after
//! `PredicatePushdown` so it sees filters already pushed into the scans.

use std::collections::HashMap;

use crate::optimizer::heuristic::context::RewriteContext;
use crate::optimizer::heuristic::pattern::Pattern;
use crate::optimizer::heuristic::result::{RewriteResult, TransformResult};
use crate::optimizer::heuristic::rule::RewriteRule;
use crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
use crate::planning::plan::core::nodes::base::plan_node_traits::{
    BinaryInputNode, MultipleInputNode, PlanNode, SingleInputNode,
};
use crate::planning::plan::core::nodes::graph_operations::aggregate_node::AggregateNode;
use crate::planning::plan::core::nodes::traversal::traversal_node::ExpandAllNode;
use graphdb_core::types::expr::Expression;

/// Whole-plan rule that annotates `ExpandAll` hops with id_only/count_only.
#[derive(Debug)]
pub struct ExpandPushdownAnnotateRule;

impl ExpandPushdownAnnotateRule {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ExpandPushdownAnnotateRule {
    fn default() -> Self {
        Self::new()
    }
}

impl RewriteRule for ExpandPushdownAnnotateRule {
    fn name(&self) -> &'static str {
        "ExpandPushdownAnnotateRule"
    }

    /// Matches any node; the rule only acts at the plan root.
    fn pattern(&self) -> Pattern {
        Pattern::new()
    }

    fn apply(
        &self,
        ctx: &mut RewriteContext,
        node: &PlanNodeEnum,
    ) -> RewriteResult<Option<TransformResult>> {
        // Whole-plan pass: fire only once at the root.
        if ctx.current_node_id() != 0 {
            return Ok(None);
        }
        let (new_root, changed) = annotate_expand_all(node);
        if !changed {
            return Ok(None);
        }
        let mut result = TransformResult::new();
        result.erase_curr = true;
        result.add_new_node(new_root);
        Ok(Some(result))
    }
}

/// Annotate every `ExpandAll` in `root` and return the rewritten tree.
fn annotate_expand_all(root: &PlanNodeEnum) -> (PlanNodeEnum, bool) {
    let mut candidates: Vec<(ExpandAllNode, Vec<&PlanNodeEnum>)> = Vec::new();
    collect_expand_alls(root, &mut Vec::new(), &mut candidates);

    let mut decisions: HashMap<i64, (bool, bool, bool)> = HashMap::new();
    for (expand, ancestors) in &candidates {
        let id_only = expand_id_only(expand, ancestors);
        let count_only = expand_count_only(expand, ancestors);
        let lightweight_source = id_only && source_unreferenced(expand, ancestors);
        if id_only != expand.id_only()
            || count_only != expand.count_only()
            || lightweight_source != expand.lightweight_source()
        {
            decisions.insert(expand.id(), (id_only, count_only, lightweight_source));
        }
    }
    if decisions.is_empty() {
        return (root.clone(), false);
    }

    let mut new_root = root.clone();
    let changed = apply_decisions(&mut new_root, &decisions);
    (new_root, changed)
}

/// Collect every `ExpandAll` node together with its ancestor path
/// (root-first).
fn collect_expand_alls<'a>(
    node: &'a PlanNodeEnum,
    ancestors: &mut Vec<&'a PlanNodeEnum>,
    out: &mut Vec<(ExpandAllNode, Vec<&'a PlanNodeEnum>)>,
) {
    if let PlanNodeEnum::ExpandAll(expand) = node {
        out.push((expand.clone(), ancestors.clone()));
    }
    for child in node.children() {
        ancestors.push(node);
        collect_expand_alls(child, ancestors, out);
        ancestors.pop();
    }
}

/// Decide `id_only` for an expand hop: neither its destination variable nor its
/// edge variable may be referenced by any ancestor, and the hop must support
/// the raw-id fast path.
///
/// The raw-id fast path emits `Value::Null` for the edge column and
/// `Value::VertexId` for the destination column, so `id_only` is only valid
/// when neither column is consumed downstream (e.g. `count(f)` would wrongly
/// become 0 because the null edge is not counted).
fn expand_id_only(expand: &ExpandAllNode, ancestors: &[&PlanNodeEnum]) -> bool {
    if !fast_path_compatible(expand) {
        return false;
    }
    let Some(dst_var) = expand.col_names().get(2) else {
        return false;
    };
    let Some(edge_var) = expand.col_names().get(1) else {
        return false;
    };
    // Every ancestor must be a known node type whose references we can audit,
    // and none of them may reference the destination or edge variable.
    // Unknown ancestors (delete operators, loops, path algorithms, ...)
    // conservatively block the annotation.
    ancestors.iter().all(|anc| {
        known_reference_ancestor(anc)
            && !node_references_var(anc, dst_var)
            && !node_references_var(anc, edge_var)
    })
}

/// Whether the hop's *source* variable (the first column) is not referenced by
/// any ancestor — the precondition for emitting the source column as a raw
/// `Value::VertexId` instead of cloning the full vertex carried from upstream.
fn source_unreferenced(expand: &ExpandAllNode, ancestors: &[&PlanNodeEnum]) -> bool {
    let Some(src_var) = expand.col_names().first() else {
        return false;
    };
    ancestors
        .iter()
        .all(|anc| known_reference_ancestor(anc) && !node_references_var(anc, src_var))
}

/// Whether `anc` is a node type whose variable references the annotation pass
/// can fully audit.  Unknown types conservatively block the annotation.
fn known_reference_ancestor(anc: &PlanNodeEnum) -> bool {
    matches!(
        anc,
        PlanNodeEnum::Filter(_)
            | PlanNodeEnum::Project(_)
            | PlanNodeEnum::Aggregate(_)
            | PlanNodeEnum::ExpandAll(_)
            | PlanNodeEnum::InnerJoin(_)
            | PlanNodeEnum::LeftJoin(_)
            | PlanNodeEnum::RightJoin(_)
            | PlanNodeEnum::FullOuterJoin(_)
            | PlanNodeEnum::SemiJoin(_)
            | PlanNodeEnum::Sort(_)
            | PlanNodeEnum::TopN(_)
            | PlanNodeEnum::Window(_)
            | PlanNodeEnum::Limit(_)
            | PlanNodeEnum::Dedup(_)
    )
}

/// Decide `count_only` for the chain-tail expand: the direct downstream
/// (through pure `Project` pass-throughs of the destination variable) is a
/// count-only aggregate, and the hop supports the raw-id fast path.
fn expand_count_only(expand: &ExpandAllNode, ancestors: &[&PlanNodeEnum]) -> bool {
    if !fast_path_compatible(expand) {
        return false;
    }
    let Some(dst_var) = expand.col_names().get(2) else {
        return false;
    };
    // Walk up from the expand (closest ancestor first).  Only pure Project
    // pass-throughs of the destination variable may separate it from the
    // count-only aggregate; a Filter or any other operator is conservative
    // grounds to skip the optimization.
    for anc in ancestors.iter().rev() {
        match anc {
            PlanNodeEnum::Project(project) => {
                if !project_passes_dst(project, dst_var) {
                    return false;
                }
            }
            PlanNodeEnum::Aggregate(agg) => return is_count_only_aggregate(agg),
            _ => return false,
        }
    }
    false
}

/// The single-step raw-id fast path (`expand_single_step`) is only taken when
/// the expand has no filter, no literal source ids and a step limit of one.
fn fast_path_compatible(expand: &ExpandAllNode) -> bool {
    expand.step_limit().unwrap_or(1) == 1
        && expand.filter().is_none()
        && expand.src_vids().is_empty()
}

/// True when every column of `project` is a bare reference to `dst_var` (i.e.
/// the project only forwards the aggregate argument of a count-only
/// aggregate).
fn project_passes_dst(
    project: &crate::planning::plan::core::nodes::operation::project_node::ProjectNode,
    dst_var: &str,
) -> bool {
    !project.columns().is_empty()
        && project.columns().iter().all(|col| {
            col.expression
                .expression()
                .and_then(|meta| {
                    if let Expression::Variable(var) = meta.inner() {
                        Some(var.as_str() == dst_var)
                    } else {
                        None
                    }
                })
                .unwrap_or(false)
        })
}

/// Whether `node` references `var` in any of its expressions, group keys,
/// aggregate function fields or join keys.
fn node_references_var(node: &PlanNodeEnum, var: &str) -> bool {
    match node {
        PlanNodeEnum::Filter(filter) => filter
            .condition()
            .get_expression()
            .map(|expr| expr.get_variables().iter().any(|v| v == var))
            .unwrap_or(false),
        PlanNodeEnum::Project(project) => project.columns().iter().any(|col| {
            col.expression
                .expression()
                .map(|meta| meta.inner().get_variables().iter().any(|v| v == var))
                .unwrap_or(false)
        }),
        PlanNodeEnum::Aggregate(agg) => {
            agg.group_keys().iter().any(|key| key == var)
                || agg
                    .aggregation_args()
                    .iter()
                    .flatten()
                    .any(|expr| matches!(expr, Expression::Variable(name) if name == var))
        }
        PlanNodeEnum::InnerJoin(join) => {
            join_references_var(join.hash_keys(), join.probe_keys(), var)
        }
        PlanNodeEnum::LeftJoin(join) => {
            join_references_var(join.hash_keys(), join.probe_keys(), var)
        }
        PlanNodeEnum::RightJoin(join) => {
            join_references_var(join.hash_keys(), join.probe_keys(), var)
        }
        PlanNodeEnum::FullOuterJoin(join) => {
            join_references_var(join.hash_keys(), join.probe_keys(), var)
        }
        PlanNodeEnum::SemiJoin(join) => {
            join_references_var(join.hash_keys(), join.probe_keys(), var)
        }
        PlanNodeEnum::ExpandAll(expand) => expand
            .filter()
            .and_then(|f| f.get_expression())
            .map(|expr| expr.get_variables().iter().any(|v| v == var))
            .unwrap_or(false),
        PlanNodeEnum::Sort(sort) => sort
            .sort_items()
            .iter()
            .any(|item| item.expression.get_variables().iter().any(|v| v == var)),
        PlanNodeEnum::TopN(topn) => topn
            .sort_items()
            .iter()
            .any(|item| item.expression.get_variables().iter().any(|v| v == var)),
        PlanNodeEnum::Window(window) => window
            .window_functions()
            .iter()
            .flat_map(|wf| {
                wf.args
                    .iter()
                    .chain(wf.partition_by.iter())
                    .chain(wf.order_by.iter())
            })
            .any(|expr| expr.get_variables().iter().any(|v| v == var)),
        _ => false,
    }
}

fn join_references_var(
    hash_keys: &[graphdb_core::types::ContextualExpression],
    probe_keys: &[graphdb_core::types::ContextualExpression],
    var: &str,
) -> bool {
    hash_keys.iter().chain(probe_keys.iter()).any(|key| {
        key.get_expression()
            .map(|expr| expr.get_variables().iter().any(|v| v == var))
            .unwrap_or(false)
    })
}

/// Whether the aggregate is count-only: no GROUP BY and only COUNT functions.
fn is_count_only_aggregate(agg: &AggregateNode) -> bool {
    agg.group_keys().is_empty()
        && !agg.aggregation_functions().is_empty()
        && agg
            .aggregation_functions()
            .iter()
            .all(|f| matches!(f, graphdb_core::types::operators::AggregateFunction::Count))
}

/// Apply the flag decisions to the matching `ExpandAll` nodes in place.
fn apply_decisions(root: &mut PlanNodeEnum, decisions: &HashMap<i64, (bool, bool, bool)>) -> bool {
    let mut changed = false;
    if let PlanNodeEnum::ExpandAll(expand) = root {
        if let Some((id_only, count_only, lightweight_source)) = decisions.get(&expand.id()) {
            if expand.id_only() != *id_only
                || expand.count_only() != *count_only
                || expand.lightweight_source() != *lightweight_source
            {
                expand.set_id_only(*id_only);
                expand.set_count_only(*count_only);
                expand.set_lightweight_source(*lightweight_source);
                changed = true;
            }
        }
    }
    use PlanNodeEnum::*;
    match root {
        Project(n) => changed |= apply_decisions(n.input_mut(), decisions),
        Filter(n) => changed |= apply_decisions(n.input_mut(), decisions),
        Sort(n) => changed |= apply_decisions(n.input_mut(), decisions),
        Limit(n) => changed |= apply_decisions(n.input_mut(), decisions),
        TopN(n) => changed |= apply_decisions(n.input_mut(), decisions),
        Sample(n) => changed |= apply_decisions(n.input_mut(), decisions),
        Dedup(n) => changed |= apply_decisions(n.input_mut(), decisions),
        DataCollect(n) => changed |= apply_decisions(n.input_mut(), decisions),
        Aggregate(n) => changed |= apply_decisions(n.input_mut(), decisions),
        Window(n) => changed |= apply_decisions(n.input_mut(), decisions),
        Unwind(n) => changed |= apply_decisions(n.input_mut(), decisions),
        Assign(n) => changed |= apply_decisions(n.input_mut(), decisions),
        Remove(n) => changed |= apply_decisions(n.input_mut(), decisions),
        Materialize(n) => changed |= apply_decisions(n.input_mut(), decisions),
        PatternApply(n) => changed |= apply_decisions(n.input_mut(), decisions),
        CorrelatedApply(n) => changed |= apply_decisions(n.input_mut(), decisions),
        RollUpApply(n) => changed |= apply_decisions(n.input_mut(), decisions),
        Traverse(n) => changed |= apply_decisions(n.input_mut(), decisions),
        PipeDeleteVertices(n) => changed |= apply_decisions(n.input_mut(), decisions),
        PipeDeleteEdges(n) => changed |= apply_decisions(n.input_mut(), decisions),
        Expand(n) => {
            for child in n.inputs_mut() {
                changed |= apply_decisions(child, decisions);
            }
        }
        ExpandAll(n) => {
            for child in n.inputs_mut() {
                changed |= apply_decisions(child, decisions);
            }
        }
        AppendVertices(n) => {
            for child in n.inputs_mut() {
                changed |= apply_decisions(child, decisions);
            }
        }
        GetVertices(n) => {
            for child in n.inputs_mut() {
                changed |= apply_decisions(child, decisions);
            }
        }
        GetNeighbors(n) => {
            for child in n.inputs_mut() {
                changed |= apply_decisions(child, decisions);
            }
        }
        InnerJoin(n) => {
            changed |= apply_decisions(n.left_input_mut(), decisions);
            changed |= apply_decisions(n.right_input_mut(), decisions);
        }
        LeftJoin(n) => {
            changed |= apply_decisions(n.left_input_mut(), decisions);
            changed |= apply_decisions(n.right_input_mut(), decisions);
        }
        RightJoin(n) => {
            changed |= apply_decisions(n.left_input_mut(), decisions);
            changed |= apply_decisions(n.right_input_mut(), decisions);
        }
        CrossJoin(n) => {
            changed |= apply_decisions(n.left_input_mut(), decisions);
            changed |= apply_decisions(n.right_input_mut(), decisions);
        }
        FullOuterJoin(n) => {
            changed |= apply_decisions(n.left_input_mut(), decisions);
            changed |= apply_decisions(n.right_input_mut(), decisions);
        }
        SemiJoin(n) => {
            changed |= apply_decisions(n.left_input_mut(), decisions);
            changed |= apply_decisions(n.right_input_mut(), decisions);
        }
        Apply(n) => {
            changed |= apply_decisions(n.left_input_mut(), decisions);
            changed |= apply_decisions(n.right_input_mut(), decisions);
        }
        BiExpand(n) => {
            changed |= apply_decisions(n.left_input_mut(), decisions);
            changed |= apply_decisions(n.right_input_mut(), decisions);
        }
        BiTraverse(n) => {
            changed |= apply_decisions(n.left_input_mut(), decisions);
            changed |= apply_decisions(n.right_input_mut(), decisions);
        }
        Union(n) => {
            for child in n.dependencies_mut() {
                changed |= apply_decisions(child, decisions);
            }
        }
        Minus(n) => {
            for child in n.dependencies_mut() {
                changed |= apply_decisions(child, decisions);
            }
        }
        Intersect(n) => {
            for child in n.dependencies_mut() {
                changed |= apply_decisions(child, decisions);
            }
        }
        _ => {}
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planning::plan::core::nodes::access::graph_scan_node::ScanVerticesNode;
    use crate::planning::plan::core::nodes::graph_operations::aggregate_node::AggregateNode;
    use crate::planning::plan::core::nodes::operation::project_node::ProjectNode;
    use crate::planning::plan::core::nodes::traversal::traversal_node::ExpandAllNode;
    use graphdb_core::types::expr::expression_context::ExpressionAnalysisContext;
    use graphdb_core::types::expr::{ContextualExpression, ExpressionMeta};
    use graphdb_core::types::operators::AggregateFunction;
    use graphdb_core::Expression;
    use graphdb_core::Value;
    use std::sync::Arc;

    fn ctx_expr(expr: Expression) -> ContextualExpression {
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let id = ctx.register_expression(ExpressionMeta::new(expr));
        ContextualExpression::new(id, ctx)
    }

    fn anchor_scan(var: &str) -> PlanNodeEnum {
        let mut scan = ScanVerticesNode::new(1, "space");
        scan.set_tag("Node");
        scan.set_col_names(vec![var.to_string()]);
        PlanNodeEnum::ScanVertices(scan)
    }

    fn hop(edge: &str, vars: [&str; 3], input: PlanNodeEnum) -> PlanNodeEnum {
        let mut expand = ExpandAllNode::new(1, vec![edge.to_string()], "OUT");
        expand.set_step_limit(1);
        expand.set_col_names(vars.iter().map(|s| s.to_string()).collect());
        expand.add_input(input);
        PlanNodeEnum::ExpandAll(expand)
    }

    fn project_pass_dst(input: PlanNodeEnum, dst: &str) -> PlanNodeEnum {
        let expr = Expression::Variable(dst.to_string());
        let col = graphdb_core::YieldColumn {
            expression: ctx_expr(expr),
            alias: dst.to_string(),
            is_matched: false,
        };
        PlanNodeEnum::Project(ProjectNode::new(input, vec![col]).expect("project should build"))
    }

    fn count_agg(input: PlanNodeEnum) -> PlanNodeEnum {
        let agg = AggregateNode::new(input, vec![], vec![AggregateFunction::Count])
            .expect("aggregate should build");
        PlanNodeEnum::Aggregate(agg)
    }

    fn project_pass_var(input: PlanNodeEnum, var: &str) -> PlanNodeEnum {
        let expr = Expression::Variable(var.to_string());
        let col = graphdb_core::YieldColumn {
            expression: ctx_expr(expr),
            alias: var.to_string(),
            is_matched: false,
        };
        PlanNodeEnum::Project(ProjectNode::new(input, vec![col]).expect("project should build"))
    }

    fn count_field_agg(input: PlanNodeEnum, field: &str) -> PlanNodeEnum {
        let mut agg = AggregateNode::new(input, vec![], vec![AggregateFunction::Count])
            .expect("aggregate should build");
        agg.set_aggregation_args(vec![vec![Expression::Variable(field.to_string())]]);
        PlanNodeEnum::Aggregate(agg)
    }

    fn expand_alls(root: &PlanNodeEnum) -> Vec<ExpandAllNode> {
        let mut out = Vec::new();
        collect_expand_alls(root, &mut Vec::new(), &mut out);
        out.into_iter().map(|(e, _)| e).collect()
    }

    fn hop_by_dst<'a>(hops: &'a [ExpandAllNode], dst: &str) -> &'a ExpandAllNode {
        hops.iter()
            .find(|h| h.col_names().get(2).map(|s| s.as_str()) == Some(dst))
            .expect("hop with dst")
    }

    #[test]
    fn two_hop_count_chain_is_annotated() {
        // MATCH (a:Node)-[:Link]->(b)-[:Link]->(c) RETURN count(c)
        let chain = count_agg(project_pass_dst(
            hop(
                "Link",
                ["b", "e2", "c"],
                hop("Link", ["a", "e1", "b"], anchor_scan("a")),
            ),
            "c",
        ));
        let (annotated, changed) = annotate_expand_all(&chain);
        assert!(changed, "annotation must change the plan");
        let hops = expand_alls(&annotated);
        assert_eq!(hops.len(), 2);
        let hop_b = hop_by_dst(&hops, "b");
        let hop_c = hop_by_dst(&hops, "c");
        assert!(hop_b.id_only(), "hop1 (b) must be id_only");
        assert!(!hop_b.count_only(), "hop1 (b) must not be count_only");
        assert!(
            hop_b.lightweight_source(),
            "hop1 source (a) is unreferenced, so its source column may be lightweight"
        );
        assert!(hop_c.count_only(), "hop2 (c) must be count_only");
        assert!(
            !hop_c.id_only(),
            "hop2 (c) dst is referenced by the aggregate"
        );
    }

    #[test]
    fn referenced_source_keeps_id_only_but_not_lightweight() {
        // MATCH (a:Node)-[:Link]->(b) RETURN a  -> the source `a` is projected
        // out, so hop1 stays id_only but must keep the full source vertex.
        let chain = project_pass_dst(hop("Link", ["a", "e1", "b"], anchor_scan("a")), "a");
        let (annotated, changed) = annotate_expand_all(&chain);
        assert!(changed, "annotation must change the plan");
        let hops = expand_alls(&annotated);
        let hop_b = hop_by_dst(&hops, "b");
        assert!(hop_b.id_only(), "dst (b) unreferenced so hop1 is id_only");
        assert!(
            !hop_b.lightweight_source(),
            "source (a) is referenced by the projection, so it must stay faithful"
        );
    }

    #[test]
    fn dst_property_access_blocks_id_only() {
        // hop1's dst `b` is used by a property access on hop2's filter.
        let mut hop2 = ExpandAllNode::new(1, vec!["Link".to_string()], "OUT");
        hop2.set_step_limit(1);
        hop2.set_col_names(vec!["b".to_string(), "e2".to_string(), "c".to_string()]);
        hop2.set_filter(ctx_expr(Expression::Binary {
            left: Box::new(Expression::Property {
                object: Box::new(Expression::Variable("b".to_string())),
                property: "value".to_string(),
            }),
            op: graphdb_core::types::operators::BinaryOperator::LessThan,
            right: Box::new(Expression::Literal(Value::Int(5))),
        }));
        hop2.add_input(hop("Link", ["a", "e1", "b"], anchor_scan("a")));

        let (annotated, _) = annotate_expand_all(&PlanNodeEnum::ExpandAll(hop2));
        let hops = expand_alls(&annotated);
        assert_eq!(hops.len(), 2);
        assert!(
            !hop_by_dst(&hops, "b").id_only(),
            "hop1 dst referenced by hop2 filter property access"
        );
    }

    #[test]
    fn projected_dst_blocks_id_only_and_count_only() {
        // MATCH (a)-[:R]->(b)-[:R]->(c) RETURN c  -> hop2 dst is projected out.
        let chain = project_pass_dst(
            hop(
                "Link",
                ["b", "e2", "c"],
                hop("Link", ["a", "e1", "b"], anchor_scan("a")),
            ),
            "c",
        );
        let (annotated, _) = annotate_expand_all(&chain);
        let hops = expand_alls(&annotated);
        assert!(hop_by_dst(&hops, "b").id_only(), "hop1 dst only feeds hop2");
        assert!(
            hop_by_dst(&hops, "b").lightweight_source(),
            "hop1 source (a) is unreferenced"
        );
        assert!(
            !hop_by_dst(&hops, "c").id_only(),
            "hop2 dst is the final projection"
        );
        assert!(
            !hop_by_dst(&hops, "c").count_only(),
            "no count-only aggregate above hop2"
        );
    }

    #[test]
    fn count_of_edge_blocks_id_only() {
        // MATCH (a:Node)-[f:Link]->(b) RETURN count(f)  -> the edge `f` is
        // consumed by the aggregate, so id_only must not be applied (it would
        // nullify the edge column and turn count(f) into 0).
        let chain = count_field_agg(
            project_pass_var(hop("Link", ["a", "f", "b"], anchor_scan("a")), "f"),
            "f",
        );
        let (annotated, _) = annotate_expand_all(&chain);
        let hops = expand_alls(&annotated);
        let hop = hop_by_dst(&hops, "b");
        assert!(
            !hop.id_only(),
            "edge variable is referenced by count(f), so id_only must be blocked"
        );
    }
}
