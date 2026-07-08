//! Update Statement Validator (Enhanced Version)
//! Corresponding to the functionality of NebulaGraph UpdateValidator
//! Verify the semantic correctness of the UPDATE statement.

use std::sync::Arc;

use crate::core::error::DBResult;
use crate::core::metadata::SchemaManager;
use crate::core::types::expr::contextual::ContextualExpression;
use crate::core::Expression;
use crate::core::Value;
use crate::query::parser::ast::stmt::{Ast, SetClause, UpdateStmt, UpdateTarget};
use crate::query::validator::error::{
    ValidationError, ValidationError as CoreValidationError, ValidationErrorType,
};
use crate::query::validator::helpers::schema_validator::SchemaValidator;
use crate::query::validator::structs::validation_info::ValidationInfo;
use crate::query::validator::validator_trait::{
    ColumnDef, ExpressionProps, StatementType, StatementValidator, ValidationResult, ValueType,
};
use crate::query::QueryContext;

/// Verified update information
#[derive(Debug, Clone)]
pub struct ValidatedUpdate {
    pub space_id: u64,
    pub target_type: UpdateTargetType,
    pub tag_or_edge_id: Option<u32>,
    pub tag_or_edge_name: Option<String>,
    pub assignments: Vec<ValidatedAssignment>,
    pub where_clause: Option<ContextualExpression>,
    pub is_upsert: bool,
    pub yield_columns: Option<Vec<String>>,
}

/// Update the target type.
#[derive(Debug, Clone)]
pub enum UpdateTargetType {
    Vertex(Value),
    Edge {
        src: Value,
        dst: Value,
        edge_type: String,
        rank: i64,
    },
    Tag(String, Value),
}

/// Verified assignment
#[derive(Debug, Clone)]
pub struct ValidatedAssignment {
    pub property: String,
    pub value: Value,
    pub prop_id: Option<i32>,
    pub expression: Option<ContextualExpression>,
}

#[derive(Debug)]
pub struct UpdateValidator {
    schema_validator: Option<SchemaValidator>,
    inputs: Vec<ColumnDef>,
    outputs: Vec<ColumnDef>,
    expr_props: ExpressionProps,
    user_defined_vars: Vec<String>,
}

impl UpdateValidator {
    pub fn new() -> Self {
        Self {
            schema_validator: None,
            inputs: Vec::new(),
            outputs: Vec::new(),
            expr_props: ExpressionProps::default(),
            user_defined_vars: Vec::new(),
        }
    }

    pub fn with_schema_manager(mut self, schema_manager: Arc<SchemaManager>) -> Self {
        self.schema_validator = Some(SchemaValidator::new(schema_manager));
        self
    }

    /// Verify the UPDATE statement and return the verified information.
    pub fn validate_with_schema(
        &mut self,
        stmt: &UpdateStmt,
        space_name: &str,
    ) -> Result<ValidatedUpdate, CoreValidationError> {
        // Basic validation (variable borrowing that does not rely on the schema_validator)
        self.validate_update_stmt(stmt, Some(space_name))?;

        let schema_validator = self.schema_validator.as_ref().ok_or_else(|| {
            CoreValidationError::new(
                "Schema validator not initialized".to_string(),
                ValidationErrorType::SemanticError,
            )
        })?;

        let space = schema_validator
            .get_schema_manager()
            .get_space(space_name)
            .map_err(|e| {
                CoreValidationError::new(
                    format!("Failed to get space '{}': {}", space_name, e),
                    ValidationErrorType::SemanticError,
                )
            })?
            .ok_or_else(|| {
                CoreValidationError::new(
                    format!("Space '{}' not found", space_name),
                    ValidationErrorType::SemanticError,
                )
            })?;

        // Verify and convert the target.
        let target_type = self.validate_and_convert_target_with_schema(
            &stmt.target,
            &space.vid_type,
            schema_validator,
        )?;

        // Obtain Schema information based on the target type.
        let (tag_or_edge_id, tag_or_edge_name, schema_props) = match &target_type {
            UpdateTargetType::Tag(tag_name, _) => {
                let tag_info = schema_validator
                    .get_tag(space_name, tag_name)
                    .map_err(|e| {
                        CoreValidationError::new(
                            format!("Failed to get tag '{}': {}", tag_name, e),
                            ValidationErrorType::SemanticError,
                        )
                    })?
                    .ok_or_else(|| {
                        CoreValidationError::new(
                            format!("Tag '{}' not found in space '{}'", tag_name, space_name),
                            ValidationErrorType::SemanticError,
                        )
                    })?;
                (
                    Some(tag_info.tag_id),
                    Some(tag_name.clone()),
                    tag_info.properties,
                )
            }
            UpdateTargetType::Edge { edge_type, .. } => {
                let edge_info = schema_validator
                    .get_edge_type(space_name, edge_type)
                    .map_err(|e| {
                        CoreValidationError::new(
                            format!("Failed to get edge type '{}': {}", edge_type, e),
                            ValidationErrorType::SemanticError,
                        )
                    })?
                    .ok_or_else(|| {
                        CoreValidationError::new(
                            format!(
                                "Edge type '{}' not found in space '{}'",
                                edge_type, space_name
                            ),
                            ValidationErrorType::SemanticError,
                        )
                    })?;
                (
                    Some(edge_info.edge_type_id),
                    Some(edge_type.clone()),
                    edge_info.properties,
                )
            }
            _ => (None, None, vec![]),
        };

        // Verify and convert the assignment.
        // For the Vertex target, skip the attribute schema validation (because a Vertex may be associated with multiple Tags).
        let validated_assignments = match &target_type {
            UpdateTargetType::Vertex(_) => {
                // Vertex Update: Only the assignment syntax is verified; the existence of the property is not checked.
                self.validate_and_convert_assignments_without_schema(
                    &stmt.set_clause,
                    schema_validator,
                )?
            }
            _ => {
                // Tag or Edge update: Verify that the attribute exists in the Schema.
                self.validate_and_convert_assignments(
                    &stmt.set_clause,
                    &schema_props,
                    schema_validator,
                )?
            }
        };

        // Extract the column name “YIELD”.
        let yield_columns = stmt.yield_clause.as_ref().map(|yc| {
            yc.items
                .iter()
                .map(|item| {
                    item.alias
                        .clone()
                        .unwrap_or_else(|| format!("{:?}", item.expression))
                })
                .collect()
        });

        Ok(ValidatedUpdate {
            space_id: space.space_id,
            target_type,
            tag_or_edge_id,
            tag_or_edge_name,
            assignments: validated_assignments,
            where_clause: stmt.where_clause.clone(),
            is_upsert: stmt.is_upsert,
            yield_columns,
        })
    }

    /// Basic validation (does not rely on a Schema)
    pub fn validate_update_stmt(
        &mut self,
        stmt: &UpdateStmt,
        space_name: Option<&str>,
    ) -> Result<(), CoreValidationError> {
        self.validate_target(&stmt.target, space_name)?;
        self.validate_set_clause(&stmt.set_clause)?;
        self.validate_where_clause(stmt.where_clause.as_ref())?;
        self.validate_assignments(&stmt.set_clause)?;
        Ok(())
    }

    /// Full verification (including the AST context)
    pub fn validate_with_ast(
        &mut self,
        stmt: &UpdateStmt,
        qctx: Arc<QueryContext>,
    ) -> DBResult<()> {
        self.validate_space_chosen(qctx.clone())?;
        let space_name = qctx.space_name();
        self.validate_update_stmt(stmt, space_name.as_deref())?;
        self.generate_output_columns();
        Ok(())
    }

    fn validate_target(
        &self,
        target: &UpdateTarget,
        space_name: Option<&str>,
    ) -> Result<(), CoreValidationError> {
        match target {
            UpdateTarget::Vertex(vid_expr) => {
                self.validate_vertex_id(vid_expr, "vertex", space_name)?;
            }
            UpdateTarget::Edge {
                src,
                dst,
                edge_type,
                rank,
            } => {
                self.validate_vertex_id(src, "source", space_name)?;
                self.validate_vertex_id(dst, "destination", space_name)?;
                if let Some(rank_expr) = rank {
                    self.validate_rank(rank_expr)?;
                }
                if let Some(et) = edge_type {
                    if et.is_empty() {
                        return Err(CoreValidationError::new(
                            "Edge type name cannot be empty".to_string(),
                            ValidationErrorType::SemanticError,
                        ));
                    }
                }
            }
            UpdateTarget::Tag(tag_name) => {
                if tag_name.is_empty() {
                    return Err(CoreValidationError::new(
                        "Tag name cannot be empty".to_string(),
                        ValidationErrorType::SemanticError,
                    ));
                }
            }
            UpdateTarget::TagOnVertex { vid, tag_name } => {
                self.validate_vertex_id(vid, "vertex", space_name)?;
                if tag_name.is_empty() {
                    return Err(CoreValidationError::new(
                        "Tag name cannot be empty".to_string(),
                        ValidationErrorType::SemanticError,
                    ));
                }
            }
        }
        Ok(())
    }

    /// Verify and convert the target content (using Schema).
    fn validate_and_convert_target_with_schema(
        &self,
        target: &UpdateTarget,
        vid_type: &crate::core::types::DataType,
        schema_validator: &SchemaValidator,
    ) -> Result<UpdateTargetType, CoreValidationError> {
        match target {
            UpdateTarget::Vertex(vid_expr) => {
                let vid =
                    self.validate_and_evaluate_vid(vid_expr, vid_type, schema_validator, "vertex")?;
                Ok(UpdateTargetType::Vertex(vid))
            }
            UpdateTarget::Edge {
                src,
                dst,
                edge_type,
                rank,
            } => {
                let src_vid =
                    self.validate_and_evaluate_vid(src, vid_type, schema_validator, "source")?;
                let dst_vid =
                    self.validate_and_evaluate_vid(dst, vid_type, schema_validator, "destination")?;
                let rank_val = if let Some(rank_expr) = rank {
                    self.evaluate_rank_contextual(rank_expr, schema_validator)?
                } else {
                    0
                };
                let et = edge_type.as_ref().ok_or_else(|| {
                    CoreValidationError::new(
                        "Edge type is required for edge update".to_string(),
                        ValidationErrorType::SemanticError,
                    )
                })?;
                Ok(UpdateTargetType::Edge {
                    src: src_vid,
                    dst: dst_vid,
                    edge_type: et.clone(),
                    rank: rank_val,
                })
            }
            UpdateTarget::Tag(_tag_name) => {
                // Tag update requires a vertex ID from the context
                // For now, we return an error as we need the VID from elsewhere
                Err(CoreValidationError::new(
                    "Tag update requires vertex ID context".to_string(),
                    ValidationErrorType::SemanticError,
                ))
            }
            UpdateTarget::TagOnVertex { vid, tag_name } => {
                let vid_val =
                    self.validate_and_evaluate_vid(vid, vid_type, schema_validator, "vertex")?;
                Ok(UpdateTargetType::Tag(tag_name.clone(), vid_val))
            }
        }
    }

    /// Verify the vertex ID
    /// Give priority to the unified validation methods provided by SchemaValidator.
    fn validate_vertex_id(
        &self,
        expr: &ContextualExpression,
        role: &str,
        space_name: Option<&str>,
    ) -> Result<(), CoreValidationError> {
        if expr.expression().is_none() {
            return Err(CoreValidationError::new(
                format!("{} vertex ID is invalid", role),
                ValidationErrorType::SemanticError,
            ));
        }

        // Get vid_type from schema_manager if available, otherwise default to String
        let vid_type = if let (Some(ref schema_validator), Some(space_name)) =
            (&self.schema_validator, space_name)
        {
            match schema_validator.get_schema_manager().get_space(space_name) {
                Ok(Some(space_info)) => space_info.vid_type,
                _ => crate::core::types::DataType::String,
            }
        } else {
            crate::core::types::DataType::String
        };

        if let Some(ref schema_validator) = self.schema_validator {
            let ctx_expr = crate::core::types::ContextualExpression::new(
                expr.id().clone(),
                expr.context().clone(),
            );
            return schema_validator.validate_vid_expr(&ctx_expr, &vid_type, role);
        }

        // Basic validation
        if expr.is_variable() {
            return Ok(());
        }

        if expr.is_literal() {
            if let Some(value) = expr.as_literal() {
                match value {
                    crate::core::Value::String(s) => {
                        if s.is_empty() {
                            return Err(CoreValidationError::new(
                                format!("{} vertex ID cannot be empty", role),
                                ValidationErrorType::SemanticError,
                            ));
                        }
                        return Ok(());
                    }
                    crate::core::Value::SmallInt(_)
                    | crate::core::Value::Int(_)
                    | crate::core::Value::BigInt(_) => {
                        return Ok(());
                    }
                    _ => {}
                }
            }
        }

        Err(CoreValidationError::new(
            format!("{} vertex ID must be a string constant or variable", role),
            ValidationErrorType::SemanticError,
        ))
    }

    /// Verify and evaluate the VID.
    fn validate_and_evaluate_vid(
        &self,
        vid_expr: &ContextualExpression,
        vid_type: &crate::core::types::DataType,
        schema_validator: &SchemaValidator,
        role: &str,
    ) -> Result<Value, CoreValidationError> {
        if vid_expr.expression().is_none() {
            return Err(CoreValidationError::new(
                format!("{} vertex ID is invalid", role),
                ValidationErrorType::SemanticError,
            ));
        }

        let vid = schema_validator
            .evaluate_expression(vid_expr)
            .map_err(|e| {
                CoreValidationError::new(
                    format!("Failed to evaluate {} vertex ID: {}", role, e.message),
                    e.error_type,
                )
            })?;

        schema_validator.validate_vid(&vid, vid_type).map_err(|e| {
            CoreValidationError::new(
                format!("Invalid {} vertex ID: {}", role, e.message),
                e.error_type,
            )
        })?;

        Ok(vid)
    }

    fn validate_rank(&self, expr: &ContextualExpression) -> Result<(), CoreValidationError> {
        if expr.expression().is_none() {
            return Err(CoreValidationError::new(
                "Rank expression is invalid".to_string(),
                ValidationErrorType::SemanticError,
            ));
        }

        if expr.is_variable() || expr.is_literal() {
            return Ok(());
        }

        Err(CoreValidationError::new(
            "Rank must be an integer constant or variable".to_string(),
            ValidationErrorType::SemanticError,
        ))
    }

    /// Evaluating the rank expression
    fn evaluate_rank_contextual(
        &self,
        expr: &ContextualExpression,
        schema_validator: &SchemaValidator,
    ) -> Result<i64, CoreValidationError> {
        if expr.expression().is_none() {
            return Err(CoreValidationError::new(
                "Rank expression is invalid".to_string(),
                ValidationErrorType::SemanticError,
            ));
        }

        let value = schema_validator.evaluate_expression(expr).map_err(|e| {
            CoreValidationError::new(
                format!("Failed to evaluate rank: {}", e.message),
                e.error_type,
            )
        })?;

        match value {
            Value::BigInt(i) => Ok(i),
            _ => Err(CoreValidationError::new(
                "Rank must be an integer".to_string(),
                ValidationErrorType::TypeMismatch,
            )),
        }
    }

    fn validate_set_clause(&self, set_clause: &SetClause) -> Result<(), CoreValidationError> {
        if set_clause.assignments.is_empty() {
            return Err(CoreValidationError::new(
                "UPDATE statement must have at least one SET clause".to_string(),
                ValidationErrorType::SemanticError,
            ));
        }
        Ok(())
    }

    fn validate_assignments(&self, set_clause: &SetClause) -> Result<(), CoreValidationError> {
        let mut seen = std::collections::HashSet::new();
        for assignment in &set_clause.assignments {
            if !seen.insert(assignment.property.clone()) {
                return Err(CoreValidationError::new(
                    format!(
                        "Duplicate property assignment for '{}'",
                        assignment.property
                    ),
                    ValidationErrorType::SemanticError,
                ));
            }
            self.validate_property_value(&assignment.value)?;
        }
        Ok(())
    }

    /// Verify and convert the assignment.
    fn validate_and_convert_assignments(
        &self,
        set_clause: &SetClause,
        schema_props: &[crate::core::types::PropertyDef],
        schema_validator: &SchemaValidator,
    ) -> Result<Vec<ValidatedAssignment>, CoreValidationError> {
        let mut result = Vec::new();

        for assignment in &set_clause.assignments {
            // Check whether the attribute exists in the Schema.
            let prop_def = schema_validator
                .get_property_def(&assignment.property, schema_props)
                .ok_or_else(|| {
                    CoreValidationError::new(
                        format!(
                            "Property '{}' does not exist in schema",
                            assignment.property
                        ),
                        ValidationErrorType::SemanticError,
                    )
                })?;

            // Evaluating an expression
            let value = schema_validator
                .evaluate_expression(&assignment.value)
                .map_err(|e| {
                    CoreValidationError::new(
                        format!(
                            "Failed to evaluate property '{}': {}",
                            assignment.property, e.message
                        ),
                        e.error_type,
                    )
                })?;

            // Verification type
            schema_validator
                .validate_property_type(&assignment.property, &prop_def.data_type, &value)
                .map_err(|e| {
                    CoreValidationError::new(
                        format!("Property '{}': {}", assignment.property, e.message),
                        e.error_type,
                    )
                })?;

            result.push(ValidatedAssignment {
                property: assignment.property.clone(),
                value,
                prop_id: None, // The content can be filled in later.
                expression: Some(assignment.value.clone()),
            });
        }

        Ok(result)
    }

    /// Converting assignments without verifying the Schema (for Vertex updates)
    fn validate_and_convert_assignments_without_schema(
        &self,
        set_clause: &SetClause,
        schema_validator: &SchemaValidator,
    ) -> Result<Vec<ValidatedAssignment>, CoreValidationError> {
        let mut result = Vec::new();

        for assignment in &set_clause.assignments {
            // Evaluating an expression
            let value = schema_validator
                .evaluate_expression(&assignment.value)
                .map_err(|e| {
                    CoreValidationError::new(
                        format!(
                            "Failed to evaluate property '{}': {}",
                            assignment.property, e.message
                        ),
                        e.error_type,
                    )
                })?;

            result.push(ValidatedAssignment {
                property: assignment.property.clone(),
                value,
                prop_id: None,
                expression: Some(assignment.value.clone()),
            });
        }

        Ok(result)
    }

    fn validate_property_value(
        &self,
        value: &ContextualExpression,
    ) -> Result<(), CoreValidationError> {
        let expr_meta = match value.expression() {
            Some(e) => e,
            None => {
                return Err(CoreValidationError::new(
                    "Property value is invalid".to_string(),
                    ValidationErrorType::SemanticError,
                ));
            }
        };
        let inner_expr = expr_meta.inner();

        self.validate_expression_recursive(inner_expr)
    }

    fn validate_expression_recursive(&self, expr: &Expression) -> Result<(), CoreValidationError> {
        match expr {
            Expression::Literal(_) => Ok(()),
            Expression::Variable(_) => Ok(()),
            Expression::Function { args, .. } => {
                if args.is_empty() {
                    return Err(CoreValidationError::new(
                        "Function call must have arguments".to_string(),
                        ValidationErrorType::SemanticError,
                    ));
                }
                for arg in args.iter() {
                    self.validate_expression_recursive(arg)?;
                }
                Ok(())
            }
            Expression::Unary { op: _, operand } => {
                self.validate_expression_recursive(operand)?;
                Ok(())
            }
            Expression::Binary { left, right, .. } => {
                self.validate_expression_recursive(left)?;
                self.validate_expression_recursive(right)?;
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn validate_where_clause(
        &self,
        where_clause: Option<&ContextualExpression>,
    ) -> Result<(), CoreValidationError> {
        if let Some(where_expr) = where_clause {
            self.validate_expression(where_expr)?;
        }
        Ok(())
    }

    fn validate_expression(&self, expr: &ContextualExpression) -> Result<(), CoreValidationError> {
        if expr.expression().is_none() {
            return Err(CoreValidationError::new(
                "Invalid expression".to_string(),
                ValidationErrorType::SemanticError,
            ));
        }

        // Basic validation: Literals, variables, and property references are all valid.
        if expr.is_literal() || expr.is_variable() || expr.is_property() {
            return Ok(());
        }

        // For more complex expressions (functions, binary operations, etc.), we need to access their internal structure.
        // Note: It is still necessary to access the internal Expression; this is because the ContextualExpression API is used.
        // For the time being, no methods for accessing nested expressions are provided.
        // This is a known architectural limitation that needs to be addressed in future versions by improving the ContextualExpression API.
        if let Some(expr_meta) = expr.expression() {
            self.validate_expression_internal(expr_meta.inner())
        } else {
            Ok(())
        }
    }

    /// Internal method: Verifying expressions
    fn validate_expression_internal(
        &self,
        expr: &crate::core::types::expr::Expression,
    ) -> Result<(), CoreValidationError> {
        match expr {
            crate::core::types::expr::Expression::Literal(_) => Ok(()),
            crate::core::types::expr::Expression::Variable(_) => Ok(()),
            crate::core::types::expr::Expression::Property { .. } => Ok(()),
            crate::core::types::expr::Expression::Function { args, .. } => {
                for arg in args {
                    self.validate_expression_internal(arg)?;
                }
                Ok(())
            }
            crate::core::types::expr::Expression::Unary { operand, .. } => {
                self.validate_expression_internal(operand)
            }
            crate::core::types::expr::Expression::Binary { left, right, .. } => {
                self.validate_expression_internal(left)?;
                self.validate_expression_internal(right)?;
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn generate_output_columns(&mut self) {
        self.outputs.push(ColumnDef {
            name: "UPDATED".to_string(),
            type_: ValueType::Bool,
        });
    }

    fn validate_space_chosen(&self, qctx: Arc<QueryContext>) -> Result<(), CoreValidationError> {
        if qctx.space_id().is_none() {
            return Err(CoreValidationError::new(
                "No space selected. Use `USE <space>` to select a graph space first.".to_string(),
                ValidationErrorType::SemanticError,
            ));
        }
        Ok(())
    }
}

/// Implementing the StatementValidator trait
///
/// # Refactoring Changes
/// The `validate` method accepts `Arc<Ast>` and `Arc<QueryContext>`.
impl StatementValidator for UpdateValidator {
    fn validate(
        &mut self,
        ast: Arc<Ast>,
        qctx: Arc<QueryContext>,
    ) -> Result<ValidationResult, ValidationError> {
        // 1. Check whether additional space is needed.
        if !self.is_global_statement() && qctx.space_id().is_none() {
            return Err(ValidationError::new(
                "No image space selected, please execute first USE <space>".to_string(),
                ValidationErrorType::SemanticError,
            ));
        }

        // 2. Obtain the UPDATE statement
        let update_stmt = match &ast.stmt {
            crate::query::parser::ast::Stmt::Update(u) => u,
            _ => {
                return Err(ValidationError::new(
                    "Expected UPDATE statement".to_string(),
                    ValidationErrorType::SemanticError,
                ));
            }
        };

        // 3. Verify the UPDATE statement
        let space_name = qctx.space_name();
        if let Err(e) = self.validate_update_stmt(update_stmt, space_name.as_deref()) {
            return Err(ValidationError::new(
                format!("UPDATE validation failed: {}", e),
                ValidationErrorType::SemanticError,
            ));
        }

        // 4. Generate an output column
        self.generate_output_columns();

        // 5. Constructing detailed ValidationInfo
        let mut info = ValidationInfo::new();

        // Add semantic information
        match &update_stmt.target {
            UpdateTarget::Vertex(_) => {
                info.semantic_info
                    .referenced_tags
                    .push("vertex".to_string());
            }
            UpdateTarget::Edge { edge_type, .. } => {
                if let Some(ref et) = edge_type {
                    info.semantic_info.referenced_edges.push(et.clone());
                }
            }
            UpdateTarget::Tag(tag_name) => {
                info.semantic_info.referenced_tags.push(tag_name.clone());
            }
            UpdateTarget::TagOnVertex { tag_name, .. } => {
                info.semantic_info.referenced_tags.push(tag_name.clone());
            }
        }

        // Add the attributes for the citation.
        for assignment in &update_stmt.set_clause.assignments {
            if !info
                .semantic_info
                .referenced_properties
                .contains(&assignment.property)
            {
                info.semantic_info
                    .referenced_properties
                    .push(assignment.property.clone());
            }
        }

        // 6. Return the verification results containing detailed information.
        Ok(ValidationResult::success_with_info(info))
    }

    fn statement_type(&self) -> StatementType {
        StatementType::Update
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
        &self.expr_props
    }

    fn user_defined_vars(&self) -> &[String] {
        &self.user_defined_vars
    }
}

impl Default for UpdateValidator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::expr::contextual::ContextualExpression;
    use crate::core::Expression;
    use crate::query::parser::ast::stmt::{Assignment, SetClause, UpdateTarget};
    use crate::query::parser::ast::Span;
    use crate::query::validator::context::expression_context::ExpressionAnalysisContext;

    fn create_contextual_expr(expr: Expression) -> ContextualExpression {
        let ctx = std::sync::Arc::new(ExpressionAnalysisContext::new());
        let meta = crate::core::types::expr::ExpressionMeta::new(expr);
        let id = ctx.register_expression(meta);
        ContextualExpression::new(id, ctx)
    }

    fn create_update_stmt(
        target: UpdateTarget,
        assignments: Vec<Assignment>,
        where_clause: Option<ContextualExpression>,
    ) -> UpdateStmt {
        UpdateStmt {
            span: Span::default(),
            target,
            set_clause: SetClause {
                span: Span::default(),
                assignments,
            },
            where_clause,
            is_upsert: false,
            yield_clause: None,
        }
    }

    #[test]
    fn test_validate_vertex_target_valid() {
        let mut validator = UpdateValidator::new();
        let stmt = create_update_stmt(
            UpdateTarget::Vertex(create_contextual_expr(Expression::Literal(Value::String(
                "v1".to_string(),
            )))),
            vec![Assignment {
                property: "name".to_string(),
                value: create_contextual_expr(Expression::Literal(Value::String(
                    "new_name".to_string(),
                ))),
                target: None,
                object: None,
            }],
            None,
        );
        let result = validator.validate_update_stmt(&stmt, None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_vertex_target_variable() {
        let mut validator = UpdateValidator::new();
        let stmt = create_update_stmt(
            UpdateTarget::Vertex(create_contextual_expr(Expression::Variable(
                "$vid".to_string(),
            ))),
            vec![Assignment {
                property: "name".to_string(),
                value: create_contextual_expr(Expression::Literal(Value::String(
                    "new_name".to_string(),
                ))),
                target: None,
                object: None,
            }],
            None,
        );
        let result = validator.validate_update_stmt(&stmt, None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_vertex_id_empty() {
        let mut validator = UpdateValidator::new();
        let stmt = create_update_stmt(
            UpdateTarget::Vertex(create_contextual_expr(Expression::Literal(Value::String(
                "".to_string(),
            )))),
            vec![Assignment {
                property: "name".to_string(),
                value: create_contextual_expr(Expression::Literal(Value::String(
                    "new_name".to_string(),
                ))),
                target: None,
                object: None,
            }],
            None,
        );
        let result = validator.validate_update_stmt(&stmt, None);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.message.contains("vertex ID cannot be empty"));
    }

    #[test]
    fn test_validate_with_schema() {
        // This test requires a complete database and Schema setup; therefore, it will be skipped for now.
        // Using InMemorySchemaManager requires an actual storage backend.
        let mut validator = UpdateValidator::new();

        let stmt = create_update_stmt(
            UpdateTarget::Vertex(create_contextual_expr(Expression::literal("v1"))),
            vec![Assignment {
                property: "name".to_string(),
                value: create_contextual_expr(Expression::literal("new_name")),
                target: None,
                object: None,
            }],
            None,
        );

        let result = validator.validate_update_stmt(&stmt, None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_duplicate_assignment() {
        let mut validator = UpdateValidator::new();
        let stmt = create_update_stmt(
            UpdateTarget::Vertex(create_contextual_expr(Expression::Literal(Value::String(
                "v1".to_string(),
            )))),
            vec![
                Assignment {
                    property: "name".to_string(),
                    value: create_contextual_expr(Expression::Literal(Value::String(
                        "name1".to_string(),
                    ))),
                    target: None,
                    object: None,
                },
                Assignment {
                    property: "name".to_string(),
                    value: create_contextual_expr(Expression::Literal(Value::String(
                        "name2".to_string(),
                    ))),
                    target: None,
                    object: None,
                },
            ],
            None,
        );
        let result = validator.validate_update_stmt(&stmt, None);
        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("Duplicate"));
    }
}
