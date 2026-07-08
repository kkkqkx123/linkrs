//! Verification Results Information Module
//!
//! This module defines the structured information generated during the validation phase, which is used to be transmitted to the planning phase.
//! Avoid having the planner parse the AST multiple times.

use std::collections::HashMap;
use std::sync::Arc;

use crate::core::types::expr::contextual::ContextualExpression;
use crate::core::types::Span;
use crate::query::parser::ast::stmt::Ast;
use crate::query::validator::context::ExpressionAnalysisContext;

use crate::query::validator::structs::AliasType;
use crate::query::validator::validator_trait::ValueType;

/// Verified Statement Wrapper
/// The text to be translated contains both the original AST (Abstract Syntax Tree) information (which includes the statements and the context of the expressions) as well as verification details.
///
/// # Refactoring Changes
/// Replace `Arc<Stmt>` with `Arc<Ast>`.
/// The `Ast` class contains both `Stmt` and `ExpressionAnalysisContext` objects.
#[derive(Debug, Clone)]
pub struct ValidatedStatement {
    /// Original AST (using Arc for shared ownership)
    pub ast: Arc<Ast>,
    /// Information collected during the verification phase
    pub validation_info: ValidationInfo,
}

impl ValidatedStatement {
    /// Create a new sentence that has been verified.
    pub fn new(ast: Arc<Ast>, validation_info: ValidationInfo) -> Self {
        Self {
            ast,
            validation_info,
        }
    }

    /// Obtain statement references
    pub fn stmt(&self) -> &crate::query::parser::ast::Stmt {
        &self.ast.stmt
    }

    /// Determine the type of the sentence
    pub fn statement_type(&self) -> &'static str {
        self.ast.stmt.kind()
    }

    /// Obtain the alias mapping.
    pub fn alias_map(&self) -> &HashMap<String, AliasType> {
        &self.validation_info.alias_map
    }

    /// Obtain the context of the expression.
    pub fn expr_context(&self) -> &Arc<ExpressionAnalysisContext> {
        &self.ast.expr_context
    }
}

/// Verification Information Structure
/// Include all useful information collected during the validation phase.
///
/// Design Description:
/// The information about the expression types is stored uniformly in the ExpressionContext and can be accessed through the ContextualExpression.
/// This structure no longer maintains a separate cache for expression types, ensuring a single data source.
#[derive(Debug, Clone, Default)]
pub struct ValidationInfo {
    /// Alias mapping (variable name -> type)
    pub alias_map: HashMap<String, AliasType>,

    /// Path analysis results
    pub path_analysis: Vec<PathAnalysis>,

    /// Optimization suggestions
    pub optimization_hints: Vec<OptimizationHint>,

    /// Location of variable definition
    pub variable_definitions: HashMap<String, Span>,

    /// The index information that was used
    pub index_hints: Vec<IndexHint>,

    /// Validated sub-sentences
    pub validated_clauses: Vec<ClauseKind>,

    /// Results of semantic analysis
    pub semantic_info: SemanticInfo,
}

impl ValidationInfo {
    /// Create empty verification information.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add an alias
    pub fn add_alias(&mut self, name: String, alias_type: AliasType) {
        self.alias_map.insert(name, alias_type);
    }

    /// Determine the type of the expression.
    ///
    /// Retrieve type information from the ExpressionContext to ensure a single data source.
    /// All types of information are stored in the ExpressionContext during the validation phase using the ExpressionAnalyzer.
    pub fn get_expr_type(&self, expr: &ContextualExpression) -> Option<ValueType> {
        expr.data_type()
            .map(|data_type| ValueType::from_data_type(&data_type))
    }

    /// Analyze expressions using ExpressionAnalyzer.
    /// Store type and constant information in the ExpressionContext.
    pub fn analyze_expression(
        &mut self,
        expr: &ContextualExpression,
        variable_types: Option<&std::collections::HashMap<String, crate::core::DataType>>,
    ) -> Result<
        crate::query::validator::ExpressionAnalysisResult,
        crate::query::validator::error::ValidationError,
    > {
        use crate::query::validator::ExpressionAnalyzer;

        let analyzer = ExpressionAnalyzer::new();
        let result = analyzer.analyze(expr, variable_types)?;

        Ok(result)
    }

    /// Add path analysis
    pub fn add_path_analysis(&mut self, analysis: PathAnalysis) {
        self.path_analysis.push(analysis);
    }

    /// Add optimization suggestions
    pub fn add_optimization_hint(&mut self, hint: OptimizationHint) {
        self.optimization_hints.push(hint);
    }

    /// Add an index hint
    pub fn add_index_hint(&mut self, hint: IndexHint) {
        self.index_hints.push(hint);
    }

    /// Obtaining the type of a variable
    pub fn get_alias_type(&self, name: &str) -> Option<&AliasType> {
        self.alias_map.get(name)
    }

    /// Check whether the variable is of the node type.
    pub fn is_node_variable(&self, name: &str) -> bool {
        matches!(
            self.alias_map.get(name),
            Some(AliasType::Node) | Some(AliasType::NodeList)
        )
    }

    /// Check whether the variable is of the edge type.
    pub fn is_edge_variable(&self, name: &str) -> bool {
        matches!(
            self.alias_map.get(name),
            Some(AliasType::Edge) | Some(AliasType::EdgeList)
        )
    }
}

/// Path analysis information
#[derive(Debug, Clone)]
pub struct PathAnalysis {
    /// Path alias
    pub alias: Option<String>,
    /// Number of nodes
    pub node_count: usize,
    /// Number of edges
    pub edge_count: usize,
    /// Is there a direction to follow?
    pub has_direction: bool,
    /// Minimum number of jumps
    pub min_hops: Option<usize>,
    /// Maximum number of jumps
    pub max_hops: Option<usize>,
    /// Variables in the path
    pub variables: Vec<String>,
    /// Tags in the path
    pub labels: Vec<String>,
    /// Edge types in the path
    pub edge_types: Vec<String>,
}

impl PathAnalysis {
    /// Create a new path analysis.
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

/// Optimize the type of suggestions.
#[derive(Debug, Clone)]
pub enum OptimizationHint {
    /// It is recommended to use index scanning.
    UseIndexScan {
        table: String,
        column: String,
        condition: ContextualExpression,
    },
    /// It is recommended to limit the number of results.
    LimitResults {
        reason: String,
        suggested_limit: usize,
    },
    /// It is recommended to perform a pre-filtering step.
    PreFilter {
        condition: ContextualExpression,
        selectivity: f64,
    },
    /// Suggested order of connection
    JoinOrder {
        optimal_order: Vec<String>,
        estimated_cost: f64,
    },
    /// Indicate potential performance issues
    PerformanceWarning {
        message: String,
        severity: HintSeverity,
    },
}

/// Indication of the severity
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HintSeverity {
    Info,
    Warning,
    Critical,
}

/// Index tips
#[derive(Debug, Clone)]
pub struct IndexHint {
    /// Index name
    pub index_name: String,
    /// Table/Tag Name
    pub table_name: String,
    /// Index column
    pub columns: Vec<String>,
    /// Applicable Conditions
    pub applicable_conditions: Vec<ContextualExpression>,
    /// Estimated selectivity
    pub estimated_selectivity: f64,
    /// Whether this is an edge index
    pub is_edge: bool,
}

/// Sentence types
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

/// Semantic information
///
/// The semantic information collected during the storage validation phase is used by the optimizer and the executor.
/// Design principle: Only retain the information that is truly necessary during the planning phase, and avoid redundancy.
#[derive(Debug, Clone, Default)]
pub struct SemanticInfo {
    /// Cited tags
    pub referenced_tags: Vec<String>,
    /// Type of the referenced edge
    pub referenced_edges: Vec<String>,
    /// Cited attributes
    pub referenced_properties: Vec<String>,
    /// Variables used
    pub used_variables: Vec<String>,
    /// Defined variables
    pub defined_variables: Vec<String>,
    /// Aggregate function calls
    pub aggregate_calls: Vec<AggregateCallInfo>,
    /// Of course! Please provide the text you would like to have translated.
    pub output_fields: Vec<String>,
    /// Sorting field
    pub ordering_fields: Vec<String>,
    /// Pagination offset
    pub pagination_offset: Option<usize>,
    /// Pagination limits
    pub pagination_limit: Option<usize>,
    /// Query type
    pub query_type: Option<String>,
    /// Query complexity
    pub query_complexity: Option<usize>,
    /// Space Name
    pub space_name: Option<String>,
    /// The referenced Schema (types of tags or edges)
    pub referenced_schemas: Vec<String>,
}

/// Information on aggregate function calls
#[derive(Debug, Clone)]
pub struct AggregateCallInfo {
    /// Function name
    pub function_name: String,
    /// Parameter expression
    pub arguments: Vec<ContextualExpression>,
    /// Should duplicates be removed?
    pub distinct: bool,
    /// Alias
    pub alias: Option<String>,
}
