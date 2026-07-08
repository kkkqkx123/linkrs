//! DELETE Statement Validator – New Version
//! Corresponding to the functionality of NebulaGraph’s DeleteValidator
//! Verify the semantic correctness of the DELETE statement.
//!
//! This document has been restructured in accordance with the new trait + enumeration validator framework.
//! The StatementValidator trait has been implemented to unify the interface.
//! 2. All functions are retained.
//! Verify Lifecycle Management
//! Management of input/output columns
//! Expression property tracing
//! User-defined variable management
//! Permission check
//! Execution plan generation
//! 3. The lifecycle parameters have been removed, and SchemaManager is now managed using Arc.
//! 4. Use AstContext to manage the context in a unified manner.

use std::sync::Arc;

use crate::core::metadata::SchemaManager;
use crate::core::types::expr::contextual::ContextualExpression;
use crate::core::Expression;
use crate::core::Value;
use crate::query::parser::ast::stmt::{Ast, DeleteStmt, DeleteTarget};
use crate::query::validator::error::{ValidationError, ValidationErrorType};
use crate::query::validator::structs::validation_info::ValidationInfo;
use crate::query::validator::validator_trait::{
    ColumnDef, ExpressionProps, StatementType, StatementValidator, ValidationResult, ValueType,
};
use crate::query::QueryContext;

/// Verified deletion information
#[derive(Debug, Clone)]
pub struct ValidatedDelete {
    pub space_id: u64,
    pub target_type: DeleteTargetType,
    pub with_edge: bool,
    pub where_clause: Option<ContextualExpression>,
}

/// Delete the target type.
#[derive(Debug, Clone)]
pub enum DeleteTargetType {
    Vertices(Vec<Value>),
    Edges {
        edge_type: Option<String>,
        edge_type_id: Option<i32>,
        edges: Vec<EdgeKey>,
    },
    Tags {
        tag_names: Vec<String>,
        tag_ids: Vec<i32>,
        vertex_ids: Vec<Value>,
    },
    Index(String),
}

/// The unique identifier for the edge
#[derive(Debug, Clone)]
pub struct EdgeKey {
    pub src: Value,
    pub dst: Value,
    pub rank: i64,
}

/// DELETE Statement Validator – New System Implementation
///
/// Functionality completeness assurance:
/// 1. Complete validation lifecycle
/// 2. Management of input/output columns
/// 3. Tracking of expression property values
/// 4. Management of user-defined variables
/// 5. Permission checking (scalable)
/// 6. Generation of execution plans (scalable)
#[derive(Debug)]
pub struct DeleteValidator {
    // Schema management
    schema_manager: Option<Arc<SchemaManager>>,
    // Input column definition
    inputs: Vec<ColumnDef>,
    // Column definition
    outputs: Vec<ColumnDef>,
    // Expression properties
    expr_props: ExpressionProps,
    // User-defined variables
    user_defined_vars: Vec<String>,
    // Cache validation results
    validated_result: Option<ValidatedDelete>,
}

impl DeleteValidator {
    pub fn new() -> Self {
        Self {
            schema_manager: None,
            inputs: Vec::new(),
            outputs: Vec::new(),
            expr_props: ExpressionProps::default(),
            user_defined_vars: Vec::new(),
            validated_result: None,
        }
    }

    pub fn with_schema_manager(mut self, schema_manager: Arc<SchemaManager>) -> Self {
        self.schema_manager = Some(schema_manager);
        self
    }

    pub fn set_schema_manager(&mut self, schema_manager: Arc<SchemaManager>) {
        self.schema_manager = Some(schema_manager);
    }

    /// Obtain the verification results.
    pub fn validated_result(&self) -> Option<&ValidatedDelete> {
        self.validated_result.as_ref()
    }

    /// Basic validation (not dependent on a Schema)
    fn validate_delete(
        &self,
        stmt: &DeleteStmt,
        space_name: Option<&str>,
    ) -> Result<(), ValidationError> {
        self.validate_target(&stmt.target, space_name)?;
        self.validate_where_clause(stmt.where_clause.as_ref())?;
        Ok(())
    }

    /// Verify the deletion target.
    fn validate_target(
        &self,
        target: &DeleteTarget,
        space_name: Option<&str>,
    ) -> Result<(), ValidationError> {
        match target {
            DeleteTarget::Vertices(vids) => {
                if vids.is_empty() {
                    return Err(ValidationError::new(
                        "DELETE VERTICES must specify at least one vertex".to_string(),
                        ValidationErrorType::SemanticError,
                    ));
                }
                for (idx, vid) in vids.iter().enumerate() {
                    self.validate_vertex_id(vid, idx + 1, space_name)?;
                }
            }
            DeleteTarget::Edges { edge_type, edges } => {
                for (idx, (src, dst, rank)) in edges.iter().enumerate() {
                    self.validate_vertex_id(src, idx * 2, space_name)?;
                    self.validate_vertex_id(dst, idx * 2 + 1, space_name)?;
                    if let Some(rank_expr) = rank {
                        self.validate_rank(rank_expr)?;
                    }
                }
                if let Some(et) = edge_type {
                    if et.is_empty() {
                        return Err(ValidationError::new(
                            "Edge type name cannot be empty".to_string(),
                            ValidationErrorType::SemanticError,
                        ));
                    }
                }
            }
            DeleteTarget::Tags {
                tag_names,
                vertex_ids,
                is_all_tags,
            } => {
                // If you do not want to delete all tags, you need to specify at least one tag name.
                if !is_all_tags && tag_names.is_empty() {
                    return Err(ValidationError::new(
                        "DELETE TAG must specify at least one tag name or use *".to_string(),
                        ValidationErrorType::SemanticError,
                    ));
                }
                for tag_name in tag_names {
                    if tag_name.is_empty() {
                        return Err(ValidationError::new(
                            "Tag name cannot be empty".to_string(),
                            ValidationErrorType::SemanticError,
                        ));
                    }
                }
                if vertex_ids.is_empty() {
                    return Err(ValidationError::new(
                        "DELETE TAG must specify at least one vertex ID".to_string(),
                        ValidationErrorType::SemanticError,
                    ));
                }
                for (idx, vid) in vertex_ids.iter().enumerate() {
                    self.validate_vertex_id(vid, idx + 1, space_name)?;
                }
            }
            DeleteTarget::Index(index_name) => {
                if index_name.is_empty() {
                    return Err(ValidationError::new(
                        "Index name cannot be empty".to_string(),
                        ValidationErrorType::SemanticError,
                    ));
                }
            }
        }
        Ok(())
    }

    /// Verify the vertex ID
    /// Using the unified validation method provided by SchemaValidator
    fn validate_vertex_id(
        &self,
        expr: &ContextualExpression,
        idx: usize,
        space_name: Option<&str>,
    ) -> Result<(), ValidationError> {
        let role = &format!("vertex {}", idx);

        // Get vid_type from schema_manager if available, otherwise default to String
        let vid_type = if let (Some(ref schema_manager), Some(space_name)) =
            (&self.schema_manager, space_name)
        {
            match schema_manager.get_space(space_name) {
                Ok(Some(space_info)) => space_info.vid_type,
                _ => crate::core::types::DataType::String,
            }
        } else {
            crate::core::types::DataType::String
        };

        if let Some(ref schema_manager) = self.schema_manager {
            if expr.get_expression().is_some() {
                let schema_validator =
                    crate::query::validator::SchemaValidator::new(schema_manager.clone());
                let ctx_expr = crate::core::types::ContextualExpression::new(
                    expr.id().clone(),
                    expr.context().clone(),
                );
                schema_validator
                    .validate_vid_expr(&ctx_expr, &vid_type, role)
                    .map_err(|e| ValidationError::new(e.message, e.error_type))
            } else {
                Err(ValidationError::new(
                    "Vertex ID expression is invalid".to_string(),
                    ValidationErrorType::SemanticError,
                ))
            }
        } else {
            Self::basic_validate_vertex_id(expr, idx)
        }
    }

    /// Basic vertex ID verification (when no SchemaManager is available)
    fn basic_validate_vertex_id(
        expr: &ContextualExpression,
        idx: usize,
    ) -> Result<(), ValidationError> {
        if let Some(e) = expr.get_expression() {
            match e {
                crate::core::types::expr::Expression::Literal(Value::String(s)) => {
                    if s.is_empty() {
                        return Err(ValidationError::new(
                            format!("Vertex ID at position {} cannot be empty", idx),
                            ValidationErrorType::SemanticError,
                        ));
                    }
                    Ok(())
                }
                crate::core::types::expr::Expression::Literal(Value::Int(_)) => Ok(()),
                crate::core::types::expr::Expression::Variable(_) => Ok(()),
                _ => Err(ValidationError::new(
                    format!(
                        "Vertex ID at position {} must be a string constant or variable",
                        idx
                    ),
                    ValidationErrorType::SemanticError,
                )),
            }
        } else {
            Err(ValidationError::new(
                "Vertex ID expression is invalid".to_string(),
                ValidationErrorType::SemanticError,
            ))
        }
    }

    /// Verify the rank.
    fn validate_rank(&self, expr: &ContextualExpression) -> Result<(), ValidationError> {
        if let Some(e) = expr.get_expression() {
            match e {
                crate::core::types::expr::Expression::Literal(Value::Int(_)) => Ok(()),
                crate::core::types::expr::Expression::Variable(_) => Ok(()),
                _ => Err(ValidationError::new(
                    "Rank must be an integer constant or variable".to_string(),
                    ValidationErrorType::SemanticError,
                )),
            }
        } else {
            Err(ValidationError::new(
                "Rank expression is invalid".to_string(),
                ValidationErrorType::SemanticError,
            ))
        }
    }

    /// Verify the WHERE clause
    fn validate_where_clause(
        &self,
        where_clause: Option<&ContextualExpression>,
    ) -> Result<(), ValidationError> {
        if let Some(where_expr) = where_clause {
            self.validate_expression(where_expr)?;
        }
        Ok(())
    }

    /// Verify the expression
    fn validate_expression(&self, expr: &ContextualExpression) -> Result<(), ValidationError> {
        let expr_meta = match expr.get_expression() {
            Some(e) => e,
            None => {
                return Err(ValidationError::new(
                    "Invalid expression".to_string(),
                    ValidationErrorType::SemanticError,
                ))
            }
        };
        self.validate_expression_internal(&expr_meta)
    }

    /// Internal method: Verifying expressions
    fn validate_expression_internal(
        &self,
        expr: &crate::core::types::expr::Expression,
    ) -> Result<(), ValidationError> {
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

    /// Verify and convert the target content (using Schema).
    fn validate_and_convert_target(
        &self,
        target: &DeleteTarget,
        space_id: u64,
    ) -> Result<DeleteTargetType, ValidationError> {
        match target {
            DeleteTarget::Vertices(vids) => {
                let mut validated_vids = Vec::new();
                for (idx, vid_expr) in vids.iter().enumerate() {
                    let vid = self.evaluate_vid(vid_expr, idx + 1)?;
                    validated_vids.push(vid);
                }
                Ok(DeleteTargetType::Vertices(validated_vids))
            }
            DeleteTarget::Edges { edge_type, edges } => {
                // Obtain the EdgeType ID
                let edge_type_id = if let Some(et) = edge_type {
                    self.get_edge_type_id(et, space_id)?
                } else {
                    None
                };

                let mut validated_edges = Vec::new();
                for (idx, (src, dst, rank)) in edges.iter().enumerate() {
                    let src_vid = self.evaluate_vid(src, idx * 2)?;
                    let dst_vid = self.evaluate_vid(dst, idx * 2 + 1)?;
                    let rank_val = if let Some(rank_expr) = rank {
                        self.evaluate_rank(rank_expr)?
                    } else {
                        0
                    };
                    validated_edges.push(EdgeKey {
                        src: src_vid,
                        dst: dst_vid,
                        rank: rank_val,
                    });
                }

                Ok(DeleteTargetType::Edges {
                    edge_type: edge_type.clone(),
                    edge_type_id,
                    edges: validated_edges,
                })
            }
            DeleteTarget::Tags {
                tag_names,
                vertex_ids,
                is_all_tags,
            } => {
                // Obtaining Tag IDs
                let mut tag_ids = Vec::new();
                let final_tag_names = if *is_all_tags {
                    // If all tags are to be deleted, the execution layer will handle the logic for retrieving all the tags.
                    vec![]
                } else {
                    for tag_name in tag_names {
                        let tag_id = self.get_tag_id(tag_name, space_id)?;
                        if let Some(id) = tag_id {
                            tag_ids.push(id);
                        }
                    }
                    tag_names.clone()
                };

                let mut validated_vids = Vec::new();
                for (idx, vid_expr) in vertex_ids.iter().enumerate() {
                    let vid = self.evaluate_vid(vid_expr, idx + 1)?;
                    validated_vids.push(vid);
                }

                Ok(DeleteTargetType::Tags {
                    tag_names: final_tag_names,
                    tag_ids,
                    vertex_ids: validated_vids,
                })
            }
            DeleteTarget::Index(index_name) => Ok(DeleteTargetType::Index(index_name.clone())),
        }
    }

    /// Evaluating VID expressions
    fn evaluate_vid(
        &self,
        vid_expr: &ContextualExpression,
        idx: usize,
    ) -> Result<Value, ValidationError> {
        let inner_expr = match vid_expr.get_expression() {
            Some(e) => e,
            None => {
                return Err(ValidationError::new(
                    format!("Failed to evaluate vertex ID at position {}", idx),
                    ValidationErrorType::SemanticError,
                ));
            }
        };

        match inner_expr {
            Expression::Literal(v) => Ok(v.clone()),
            Expression::Variable(name) => Ok(Value::String(format!("${}", name))),
            _ => Err(ValidationError::new(
                format!("Failed to evaluate vertex ID at position {}", idx),
                ValidationErrorType::SemanticError,
            )),
        }
    }

    /// Evaluating the rank expression
    fn evaluate_rank(&self, expr: &ContextualExpression) -> Result<i64, ValidationError> {
        let inner_expr = match expr.get_expression() {
            Some(e) => e,
            None => {
                return Err(ValidationError::new(
                    "Failed to evaluate rank".to_string(),
                    ValidationErrorType::SemanticError,
                ));
            }
        };

        match inner_expr {
            Expression::Literal(Value::BigInt(i)) => Ok(i),
            Expression::Variable(_) => Ok(0),
            _ => Err(ValidationError::new(
                "Rank must be an integer".to_string(),
                ValidationErrorType::TypeMismatch,
            )),
        }
    }

    /// Obtain the EdgeType ID
    fn get_edge_type_id(
        &self,
        edge_type_name: &str,
        _space_id: u64,
    ) -> Result<Option<i32>, ValidationError> {
        // If there is a schema_manager, it is possible to retrieve the actual edge_type_id.
        // The simplification here is to return `None`, allowing the execution layer to handle the task accordingly.
        let _ = edge_type_name;
        Ok(None)
    }

    /// Obtain the Tag ID
    fn get_tag_id(&self, tag_name: &str, _space_id: u64) -> Result<Option<i32>, ValidationError> {
        // If there is a schema_manager, it is possible to retrieve the actual tag_id.
        // The simplification here is to return `None`, allowing the execution layer to handle the task accordingly.
        let _ = tag_name;
        Ok(None)
    }
}

impl Default for DeleteValidator {
    fn default() -> Self {
        Self::new()
    }
}

/// Implementing the StatementValidator trait
///
/// # Refactoring Changes
/// The `validate` method accepts `Arc<Ast>` and `Arc<QueryContext>` as arguments.
impl StatementValidator for DeleteValidator {
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

        // 2. Obtain the DELETE statement
        let delete_stmt = match &ast.stmt {
            crate::query::parser::ast::Stmt::Delete(delete_stmt) => delete_stmt,
            _ => {
                return Err(ValidationError::new(
                    "Expected DELETE statement".to_string(),
                    ValidationErrorType::SemanticError,
                ));
            }
        };

        // 3. Perform basic validation.
        let space_name = qctx.space_name();
        self.validate_delete(delete_stmt, space_name.as_deref())?;

        // 4. Obtain the space_id
        let space_id = qctx.space_id().unwrap_or(0);

        // 5. Verify and convert the target content.
        let target_type = self.validate_and_convert_target(&delete_stmt.target, space_id)?;

        // 6. Create the verification results
        let validated = ValidatedDelete {
            space_id,
            target_type,
            with_edge: delete_stmt.with_edge,
            where_clause: delete_stmt.where_clause.clone(),
        };

        self.validated_result = Some(validated.clone());

        // 7. Set the output columns
        self.outputs.clear();
        self.outputs.push(ColumnDef {
            name: "DELETED".to_string(),
            type_: ValueType::Bool,
        });

        // 8. Constructing detailed ValidationInfo
        let mut info = ValidationInfo::new();

        // Add semantic information
        match &delete_stmt.target {
            DeleteTarget::Vertices(_) => {
                info.semantic_info
                    .referenced_tags
                    .push("vertex".to_string());
            }
            DeleteTarget::Edges { edge_type, .. } => {
                if let Some(ref et) = edge_type {
                    info.semantic_info.referenced_edges.push(et.clone());
                }
            }
            DeleteTarget::Tags { tag_names, .. } => {
                for tag_name in tag_names {
                    if !info.semantic_info.referenced_tags.contains(tag_name) {
                        info.semantic_info.referenced_tags.push(tag_name.clone());
                    }
                }
            }
            DeleteTarget::Index(_) => {
                // The deletion of an index does not require the addition of any semantic information.
            }
        }

        // 9. Return the verification results containing detailed information.
        Ok(ValidationResult::success_with_info(info))
    }

    fn statement_type(&self) -> StatementType {
        StatementType::Delete
    }

    fn inputs(&self) -> &[ColumnDef] {
        &self.inputs
    }

    fn outputs(&self) -> &[ColumnDef] {
        &self.outputs
    }

    fn is_global_statement(&self) -> bool {
        // The `DELETE` command is not a global statement; it is necessary to select a specific space (or database table, etc.) in advance before executing the command.
        false
    }

    fn expression_props(&self) -> &ExpressionProps {
        &self.expr_props
    }

    fn user_defined_vars(&self) -> &[String] {
        &self.user_defined_vars
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::expr::contextual::ContextualExpression;
    use crate::core::Expression;
    use crate::query::parser::ast::stmt::{DeleteStmt, DeleteTarget};
    use crate::query::parser::ast::Span;
    use crate::query::validator::context::expression_context::ExpressionAnalysisContext;

    fn create_contextual_expr(expr: Expression) -> ContextualExpression {
        let ctx = std::sync::Arc::new(ExpressionAnalysisContext::new());
        let meta = crate::core::types::expr::ExpressionMeta::new(expr);
        let id = ctx.register_expression(meta);
        ContextualExpression::new(id, ctx)
    }

    fn create_delete_stmt(
        target: DeleteTarget,
        where_clause: Option<ContextualExpression>,
    ) -> DeleteStmt {
        DeleteStmt {
            span: Span::default(),
            target,
            where_clause,
            with_edge: false,
        }
    }

    #[test]
    fn test_validate_vertices_empty_list() {
        let validator = DeleteValidator::new();
        let stmt = create_delete_stmt(DeleteTarget::Vertices(vec![]), None);
        let result = validator.validate_delete(&stmt, None);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(
            err.message,
            "DELETE VERTICES must specify at least one vertex"
        );
    }

    #[test]
    fn test_validate_vertices_valid() {
        let validator = DeleteValidator::new();
        let stmt = create_delete_stmt(
            DeleteTarget::Vertices(vec![
                create_contextual_expr(Expression::Literal(Value::String("v1".to_string()))),
                create_contextual_expr(Expression::Literal(Value::String("v2".to_string()))),
            ]),
            None,
        );
        let result = validator.validate_delete(&stmt, None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_vertices_with_variable() {
        let validator = DeleteValidator::new();
        let stmt = create_delete_stmt(
            DeleteTarget::Vertices(vec![create_contextual_expr(Expression::Variable(
                "vids".to_string(),
            ))]),
            None,
        );
        let result = validator.validate_delete(&stmt, None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_vertex_id_empty() {
        let validator = DeleteValidator::new();
        let stmt = create_delete_stmt(
            DeleteTarget::Vertices(vec![
                create_contextual_expr(Expression::Literal(Value::String("v1".to_string()))),
                create_contextual_expr(Expression::Literal(Value::String("".to_string()))),
            ]),
            None,
        );
        let result = validator.validate_delete(&stmt, None);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.message.contains("cannot be empty"));
    }

    #[test]
    fn test_validate_edges_valid() {
        let validator = DeleteValidator::new();
        let stmt = create_delete_stmt(
            DeleteTarget::Edges {
                edge_type: Some("friend".to_string()),
                edges: vec![(
                    create_contextual_expr(Expression::Literal(Value::String("v1".to_string()))),
                    create_contextual_expr(Expression::Literal(Value::String("v2".to_string()))),
                    None,
                )],
            },
            None,
        );
        let result = validator.validate_delete(&stmt, None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_edges_with_rank() {
        let validator = DeleteValidator::new();
        let stmt = create_delete_stmt(
            DeleteTarget::Edges {
                edge_type: Some("friend".to_string()),
                edges: vec![(
                    create_contextual_expr(Expression::Literal(Value::String("v1".to_string()))),
                    create_contextual_expr(Expression::Literal(Value::String("v2".to_string()))),
                    Some(create_contextual_expr(Expression::Literal(Value::Int(0)))),
                )],
            },
            None,
        );
        let result = validator.validate_delete(&stmt, None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_tags_empty_list() {
        let validator = DeleteValidator::new();
        let stmt = create_delete_stmt(
            DeleteTarget::Tags {
                tag_names: vec![],
                vertex_ids: vec![],
                is_all_tags: false,
            },
            None,
        );
        let result = validator.validate_delete(&stmt, None);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_tags_valid() {
        let validator = DeleteValidator::new();
        let stmt = create_delete_stmt(
            DeleteTarget::Tags {
                tag_names: vec!["person".to_string()],
                vertex_ids: vec![create_contextual_expr(Expression::Literal(Value::String(
                    "v1".to_string(),
                )))],
                is_all_tags: false,
            },
            None,
        );
        let result = validator.validate_delete(&stmt, None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_index_empty() {
        let validator = DeleteValidator::new();
        let stmt = create_delete_stmt(DeleteTarget::Index("".to_string()), None);
        let result = validator.validate_delete(&stmt, None);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.message, "Index name cannot be empty");
    }

    #[test]
    fn test_validate_index_valid() {
        let validator = DeleteValidator::new();
        let stmt = create_delete_stmt(DeleteTarget::Index("idx_person".to_string()), None);
        let result = validator.validate_delete(&stmt, None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_statement_validator_trait() {
        let validator = DeleteValidator::new();

        // Testing the `statement_type`
        assert_eq!(validator.statement_type(), StatementType::Delete);

        // Testing inputs/outputs
        assert!(validator.inputs().is_empty());
        assert!(validator.outputs().is_empty());

        // Testing user_defined_vars
        assert!(validator.user_defined_vars().is_empty());
    }
}
