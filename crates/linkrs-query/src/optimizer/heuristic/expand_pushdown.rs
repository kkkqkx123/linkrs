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
use linkrs_core::types::expr::Expression;
use linkrs_core::Value;

/// Columnar expand decision for one hop, applied together with the legacy
/// `id_only` / `count_only` flags.
#[derive(Debug, Clone)]
struct ExpandDecision {
    id_only: bool,
    count_only: bool,
    lightweight_source: bool,
    edge_props: Option<Vec<String>>,
    dst_props: Option<Vec<String>>,
    closed_loop: bool,
    skip_rows: bool,
}

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

    let mut decisions: HashMap<i64, ExpandDecision> = HashMap::new();
    for (expand, ancestors) in &candidates {
        let id_only = expand_id_only(expand, ancestors);
        let count_only = expand_count_only(expand, ancestors);
        let lightweight_source = id_only && source_unreferenced(expand, ancestors);
        let (edge_props, dst_props) = expand_prop_needs(expand, ancestors);
        let closed_loop = expand_closed_loop(expand);
        let skip_rows =
            expand_skip_rows(expand, ancestors, edge_props.as_ref(), dst_props.as_ref());
        let changed = id_only != expand.id_only()
            || count_only != expand.count_only()
            || lightweight_source != expand.lightweight_source()
            || edge_props.as_ref() != expand.edge_required_props()
            || dst_props.as_ref() != expand.dst_required_props()
            || closed_loop != expand.closed_loop()
            || skip_rows != expand.skip_rows();
        if changed {
            decisions.insert(
                expand.id(),
                ExpandDecision {
                    id_only,
                    count_only,
                    lightweight_source,
                    edge_props,
                    dst_props,
                    closed_loop,
                    skip_rows,
                },
            );
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
/// `Flatten` is row-preserving (it replays child rows without evaluating
/// any column), so it neither consumes the destination/edge/source
/// variables nor blocks the raw-id fast path.
///
/// Shared with the scan identity annotation, which audits the same ancestor
/// domain for whole-entity uses of the scan variable.
pub(crate) fn known_reference_ancestor(anc: &PlanNodeEnum) -> bool {
    matches!(
        anc,
        PlanNodeEnum::Filter(_)
            | PlanNodeEnum::Project(_)
            | PlanNodeEnum::Flatten(_)
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
        && expand.step_limits().is_none()
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
    hash_keys: &[linkrs_core::types::ContextualExpression],
    probe_keys: &[linkrs_core::types::ContextualExpression],
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
            .all(|f| matches!(f, linkrs_core::types::operators::AggregateFunction::Count))
}

/// Collect the downstream property demand for one hop's edge and destination
/// slots. `None` means the whole entity is needed (bare use outside a bare
/// count), `Some(vec)` lists the demanded property names with empty meaning
/// topology or identity only.
fn expand_prop_needs(
    expand: &ExpandAllNode,
    ancestors: &[&PlanNodeEnum],
) -> (Option<Vec<String>>, Option<Vec<String>>) {
    use std::collections::BTreeSet;
    let Some(edge_var) = expand.col_names().get(1).cloned() else {
        return (None, None);
    };
    let Some(dst_var) = expand.col_names().get(2).cloned() else {
        return (None, None);
    };
    let mut edge_props: BTreeSet<String> = BTreeSet::new();
    let mut dst_props: BTreeSet<String> = BTreeSet::new();
    let mut edge_full = false;
    let mut dst_full = false;
    for (pos, anc) in ancestors.iter().enumerate() {
        let is_root = pos == 0;
        if !known_reference_ancestor(anc) {
            edge_full = true;
            dst_full = true;
            break;
        }
        match anc {
            PlanNodeEnum::Project(project) => {
                for col in project.columns() {
                    let Some(meta) = col.expression.expression() else {
                        continue;
                    };
                    let expr = meta.inner();
                    if let Expression::Variable(name) = expr {
                        if name == &edge_var || name == &dst_var {
                            if col.alias == *name && !is_root {
                                continue;
                            }
                            if name == &edge_var {
                                edge_full = true;
                            } else {
                                dst_full = true;
                            }
                            continue;
                        }
                    }
                    collect_expr_needs(
                        expr,
                        &edge_var,
                        &dst_var,
                        &mut edge_props,
                        &mut dst_props,
                        &mut edge_full,
                        &mut dst_full,
                    );
                }
            }
            PlanNodeEnum::Aggregate(agg) => {
                if agg.group_keys().iter().any(|k| k == &edge_var) {
                    edge_full = true;
                }
                if agg.group_keys().iter().any(|k| k == &dst_var) {
                    dst_full = true;
                }
                let funcs = agg.aggregation_functions();
                for (idx, arg_list) in agg.aggregation_args().iter().enumerate() {
                    let is_count = funcs
                        .get(idx)
                        .is_some_and(|f| {
                            matches!(f, linkrs_core::types::operators::AggregateFunction::Count)
                        });
                    for arg in arg_list {
                        if is_count && matches!(arg, Expression::Variable(n) if n == &edge_var || n == &dst_var)
                        {
                            continue;
                        }
                        collect_expr_needs(
                            arg,
                            &edge_var,
                            &dst_var,
                            &mut edge_props,
                            &mut dst_props,
                            &mut edge_full,
                            &mut dst_full,
                        );
                    }
                }
            }
            PlanNodeEnum::Filter(filter) => {
                if let Some(expr) = filter.condition().get_expression() {
                    collect_expr_needs(
                        &expr,
                        &edge_var,
                        &dst_var,
                        &mut edge_props,
                        &mut dst_props,
                        &mut edge_full,
                        &mut dst_full,
                    );
                }
            }
            PlanNodeEnum::ExpandAll(next) => {
                if let Some(expr) = next.filter().and_then(|f| f.get_expression()) {
                    collect_expr_needs(
                        &expr,
                        &edge_var,
                        &dst_var,
                        &mut edge_props,
                        &mut dst_props,
                        &mut edge_full,
                        &mut dst_full,
                    );
                }
            }
            PlanNodeEnum::Sort(sort) => {
                for item in sort.sort_items() {
                    collect_expr_needs(
                        &item.expression,
                        &edge_var,
                        &dst_var,
                        &mut edge_props,
                        &mut dst_props,
                        &mut edge_full,
                        &mut dst_full,
                    );
                }
            }
            PlanNodeEnum::TopN(topn) => {
                for item in topn.sort_items() {
                    collect_expr_needs(
                        &item.expression,
                        &edge_var,
                        &dst_var,
                        &mut edge_props,
                        &mut dst_props,
                        &mut edge_full,
                        &mut dst_full,
                    );
                }
            }
            PlanNodeEnum::Window(window) => {
                for wf in window.window_functions() {
                    for expr in wf
                        .args
                        .iter()
                        .chain(wf.partition_by.iter())
                        .chain(wf.order_by.iter())
                    {
                        collect_expr_needs(
                            expr,
                            &edge_var,
                            &dst_var,
                            &mut edge_props,
                            &mut dst_props,
                            &mut edge_full,
                            &mut dst_full,
                        );
                    }
                }
            }
            PlanNodeEnum::InnerJoin(_)
            | PlanNodeEnum::LeftJoin(_)
            | PlanNodeEnum::RightJoin(_)
            | PlanNodeEnum::FullOuterJoin(_)
            | PlanNodeEnum::SemiJoin(_) => {
                if node_references_var(anc, &edge_var) {
                    edge_full = true;
                }
                if node_references_var(anc, &dst_var) {
                    dst_full = true;
                }
            }
            PlanNodeEnum::Flatten(_)
            | PlanNodeEnum::Limit(_)
            | PlanNodeEnum::Dedup(_) => {}
            _ => {}
        }
        if edge_full && dst_full {
            break;
        }
    }
    let edge_out = if edge_full {
        None
    } else {
        Some(edge_props.into_iter().collect())
    };
    let dst_out = if dst_full {
        None
    } else {
        Some(dst_props.into_iter().collect())
    };
    (edge_out, dst_out)
}

/// Collect property versus whole-value uses of the hop variables inside one
/// expression. A `var.prop` or `EdgeProperty(var, prop)` records a prunable
/// demand, any other occurrence of the variable marks the whole value.
fn collect_expr_needs(
    expr: &Expression,
    edge_var: &str,
    dst_var: &str,
    edge_props: &mut std::collections::BTreeSet<String>,
    dst_props: &mut std::collections::BTreeSet<String>,
    edge_full: &mut bool,
    dst_full: &mut bool,
) {
    match expr {
        Expression::Variable(name) => {
            if name == edge_var {
                *edge_full = true;
            }
            if name == dst_var {
                *dst_full = true;
            }
        }
        Expression::Property { object, property } => {
            if let Expression::Variable(name) = object.as_ref() {
                if name == edge_var {
                    edge_props.insert(property.clone());
                    return;
                }
                if name == dst_var {
                    dst_props.insert(property.clone());
                    return;
                }
            }
            collect_expr_needs(
                object, edge_var, dst_var, edge_props, dst_props, edge_full, dst_full,
            );
        }
        Expression::EdgeProperty {
            edge_name,
            property,
        } => {
            if edge_name == edge_var {
                edge_props.insert(property.clone());
            } else if edge_name == dst_var {
                dst_props.insert(property.clone());
            } else {
                if edge_name.as_str() == edge_var {
                    *edge_full = true;
                }
                if edge_name.as_str() == dst_var {
                    *dst_full = true;
                }
            }
        }
        Expression::TagProperty { tag_name, property } => {
            if tag_name == edge_var {
                edge_props.insert(property.clone());
            } else if tag_name == dst_var {
                dst_props.insert(property.clone());
            }
        }
        Expression::StructField { base, .. } => {
            if let Expression::Variable(name) = base.as_ref() {
                if name == edge_var {
                    *edge_full = true;
                    return;
                }
                if name == dst_var {
                    *dst_full = true;
                    return;
                }
            }
            collect_expr_needs(
                base, edge_var, dst_var, edge_props, dst_props, edge_full, dst_full,
            );
        }
        _ => {
            for child in expr.children() {
                collect_expr_needs(
                    child, edge_var, dst_var, edge_props, dst_props, edge_full, dst_full,
                );
                if *edge_full && *dst_full {
                    break;
                }
            }
        }
    }
}

/// Syntactic closed-loop check: typed edge fanout with a planned destination
/// tag. Storage schemas are verified at execution time with a row-path
/// fallback, so the planner only gates the obvious open shapes.
fn expand_closed_loop(expand: &ExpandAllNode) -> bool {
    if expand.any_edge_type() || expand.edge_types().is_empty() {
        return false;
    }
    true
}

/// Whether the hop may skip its row view. The direct consumer chain through
/// constant-true residual filters must end at a column-capable terminator:
/// an all-passthrough/constant project, a bare-variable count aggregate, or a
/// seed-tolerant next hop. Full-entity demands never skip.
fn expand_skip_rows(
    expand: &ExpandAllNode,
    ancestors: &[&PlanNodeEnum],
    edge_props: Option<&Vec<String>>,
    dst_props: Option<&Vec<String>>,
) -> bool {
    let Some(edge_needs) = edge_props else {
        return false;
    };
    let Some(dst_needs) = dst_props else {
        return false;
    };
    if !edge_needs.is_empty() || !dst_needs.is_empty() {
        return false;
    }
    if !expand_closed_loop(expand) {
        return false;
    }
    let mut rest = ancestors.iter().rev();
    let terminator = loop {
        match rest.next() {
            None => return false,
            Some(PlanNodeEnum::Filter(filter)) => {
                if !filter.subqueries().is_empty() {
                    return false;
                }
                let is_true = filter
                    .condition()
                    .get_expression()
                    .is_some_and(|expr| matches!(expr, Expression::Literal(Value::Bool(true))));
                if !is_true {
                    return false;
                }
            }
            Some(node) => break node,
        }
    };
    match terminator {
        PlanNodeEnum::Project(project) => {
            if !project.subqueries().is_empty() {
                return false;
            }
            project.columns().iter().all(|col| {
                col.expression
                    .expression()
                    .map(|meta| is_passthrough_or_const(meta.inner()))
                    .unwrap_or(true)
            })
        }
        PlanNodeEnum::Aggregate(agg) => {
            if !agg.group_keys().is_empty() || !agg.grouping_sets().is_empty() {
                return false;
            }
            if agg.aggregation_distinct().iter().any(|d| *d) {
                return false;
            }
            if agg.aggregation_filters().iter().any(|f| f.is_some()) {
                return false;
            }
            let funcs = agg.aggregation_functions();
            let args = agg.aggregation_args();
            if funcs.is_empty() || funcs.len() != args.len() {
                return false;
            }
            funcs.iter().zip(args.iter()).all(|(func, arg)| {
                matches!(
                    func,
                    linkrs_core::types::operators::AggregateFunction::Count
                ) && (arg.is_empty()
                    || (arg.len() == 1 && matches!(arg[0], Expression::Variable(_))))
            })
        }
        PlanNodeEnum::ExpandAll(next) => {
            let Some(dst_var) = expand.col_names().get(2) else {
                return false;
            };
            if next.col_names().first().map(String::as_str) != Some(dst_var.as_str()) {
                return false;
            }
            if next.path_semantic().is_some() {
                return false;
            }
            // Structural seed tolerance only: a count-only tail already has
            // the single-step filter-free shape, so reading its not-yet
            // computed flag here is unnecessary.
            next.step_limit().unwrap_or(1) == 1
                && next.step_limits().is_none()
                && next.filter().is_none()
                && next.src_vids().is_empty()
        }
        _ => false,
    }
}

/// Project shape that evaluates from the typed layout without reading rows.
fn is_passthrough_or_const(expression: &Expression) -> bool {
    match expression {
        Expression::Variable(_) | Expression::Literal(_) => true,
        Expression::Property { object, .. } => {
            matches!(object.as_ref(), Expression::Variable(_))
        }
        _ => false,
    }
}

/// Apply the flag decisions to the matching `ExpandAll` nodes in place.
fn apply_decisions(root: &mut PlanNodeEnum, decisions: &HashMap<i64, ExpandDecision>) -> bool {
    let mut changed = false;
    if let PlanNodeEnum::ExpandAll(expand) = root {
        if let Some(decision) = decisions.get(&expand.id()) {
            if expand.id_only() != decision.id_only
                || expand.count_only() != decision.count_only
                || expand.lightweight_source() != decision.lightweight_source
                || expand.edge_required_props() != decision.edge_props.as_ref()
                || expand.dst_required_props() != decision.dst_props.as_ref()
                || expand.closed_loop() != decision.closed_loop
                || expand.skip_rows() != decision.skip_rows
            {
                expand.set_id_only(decision.id_only);
                expand.set_count_only(decision.count_only);
                expand.set_lightweight_source(decision.lightweight_source);
                expand.set_edge_required_props(decision.edge_props.clone());
                expand.set_dst_required_props(decision.dst_props.clone());
                expand.set_closed_loop(decision.closed_loop);
                expand.set_skip_rows(decision.skip_rows);
                changed = true;
            }
        }
    }
    use PlanNodeEnum::*;
    match root {
        Project(n) => changed |= apply_decisions(n.input_mut(), decisions),
        Filter(n) => changed |= apply_decisions(n.input_mut(), decisions),
        Flatten(n) => changed |= apply_decisions(n.input_mut(), decisions),
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
    use linkrs_core::types::expr::expression_context::ExpressionAnalysisContext;
    use linkrs_core::types::expr::{ContextualExpression, ExpressionMeta};
    use linkrs_core::types::operators::AggregateFunction;
    use linkrs_core::Expression;
    use linkrs_core::Value;
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
        let col = linkrs_core::YieldColumn {
            expression: ctx_expr(expr),
            alias: dst.to_string(),
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
        let col = linkrs_core::YieldColumn {
            expression: ctx_expr(expr),
            alias: var.to_string(),
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
            op: linkrs_core::types::operators::BinaryOperator::LessThan,
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

    fn hop_tagged(edge: &str, vars: [&str; 3], dst_tag: &str, input: PlanNodeEnum) -> PlanNodeEnum {
        let mut expand = ExpandAllNode::new(1, vec![edge.to_string()], "OUT");
        expand.set_step_limit(1);
        expand.set_col_names(vars.iter().map(|s| s.to_string()).collect());
        expand.set_dst_tag(dst_tag.to_string());
        expand.add_input(input);
        PlanNodeEnum::ExpandAll(expand)
    }

    fn project_prop_col(
        input: PlanNodeEnum,
        var: &str,
        prop: &str,
        alias: &str,
    ) -> PlanNodeEnum {
        let col = linkrs_core::YieldColumn {
            expression: ctx_expr(Expression::Property {
                object: Box::new(Expression::Variable(var.to_string())),
                property: prop.to_string(),
            }),
            alias: alias.to_string(),
        };
        PlanNodeEnum::Project(ProjectNode::new(input, vec![col]).expect("project should build"))
    }

    #[test]
    fn prop_needs_empty_when_only_counted() {
        let chain = count_field_agg(
            project_pass_var(
                hop_tagged("Link", ["a", "r", "b"], "Node", anchor_scan("a")),
                "r",
            ),
            "r",
        );
        let (annotated, _) = annotate_expand_all(&chain);
        let hops = expand_alls(&annotated);
        let hop = hop_by_dst(&hops, "b");
        assert_eq!(
            hop.edge_required_props(),
            Some(&vec![]),
            "counted edge needs no properties"
        );
        assert_eq!(
            hop.dst_required_props(),
            Some(&vec![]),
            "uncounted destination needs no properties"
        );
    }

    #[test]
    fn prop_needs_collects_edge_and_dst_props() {
        let proj = project_prop_col(
            hop_tagged("Link", ["a", "r", "b"], "Node", anchor_scan("a")),
            "r",
            "weight",
            "w",
        );
        let (annotated, _) = annotate_expand_all(&proj);
        let hops = expand_alls(&annotated);
        let hop = hop_by_dst(&hops, "b");
        assert_eq!(
            hop.edge_required_props(),
            Some(&vec!["weight".to_string()]),
            "edge property demand must be collected"
        );
        let proj2 = project_prop_col(
            hop_tagged("Link", ["a", "r", "b"], "Node", anchor_scan("a")),
            "b",
            "name",
            "n",
        );
        let (annotated2, _) = annotate_expand_all(&proj2);
        let hops2 = expand_alls(&annotated2);
        assert_eq!(
            hop_by_dst(&hops2, "b").dst_required_props(),
            Some(&vec!["name".to_string()]),
            "destination property demand must be collected"
        );
    }

    #[test]
    fn full_edge_use_blocks_narrowing() {
        let chain = project_pass_var(
            hop_tagged("Link", ["a", "r", "b"], "Node", anchor_scan("a")),
            "r",
        );
        let (annotated, _) = annotate_expand_all(&chain);
        let hops = expand_alls(&annotated);
        assert_eq!(
            hop_by_dst(&hops, "b").edge_required_props(),
            None,
            "whole-edge return needs the full value"
        );
    }

    #[test]
    fn closed_loop_needs_dst_tag_and_typed_edges() {
        let tagged = hop_tagged("Link", ["a", "r", "b"], "Node", anchor_scan("a"));
        let (annotated, _) = annotate_expand_all(&tagged);
        assert!(
            expand_alls(&annotated)[0].closed_loop(),
            "typed edges with a destination tag form a closed loop"
        );
        let untagged = hop("Link", ["a", "r", "b"], anchor_scan("a"));
        let (annotated2, _) = annotate_expand_all(&untagged);
        assert!(
            expand_alls(&annotated2)[0].closed_loop(),
            "typed edges stay closed-loop syntactically; storage re-verifies schemas"
        );
        let mut open = ExpandAllNode::new(1, vec![], "OUT");
        open.set_step_limit(1);
        open.set_col_names(vec!["a".to_string(), "r".to_string(), "b".to_string()]);
        open.set_dst_tag("Node".to_string());
        open.add_input(anchor_scan("a"));
        let (annotated3, _) =
            annotate_expand_all(&PlanNodeEnum::ExpandAll(open));
        assert!(
            !expand_alls(&annotated3)[0].closed_loop(),
            "untyped fanout is not a closed loop"
        );
    }

    #[test]
    fn skip_rows_for_passthrough_and_count_but_not_sort() {
        let count_chain = count_field_agg(
            project_pass_var(
                hop_tagged("Link", ["a", "r", "b"], "Node", anchor_scan("a")),
                "r",
            ),
            "r",
        );
        let (annotated, _) = annotate_expand_all(&count_chain);
        assert!(
            expand_alls(&annotated)[0].skip_rows(),
            "bare count terminator is column-capable"
        );
        let sort_chain = PlanNodeEnum::Sort(
            crate::planning::plan::core::nodes::operation::sort_node::SortNode::new(
                hop_tagged("Link", ["a", "r", "b"], "Node", anchor_scan("a")),
                vec![crate::planning::plan::core::nodes::operation::sort_node::SortItem {
                    expression: Expression::Variable("r".to_string()),
                    direction: linkrs_core::types::graph_schema::OrderDirection::Asc,
                }],
            )
            .expect("sort should build"),
        );
        let (annotated2, _) = annotate_expand_all(&sort_chain);
        assert!(
            !expand_alls(&annotated2)[0].skip_rows(),
            "sort needs rows and blocks the rowless path"
        );
    }

    #[test]
    fn skip_rows_for_chained_seed_hop() {
        let chain = hop_tagged(
            "Link",
            ["b", "e2", "c"],
            "Node",
            hop_tagged("Link", ["a", "e1", "b"], "Node", anchor_scan("a")),
        );
        let (annotated, _) = annotate_expand_all(&chain);
        let hops = expand_alls(&annotated);
        assert!(
            hop_by_dst(&hops, "b").skip_rows(),
            "intermediate hop feeding a seed-tolerant hop may skip rows"
        );
    }
}
