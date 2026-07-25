//! Validation result types produced by the Binder.
//!
//! These types were originally defined in the `validator` crate but are now
//! produced by the Binder during the binding process (binding = validation).

use std::collections::HashMap;
use std::sync::Arc;

use crate::core::types::expr::contextual::ContextualExpression;
use crate::core::types::semantic::AliasType;

use crate::core::types::Span;

/// Verified statement wrapper.
///
/// Contains both the original AST and the validation information collected
/// during binding.
#[derive(Debug, Clone)]
pub struct ValidatedStatement {
    pub ast: Arc<crate::query::parser::ast::stmt::Ast>,
    pub validation_info: ValidationInfo,
}

impl ValidatedStatement {
    pub fn new(ast: Arc<crate::query::parser::ast::stmt::Ast>, validation_info: ValidationInfo) -> Self {
        Self { ast, validation_info }
    }

    pub fn stmt(&self) -> &crate::query::parser::ast::Stmt {
        &self.ast.stmt
    }

    pub fn statement_type(&self) -> &'static str {
        self.ast.stmt.kind()
    }

    pub fn alias_map(&self) -> &HashMap<String, AliasType> {
        &self.validation_info.alias_map
    }

    pub fn expr_context(&self) -> &Arc<crate::core::types::expr::expression_context::ExpressionAnalysisContext> {
        &self.ast.expr_context
    }
}

/// Validation information collected during binding.
#[derive(Debug, Clone, Default)]
pub struct ValidationInfo {
    pub alias_map: HashMap<String, AliasType>,
    pub path_analysis: Vec<PathAnalysis>,
    pub optimization_hints: Vec<OptimizationHint>,
    pub variable_definitions: HashMap<String, Span>,
    pub index_hints: Vec<IndexHint>,
    pub validated_clauses: Vec<ClauseKind>,
    pub semantic_info: SemanticInfo,
}

impl ValidationInfo {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_alias(&mut self, name: String, alias_type: AliasType) {
        self.alias_map.insert(name, alias_type);
    }

    pub fn add_path_analysis(&mut self, analysis: PathAnalysis) {
        self.path_analysis.push(analysis);
    }

    pub fn add_optimization_hint(&mut self, hint: OptimizationHint) {
        self.optimization_hints.push(hint);
    }

    pub fn add_index_hint(&mut self, hint: IndexHint) {
        self.index_hints.push(hint);
    }

    pub fn get_alias_type(&self, name: &str) -> Option<&AliasType> {
        self.alias_map.get(name)
    }

    pub fn is_node_variable(&self, name: &str) -> bool {
        matches!(
            self.alias_map.get(name),
            Some(AliasType::Node) | Some(AliasType::NodeList)
        )
    }

    pub fn is_edge_variable(&self, name: &str) -> bool {
        matches!(
            self.alias_map.get(name),
            Some(AliasType::Edge) | Some(AliasType::EdgeList)
        )
    }
}

#[derive(Debug, Clone)]
pub struct PathAnalysis {
    pub alias: Option<String>,
    pub node_count: usize,
    pub edge_count: usize,
    pub has_direction: bool,
    pub min_hops: Option<usize>,
    pub max_hops: Option<usize>,
    pub variables: Vec<String>,
    pub labels: Vec<String>,
    pub edge_types: Vec<String>,
}

impl PathAnalysis {
    pub fn new() -> Self {
        Self {
            alias: None,
            node_count: 0,
            edge_count: 0,
            has_direction: true,
            min_hops: None,
            max_hops: None,
            variables: Vec::new(),
            labels: Vec::new(),
            edge_types: Vec::new(),
        }
    }
}

impl Default for PathAnalysis {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone)]
pub enum OptimizationHint {
    UseIndexScan { table: String, column: String, condition: ContextualExpression },
    LimitResults { reason: String, suggested_limit: usize },
    PreFilter { condition: ContextualExpression, selectivity: f64 },
    JoinOrder { optimal_order: Vec<String>, estimated_cost: f64 },
    PerformanceWarning { message: String, severity: HintSeverity },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HintSeverity {
    Info,
    Warning,
    Critical,
}

#[derive(Debug, Clone)]
pub struct IndexHint {
    pub index_name: String,
    pub index_id: u64,
    pub table_name: String,
    pub columns: Vec<String>,
    pub applicable_conditions: Vec<ContextualExpression>,
    pub estimated_selectivity: f64,
    pub is_edge: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClauseKind {
    Match,
    Where,
    Return,
    OrderBy,
    Limit,
    Skip,
    With,
    Unwind,
    Create,
    Delete,
    Set,
    Remove,
    Yield,
    Go,
    Over,
    From,
}

#[derive(Debug, Clone, Default)]
pub struct SemanticInfo {
    pub referenced_tags: Vec<String>,
    pub referenced_edges: Vec<String>,
    pub referenced_properties: Vec<String>,
    pub used_variables: Vec<String>,
    pub defined_variables: Vec<String>,
    pub aggregate_calls: Vec<AggregateCallInfo>,
    pub output_fields: Vec<String>,
    pub ordering_fields: Vec<String>,
    pub pagination_offset: Option<usize>,
    pub pagination_limit: Option<usize>,
    pub query_type: Option<String>,
    pub query_complexity: Option<usize>,
    pub space_name: Option<String>,
    pub referenced_schemas: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct AggregateCallInfo {
    pub function_name: String,
    pub arguments: Vec<ContextualExpression>,
    pub distinct: bool,
    pub alias: Option<String>,
}

/// Cypher clause types.
#[derive(Debug, Clone, PartialEq, Eq, Copy, Hash)]
pub enum CypherClauseKind {
    Match,
    Where,
    Return,
    With,
    Unwind,
    Yield,
    OrderBy,
    Pagination,
}

impl CypherClauseKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Match => "MATCH",
            Self::Where => "WHERE",
            Self::Return => "RETURN",
            Self::With => "WITH",
            Self::Unwind => "UNWIND",
            Self::Yield => "YIELD",
            Self::OrderBy => "ORDER BY",
            Self::Pagination => "PAGINATION",
        }
    }
}

// ── Clause contexts (used by planners) ───────────────────────────────────────

use crate::core::types::OrderDirection;
use crate::core::YieldColumn;

#[derive(Debug, Clone)]
pub struct WhereClauseContext {
    pub filter: Option<ContextualExpression>,
    pub aliases_available: HashMap<String, AliasType>,
    pub aliases_generated: HashMap<String, AliasType>,
}

#[derive(Debug, Clone)]
pub struct OrderByClauseContext {
    pub indexed_order_factors: Vec<(usize, OrderDirection)>,
}

#[derive(Debug, Clone)]
pub struct PaginationContext {
    pub skip: i64,
    pub limit: i64,
}

#[derive(Debug, Clone)]
pub struct YieldClauseContext {
    pub yield_columns: Vec<YieldColumn>,
    pub aliases_available: HashMap<String, AliasType>,
    pub aliases_generated: HashMap<String, AliasType>,
    pub distinct: bool,
    pub has_agg: bool,
    pub group_keys: Vec<ContextualExpression>,
    pub group_items: Vec<ContextualExpression>,
    pub need_gen_project: bool,
    pub agg_output_column_names: Vec<String>,
    pub proj_output_column_names: Vec<String>,
    pub filter_condition: Option<ContextualExpression>,
    pub skip: Option<usize>,
    pub limit: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct WithClauseContext {
    pub yield_clause: YieldClauseContext,
    pub aliases_available: HashMap<String, AliasType>,
    pub aliases_generated: HashMap<String, AliasType>,
    pub where_clause: Option<WhereClauseContext>,
    pub pagination: Option<PaginationContext>,
    pub order_by: Option<OrderByClauseContext>,
    pub distinct: bool,
}
