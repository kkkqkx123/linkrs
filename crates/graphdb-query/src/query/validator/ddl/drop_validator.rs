//! Drop Statement Validator
//! Corresponding to the functionality of the Drop-related validators in NebulaGraph
//! Verify statements such as DROP SPACE, DROP TAG, DROP EDGE, and DROP INDEX.
//!
//! Design principles:
//! The StatementValidator trait has been implemented to unify the interface.
//! 2. The `DROP SPACE` statement is a global statement; other `DROP` statements require the selection of a specific space to be deleted.
//! 3. Verify whether the target object exists (based on the if_exists flag).

use crate::query::parser::ast::stmt::{Ast, DropStmt, DropTarget};
use crate::query::validator::error::{ValidationError, ValidationErrorType};
use crate::query::validator::structs::validation_info::ValidationInfo;
use crate::query::validator::validator_trait::{
    ColumnDef, ExpressionProps, StatementType, StatementValidator, ValidationResult, ValueType,
};
use crate::query::QueryContext;
use std::sync::Arc;

/// Verified Drop information
#[derive(Debug, Clone)]
pub struct ValidatedDrop {
    pub target_type: DropTargetType,
    pub target_name: String,
    pub space_name: Option<String>,
    pub if_exists: bool,
}

/// Drop target type
#[derive(Debug, Clone)]
pub enum DropTargetType {
    Space,
    Tag,
    Edge,
    TagIndex,
    EdgeIndex,
}

/// Drop Statement Validator
#[derive(Debug)]
pub struct DropValidator {
    target_type: DropTargetType,
    target_name: String,
    space_name: Option<String>,
    if_exists: bool,
    inputs: Vec<ColumnDef>,
    outputs: Vec<ColumnDef>,
    expr_props: ExpressionProps,
    user_defined_vars: Vec<String>,
}

impl DropValidator {
    pub fn new() -> Self {
        Self {
            target_type: DropTargetType::Space,
            target_name: String::new(),
            space_name: None,
            if_exists: false,
            inputs: Vec::new(),
            outputs: vec![ColumnDef {
                name: "Result".to_string(),
                type_: ValueType::String,
            }],
            expr_props: ExpressionProps::default(),
            user_defined_vars: Vec::new(),
        }
    }

    fn validate_impl(&mut self, stmt: &DropStmt) -> Result<(), ValidationError> {
        self.if_exists = stmt.if_exists;

        match &stmt.target {
            DropTarget::Space(name) => {
                self.target_type = DropTargetType::Space;
                self.target_name = name.clone();
                self.space_name = Some(name.clone());

                // Verify that the space name is not empty.
                if self.target_name.is_empty() {
                    return Err(ValidationError::new(
                        "Space name cannot be empty".to_string(),
                        ValidationErrorType::SemanticError,
                    ));
                }
            }
            DropTarget::Tags(tags) => {
                if tags.is_empty() {
                    return Err(ValidationError::new(
                        "At least one tag must be specified".to_string(),
                        ValidationErrorType::SemanticError,
                    ));
                }
                // When multiple tags are deleted, only the first one is processed (simplified implementation).
                self.target_type = DropTargetType::Tag;
                self.target_name = tags[0].clone();
            }
            DropTarget::Edges(edges) => {
                if edges.is_empty() {
                    return Err(ValidationError::new(
                        "At least one edge type must be specified".to_string(),
                        ValidationErrorType::SemanticError,
                    ));
                }
                // When multiple edges are deleted, only the first one is processed (simplified implementation).
                self.target_type = DropTargetType::Edge;
                self.target_name = edges[0].clone();
            }
            DropTarget::TagIndex {
                space_name,
                index_name,
            } => {
                self.target_type = DropTargetType::TagIndex;
                self.space_name = if space_name.is_empty() {
                    None
                } else {
                    Some(space_name.clone())
                };
                self.target_name = index_name.clone();

                if index_name.is_empty() {
                    return Err(ValidationError::new(
                        "Index name cannot be empty".to_string(),
                        ValidationErrorType::SemanticError,
                    ));
                }
            }
            DropTarget::EdgeIndex {
                space_name,
                index_name,
            } => {
                self.target_type = DropTargetType::EdgeIndex;
                self.space_name = if space_name.is_empty() {
                    None
                } else {
                    Some(space_name.clone())
                };
                self.target_name = index_name.clone();

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

    /// Obtain the target type
    pub fn target_type(&self) -> &DropTargetType {
        &self.target_type
    }

    /// Obtain the target name.
    pub fn target_name(&self) -> &str {
        &self.target_name
    }

    /// Obtain the space name
    pub fn space_name(&self) -> Option<&String> {
        self.space_name.as_ref()
    }

    /// Obtain the if_exists flag
    pub fn if_exists(&self) -> bool {
        self.if_exists
    }

    pub fn validated_result(&self) -> ValidatedDrop {
        ValidatedDrop {
            target_type: self.target_type.clone(),
            target_name: self.target_name.clone(),
            space_name: self.space_name.clone(),
            if_exists: self.if_exists,
        }
    }
}

/// Implementing the StatementValidator trait
///
/// # Refactoring changes
/// The `validate` method accepts `Arc<Ast>` and `Arc<QueryContext>` as parameters.
impl StatementValidator for DropValidator {
    fn validate(
        &mut self,
        ast: Arc<Ast>,
        qctx: Arc<QueryContext>,
    ) -> Result<ValidationResult, ValidationError> {
        let drop_stmt = match &ast.stmt {
            crate::query::parser::ast::Stmt::Drop(drop_stmt) => drop_stmt,
            _ => {
                return Err(ValidationError::new(
                    "Expected DROP statement".to_string(),
                    ValidationErrorType::SemanticError,
                ));
            }
        };

        self.validate_impl(drop_stmt)?;

        // For TAG, EDGE, and INDEX operations, get space name from query context if not already set
        if self.space_name.is_none() {
            if let Some(space_info) = qctx.space_info() {
                self.space_name = Some(space_info.space_name.clone());
            }
        }

        let mut info = ValidationInfo::new();
        info.semantic_info.space_name = self.space_name.clone();

        Ok(ValidationResult::success_with_info(info))
    }

    fn statement_type(&self) -> StatementType {
        StatementType::Drop
    }

    fn inputs(&self) -> &[ColumnDef] {
        &self.inputs
    }

    fn outputs(&self) -> &[ColumnDef] {
        &self.outputs
    }

    fn is_global_statement(&self) -> bool {
        // The `DROP SPACE` statement is a global statement; the other `DROP` statements are not.
        matches!(self.target_type, DropTargetType::Space)
    }

    fn expression_props(&self) -> &ExpressionProps {
        &self.expr_props
    }

    fn user_defined_vars(&self) -> &[String] {
        &self.user_defined_vars
    }
}

impl Default for DropValidator {
    fn default() -> Self {
        Self::new()
    }
}
