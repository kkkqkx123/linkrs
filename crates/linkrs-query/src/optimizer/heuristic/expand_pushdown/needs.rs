use crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
use crate::planning::plan::core::nodes::base::plan_node_traits::PlanNode;
use crate::planning::plan::core::nodes::traversal::traversal_node::ExpandAllNode;
use linkrs_core::types::expr::Expression;

use super::{known_reference_ancestor, node_references_var};

/// Collect the downstream property demand for one hop's edge and destination
/// slots. `None` means the whole entity is needed (bare use outside a bare
/// count), `Some(vec)` lists the demanded property names with empty meaning
/// topology or identity only.
pub(super) fn expand_prop_needs(
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
                    let is_count = funcs.get(idx).is_some_and(|f| {
                        matches!(f, linkrs_core::types::operators::AggregateFunction::Count)
                    });
                    for arg in arg_list {
                        if is_count
                            && matches!(arg, Expression::Variable(n) if n == &edge_var || n == &dst_var)
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
            PlanNodeEnum::Flatten(_) | PlanNodeEnum::Limit(_) | PlanNodeEnum::Dedup(_) => {}
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
