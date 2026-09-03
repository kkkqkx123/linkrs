//! Unified MATCH Statement Planner
//!
//! Implement the StatementPlanner interface to handle the complete planning of MATCH queries.
//! It integrates the following functions:
//!   - Node and edge pattern matching (supports multiple paths)
//!   - WHERE condition filtering
//!   - RETURN Projection
//!   - ORDER BY: Sorting
//!   - LIMIT/SKIP – Pagination options
//!   - Selection of intelligent scanning strategies (index scanning, attribute scanning, full table scanning)

use crate::binder::validation::CypherClauseKind;
use crate::binder::validation::ValidationInfo;
use crate::binder::BoundStatement;
use crate::metadata::MetadataContext;
use crate::parser::ast::Stmt;
use crate::planning::join_order::JoinOrderStats;
use crate::planning::plan::SubPlan;
use crate::planning::planner::{Planner, PlannerError, ValidatedStatement};
use crate::planning::statements::clauses::{
    OrderByClausePlanner, PaginationPlanner, ReturnClausePlanner, WhereClausePlanner,
};
use crate::planning::statements::pattern_planner;
use crate::planning::statements::pattern_planner::PlanningContext;
use crate::planning::statements::plan_combiner;
use crate::planning::statements::statement_planner::{ClausePlanner, StatementPlanner};
use crate::QueryContext;
use graphdb_core::types::expr::expression_context::ExpressionAnalysisContext;
use std::sync::Arc;

/// Pagination Information Structure
#[derive(Debug, Clone)]
pub struct PaginationInfo {
    pub skip: usize,
    pub limit: usize,
}

/// MATCH Statement Planner
///
/// Responsible for converting MATCH queries into executable execution plans.
/// Implement the StatementPlanner interface to provide a unified planning entry point.
/// Delegates clause-level planning (WHERE, RETURN, ORDER BY, LIMIT) to ClausePlanner implementations.
/// Delegates pattern planning, index selection, plan combination, and expression construction
/// to dedicated sub-modules.
#[derive(Debug, Clone)]
pub struct MatchStatementPlanner {
    config: MatchPlannerConfig,
    expr_context: Option<Arc<ExpressionAnalysisContext>>,
    metadata_context: Option<MetadataContext>,
    join_order_stats: Option<JoinOrderStats>,
    where_planner: WhereClausePlanner,
    return_planner: ReturnClausePlanner,
    order_by_planner: OrderByClausePlanner,
    pagination_planner: PaginationPlanner,
}

#[derive(Debug, Clone)]
pub struct MatchPlannerConfig {
    pub default_limit: usize,
    pub max_limit: usize,
    pub enable_index_optimization: bool,
}

impl Default for MatchPlannerConfig {
    fn default() -> Self {
        Self {
            default_limit: 10000,
            max_limit: 100000,
            enable_index_optimization: true,
        }
    }
}

impl Default for MatchStatementPlanner {
    fn default() -> Self {
        Self::new()
    }
}

impl MatchStatementPlanner {
    pub fn new() -> Self {
        Self {
            config: MatchPlannerConfig::default(),
            expr_context: None,
            metadata_context: None,
            join_order_stats: None,
            where_planner: WhereClausePlanner::new(),
            return_planner: ReturnClausePlanner::new(),
            order_by_planner: OrderByClausePlanner::new(),
            pagination_planner: PaginationPlanner::new(),
        }
    }

    pub fn with_config(config: MatchPlannerConfig) -> Self {
        Self {
            config,
            expr_context: None,
            metadata_context: None,
            join_order_stats: None,
            where_planner: WhereClausePlanner::new(),
            return_planner: ReturnClausePlanner::new(),
            order_by_planner: OrderByClausePlanner::new(),
            pagination_planner: PaginationPlanner::new(),
        }
    }

    /// Attach real cardinality statistics for the join-order path.
    /// Without statistics the enumerator falls back to default constants.
    pub fn with_join_order_stats(mut self, stats: JoinOrderStats) -> Self {
        self.join_order_stats = Some(stats);
        self
    }
}

impl Planner for MatchStatementPlanner {
    fn match_planner(&self, stmt: &Stmt) -> bool {
        matches!(stmt, Stmt::Match(_))
    }

    fn transform(
        &mut self,
        validated: &ValidatedStatement,
        qctx: Arc<QueryContext>,
    ) -> Result<SubPlan, PlannerError> {
        let space_id = qctx.space_id().unwrap_or(1);
        let space_name = qctx.space_name().unwrap_or_else(|| "default".to_string());

        let validation_info = &validated.validation_info;

        self.expr_context = Some(validated.ast.expr_context().clone());

        for hint in &validation_info.optimization_hints {
            log::debug!("Optimization Tip: {:?}", hint);
        }

        self.plan_match_pattern(validated, space_id, &space_name, validation_info, &qctx)
    }

    fn plan_bound(
        &mut self,
        ctx: &crate::planning::context::PlanContext<'_>,
    ) -> Result<SubPlan, PlannerError> {
        let bound = ctx.bound;
        let qctx = ctx.qctx.clone();
        let metadata = ctx.metadata;
        let validated = ctx.validated;
        let _ = (&bound, &qctx, &metadata, &validated);
        if !matches!(bound, BoundStatement::Match(_)) {
            return Err(PlannerError::UnsupportedOperation(
                "Expected a MATCH bound statement".to_string(),
            ));
        }

        let space_id = qctx.space_id().unwrap_or(1);
        let space_name = qctx.space_name().unwrap_or_else(|| "default".to_string());

        if let Some(metadata_context) = metadata {
            self.metadata_context = Some(metadata_context.clone());
        }

        self.expr_context = Some(validated.ast.expr_context().clone());

        let validation_info = &validated.validation_info;

        for hint in &validation_info.optimization_hints {
            log::debug!("Optimization Tip: {:?}", hint);
        }

        self.plan_match_pattern(validated, space_id, &space_name, validation_info, &qctx)
    }
}

impl StatementPlanner for MatchStatementPlanner {
    fn statement_type(&self) -> &'static str {
        "MATCH"
    }

    fn supported_clause_kinds(&self) -> &[CypherClauseKind] {
        const SUPPORTED_CLAUSES: &[CypherClauseKind] = &[
            CypherClauseKind::Match,
            CypherClauseKind::Where,
            CypherClauseKind::Return,
            CypherClauseKind::OrderBy,
            CypherClauseKind::Pagination,
        ];
        SUPPORTED_CLAUSES
    }
}

impl MatchStatementPlanner {
    fn plan_match_pattern(
        &mut self,
        validated: &ValidatedStatement,
        space_id: u64,
        space_name: &str,
        validation_info: &ValidationInfo,
        qctx: &Arc<QueryContext>,
    ) -> Result<SubPlan, PlannerError> {
        let stmt = validated.stmt();
        match stmt {
            Stmt::Match(match_stmt) => {
                let referenced_tags = &validation_info.semantic_info.referenced_tags;
                if !referenced_tags.is_empty() {
                    log::debug!("Quoted tags: {:?}", referenced_tags);
                }

                let planning_ctx = PlanningContext {
                    space_id,
                    space_name,
                    validation_info,
                    qctx,
                    enable_index_optimization: self.config.enable_index_optimization,
                    metadata_context: &self.metadata_context,
                    expr_context: &self.expr_context,
                    where_expression: match_stmt.where_clause.as_ref(),
                };

                // The join-order path covers every pattern at once; the
                // legacy path plans the first pattern here and cross joins
                // the rest below.
                let (mut plan, joined_by_optimizer) = if match_stmt.patterns.is_empty() {
                    (
                        pattern_planner::plan_node_pattern(space_id, space_name)?,
                        false,
                    )
                } else if let Some(join_plan) =
                    self.try_join_order_plan(match_stmt, space_id, space_name)
                {
                    (join_plan, true)
                } else {
                    let first_pattern = &match_stmt.patterns[0];
                    (
                        pattern_planner::plan_path_pattern(first_pattern, &planning_ctx)?,
                        false,
                    )
                };

                if !joined_by_optimizer {
                    for pattern in match_stmt.patterns.iter().skip(1) {
                        let path_plan = pattern_planner::plan_path_pattern(pattern, &planning_ctx)?;
                        plan = plan_combiner::cross_join_plans(plan, path_plan)?;
                    }
                }

                if has_where_clause(stmt) {
                    plan = self
                        .where_planner
                        .transform_clause(qctx.clone(), stmt, plan)?;
                }

                if has_return_clause(stmt) {
                    let distinct = extract_distinct_flag_from_stmt(stmt);
                    self.return_planner.set_distinct(distinct);
                    plan = self
                        .return_planner
                        .transform_clause(qctx.clone(), stmt, plan)?;
                }

                if has_order_by_clause(stmt) {
                    plan = self
                        .order_by_planner
                        .transform_clause(qctx.clone(), stmt, plan)?;
                }

                if has_pagination(stmt) {
                    plan = self
                        .pagination_planner
                        .transform_clause(qctx.clone(), stmt, plan)?;
                }

                if let Some(delete_clause) = &match_stmt.delete_clause {
                    plan = pattern_planner::plan_match_delete(
                        plan,
                        delete_clause,
                        space_name,
                        match_stmt,
                    )?;
                }

                Ok(plan)
            }
            _ => Err(PlannerError::InvalidOperation(
                "Expected MATCH statement".to_string(),
            )),
        }
    }

    /// Try the join-order path for conjunctive MATCH patterns.
    ///
    /// Returns `None` (caller falls back to the legacy `ExpandAll` chain)
    /// when the patterns are ineligible: optional matches, deletes,
    /// patterns the graph converter rejects, or graphs without rels
    /// (single-node lookups plan better as direct scans).
    fn try_join_order_plan(
        &self,
        match_stmt: &crate::parser::ast::MatchStmt,
        space_id: u64,
        space_name: &str,
    ) -> Option<SubPlan> {
        if match_stmt.optional || match_stmt.delete_clause.is_some() {
            return None;
        }
        let graph =
            crate::planning::join_order::query_graph_from_match_patterns(&match_stmt.patterns)?;
        if graph.num_rels() == 0 {
            return None;
        }
        let graph = std::sync::Arc::new(graph);
        let mut enumerator = crate::planning::join_order::JoinOrderEnumerator::new()
            .with_space(space_id, space_name);
        if let Some(stats) = &self.join_order_stats {
            enumerator = enumerator.with_stats(stats.clone());
        }
        // A user join hint (parsed `USING JOIN ...`) takes precedence over
        // automatic enumeration; an unresolvable hint falls back to the
        // legacy path rather than failing the query.
        let logical = match &match_stmt.join_hint {
            Some(hint) => {
                let join_hint = crate::planning::join_order::JoinHint::from_ast(hint)?;
                enumerator.plan_with_hint(&graph, &join_hint).ok()?
            }
            None => enumerator.plan_query_graph(&graph)?,
        };
        Some(SubPlan::from_logical_root(logical))
    }
}

fn has_where_clause(stmt: &Stmt) -> bool {
    matches!(stmt, Stmt::Match(match_stmt) if match_stmt.where_clause.is_some())
}

fn has_return_clause(stmt: &Stmt) -> bool {
    matches!(stmt, Stmt::Match(match_stmt) if match_stmt.return_clause.is_some())
}

fn has_order_by_clause(stmt: &Stmt) -> bool {
    matches!(stmt, Stmt::Match(match_stmt) if match_stmt.order_by.is_some())
}

fn has_pagination(stmt: &Stmt) -> bool {
    matches!(stmt, Stmt::Match(match_stmt) if match_stmt.limit.is_some() || match_stmt.skip.is_some())
}

fn extract_distinct_flag_from_stmt(stmt: &Stmt) -> bool {
    if let Stmt::Match(match_stmt) = stmt {
        if let Some(return_clause) = &match_stmt.return_clause {
            return return_clause.distinct;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::ast::hint::JoinHintAst;
    use crate::parser::ast::pattern::{
        EdgePattern, NodePattern, PathElement, PathPattern, Pattern,
    };
    use crate::parser::ast::MatchStmt;
    use graphdb_core::types::graph_schema::EdgeDirection;
    use graphdb_core::types::Span;

    fn node(var: &str) -> PathElement {
        PathElement::Node(NodePattern::new(
            Some(var.to_string()),
            Vec::new(),
            None,
            Vec::new(),
            Span::default(),
        ))
    }

    fn edge(var: &str) -> PathElement {
        PathElement::Edge(EdgePattern::new(
            Some(var.to_string()),
            Vec::new(),
            None,
            Vec::new(),
            EdgeDirection::Out,
            None,
            Span::default(),
        ))
    }

    fn path(elements: Vec<PathElement>) -> Pattern {
        Pattern::Path(PathPattern::new(elements, Span::default()))
    }

    fn stmt(patterns: Vec<Pattern>, join_hint: Option<JoinHintAst>, optional: bool) -> MatchStmt {
        MatchStmt {
            span: Span::default(),
            patterns,
            join_hint,
            where_clause: None,
            return_clause: None,
            order_by: None,
            limit: None,
            skip: None,
            optional,
            delete_clause: None,
        }
    }

    fn two_hop() -> Vec<Pattern> {
        vec![
            path(vec![node("a"), edge("e1"), node("b")]),
            path(vec![node("a"), edge("e2"), node("c")]),
        ]
    }

    #[test]
    fn conjunctive_match_uses_join_order() {
        let planner = MatchStatementPlanner::new();
        let plan = planner.try_join_order_plan(&stmt(two_hop(), None, false), 1, "default");
        assert!(plan.is_some(), "two-hop MATCH should take the join path");
    }

    #[test]
    fn binary_hint_plans_through_solver() {
        let planner = MatchStatementPlanner::new();
        let hint = JoinHintAst::Binary {
            left: "e1".to_string(),
            right: "e2".to_string(),
        };
        let plan = planner.try_join_order_plan(&stmt(two_hop(), Some(hint), false), 1, "default");
        assert!(plan.is_some(), "hinted MATCH should plan");
    }

    #[test]
    fn optional_match_falls_back() {
        let planner = MatchStatementPlanner::new();
        let plan = planner.try_join_order_plan(&stmt(two_hop(), None, true), 1, "default");
        assert!(plan.is_none(), "OPTIONAL MATCH keeps the legacy path");
    }

    #[test]
    fn unknown_hint_variable_falls_back() {
        let planner = MatchStatementPlanner::new();
        let hint = JoinHintAst::Binary {
            left: "e1".to_string(),
            right: "zzz".to_string(),
        };
        let plan = planner.try_join_order_plan(&stmt(two_hop(), Some(hint), false), 1, "default");
        assert!(plan.is_none(), "bad hint falls back instead of failing");
    }
}
