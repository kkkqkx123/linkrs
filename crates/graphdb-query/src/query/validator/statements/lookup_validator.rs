//! LOOKUP Statement Validator
//! Verify the validity of the LOOKUP statement.

use std::collections::HashMap;
use std::sync::Arc;

use crate::core::types::expr::contextual::ContextualExpression;
use crate::core::Expression;
use crate::query::validator::error::{ValidationError, ValidationErrorType};

use crate::core::metadata::index_manager::IndexMetadataManager;
use crate::core::metadata::SchemaManager;
use crate::query::parser::ast::stmt::Ast;
use crate::query::parser::ast::stmt::{Stmt, YieldItem as StmtYieldItem};
use crate::query::validator::structs::validation_info::{IndexHint, ValidationInfo};
use crate::query::validator::validator_trait::{
    ColumnDef, ExpressionProps, StatementType, StatementValidator, ValidationResult, ValueType,
};
use crate::query::QueryContext;

/// Verified LOOKUP information
#[derive(Debug, Clone)]
pub struct ValidatedLookup {
    pub space_id: u64,
    pub label: String,
    pub is_edge: bool,
    pub index_type: LookupIndexType,
    pub filter_expression: Option<ContextualExpression>,
    pub yield_columns: Vec<LookupYieldColumn>,
    pub is_yield_all: bool,
}

#[derive(Debug, Clone)]
pub struct LookupYieldColumn {
    pub name: String,
    pub alias: Option<String>,
    pub expression: Option<ContextualExpression>,
}

#[derive(Debug, Clone)]
pub enum LookupIndexType {
    None,
    Single(String),
    Composite(Vec<String>),
}

#[derive(Debug, Clone)]
pub struct LookupProperty {
    pub name: String,
    pub type_: ValueType,
}

/// LOOKUP Validator
/// Parse the LOOKUP statement entirely from the AST (Abstract Syntax Tree), without relying on any external preset values.
#[derive(Debug)]
pub struct LookupValidator {
    inputs: Vec<ColumnDef>,
    outputs: Vec<ColumnDef>,
    expression_props: ExpressionProps,
    user_defined_vars: Vec<String>,
    validated_result: Option<ValidatedLookup>,
    schema_manager: Option<Arc<SchemaManager>>,
    index_metadata_manager: Option<Arc<dyn IndexMetadataManager>>,
}

impl LookupValidator {
    pub fn new() -> Self {
        Self {
            inputs: Vec::new(),
            outputs: Vec::new(),
            expression_props: ExpressionProps::default(),
            user_defined_vars: Vec::new(),
            validated_result: None,
            schema_manager: None,
            index_metadata_manager: None,
        }
    }

    pub fn with_schema_manager(mut self, schema_manager: Arc<SchemaManager>) -> Self {
        self.schema_manager = Some(schema_manager);
        self
    }

    pub fn with_index_metadata_manager(
        mut self,
        index_metadata_manager: Arc<dyn IndexMetadataManager>,
    ) -> Self {
        self.index_metadata_manager = Some(index_metadata_manager);
        self
    }

    pub fn set_schema_manager(&mut self, schema_manager: Arc<SchemaManager>) {
        self.schema_manager = Some(schema_manager);
    }

    pub fn set_index_metadata_manager(
        &mut self,
        index_metadata_manager: Arc<dyn IndexMetadataManager>,
    ) {
        self.index_metadata_manager = Some(index_metadata_manager);
    }

    /// Parsing a LOOKUP statement from AST (Abstract Syntax Tree)
    fn parse_from_ast(&self, ast: &Arc<Ast>) -> Result<ParsedLookupInfo, ValidationError> {
        let lookup_stmt = match &ast.stmt {
            Stmt::Lookup(lookup_stmt) => lookup_stmt,
            _ => {
                return Err(ValidationError::new(
                    "Expected LOOKUP statement".to_string(),
                    ValidationErrorType::SemanticError,
                ));
            }
        };

        // Analysis target (Tag or Edge)
        let (label, is_edge, target_type_specified) = match &lookup_stmt.target {
            crate::query::parser::ast::stmt::LookupTarget::Tag(name) => (name.clone(), false, true),
            crate::query::parser::ast::stmt::LookupTarget::Edge(name) => (name.clone(), true, true),
            crate::query::parser::ast::stmt::LookupTarget::Unspecified(name) => {
                // Type will be resolved during validation
                (name.clone(), false, false)
            }
        };

        if label.is_empty() {
            return Err(ValidationError::new(
                "LOOKUP must specify the Tag or Edge name.".to_string(),
                ValidationErrorType::SemanticError,
            ));
        }

        // Analyzing the WHERE clause
        let filter_expression = lookup_stmt.where_clause.clone();

        // Analyzing the YIELD clause
        let mut yield_columns = Vec::new();
        let mut is_yield_all = false;

        if let Some(ref yield_clause) = lookup_stmt.yield_clause {
            for item in &yield_clause.items {
                yield_columns.push(self.parse_yield_item(item)?);
            }
            // Check whether it is YIELD *
            if yield_columns.len() == 1 && yield_columns[0].name == "*" {
                is_yield_all = true;
            }
        }

        Ok(ParsedLookupInfo {
            label,
            is_edge,
            target_type_specified,
            filter_expression,
            yield_columns,
            is_yield_all,
        })
    }

    /// Analyzing a single YIELD entry
    fn parse_yield_item(&self, item: &StmtYieldItem) -> Result<LookupYieldColumn, ValidationError> {
        let name = self.extract_column_name(&item.expression)?;
        Ok(LookupYieldColumn {
            name,
            alias: item.alias.clone(),
            expression: Some(item.expression.clone()),
        })
    }

    /// Extract column names from the expression.
    fn extract_column_name(&self, expr: &ContextualExpression) -> Result<String, ValidationError> {
        if let Some(inner_expr) = expr.expression() {
            let expr_inner = inner_expr.inner();
            match expr_inner {
                Expression::Variable(name) => Ok(name.clone()),
                Expression::Label(name) => Ok(name.clone()),
                Expression::Property { property, .. } => Ok(property.clone()),
                _ => Err(ValidationError::new(
                    "Unable to extract column names from expressions".to_string(),
                    ValidationErrorType::SemanticError,
                )),
            }
        } else {
            Err(ValidationError::new(
                "Invalid expression".to_string(),
                ValidationErrorType::SemanticError,
            ))
        }
    }

    /// Verify the LOOKUP target
    /// 对应 NebulaGraph 的 validateFrom() 方法
    fn validate_lookup_target(
        &self,
        space_name: &str,
        label: &str,
        is_edge: bool,
        target_type_specified: bool,
    ) -> Result<(LookupIndexType, bool), ValidationError> {
        // Check whether schema_manager is available.
        let schema_manager = self.schema_manager.as_ref().ok_or_else(|| {
            ValidationError::new(
                "Schema manager not available".to_string(),
                ValidationErrorType::SemanticError,
            )
        })?;

        if target_type_specified {
            // Type was explicitly specified, use it directly
            if is_edge {
                // Verify whether the Edge Type exists.
                match schema_manager.as_ref().get_edge_type(space_name, label) {
                    Ok(Some(_edge_info)) => Ok((LookupIndexType::Single(label.to_string()), true)),
                    Ok(None) => Err(ValidationError::new(
                        format!("Edge type '{}' not found in space '{}'", label, space_name),
                        ValidationErrorType::SemanticError,
                    )),
                    Err(e) => Err(ValidationError::new(
                        format!("Failed to get edge type '{}': {}", label, e),
                        ValidationErrorType::SemanticError,
                    )),
                }
            } else {
                // Verify whether the Tag exists.
                match schema_manager.as_ref().get_tag(space_name, label) {
                    Ok(Some(_tag_info)) => Ok((LookupIndexType::Single(label.to_string()), false)),
                    Ok(None) => Err(ValidationError::new(
                        format!("Tag '{}' not found in space '{}'", label, space_name),
                        ValidationErrorType::SemanticError,
                    )),
                    Err(e) => Err(ValidationError::new(
                        format!("Failed to get tag '{}': {}", label, e),
                        ValidationErrorType::SemanticError,
                    )),
                }
            }
        } else {
            // Type was not specified, try to infer from schema
            // First try as Tag
            match schema_manager.as_ref().get_tag(space_name, label) {
                Ok(Some(_tag_info)) => Ok((LookupIndexType::Single(label.to_string()), false)),
                Ok(None) => {
                    // Tag not found, try as Edge
                    match schema_manager.as_ref().get_edge_type(space_name, label) {
                        Ok(Some(_edge_info)) => {
                            Ok((LookupIndexType::Single(label.to_string()), true))
                        }
                        Ok(None) => Err(ValidationError::new(
                            format!(
                                "Tag or Edge type '{}' not found in space '{}'",
                                label, space_name
                            ),
                            ValidationErrorType::SemanticError,
                        )),
                        Err(e) => Err(ValidationError::new(
                            format!("Failed to get edge type '{}': {}", label, e),
                            ValidationErrorType::SemanticError,
                        )),
                    }
                }
                Err(e) => Err(ValidationError::new(
                    format!("Failed to get tag '{}': {}", label, e),
                    ValidationErrorType::SemanticError,
                )),
            }
        }
    }

    /// Verify the filtering criteria.
    fn validate_filter(
        &self,
        filter: &Option<ContextualExpression>,
    ) -> Result<(), ValidationError> {
        if let Some(ref filter_expr) = filter {
            let expr_meta = match filter_expr.expression() {
                Some(m) => m,
                None => {
                    return Err(ValidationError::new(
                        "Invalid filter expression".to_string(),
                        ValidationErrorType::SemanticError,
                    ))
                }
            };
            let expr = expr_meta.inner();

            self.validate_filter_type(expr)?;

            if self.has_aggregate_expression(expr) {
                return Err(ValidationError::new(
                    "LOOKUP filter cannot contain aggregate expressions".to_string(),
                    ValidationErrorType::SemanticError,
                ));
            }
        }
        Ok(())
    }

    /// Verify the filter type.
    fn validate_filter_type(&self, filter: &Expression) -> Result<(), ValidationError> {
        match filter {
            Expression::Binary { op, .. } => {
                use crate::core::BinaryOperator;
                match op {
                    BinaryOperator::Equal
                    | BinaryOperator::NotEqual
                    | BinaryOperator::LessThan
                    | BinaryOperator::LessThanOrEqual
                    | BinaryOperator::GreaterThan
                    | BinaryOperator::GreaterThanOrEqual
                    | BinaryOperator::And
                    | BinaryOperator::Or => Ok(()),
                    _ => Err(ValidationError::new(
                        "Filter expression must return bool type".to_string(),
                        ValidationErrorType::TypeError,
                    )),
                }
            }
            _ => Ok(()),
        }
    }

    /// Check whether it contains aggregate expressions.
    fn has_aggregate_expression(&self, expression: &Expression) -> bool {
        match expression {
            Expression::Aggregate { .. } => true,
            Expression::Binary { left, right, .. } => {
                self.has_aggregate_expression(left) || self.has_aggregate_expression(right)
            }
            Expression::Unary { operand, .. } => self.has_aggregate_expression(operand),
            Expression::Function { args, .. } => {
                args.iter().any(|arg| self.has_aggregate_expression(arg))
            }
            _ => false,
        }
    }

    /// Verify the YIELD clause
    fn validate_yields(
        &self,
        yield_columns: &[LookupYieldColumn],
        is_yield_all: bool,
    ) -> Result<(), ValidationError> {
        if is_yield_all || yield_columns.is_empty() {
            return Ok(());
        }

        let mut seen_names: HashMap<String, usize> = HashMap::new();
        for col in yield_columns {
            let count = seen_names.entry(col.name.clone()).or_insert(0);
            *count += 1;
            if *count > 1 {
                return Err(ValidationError::new(
                    format!("Duplicate column name '{}' in YIELD clause", col.name),
                    ValidationErrorType::SemanticError,
                ));
            }
        }
        Ok(())
    }

    /// Generate a column of outputs.
    fn generate_output_columns(
        &self,
        yield_columns: &[LookupYieldColumn],
        is_yield_all: bool,
    ) -> Vec<ColumnDef> {
        let mut outputs = Vec::new();
        if is_yield_all {
            outputs.push(ColumnDef {
                name: "*".to_string(),
                type_: ValueType::List,
            });
        } else {
            for col in yield_columns {
                outputs.push(ColumnDef {
                    name: col.alias.clone().unwrap_or_else(|| col.name.clone()),
                    type_: ValueType::String,
                });
            }
        }
        outputs
    }
}

/// LOOKUP information parsed from AST
#[derive(Debug)]
struct ParsedLookupInfo {
    label: String,
    is_edge: bool,
    /// Whether the target type (tag/edge) was explicitly specified
    target_type_specified: bool,
    filter_expression: Option<ContextualExpression>,
    yield_columns: Vec<LookupYieldColumn>,
    is_yield_all: bool,
}

impl Default for LookupValidator {
    fn default() -> Self {
        Self::new()
    }
}

/// Implementing the StatementValidator trait
///
/// # Design Notes
/// - Completely parse LOOKUP information from AST
/// - Do not rely on any external preset values
/// - All required information is parsed from AST
impl StatementValidator for LookupValidator {
    fn validate(
        &mut self,
        ast: Arc<Ast>,
        ctx: Arc<QueryContext>,
    ) -> Result<ValidationResult, ValidationError> {
        // Parsing the LOOKUP statement
        let parsed_info = self.parse_from_ast(&ast)?;

        // Get the current space name
        let space_name = ctx.space_name().ok_or_else(|| {
            ValidationError::new(
                "No current space selected".to_string(),
                ValidationErrorType::SemanticError,
            )
        })?;

        // Verify the LOOKUP target (and determine if it's an edge)
        let (index_type, is_edge) = self.validate_lookup_target(
            &space_name,
            &parsed_info.label,
            parsed_info.is_edge,
            parsed_info.target_type_specified,
        )?;

        // Verify the filtering criteria
        self.validate_filter(&parsed_info.filter_expression)?;

        // Verify the YIELD clause
        self.validate_yields(&parsed_info.yield_columns, parsed_info.is_yield_all)?;

        // Generate output columns
        self.outputs =
            self.generate_output_columns(&parsed_info.yield_columns, parsed_info.is_yield_all);

        // Get space_id
        let space_id = ctx.space_id().unwrap_or(0);

        // Find index name and columns from index manager
        let (index_name, index_columns) = if let Some(ref index_mgr) = self.index_metadata_manager {
            if is_edge {
                // Find edge index for this edge type
                match index_mgr.list_edge_indexes(space_id) {
                    Ok(indexes) => indexes
                        .into_iter()
                        .find(|idx| idx.schema_name == parsed_info.label)
                        .map(|idx| {
                            let cols: Vec<String> =
                                idx.fields.iter().map(|f| f.name.clone()).collect();
                            (idx.name, cols)
                        })
                        .unwrap_or_default(),
                    Err(_) => (String::new(), Vec::new()),
                }
            } else {
                // Find tag index for this tag
                match index_mgr.list_tag_indexes(space_id) {
                    Ok(indexes) => indexes
                        .into_iter()
                        .find(|idx| idx.schema_name == parsed_info.label)
                        .map(|idx| {
                            let cols: Vec<String> =
                                idx.fields.iter().map(|f| f.name.clone()).collect();
                            (idx.name, cols)
                        })
                        .unwrap_or_default(),
                    Err(_) => (String::new(), Vec::new()),
                }
            }
        } else {
            (String::new(), Vec::new())
        };

        // Store verification results
        self.validated_result = Some(ValidatedLookup {
            space_id,
            label: parsed_info.label.clone(),
            is_edge,
            index_type,
            filter_expression: parsed_info.filter_expression,
            yield_columns: parsed_info.yield_columns,
            is_yield_all: parsed_info.is_yield_all,
        });

        // Build ValidationInfo
        let validation_info = ValidationInfo {
            alias_map: HashMap::new(),
            path_analysis: Vec::new(),
            optimization_hints: Vec::new(),
            variable_definitions: HashMap::new(),
            index_hints: vec![IndexHint {
                index_name,
                table_name: parsed_info.label,
                columns: index_columns,
                applicable_conditions: Vec::new(),
                estimated_selectivity: 0.0,
                is_edge,
            }],
            validated_clauses: Vec::new(),
            semantic_info: Default::default(),
        };

        Ok(ValidationResult::success_with_info(validation_info))
    }

    fn statement_type(&self) -> StatementType {
        StatementType::Lookup
    }

    fn inputs(&self) -> &[ColumnDef] {
        &self.inputs
    }

    fn outputs(&self) -> &[ColumnDef] {
        &self.outputs
    }

    fn is_global_statement(&self) -> bool {
        false
    }

    fn expression_props(&self) -> &ExpressionProps {
        &self.expression_props
    }

    fn user_defined_vars(&self) -> &[String] {
        &self.user_defined_vars
    }
}
