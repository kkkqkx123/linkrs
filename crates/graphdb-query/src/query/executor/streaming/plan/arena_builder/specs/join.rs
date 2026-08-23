//! Join and apply (correlated subquery) spec builders.

use crate::core::types::expr::Expression;
use crate::query::executor::build_error::PlanBuildError;
use crate::query::executor::streaming::operators::spec::{ApplySpec, BuildSide, JoinSpec};

use super::contextual_to_expression;

// ── Join spec builders ────────────────────────────────────────────────────────

pub(in crate::query::executor::streaming::plan::arena_builder) fn build_inner_join_spec(
    node: &crate::query::planning::plan::core::nodes::join::join_node::InnerJoinNode,
) -> Result<JoinSpec, PlanBuildError> {
    // Default path: a valid equi-condition produces the condition
    // (nested-loop) form; a join without usable keys keeps the default.
    build_join_with_condition(
        node.hash_keys(),
        node.probe_keys(),
        JoinSpec::InnerJoin {
            join_condition: None,
        },
    )
}

/// Force the condition (nested-loop) form for an inner join, regardless of
/// hash keys.  Selected by the cost-based `JoinAlgorithm` decision when both
/// operands are small enough that building a hash table is not worth it.
pub(in crate::query::executor::streaming::plan::arena_builder) fn build_inner_join_nested_loop_spec(
    node: &crate::query::planning::plan::core::nodes::join::join_node::InnerJoinNode,
) -> Result<JoinSpec, PlanBuildError> {
    build_join_with_condition(
        node.hash_keys(),
        node.probe_keys(),
        JoinSpec::InnerJoin {
            join_condition: None,
        },
    )
}

/// Force the hash-join form for an inner join when valid equi keys exist.
/// Selected by the cost-based `JoinAlgorithm::HashJoin` decision; falls back
/// to the condition form when the keys do not form a valid equi join.
pub(in crate::query::executor::streaming::plan::arena_builder) fn build_inner_join_hash_spec(
    node: &crate::query::planning::plan::core::nodes::join::join_node::InnerJoinNode,
) -> Result<JoinSpec, PlanBuildError> {
    build_join_with_keys(
        node.hash_keys(),
        node.probe_keys(),
        JoinSpec::InnerJoin {
            join_condition: None,
        },
    )
}

pub(in crate::query::executor::streaming::plan::arena_builder) fn build_left_join_spec(
    node: &crate::query::planning::plan::core::nodes::join::join_node::LeftJoinNode,
) -> Result<JoinSpec, PlanBuildError> {
    build_join_with_condition(
        node.hash_keys(),
        node.probe_keys(),
        JoinSpec::LeftJoin {
            join_condition: None,
        },
    )
}

/// Force the condition (nested-loop) form for a left join (see the inner
/// variant for the rationale).
pub(in crate::query::executor::streaming::plan::arena_builder) fn build_left_join_nested_loop_spec(
    node: &crate::query::planning::plan::core::nodes::join::join_node::LeftJoinNode,
) -> Result<JoinSpec, PlanBuildError> {
    build_join_with_condition(
        node.hash_keys(),
        node.probe_keys(),
        JoinSpec::LeftJoin {
            join_condition: None,
        },
    )
}

/// Force the hash-join form for a left join when valid equi keys exist (see
/// the inner variant for the rationale).
pub(in crate::query::executor::streaming::plan::arena_builder) fn build_left_join_hash_spec(
    node: &crate::query::planning::plan::core::nodes::join::join_node::LeftJoinNode,
) -> Result<JoinSpec, PlanBuildError> {
    build_join_with_keys(
        node.hash_keys(),
        node.probe_keys(),
        JoinSpec::LeftJoin {
            join_condition: None,
        },
    )
}

pub(in crate::query::executor::streaming::plan::arena_builder) fn build_right_join_spec(
    node: &crate::query::planning::plan::core::nodes::join::join_node::RightJoinNode,
) -> Result<JoinSpec, PlanBuildError> {
    build_join_with_condition(
        node.hash_keys(),
        node.probe_keys(),
        JoinSpec::RightJoin {
            join_condition: None,
        },
    )
}

pub(in crate::query::executor::streaming::plan::arena_builder) fn build_full_outer_join_spec(
    node: &crate::query::planning::plan::core::nodes::join::join_node::FullOuterJoinNode,
) -> Result<JoinSpec, PlanBuildError> {
    build_join_with_condition(
        node.hash_keys(),
        node.probe_keys(),
        JoinSpec::FullOuterJoin {
            join_condition: None,
        },
    )
}

pub(in crate::query::executor::streaming::plan::arena_builder) fn build_semi_join_spec(
    node: &crate::query::planning::plan::core::nodes::join::join_node::SemiJoinNode,
) -> Result<JoinSpec, PlanBuildError> {
    // Merge the equi condition derived from the hash/probe keys with the
    // Mark-Join residual condition (non-equi correlation) so both survive
    // into the physical operator.
    let mut join_condition = equi_condition_from_keys(node.hash_keys(), node.probe_keys())?;
    if let Some(residual) = node.join_condition().and_then(|c| c.get_expression()) {
        join_condition = Some(match join_condition {
            Some(equi) => Expression::Binary {
                left: Box::new(equi),
                op: crate::core::types::operators::BinaryOperator::And,
                right: Box::new(residual),
            },
            None => residual,
        });
    }
    Ok(JoinSpec::SemiJoin {
        join_condition,
        anti: node.is_anti(),
    })
}

/// Build an equi-condition from the hash/probe key pairs.
///
/// Returns `Ok(None)` when the keys do not form a valid equi join (missing,
/// empty or unequal-length key lists).
fn equi_condition_from_keys(
    hash_keys: &[crate::core::types::expr::ContextualExpression],
    probe_keys: &[crate::core::types::expr::ContextualExpression],
) -> Result<Option<Expression>, PlanBuildError> {
    if hash_keys.is_empty() || probe_keys.is_empty() || hash_keys.len() != probe_keys.len() {
        return Ok(None);
    }
    let left_first = hash_keys[0].get_expression().ok_or_else(|| {
        PlanBuildError::expression(
            "JoinCondition",
            0,
            format!("{:?}", hash_keys[0]),
            "Failed to resolve hash key expression",
        )
    })?;
    let right_first = probe_keys[0].get_expression().ok_or_else(|| {
        PlanBuildError::expression(
            "JoinCondition",
            0,
            format!("{:?}", probe_keys[0]),
            "Failed to resolve probe key expression",
        )
    })?;
    let mut condition = Expression::Binary {
        left: Box::new(left_first),
        op: crate::core::types::operators::BinaryOperator::Equal,
        right: Box::new(right_first),
    };
    for i in 1..hash_keys.len() {
        let left = hash_keys[i].get_expression().ok_or_else(|| {
            PlanBuildError::expression(
                "JoinCondition",
                0,
                format!("{:?}", hash_keys[i]),
                "Failed to resolve hash key expression",
            )
        })?;
        let right = probe_keys[i].get_expression().ok_or_else(|| {
            PlanBuildError::expression(
                "JoinCondition",
                0,
                format!("{:?}", probe_keys[i]),
                "Failed to resolve probe key expression",
            )
        })?;
        let eq = Expression::Binary {
            left: Box::new(left),
            op: crate::core::types::operators::BinaryOperator::Equal,
            right: Box::new(right),
        };
        condition = Expression::Binary {
            left: Box::new(condition),
            op: crate::core::types::operators::BinaryOperator::And,
            right: Box::new(eq),
        };
    }
    Ok(Some(condition))
}

fn hash_key_expressions(
    keys: &[crate::core::types::expr::ContextualExpression],
) -> Result<Vec<Expression>, PlanBuildError> {
    keys.iter().map(contextual_to_expression).collect()
}

/// Default join spec builder: valid equi keys produce the hash join form
/// (`HashJoin`/`HashLeftJoin`), invalid keys fall back to `default`.
pub(in crate::query::executor::streaming::plan::arena_builder) fn build_join_with_keys(
    hash_keys: &[crate::core::types::expr::ContextualExpression],
    probe_keys: &[crate::core::types::expr::ContextualExpression],
    default: JoinSpec,
) -> Result<JoinSpec, PlanBuildError> {
    match equi_condition_from_keys(hash_keys, probe_keys)? {
        Some(_condition) => match default {
            JoinSpec::InnerJoin { .. } => Ok(JoinSpec::HashJoin {
                join_condition: None,
                hash_keys: hash_key_expressions(hash_keys)?,
                probe_keys: hash_key_expressions(probe_keys)?,
                build_side: BuildSide::default(),
            }),
            JoinSpec::LeftJoin { .. } => Ok(JoinSpec::HashLeftJoin {
                join_condition: None,
                hash_keys: hash_key_expressions(hash_keys)?,
                probe_keys: hash_key_expressions(probe_keys)?,
                build_side: BuildSide::default(),
            }),
            _ => build_join_with_condition(hash_keys, probe_keys, default),
        },
        None => Ok(default),
    }
}

/// Condition (nested-loop) join spec builder: the equi-condition is attached
/// to `default` but no hash table is requested.
fn build_join_with_condition(
    hash_keys: &[crate::core::types::expr::ContextualExpression],
    probe_keys: &[crate::core::types::expr::ContextualExpression],
    default: JoinSpec,
) -> Result<JoinSpec, PlanBuildError> {
    let Some(condition) = equi_condition_from_keys(hash_keys, probe_keys)? else {
        return Ok(default);
    };
    match default {
        JoinSpec::InnerJoin { .. } => Ok(JoinSpec::InnerJoin {
            join_condition: Some(condition),
        }),
        JoinSpec::LeftJoin { .. } => Ok(JoinSpec::LeftJoin {
            join_condition: Some(condition),
        }),
        JoinSpec::RightJoin { .. } => Ok(JoinSpec::RightJoin {
            join_condition: Some(condition),
        }),
        JoinSpec::FullOuterJoin { .. } => Ok(JoinSpec::FullOuterJoin {
            join_condition: Some(condition),
        }),
        JoinSpec::SemiJoin { anti, .. } => Ok(JoinSpec::SemiJoin {
            join_condition: Some(condition),
            anti,
        }),
        _ => Ok(default),
    }
}

// ── Set/Apply spec builders ───────────────────────────────────────────────────

pub(in crate::query::executor::streaming::plan::arena_builder) fn build_pattern_apply_spec(
    node: &crate::query::planning::plan::core::nodes::graph_operations::graph_operations_node::PatternApplyNode,
) -> Result<ApplySpec, PlanBuildError> {
    Ok(ApplySpec::PatternApply {
        hash_keys: node
            .hash_keys()
            .iter()
            .map(contextual_to_expression)
            .collect::<Result<Vec<_>, _>>()?,
        probe_keys: node
            .probe_keys()
            .iter()
            .map(contextual_to_expression)
            .collect::<Result<Vec<_>, _>>()?,
        anti: node.is_anti_predicate(),
    })
}

pub(in crate::query::executor::streaming::plan::arena_builder) fn build_rollup_apply_spec(
    node: &crate::query::planning::plan::core::nodes::graph_operations::graph_operations_node::RollUpApplyNode,
) -> Result<ApplySpec, PlanBuildError> {
    Ok(ApplySpec::RollUpApply {
        compare_columns: node.compare_cols().to_vec(),
        collect_column: node.collect_col().map(|column| column.to_string()),
    })
}

pub(in crate::query::executor::streaming::plan::arena_builder) fn build_apply_spec(
    node: &crate::query::planning::plan::core::nodes::graph_operations::graph_operations_node::ApplyNode,
) -> Result<ApplySpec, PlanBuildError> {
    Ok(ApplySpec::Apply {
        kind: match node.apply_kind() {
            crate::query::planning::plan::core::nodes::graph_operations::graph_operations_node::ApplyKind::Semi => crate::query::executor::streaming::operators::spec::ApplyKind::Semi,
            crate::query::planning::plan::core::nodes::graph_operations::graph_operations_node::ApplyKind::Anti => crate::query::executor::streaming::operators::spec::ApplyKind::Anti,
            crate::query::planning::plan::core::nodes::graph_operations::graph_operations_node::ApplyKind::Single => crate::query::executor::streaming::operators::spec::ApplyKind::Single,
            crate::query::planning::plan::core::nodes::graph_operations::graph_operations_node::ApplyKind::All => crate::query::executor::streaming::operators::spec::ApplyKind::All,
            crate::query::planning::plan::core::nodes::graph_operations::graph_operations_node::ApplyKind::Standard => crate::query::executor::streaming::operators::spec::ApplyKind::Standard,
        },
        correlated_columns: node.correlated_cols().to_vec(),
    })
}
