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
use crate::planning::plan::core::nodes::base::plan_node_traits::PlanNode;
use crate::planning::plan::core::nodes::graph_operations::aggregate_node::AggregateNode;
use crate::planning::plan::core::nodes::traversal::traversal_node::ExpandAllNode;
use linkrs_core::types::expr::Expression;

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

    let mut needs_map: HashMap<i64, (Option<Vec<String>>, Option<Vec<String>>)> = HashMap::new();
    for (expand, ancestors) in &candidates {
        let (edge_props, dst_props) = expand_prop_needs(expand, ancestors);
        needs_map.insert(expand.id(), (edge_props, dst_props));
    }
    let mut decisions: HashMap<i64, ExpandDecision> = HashMap::new();
    for (expand, ancestors) in &candidates {
        let (edge_props, dst_props) = needs_map.get(&expand.id()).cloned().unwrap_or((None, None));
        let id_only = expand_id_only(expand, ancestors, edge_props.as_ref(), dst_props.as_ref());
        let count_only =
            expand_count_only(expand, ancestors, edge_props.as_ref(), dst_props.as_ref());
        let lightweight_source = id_only && source_unreferenced(expand, ancestors);
        let closed_loop = expand_closed_loop(expand);
        let conflict =
            has_input_bypass_conflict(expand, &edge_props, &dst_props, &candidates, &needs_map);
        let skip_rows = if conflict {
            false
        } else {
            expand_skip_rows(expand, ancestors, edge_props.as_ref(), dst_props.as_ref())
        };
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
/// become 0 because the null edge is not counted). A non-empty bypass demand
/// also blocks the raw-id path: raw ids carry no properties.
fn expand_id_only(
    expand: &ExpandAllNode,
    ancestors: &[&PlanNodeEnum],
    edge_props: Option<&Vec<String>>,
    dst_props: Option<&Vec<String>>,
) -> bool {
    if !fast_path_compatible(expand) {
        return false;
    }
    if !demand_is_empty(edge_props) || !demand_is_empty(dst_props) {
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

/// Empty property demand means topology or identity only. Whole-entity use
/// (`None`) and any named property both require materialized values.
fn demand_is_empty(demand: Option<&Vec<String>>) -> bool {
    matches!(demand, Some(props) if props.is_empty())
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
/// count-only aggregate, the hop supports the raw-id fast path, and neither
/// the edge nor the destination demand carries properties. Attribute counts
/// such as `count(var.prop)` stay on the normal expand path so the aggregate
/// observes nullability instead of edge degrees.
fn expand_count_only(
    expand: &ExpandAllNode,
    ancestors: &[&PlanNodeEnum],
    edge_props: Option<&Vec<String>>,
    dst_props: Option<&Vec<String>>,
) -> bool {
    if !fast_path_compatible(expand) {
        return false;
    }
    if !demand_is_empty(edge_props) || !demand_is_empty(dst_props) {
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

mod apply;
mod decision;
mod needs;
#[cfg(test)]
mod tests;

use apply::apply_decisions;
use decision::{expand_closed_loop, expand_skip_rows, has_input_bypass_conflict};
use needs::expand_prop_needs;
