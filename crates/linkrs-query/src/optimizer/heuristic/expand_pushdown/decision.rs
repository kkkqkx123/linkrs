use std::collections::HashMap;

use crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
use crate::planning::plan::core::nodes::base::plan_node_traits::PlanNode;
use crate::planning::plan::core::nodes::traversal::traversal_node::ExpandAllNode;
use linkrs_core::types::expr::Expression;
use linkrs_core::Value;

/// Flat bypass names demanded by one hop in layout order.
fn flat_bypass_names(
    col_names: &[String],
    edge_props: &Option<Vec<String>>,
    dst_props: &Option<Vec<String>>,
) -> Vec<String> {
    let (Some(edge_var), Some(dst_var)) = (col_names.get(1), col_names.get(2)) else {
        return Vec::new();
    };
    let mut names = Vec::new();
    if let Some(props) = edge_props {
        for prop in props {
            names.push(format!("{edge_var}.{prop}"));
        }
    }
    if let Some(props) = dst_props {
        for prop in props {
            names.push(format!("{dst_var}.{prop}"));
        }
    }
    names
}

/// Per-hop property demand: `(edge property demand, destination property demand)`.
pub(super) type PropDemand = (Option<Vec<String>>, Option<Vec<String>>);

/// Whether the hop's demanded bypass names already exist in its input prefix.
/// Inputs come from descendant hops below the current one: a shared flat name
/// means the current hop rebinds a variable whose old bypass column is still
/// carried, so reusing that slot would read the old entity. Conflicting hops
/// conservatively stay on the row path.
pub(super) fn has_input_bypass_conflict(
    expand: &ExpandAllNode,
    edge_props: &Option<Vec<String>>,
    dst_props: &Option<Vec<String>>,
    candidates: &[(ExpandAllNode, Vec<&PlanNodeEnum>)],
    needs_map: &HashMap<i64, PropDemand>,
) -> bool {
    let current: std::collections::HashSet<String> =
        flat_bypass_names(expand.col_names(), edge_props, dst_props)
            .into_iter()
            .collect();
    if current.is_empty() {
        return false;
    }
    for (other, other_ancestors) in candidates {
        if other.id() == expand.id() {
            continue;
        }
        let below = other_ancestors.iter().any(|anc| match anc {
            PlanNodeEnum::ExpandAll(parent) => parent.id() == expand.id(),
            _ => false,
        });
        if !below {
            continue;
        }
        let Some((other_edge, other_dst)) = needs_map.get(&other.id()) else {
            continue;
        };
        for name in flat_bypass_names(other.col_names(), other_edge, other_dst) {
            if current.contains(&name) {
                return true;
            }
        }
    }
    false
}

/// Syntactic closed-loop check: typed edge fanout with a planned destination
/// tag. Storage schemas are verified at execution time with a row-path
/// fallback, so the planner only gates the obvious open shapes.
pub(super) fn expand_closed_loop(expand: &ExpandAllNode) -> bool {
    if expand.any_edge_type() || expand.edge_types().is_empty() {
        return false;
    }
    true
}

/// Whether the hop may skip its row view. The direct consumer chain through
/// constant-true residual filters must end at a column-capable terminator:
/// an all-passthrough/constant project, a bare-variable count aggregate, or a
/// seed-tolerant next hop. Full-entity demands never skip. Non-empty property
/// demands may skip only as flat bypass columns: the project terminator must
/// stay passthrough/constant (direct `var.prop` reads hit the compound slot),
/// the next-hop terminator must stay seed-tolerant, and the count terminator
/// never carries bypass columns.
pub(super) fn expand_skip_rows(
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
    let has_props = !edge_needs.is_empty() || !dst_needs.is_empty();
    if !expand_closed_loop(expand) {
        return false;
    }
    // Rowless output carries only the seed prefix plus edge/dst bypass
    // columns. A downstream `seed.prop` read without a flat slot would fall
    // back to per-row evaluation over an empty row view and drop rows, so
    // any seed property demand keeps the row path.
    if let Some(seed_var) = expand.col_names().first() {
        if ancestors.iter().any(|anc| ancestor_uses_seed_prop(anc, seed_var)) {
            return false;
        }
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
            // The count terminator never carries bypass columns: any property
            // demand keeps the row path so no useless vertex decode is paid.
            if has_props {
                return false;
            }
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
            // Chained rowless needs a rowless-capable downstream hop, not
            // just structural seed tolerance: the next hop must itself be a
            // syntactic closed loop so it can consume the bypass prefix
            // without an immediate row fallback.
            if !expand_closed_loop(next) {
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


/// Whether any ancestor demands `seed_var.prop`: such reads need the seed
/// row view (no flat slot is guaranteed for the seed), so the hop must stay
/// rowful. Bare `seed` references stay columnar-safe via typed gathering.
fn ancestor_uses_seed_prop(anc: &PlanNodeEnum, seed_var: &str) -> bool {
    match anc {
        PlanNodeEnum::Project(p) => p.columns().iter().any(|c| {
            c.expression
                .expression()
                .is_some_and(|meta| expr_uses_seed_prop(meta.inner(), seed_var))
        }),
        PlanNodeEnum::Filter(f) => f
            .condition()
            .get_expression()
            .is_some_and(|e| expr_uses_seed_prop(&e, seed_var)),
        PlanNodeEnum::Aggregate(a) => {
            if a.group_keys().iter().any(|k| k == seed_var) {
                return false;
            }
            a.aggregation_args().iter().flatten().any(|e| expr_uses_seed_prop(e, seed_var))
        }
        PlanNodeEnum::Sort(s) => s
            .sort_items()
            .iter()
            .any(|i| expr_uses_seed_prop(&i.expression, seed_var)),
        PlanNodeEnum::TopN(t) => t
            .sort_items()
            .iter()
            .any(|i| expr_uses_seed_prop(&i.expression, seed_var)),
        PlanNodeEnum::ExpandAll(e) => e
            .filter()
            .and_then(|f| f.get_expression())
            .is_some_and(|ex| expr_uses_seed_prop(&ex, seed_var)),
        _ => false,
    }
}

/// Whether `expr` reads `seed_var.prop` (directly or nested). Bare variable
/// uses stay columnar-safe and do not count.
fn expr_uses_seed_prop(expr: &Expression, seed_var: &str) -> bool {
    match expr {
        Expression::Property { object, .. } => {
            if let Expression::Variable(v) = object.as_ref() {
                if v == seed_var {
                    return true;
                }
            }
            expr_uses_seed_prop(object, seed_var)
        }
        _ => expr.children().iter().any(|c| expr_uses_seed_prop(c, seed_var)),
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
