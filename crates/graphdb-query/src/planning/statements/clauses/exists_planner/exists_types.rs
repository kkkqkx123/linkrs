use graphdb_core::types::operators::{AggregateFunction, BinaryOperator, UnaryOperator};
use graphdb_core::types::{ContextualExpression, Expression};

/// An EXISTS / IN subquery at a conjunctive WHERE position.
#[derive(Debug, Clone)]
pub struct ExistsSpec {
    /// The subquery body (patterns + WHERE + optional RETURN).
    pub body: graphdb_core::types::expr::SubqueryBody,
    /// NOT EXISTS / NOT IN.
    pub negated: bool,
    /// IN's left-hand expression (`None` for EXISTS).
    pub left_expr: Option<Expression>,
}

/// A planned subquery: the subquery plan plus its correlated keys.
#[derive(Debug, Clone)]
pub struct PlannedSubquery {
    /// Stable identity (`SubqueryBody.id`) assigned by the id allocator; the
    /// runtime dispatches on it to find the compiled runner.
    /// The conjunctive path (`plan_subquery`) leaves it 0 — those subqueries
    /// never materialize runners.
    pub id: u64,
    /// The subquery plan (pattern scans + subquery-local filters, or the
    /// `Filter -> CrossJoin -> Argument` correlated right subtree).
    ///
    /// Boxed so plan nodes can carry `Vec<PlannedSubquery>` without an
    /// infinite-size type cycle (SubPlan roots embed `PlanNodeEnum` by value).
    pub plan: Box<crate::planning::plan::SubPlan>,
    /// Outer-side (left layout) key expressions.
    pub hash_keys: Vec<ContextualExpression>,
    /// Subquery-side (right layout) key expressions.
    pub probe_keys: Vec<ContextualExpression>,
    /// True when the subquery is planned as a `CorrelatedApply` (per-row
    /// re-execution over an `Argument` frame) instead of a key-based
    /// `PatternApply`. This is a planning-time routing flag only.
    pub correlated: bool,
    /// When set, the non-equi correlated subquery is decorrelated as a
    /// Mark-Join (a `SemiJoin` carrying the correlated residual as its join
    /// condition) instead of a per-row `CorrelatedApply`. The caller wraps
    /// the join with [`wrap_mark_join`](super::wrap_mark_join).
    pub mark_join_condition: Option<ContextualExpression>,
    /// When set, the correlated scalar aggregate subquery
    /// (`RETURN agg(...) WHERE corr = outer.x`) is decorrelated as a
    /// Group-Join: the right subtree pre-aggregates per probe key and the
    /// runtime backfills the outer row by hash-key lookup.
    pub group_join: Option<PlannedGroupJoin>,
}

/// Group-Join decorrelation info for a scalar aggregate correlated subquery.
///
/// The planned right subtree ends in `Project(probe keys) -> Aggregate`
/// whose output rows are `[group_key..., agg_value]`; the runtime
/// materializes them once into a `HashMap<Vec<Value>, Value>` keyed by the
/// group key and answers per-row lookups with the outer-side
/// [`PlannedGroupJoin::hash_keys`] expressions.
#[derive(Debug, Clone)]
pub struct PlannedGroupJoin {
    /// Outer-side key expressions, evaluated against the hosting row's
    /// layout (same convention as `PatternApply.hash_keys`).
    pub hash_keys: Vec<Expression>,
    /// Number of leading group-key columns in each materialized row; the
    /// aggregated value follows at that index.
    pub key_columns: usize,
    /// The single aggregate function computed by the right subtree.
    pub function: AggregateFunction,
    /// DISTINCT flag of the aggregate.
    pub distinct: bool,
}

/// Extracted keys and residual conditions of a subquery.
pub(crate) type ExtractedKeys = (Vec<Expression>, Vec<Expression>, Vec<Expression>);

/// Monotonically increasing allocator for stable `SubqueryBody.id`s within a
/// single query planning pass. Re-planning the same AST with a fresh
/// allocator re-assigns ids from 0.
#[derive(Debug, Default)]
pub struct SubqueryIdAllocator {
    next: u64,
}

impl SubqueryIdAllocator {
    pub fn new() -> Self {
        Self { next: 0 }
    }

    /// Allocate the next stable id.
    pub fn allocate(&mut self) -> u64 {
        let id = self.next;
        self.next += 1;
        id
    }
}

/// Walk the AND-tree of `expr`, collect every conjunctive EXISTS/IN into
/// `specs`, and rebuild the condition with `true` substituted at the
/// extraction sites.
pub fn extract_conjunctive_exists(expr: &Expression, specs: &mut Vec<ExistsSpec>) -> Expression {
    match expr {
        Expression::Binary {
            left,
            op: BinaryOperator::And,
            right,
        } => {
            let left_res = extract_conjunctive_exists(left, specs);
            let right_res = extract_conjunctive_exists(right, specs);
            Expression::binary(left_res, BinaryOperator::And, right_res)
        }
        Expression::Exists { body } => {
            specs.push(ExistsSpec {
                body: body.as_ref().clone(),
                negated: false,
                left_expr: None,
            });
            Expression::literal(true)
        }
        Expression::In {
            expr,
            subquery,
            negated,
        } => {
            specs.push(ExistsSpec {
                body: subquery.as_ref().clone(),
                negated: *negated,
                left_expr: Some(expr.as_ref().clone()),
            });
            Expression::literal(true)
        }
        // `NOT EXISTS { … }` / `NOT (x IN { … })` parse as a `NOT` prefix.
        Expression::Unary {
            op: UnaryOperator::Not,
            operand,
        } => match operand.as_ref() {
            Expression::Exists { body } => {
                specs.push(ExistsSpec {
                    body: body.as_ref().clone(),
                    negated: true,
                    left_expr: None,
                });
                Expression::literal(true)
            }
            Expression::In {
                expr,
                subquery,
                negated,
            } => {
                specs.push(ExistsSpec {
                    body: subquery.as_ref().clone(),
                    negated: !*negated,
                    left_expr: Some(expr.as_ref().clone()),
                });
                Expression::literal(true)
            }
            _ => expr.clone(),
        },
        _ => expr.clone(),
    }
}

/// Whether the (residual) condition is a plain `true` AND-chain and thus
/// needs no filter node.
pub fn is_trivially_true(expr: &Expression) -> bool {
    match expr {
        Expression::Literal(graphdb_core::Value::Bool(true)) => true,
        Expression::Binary {
            left,
            op: BinaryOperator::And,
            right,
        } => is_trivially_true(left) && is_trivially_true(right),
        _ => false,
    }
}

/// Collect every EXISTS / IN subquery at expression level of `expr`,
/// assigning each body a stable id from `id_alloc` (in place).
pub fn collect_expression_subqueries(
    expr: &mut Expression,
    id_alloc: &mut SubqueryIdAllocator,
) -> Vec<graphdb_core::types::expr::SubqueryBody> {
    let mut bodies = Vec::new();
    collect_expression_subqueries_inner(expr, id_alloc, &mut bodies);
    bodies
}

fn collect_expression_subqueries_inner(
    expr: &mut Expression,
    id_alloc: &mut SubqueryIdAllocator,
    out: &mut Vec<graphdb_core::types::expr::SubqueryBody>,
) {
    match expr {
        Expression::Exists { body } => {
            body.id = id_alloc.allocate();
            out.push(body.as_ref().clone());
        }
        Expression::In {
            expr: left,
            subquery,
            ..
        } => {
            subquery.id = id_alloc.allocate();
            out.push(subquery.as_ref().clone());
            collect_expression_subqueries_inner(left, id_alloc, out);
        }
        Expression::Aggregate { args, filter, .. } => {
            for arg in args.iter_mut() {
                collect_expression_subqueries_inner(arg, id_alloc, out);
            }
            if let Some(filter_expr) = filter {
                collect_expression_subqueries_inner(filter_expr, id_alloc, out);
            }
        }
        _ => {
            for child in expr.children_mut() {
                collect_expression_subqueries_inner(child, id_alloc, out);
            }
        }
    }
}
