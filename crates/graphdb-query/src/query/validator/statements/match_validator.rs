//! Match Statement Validator (New System)
//! Use the trait+enumeration architecture to replace the existing Strategy pattern.

use std::collections::HashMap;
use std::sync::Arc;

use crate::core::metadata::SchemaManager;
use crate::core::types::expr::contextual::ContextualExpression;
use crate::core::YieldColumn;
use crate::query::parser::ast::stmt::{Ast, MatchStmt, OrderByClause, ReturnClause, ReturnItem};
use crate::query::parser::ast::{Pattern, Stmt};
use crate::query::validator::context::ExpressionAnalysisContext;
use crate::query::validator::error::{ValidationError, ValidationErrorType};
use crate::query::QueryContext;

use crate::query::validator::strategies::ExpressionValidationStrategy;
use crate::query::validator::structs::validation_info::{PathAnalysis, ValidationInfo};
use crate::query::validator::structs::{
    AliasType, MatchStepRange, PaginationContext, Path, QueryPart, ReturnClauseContext,
    UnwindClauseContext, WhereClauseContext, WithClauseContext, YieldClauseContext,
};
use crate::query::validator::validator_trait::{
    ColumnDef, ExpressionProps, StatementType, StatementValidator, ValidationResult, ValueType,
};

/// Verified MATCH information
#[derive(Debug, Clone)]
pub struct ValidatedMatch {
    pub space_id: u64,
    pub patterns: Vec<Pattern>,
    pub where_clause: Option<ContextualExpression>,
    pub return_clause: Option<ReturnClause>,
    pub order_by: Option<OrderByClause>,
    pub limit: Option<usize>,
    pub skip: Option<usize>,
    pub optional: bool,
    pub aliases: HashMap<String, AliasType>,
}

/// Match Statement Validator
#[derive(Debug)]
pub struct MatchValidator {
    /// Input column
    inputs: Vec<ColumnDef>,
    /// Output column
    outputs: Vec<ColumnDef>,
    /// Verified result
    validated_result: Option<ValidatedMatch>,
    /// Alias mapping
    aliases: HashMap<String, AliasType>,
    /// List of paths
    paths: Vec<Path>,
    /// Pagination context
    pagination: Option<PaginationContext>,
    /// Is it an optional match?
    optional: bool,
    /// Expression properties
    expression_props: ExpressionProps,
    /// User-defined variables
    user_defined_vars: Vec<String>,
    /// Schema manager for validation
    schema_manager: Option<Arc<SchemaManager>>,
    /// Space name for schema validation
    space_name: Option<String>,
}

impl MatchValidator {
    /// Create a new Match validator.
    pub fn new() -> Self {
        Self {
            inputs: Vec::new(),
            outputs: Vec::new(),
            validated_result: None,
            aliases: HashMap::new(),
            paths: Vec::new(),
            pagination: None,
            optional: false,
            expression_props: ExpressionProps::default(),
            user_defined_vars: Vec::new(),
            schema_manager: None,
            space_name: None,
        }
    }

    /// Creating a validator with pagination context
    pub fn with_pagination(skip: i64, limit: i64) -> Self {
        let mut validator = Self::new();
        validator.pagination = Some(PaginationContext { skip, limit });
        validator
    }

    /// Set schema manager
    pub fn with_schema_manager(mut self, schema_manager: Arc<SchemaManager>) -> Self {
        self.schema_manager = Some(schema_manager);
        self
    }

    /// Set space name
    pub fn set_space_name(&mut self, space_name: String) {
        self.space_name = Some(space_name);
    }

    /// Set schema manager
    pub fn set_schema_manager(&mut self, schema_manager: Arc<SchemaManager>) {
        self.schema_manager = Some(schema_manager);
    }

    /// Obtain the verified results.
    pub fn validated_result(&self) -> Option<&ValidatedMatch> {
        self.validated_result.as_ref()
    }

    /// Obtain the alias mapping.
    pub fn aliases(&self) -> &HashMap<String, AliasType> {
        &self.aliases
    }

    /// Obtain a list of paths
    pub fn paths(&self) -> &[Path] {
        &self.paths
    }

    /// Is it necessary to select a graph space?
    pub fn requires_space(&self) -> bool {
        true
    }

    /// Is it necessary to grant write permissions?
    pub fn requires_write_permission(&self) -> bool {
        false
    }

    /// Verify the complete MATCH statement.
    pub fn validate_match_statement(
        &mut self,
        match_stmt: &MatchStmt,
    ) -> Result<(), ValidationError> {
        // 1. The validation model is not empty
        if match_stmt.patterns.is_empty() {
            return Err(ValidationError::new(
                "The MATCH statement must contain at least one pattern".to_string(),
                ValidationErrorType::SemanticError,
            ));
        }

        // 2. First pass: collect all aliases (variable definitions)
        self.collect_aliases_from_patterns(&match_stmt.patterns)?;

        // 3. Second pass: validation of schema structure and variable references
        for (idx, pattern) in match_stmt.patterns.iter().enumerate() {
            self.validate_pattern(pattern, idx)?
        }

        // 4. Verify the existence of the RETURN clause.
        // Note: MATCH can be followed by WITH clause instead of RETURN clause in multi-part queries.
        // The RETURN clause is optional here - it will be validated at the query pipeline level.
        // if match_stmt.return_clause.is_none() {
        //     return Err(ValidationError::new(
        //         "The MATCH statement must contain a RETURN clause.".to_string(),
        //         ValidationErrorType::SemanticError,
        //     ));
        // }

        // 5. Validate the WHERE clause (if present)
        if let Some(ref where_clause) = match_stmt.where_clause {
            self.validate_where_clause(where_clause)?
        }

        // 6. Validating the RETURN clause
        if let Some(ref return_clause) = match_stmt.return_clause {
            self.validate_return_clause(return_clause)?
        }

        // 7. Validate the ORDER BY clause (if present)
        if let Some(ref order_by) = match_stmt.order_by {
            self.validate_order_by(order_by)?
        }

        // 8. Validation of paging parameters
        // SKIP and LIMIT should be non-negative
        if let Some(skip) = match_stmt.skip {
            if skip > i64::MAX as usize {
                return Err(ValidationError::new(
                    format!("SKIP value ({}) is too large", skip),
                    ValidationErrorType::SemanticError,
                ));
            }
        }
        if let Some(limit) = match_stmt.limit {
            if limit > i64::MAX as usize {
                return Err(ValidationError::new(
                    format!("LIMIT value ({}) is too large", limit),
                    ValidationErrorType::SemanticError,
                ));
            }
        }

        Ok(())
    }

    /// Verify a single pattern
    fn validate_pattern(&mut self, pattern: &Pattern, idx: usize) -> Result<(), ValidationError> {
        match pattern {
            Pattern::Node(node_pattern) => {
                if node_pattern.variable.is_none() && node_pattern.labels.is_empty() {
                    return Err(ValidationError::new(
                        format!(
                            "The anonymous node at pattern {} must specify a label.",
                            idx + 1
                        ),
                        ValidationErrorType::SemanticError,
                    ));
                }
                // Validate tags exist
                for label in &node_pattern.labels {
                    self.validate_tag_exists(label)?;
                }
            }
            Pattern::Edge(edge_pattern) => {
                // Validate edge types exist
                for edge_type in &edge_pattern.edge_types {
                    self.validate_edge_type_exists(edge_type)?;
                }
            }
            Pattern::Path(path_pattern) => {
                if path_pattern.elements.is_empty() {
                    return Err(ValidationError::new(
                        format!("Pattern {}: path cannot be empty", idx + 1),
                        ValidationErrorType::SemanticError,
                    ));
                }
                // Validate tags and edges in path
                for element in &path_pattern.elements {
                    self.validate_path_element(element)?;
                }
            }
            Pattern::Variable(var_pattern) => {
                if !self.aliases.contains_key(&var_pattern.name) {
                    return Err(ValidationError::new(
                        format!(
                            "Pattern {}: references the undefined variable '{}'.",
                            idx + 1,
                            var_pattern.name
                        ),
                        ValidationErrorType::SemanticError,
                    ));
                }

                if let Some(alias_type) = self.aliases.get(&var_pattern.name) {
                    if matches!(alias_type, AliasType::Runtime) {
                        return Err(ValidationError::new(
                            format!(
                                "Pattern {}: The variable '{}' is an alias for the runtime computation, it cannot be referenced as a pattern",
                                idx + 1,
                                var_pattern.name
                            ),
                            ValidationErrorType::SemanticError,
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    /// Validate path element
    fn validate_path_element(
        &mut self,
        element: &crate::query::parser::ast::PathElement,
    ) -> Result<(), ValidationError> {
        match element {
            crate::query::parser::ast::PathElement::Node(node) => {
                for label in &node.labels {
                    self.validate_tag_exists(label)?;
                }
            }
            crate::query::parser::ast::PathElement::Edge(edge) => {
                for edge_type in &edge.edge_types {
                    self.validate_edge_type_exists(edge_type)?;
                }
            }
            crate::query::parser::ast::PathElement::Alternative(patterns) => {
                for pattern in patterns {
                    self.validate_pattern(pattern, 0)?;
                }
            }
            crate::query::parser::ast::PathElement::Optional(inner) => {
                self.validate_path_element(inner)?;
            }
            crate::query::parser::ast::PathElement::Repeated(inner, _) => {
                self.validate_path_element(inner)?;
            }
        }
        Ok(())
    }

    /// Validate that the tag exists in the schema
    fn validate_tag_exists(&self, tag_name: &str) -> Result<(), ValidationError> {
        if let (Some(ref schema_manager), Some(space)) = (&self.schema_manager, &self.space_name) {
            match schema_manager.get_tag(space, tag_name) {
                Ok(Some(_)) => Ok(()),
                Ok(None) => Err(ValidationError::new(
                    format!("Tag '{}' not found in space '{}'", tag_name, space),
                    ValidationErrorType::SemanticError,
                )),
                Err(e) => Err(ValidationError::new(
                    format!("Failed to get tag '{}': {}", tag_name, e),
                    ValidationErrorType::SemanticError,
                )),
            }
        } else {
            // Without schema_manager, we can't validate, so just pass
            Ok(())
        }
    }

    /// Validate that the edge type exists in the schema
    fn validate_edge_type_exists(&self, edge_name: &str) -> Result<(), ValidationError> {
        if let (Some(ref schema_manager), Some(space)) = (&self.schema_manager, &self.space_name) {
            match schema_manager.get_edge_type(space, edge_name) {
                Ok(Some(_)) => Ok(()),
                Ok(None) => Err(ValidationError::new(
                    format!("Edge type '{}' not found in space '{}'", edge_name, space),
                    ValidationErrorType::SemanticError,
                )),
                Err(e) => Err(ValidationError::new(
                    format!("Failed to get edge type '{}': {}", edge_name, e),
                    ValidationErrorType::SemanticError,
                )),
            }
        } else {
            // Without schema_manager, we can't validate, so just pass
            Ok(())
        }
    }

    /// Collect aliases from the pattern (during the first scan)
    fn collect_aliases_from_patterns(
        &mut self,
        patterns: &[Pattern],
    ) -> Result<(), ValidationError> {
        for pattern in patterns.iter() {
            match pattern {
                Pattern::Node(node) => {
                    if let Some(ref var) = node.variable {
                        self.aliases.insert(var.clone(), AliasType::Node);
                    }
                }
                Pattern::Edge(edge) => {
                    if let Some(ref var) = edge.variable {
                        self.aliases.insert(var.clone(), AliasType::Edge);
                    }
                }
                Pattern::Path(path) => {
                    self.collect_aliases_from_path(path)?;
                }
                Pattern::Variable(_var) => {}
            }
        }
        Ok(())
    }

    /// Collect aliases from path pattern
    fn collect_aliases_from_path(
        &mut self,
        path: &crate::query::parser::ast::PathPattern,
    ) -> Result<(), ValidationError> {
        for element in &path.elements {
            self.collect_aliases_from_path_element(element)?;
        }
        Ok(())
    }

    /// Collect aliases from path element
    fn collect_aliases_from_path_element(
        &mut self,
        element: &crate::query::parser::ast::PathElement,
    ) -> Result<(), ValidationError> {
        use crate::query::parser::ast::PathElement;

        match element {
            PathElement::Node(node) => {
                if let Some(ref var) = node.variable {
                    self.aliases.insert(var.clone(), AliasType::Node);
                }
            }
            PathElement::Edge(edge) => {
                if let Some(ref var) = edge.variable {
                    self.aliases.insert(var.clone(), AliasType::Edge);
                }
            }
            PathElement::Alternative(patterns) => {
                for pattern in patterns {
                    self.collect_aliases_from_patterns(std::slice::from_ref(pattern))?;
                }
            }
            PathElement::Optional(inner) => {
                self.collect_aliases_from_path_element(inner)?;
            }
            PathElement::Repeated(inner, _) => {
                self.collect_aliases_from_path_element(inner)?;
            }
        }
        Ok(())
    }

    /// Verify the WHERE clause
    fn validate_where_clause(
        &mut self,
        where_expr: &ContextualExpression,
    ) -> Result<(), ValidationError> {
        let strategy = ExpressionValidationStrategy::new();
        let context = WhereClauseContext {
            filter: Some(where_expr.clone()),
            aliases_available: self.aliases.clone(),
            aliases_generated: HashMap::new(),
            paths: Vec::new(),
            query_parts: Vec::new(),
            errors: Vec::new(),
        };
        strategy.validate_filter(where_expr, &context)
    }

    /// Verify the RETURN statement
    fn validate_return_clause(
        &mut self,
        return_clause: &ReturnClause,
    ) -> Result<(), ValidationError> {
        if return_clause.items.is_empty() {
            return Err(ValidationError::new(
                "The RETURN clause must contain at least one return item".to_string(),
                ValidationErrorType::SemanticError,
            ));
        }

        for (idx, item) in return_clause.items.iter().enumerate() {
            match item {
                ReturnItem::Expression { expression, alias } => {
                    self.validate_return_expression(expression, idx)?;

                    if let Some(ref alias_name) = alias {
                        if alias_name.is_empty() {
                            return Err(ValidationError::new(
                                format!(
                                    "The alias of the {}th return item cannot be null.",
                                    idx + 1
                                ),
                                ValidationErrorType::SemanticError,
                            ));
                        }
                        self.aliases.insert(alias_name.clone(), AliasType::Runtime);
                    }
                }
            }
        }

        Ok(())
    }

    /// Verify the returned expression.
    fn validate_return_expression(
        &mut self,
        expr: &ContextualExpression,
        idx: usize,
    ) -> Result<(), ValidationError> {
        if expr.expression().is_none() {
            return Err(ValidationError::new(
                format!("The {}th return expression is invalid", idx + 1),
                ValidationErrorType::SemanticError,
            ));
        }

        // Verify that the variable is defined
        let variables = expr.get_variables();
        for var_name in variables {
            if !self.aliases.contains_key(&var_name) {
                return Err(ValidationError::new(
                    format!(
                        "The {}th return item references the undefined variable '{}'.",
                        idx + 1,
                        var_name
                    ),
                    ValidationErrorType::SemanticError,
                ));
            }
        }

        // Note: The function name needs to be accessed, but the `ContextualExpression` class does not provide such a method.
        // Skip function validation for now and wait for further improvements.ntextualExpression 没有提供此方法
        // Skip function validation for now and wait for subsequent refinements

        Ok(())
    }

    /// Verify the ORDER BY clause
    fn validate_order_by(&mut self, order_by: &OrderByClause) -> Result<(), ValidationError> {
        if order_by.items.is_empty() {
            return Err(ValidationError::new(
                "The ORDER BY clause must contain at least one sort term".to_string(),
                ValidationErrorType::SemanticError,
            ));
        }

        for (idx, item) in order_by.items.iter().enumerate() {
            if item.expression.expression().is_none() {
                return Err(ValidationError::new(
                    format!("The {}th sort expression is invalid", idx + 1),
                    ValidationErrorType::SemanticError,
                ));
            }

            let variables = item.expression.get_variables();
            for var_name in variables {
                if !self.aliases.contains_key(&var_name) {
                    return Err(ValidationError::new(
                        format!(
                            "The {}th sort item references the undefined variable '{}'",
                            idx + 1,
                            var_name
                        ),
                        ValidationErrorType::SemanticError,
                    ));
                }
            }
        }

        Ok(())
    }

    /// Verify the alias
    pub fn validate_aliases(
        &mut self,
        exprs: &[ContextualExpression],
        aliases: &HashMap<String, AliasType>,
    ) -> Result<(), ValidationError> {
        for (idx, expr) in exprs.iter().enumerate() {
            if expr.expression().is_none() {
                return Err(ValidationError::new(
                    format!("The {} expression is invalid", idx + 1),
                    ValidationErrorType::SemanticError,
                ));
            }

            // Verify that the variable is defined in an alias
            let variables = expr.get_variables();
            for var_name in variables {
                if !aliases.contains_key(&var_name) {
                    return Err(ValidationError::new(
                        format!(
                            "The {}th expression references the undefined alias '{}'",
                            idx + 1,
                            var_name
                        ),
                        ValidationErrorType::SemanticError,
                    ));
                }
            }
        }
        Ok(())
    }

    /// Check whether the expression contains aggregate functions.
    pub fn has_aggregate_expression(&self, expression: &ContextualExpression) -> bool {
        expression.contains_aggregate()
    }

    /// Verify pagination
    pub fn validate_pagination(
        &mut self,
        _skip_expression: Option<&ContextualExpression>,
        _limit_expression: Option<&ContextualExpression>,
        context: &PaginationContext,
    ) -> Result<(), ValidationError> {
        // Validate skip value
        if context.skip < 0 {
            return Err(ValidationError::new(
                "SKIP value cannot be negative".to_string(),
                ValidationErrorType::SemanticError,
            ));
        }

        // Validating Limit Values
        if context.limit < 0 {
            return Err(ValidationError::new(
                "The LIMIT value cannot be negative.".to_string(),
                ValidationErrorType::SemanticError,
            ));
        }

        self.pagination = Some(context.clone());
        Ok(())
    }

    /// Verify the range of steps
    pub fn validate_step_range(&self, range: &MatchStepRange) -> Result<(), ValidationError> {
        if range.min() > range.max() {
            return Err(ValidationError::new(
                format!(
                    "Invalid step range: min ({}) greater than max ({})",
                    range.min(),
                    range.max()
                ),
                ValidationErrorType::SemanticError,
            ));
        }
        Ok(())
    }

    /// Verify the filtering criteria.
    pub fn validate_filter(
        &mut self,
        filter: &ContextualExpression,
        _context: &WhereClauseContext,
    ) -> Result<(), ValidationError> {
        self.validate_where_clause(filter)
    }

    /// Verify the “Return” clause (full context version)
    pub fn validate_return(
        &mut self,
        _return_expression: &ContextualExpression,
        return_items: &[YieldColumn],
        _context: &ReturnClauseContext,
    ) -> Result<(), ValidationError> {
        if return_items.is_empty() {
            return Err(ValidationError::new(
                "The RETURN clause must contain at least one return item".to_string(),
                ValidationErrorType::SemanticError,
            ));
        }
        Ok(())
    }

    /// Verify the “With” clause
    pub fn validate_with(
        &mut self,
        _with_expression: &ContextualExpression,
        with_items: &[YieldColumn],
        _context: &WithClauseContext,
    ) -> Result<(), ValidationError> {
        if with_items.is_empty() {
            return Err(ValidationError::new(
                "The WITH clause must contain at least one item".to_string(),
                ValidationErrorType::SemanticError,
            ));
        }
        Ok(())
    }

    /// Verify the Unwind clause
    pub fn validate_unwind(
        &mut self,
        unwind_expression: &ContextualExpression,
        context: &UnwindClauseContext,
    ) -> Result<(), ValidationError> {
        if unwind_expression.expression().is_none() {
            return Err(ValidationError::new(
                "UNWIND expression is invalid".to_string(),
                ValidationErrorType::SemanticError,
            ));
        }

        // Verify that the variable is defined
        let variables = unwind_expression.get_variables();
        for var_name in variables {
            if !self.aliases.contains_key(&var_name) {
                return Err(ValidationError::new(
                    format!("UNWIND references the undefined variable '{}'", var_name),
                    ValidationErrorType::SemanticError,
                ));
            }
        }

        // Add unwind alias
        self.aliases
            .insert(context.alias.clone(), AliasType::Variable);
        Ok(())
    }

    /// Verify the Yield clause
    pub fn validate_yield(&mut self, context: &YieldClauseContext) -> Result<(), ValidationError> {
        if context.yield_columns.is_empty() {
            return Err(ValidationError::new(
                "The YIELD clause must contain at least one column".to_string(),
                ValidationErrorType::SemanticError,
            ));
        }
        Ok(())
    }

    /// Create all columns with their corresponding aliases.
    pub fn build_columns_for_all_named_aliases(
        &mut self,
        query_parts: &[QueryPart],
        columns: &mut Vec<YieldColumn>,
    ) -> Result<(), ValidationError> {
        for part in query_parts {
            for alias in part.aliases_generated.keys() {
                let ctx = ContextualExpression::new(
                    crate::core::types::expr::ExpressionId::new(0),
                    std::sync::Arc::new(ExpressionAnalysisContext::new()),
                );
                let col = YieldColumn::new(ctx, alias.clone());
                columns.push(col);
            }
        }
        Ok(())
    }

    /// Taking into account the aliases…
    pub fn combine_aliases(
        &mut self,
        cur_aliases: &mut HashMap<String, AliasType>,
        last_aliases: &HashMap<String, AliasType>,
    ) -> Result<(), ValidationError> {
        for (alias, alias_type) in last_aliases {
            if cur_aliases.contains_key(alias) {
                if cur_aliases.get(alias) != Some(alias_type) {
                    return Err(ValidationError::new(
                        format!("Inconsistency in the type of the alias '{}'.", alias),
                        ValidationErrorType::SemanticError,
                    ));
                }
            } else {
                cur_aliases.insert(alias.clone(), alias_type.clone());
            }
        }
        Ok(())
    }

    /// Build the output.
    pub fn build_output(&mut self, paths: &[Path]) -> Result<(), ValidationError> {
        for path in paths.iter() {
            for node_info in &path.node_infos {
                if !node_info.alias.is_empty() {
                    let col = ColumnDef {
                        name: node_info.alias.clone(),
                        type_: ValueType::Vertex,
                    };
                    self.outputs.push(col);
                }
            }
            for edge_info in &path.edge_infos {
                if !edge_info.alias.is_empty() {
                    let col = ColumnDef {
                        name: edge_info.alias.clone(),
                        type_: ValueType::Edge,
                    };
                    self.outputs.push(col);
                }
            }
        }
        Ok(())
    }

    /// Check the aliases.
    pub fn check_alias(
        &mut self,
        ref_expression: &ContextualExpression,
        aliases_available: &HashMap<String, AliasType>,
    ) -> Result<(), ValidationError> {
        if ref_expression.expression().is_none() {
            return Err(ValidationError::new(
                "Invalid quoted expression".to_string(),
                ValidationErrorType::SemanticError,
            ));
        }

        // Verify that the variable is defined in an alias
        let variables = ref_expression.get_variables();
        for var_name in variables {
            if !aliases_available.contains_key(&var_name) {
                return Err(ValidationError::new(
                    format!("Reference to undefined alias '{}'", var_name),
                    ValidationErrorType::SemanticError,
                ));
            }
        }

        Ok(())
    }

    /// Generate a column of outputs.
    fn generate_output_columns(&mut self, match_stmt: &MatchStmt) {
        self.outputs.clear();

        if let Some(ref return_clause) = match_stmt.return_clause {
            for item in &return_clause.items {
                match item {
                    ReturnItem::Expression { expression, alias } => {
                        let name = alias.clone().unwrap_or_else(|| {
                            expression
                                .as_variable()
                                .unwrap_or_else(|| format!("col_{}", self.outputs.len()))
                        });
                        let col = ColumnDef {
                            name,
                            type_: ValueType::Unknown,
                        };
                        self.outputs.push(col);
                    }
                }
            }
        }
    }
}

/// Implementing the StatementValidator trait
///
/// # Refactoring changes
/// The `validate` method accepts `Arc<Ast>` and `Arc<QueryContext>` as parameters.
/// The `validate` function returns a complete validation result that contains the `ValidationInfo`.
impl StatementValidator for MatchValidator {
    fn validate(
        &mut self,
        ast: Arc<Ast>,
        qctx: Arc<QueryContext>,
    ) -> Result<ValidationResult, ValidationError> {
        // 1. Check if space is needed
        if !self.is_global_statement() && qctx.space_id().is_none() {
            return Err(ValidationError::new(
                "No image space selected, please execute first USE <space>".to_string(),
                ValidationErrorType::SemanticError,
            ));
        }

        // 2. Set space name for schema validation
        if let Some(space_name) = qctx.space_name() {
            self.space_name = Some(space_name);
        }

        // 3. Getting the MATCH statement
        let match_stmt = match &ast.stmt {
            Stmt::Match(m) => m,
            _ => {
                return Err(ValidationError::new(
                    "Expected MATCH statement".to_string(),
                    ValidationErrorType::SemanticError,
                ));
            }
        };

        // 4. Validating the MATCH statement
        self.validate_match_statement(match_stmt)?;

        // 5. Get space_id
        let space_id = qctx.space_id().unwrap_or(0);

        // 6. Generation of output columns
        self.generate_output_columns(match_stmt);

        // 7. Constructing a detailed ValidationInfo
        let mut info = ValidationInfo::new();

        // 7.1 Adding an Alias Map
        for (name, alias_type) in &self.aliases {
            info.add_alias(name.clone(), alias_type.clone());
        }

        // 6.2 Adding Path Analysis
        for pattern in &match_stmt.patterns {
            if let crate::query::parser::ast::Pattern::Path(path) = pattern {
                let mut analysis = PathAnalysis::new();
                analysis.node_count = path
                    .elements
                    .iter()
                    .filter(|e| matches!(e, crate::query::parser::ast::PathElement::Node(_)))
                    .count();
                analysis.edge_count = path
                    .elements
                    .iter()
                    .filter(|e| matches!(e, crate::query::parser::ast::PathElement::Edge(_)))
                    .count();
                info.add_path_analysis(analysis);
            }
        }

        // 6.3 Adding Optimization Tips
        if self.aliases.len() > 10 {
            info.add_optimization_hint(
                crate::query::validator::OptimizationHint::PerformanceWarning {
                    message:
                        "Queries contain a large number of aliases, which may affect performance"
                            .to_string(),
                    severity: crate::query::validator::HintSeverity::Warning,
                },
            );
        }

        // 6.4 Adding semantic information
        info.semantic_info.referenced_tags = self.get_referenced_tags();
        info.semantic_info.referenced_edges = self.get_referenced_edges();

        // 7. Create the validation result (put it in the last step to avoid unnecessary clone)
        let validated = ValidatedMatch {
            space_id,
            patterns: match_stmt.patterns.clone(),
            where_clause: match_stmt.where_clause.clone(),
            return_clause: match_stmt.return_clause.clone(),
            order_by: match_stmt.order_by.clone(),
            limit: match_stmt.limit,
            skip: match_stmt.skip,
            optional: match_stmt.optional,
            aliases: self.aliases.clone(),
        };

        self.validated_result = Some(validated);
        self.optional = match_stmt.optional;

        // 8. Returning validation results with detailed information
        Ok(ValidationResult::success_with_info(info))
    }

    fn statement_type(&self) -> StatementType {
        StatementType::Match
    }

    fn inputs(&self) -> &[ColumnDef] {
        &self.inputs
    }

    fn outputs(&self) -> &[ColumnDef] {
        &self.outputs
    }

    fn is_global_statement(&self) -> bool {
        // "MATCH" is not a global statement; it is necessary to select a domain (a specific "space") in advance.
        false
    }

    fn expression_props(&self) -> &ExpressionProps {
        &self.expression_props
    }

    fn user_defined_vars(&self) -> &[String] {
        &self.user_defined_vars
    }
}

impl Default for MatchValidator {
    fn default() -> Self {
        Self::new()
    }
}

impl MatchValidator {
    /// Obtain the list of tags used in the citation.
    fn get_referenced_tags(&self) -> Vec<String> {
        let mut tags = Vec::new();
        if let Some(ref validated) = self.validated_result {
            for pattern in &validated.patterns {
                if let crate::query::parser::ast::Pattern::Node(node) = pattern {
                    tags.extend(node.labels.clone());
                }
            }
        }
        tags
    }

    /// Obtain the list of edge types that are referenced.
    fn get_referenced_edges(&self) -> Vec<String> {
        let mut edges = Vec::new();
        if let Some(ref validated) = self.validated_result {
            for pattern in &validated.patterns {
                if let crate::query::parser::ast::Pattern::Edge(edge) = pattern {
                    edges.extend(edge.edge_types.clone());
                }
            }
        }
        edges
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::expr::contextual::ContextualExpression;
    use crate::core::Expression;
    use crate::core::Value;
    use ExpressionAnalysisContext;

    fn create_contextual_expr(expr: Expression) -> ContextualExpression {
        let ctx = std::sync::Arc::new(ExpressionAnalysisContext::new());
        let meta = crate::core::types::expr::ExpressionMeta::new(expr);
        let id = ctx.register_expression(meta);
        ContextualExpression::new(id, ctx)
    }

    #[test]
    fn test_match_validator_creation() {
        let validator = MatchValidator::new();
        assert_eq!(validator.inputs.len(), 0);
        assert_eq!(validator.outputs.len(), 0);
    }

    #[test]
    fn test_match_validator_with_pagination() {
        let validator = MatchValidator::with_pagination(10, 100);
        assert!(validator.pagination.is_some());
        let ctx = validator
            .pagination
            .expect("Failed to get pagination context");
        assert_eq!(ctx.skip, 10);
        assert_eq!(ctx.limit, 100);
    }

    #[test]
    fn test_validate_step_range() {
        let validator = MatchValidator::new();

        // Test valid range (min <= max)
        let valid_range = MatchStepRange::new(1, 3);
        assert!(validator.validate_step_range(&valid_range).is_ok());

        // Test invalid range (min > max)
        let invalid_range = MatchStepRange::new(3, 1);
        assert!(validator.validate_step_range(&invalid_range).is_err());
    }

    #[test]
    fn test_validate_aliases() {
        let mut validator = MatchValidator::new();

        // Creating an alias map
        let mut aliases = HashMap::new();
        aliases.insert("n".to_string(), AliasType::Node);
        aliases.insert("e".to_string(), AliasType::Edge);

        // Testing for valid alias references
        let expression = create_contextual_expr(Expression::Variable("n".to_string()));
        assert!(validator.validate_aliases(&[expression], &aliases).is_ok());

        // Testing for invalid alias references
        let invalid_expression =
            create_contextual_expr(Expression::Variable("invalid".to_string()));
        assert!(validator
            .validate_aliases(&[invalid_expression], &aliases)
            .is_err());
    }

    #[test]
    fn test_has_aggregate_expression() {
        let validator = MatchValidator::new();
        let non_agg_expression = create_contextual_expr(Expression::Literal(Value::Int(1)));
        assert!(!validator.has_aggregate_expression(&non_agg_expression));

        // Testing Expressions with Aggregate Functions
        let agg_expression = create_contextual_expr(Expression::Aggregate {
            func: crate::core::types::operators::AggregateFunction::Count(None),
            arg: Box::new(Expression::Variable("n".to_string())),
            distinct: false,
        });
        assert!(validator.has_aggregate_expression(&agg_expression));
    }

    #[test]
    fn test_combine_aliases() {
        let mut validator = MatchValidator::new();

        let mut cur_aliases = HashMap::new();
        cur_aliases.insert("a".to_string(), AliasType::Node);

        let mut last_aliases = HashMap::new();
        last_aliases.insert("b".to_string(), AliasType::Edge);
        last_aliases.insert("c".to_string(), AliasType::Path);

        // portfolio alias
        assert!(validator
            .combine_aliases(&mut cur_aliases, &last_aliases)
            .is_ok());
        assert_eq!(cur_aliases.len(), 3);
        assert!(cur_aliases.contains_key("a"));
        assert!(cur_aliases.contains_key("b"));
        assert!(cur_aliases.contains_key("c"));
    }

    #[test]
    fn test_validate_pagination() {
        let mut validator = MatchValidator::new();

        let ctx = PaginationContext { skip: 0, limit: 10 };
        assert!(validator.validate_pagination(None, None, &ctx).is_ok());

        let invalid_ctx = PaginationContext {
            skip: -1,
            limit: 10,
        };
        assert!(validator
            .validate_pagination(None, None, &invalid_ctx)
            .is_err());

        let ctx2 = PaginationContext { skip: 10, limit: 5 };
        assert!(validator.validate_pagination(None, None, &ctx2).is_ok());
    }

    #[test]
    fn test_statement_type() {
        let validator = MatchValidator::new();
        assert_eq!(validator.statement_type(), StatementType::Match);
    }

    #[test]
    fn test_requires_space() {
        let validator = MatchValidator::new();
        assert!(validator.requires_space());
    }

    #[test]
    fn test_requires_write_permission() {
        let validator = MatchValidator::new();
        assert!(!validator.requires_write_permission());
    }

    #[test]
    fn test_validate_variable_pattern_valid() {
        use crate::query::parser::ast::pattern::{NodePattern, Pattern, VariablePattern};
        use crate::query::parser::ast::types::Span;

        let mut validator = MatchValidator::new();

        // First define a node variable
        let node_pattern = NodePattern::new(
            Some("a".to_string()),
            vec!["Person".to_string()],
            None,
            vec![],
            Span::default(),
        );
        let node_var_pattern = Pattern::Node(node_pattern);

        // Collecting aliases
        validator
            .collect_aliases_from_patterns(&[node_var_pattern])
            .expect("Failed to collect aliases");

        // Verify that the variable pattern references a defined variable
        let var_pattern = VariablePattern::new("a".to_string(), Span::default());
        let pattern = Pattern::Variable(var_pattern);

        assert!(validator.validate_pattern(&pattern, 0).is_ok());
    }

    #[test]
    fn test_validate_variable_pattern_undefined() {
        use crate::query::parser::ast::pattern::{Pattern, VariablePattern};
        use crate::query::parser::ast::types::Span;

        let mut validator = MatchValidator::new();

        // Verify that variable patterns referencing undefined variables should fail
        let var_pattern = VariablePattern::new("undefined".to_string(), Span::default());
        let pattern = Pattern::Variable(var_pattern);

        assert!(validator.validate_pattern(&pattern, 0).is_err());
    }

    #[test]
    fn test_validate_variable_pattern_runtime_alias() {
        use crate::query::parser::ast::pattern::{Pattern, VariablePattern};
        use crate::query::parser::ast::types::Span;

        let mut validator = MatchValidator::new();

        // Add a runtime alias (as defined in the RETURN clause)
        validator
            .aliases
            .insert("runtime_alias".to_string(), AliasType::Runtime);

        // Validating variable mode references to runtime aliases should fail
        let var_pattern = VariablePattern::new("runtime_alias".to_string(), Span::default());
        let pattern = Pattern::Variable(var_pattern);

        let result = validator.validate_pattern(&pattern, 0);
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert!(error.message.contains("runtime computation"));
    }

    #[test]
    fn test_collect_aliases_skips_variable_pattern() {
        use crate::query::parser::ast::pattern::{Pattern, VariablePattern};
        use crate::query::parser::ast::types::Span;

        let mut validator = MatchValidator::new();

        // VariablePattern should not be collected as an alias
        let var_pattern = VariablePattern::new("var".to_string(), Span::default());
        let pattern = Pattern::Variable(var_pattern);

        validator
            .collect_aliases_from_patterns(&[pattern])
            .expect("Failed to collect aliases");

        // Verify that the alias map is empty, since VariablePattern is a reference and not a definition.
        assert!(validator.aliases.is_empty());
    }
}
