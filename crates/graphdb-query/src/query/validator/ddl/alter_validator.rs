//! Alter Statement Validator
//! Corresponding to the functionality of the Alter-related validator in NebulaGraph
//! Verify statements such as ALTER TAG, ALTER EDGE, and ALTER SPACE.
//!
//! Design principles:
//! The StatementValidator trait has been implemented, unifying the interface.
//! 2. The `ALTER SPACE` statement is a global statement; other `ALTER` statements require you to specify a specific space.
//! 3. Verify the legitimacy of attribute modifications (adding, deleting, modifying)

use crate::core::types::PropertyDef;
use crate::query::parser::ast::stmt::{AlterStmt, AlterTarget, Ast, PropertyChange};
use crate::query::validator::error::{ValidationError, ValidationErrorType};
use crate::query::validator::structs::validation_info::ValidationInfo;
use crate::query::validator::validator_trait::{
    ColumnDef, ExpressionProps, StatementType, StatementValidator, ValidationResult, ValueType,
};
use crate::query::QueryContext;
use std::sync::Arc;

/// Verified Alter information
#[derive(Debug, Clone)]
pub struct ValidatedAlter {
    pub target_type: AlterTargetType,
    pub target_name: String,
    pub space_name: Option<String>,
    pub additions: Vec<PropertyDef>,
    pub deletions: Vec<String>,
    pub changes: Vec<PropertyChange>,
}

/// Alter target type
#[derive(Debug, Clone)]
pub enum AlterTargetType {
    Tag,
    Edge,
    Space,
}

/// Alter Statement Validator
#[derive(Debug)]
pub struct AlterValidator {
    target_type: AlterTargetType,
    target_name: String,
    space_name: Option<String>,
    additions: Vec<PropertyDef>,
    deletions: Vec<String>,
    changes: Vec<PropertyChange>,
    inputs: Vec<ColumnDef>,
    outputs: Vec<ColumnDef>,
    expr_props: ExpressionProps,
    user_defined_vars: Vec<String>,
}

impl AlterValidator {
    pub fn new() -> Self {
        Self {
            target_type: AlterTargetType::Tag,
            target_name: String::new(),
            space_name: None,
            additions: Vec::new(),
            deletions: Vec::new(),
            changes: Vec::new(),
            inputs: Vec::new(),
            outputs: vec![ColumnDef {
                name: "Result".to_string(),
                type_: ValueType::String,
            }],
            expr_props: ExpressionProps::default(),
            user_defined_vars: Vec::new(),
        }
    }

    fn validate_impl(&mut self, stmt: &AlterStmt) -> Result<(), ValidationError> {
        match &stmt.target {
            AlterTarget::Tag {
                tag_name,
                additions,
                deletions,
                changes,
            } => {
                self.target_type = AlterTargetType::Tag;
                self.target_name = tag_name.clone();
                self.additions = additions.clone();
                self.deletions = deletions.clone();
                self.changes = changes.clone();

                // Verify that the tag name is not empty.
                if self.target_name.is_empty() {
                    return Err(ValidationError::new(
                        "Tag name cannot be empty".to_string(),
                        ValidationErrorType::SemanticError,
                    ));
                }

                // Verify that at least one modification has been made.
                if additions.is_empty() && deletions.is_empty() && changes.is_empty() {
                    return Err(ValidationError::new(
                        "At least one alter operation is required".to_string(),
                        ValidationErrorType::SemanticError,
                    ));
                }

                // Verify attribute modification
                self.validate_property_changes(additions, deletions, changes)?;
            }
            AlterTarget::Edge {
                edge_name,
                additions,
                deletions,
                changes,
            } => {
                self.target_type = AlterTargetType::Edge;
                self.target_name = edge_name.clone();
                self.additions = additions.clone();
                self.deletions = deletions.clone();
                self.changes = changes.clone();

                // Verify that the "edge" name is not empty.
                if self.target_name.is_empty() {
                    return Err(ValidationError::new(
                        "Edge name cannot be empty".to_string(),
                        ValidationErrorType::SemanticError,
                    ));
                }

                // Verify that at least one modification has been made.
                if additions.is_empty() && deletions.is_empty() && changes.is_empty() {
                    return Err(ValidationError::new(
                        "At least one alter operation is required".to_string(),
                        ValidationErrorType::SemanticError,
                    ));
                }

                // Verify the modification of the attribute.
                self.validate_property_changes(additions, deletions, changes)?;

                // For TAG and EDGE operations, space_name should be obtained from query context
                // This is handled in validate_impl_with_context
            }
            AlterTarget::Space {
                space_name,
                comment,
            } => {
                self.target_type = AlterTargetType::Space;
                self.target_name = space_name.clone();
                self.space_name = Some(space_name.clone());

                // Verify that the value of the `space` variable is not empty.
                if self.target_name.is_empty() {
                    return Err(ValidationError::new(
                        "Space name cannot be empty".to_string(),
                        ValidationErrorType::SemanticError,
                    ));
                }

                // Verify that at least one modification parameter is present.
                if comment.is_none() {
                    return Err(ValidationError::new(
                        "At least one alter parameter is required".to_string(),
                        ValidationErrorType::SemanticError,
                    ));
                }
            }
        }

        Ok(())
    }

    fn validate_impl_with_context(
        &mut self,
        stmt: &AlterStmt,
        qctx: &QueryContext,
    ) -> Result<(), ValidationError> {
        self.validate_impl(stmt)?;

        // For TAG and EDGE operations, get space name from query context
        match &stmt.target {
            AlterTarget::Tag { .. } | AlterTarget::Edge { .. } => {
                if let Some(space_info) = qctx.space_info() {
                    self.space_name = Some(space_info.space_name.clone());
                } else {
                    return Err(ValidationError::new(
                        "No graph space selected, please execute USE <space> first".to_string(),
                        ValidationErrorType::SemanticError,
                    ));
                }
            }
            AlterTarget::Space { .. } => {
                // Space operations already have space_name set in validate_impl
            }
        }

        Ok(())
    }

    fn validate_property_changes(
        &self,
        additions: &[PropertyDef],
        deletions: &[String],
        changes: &[PropertyChange],
    ) -> Result<(), ValidationError> {
        // Verify the added attributes.
        for prop in additions {
            self.validate_property_name(&prop.name)?;
        }

        // Verify the names of the attributes that were deleted.
        for name in deletions {
            self.validate_property_name(name)?;
        }

        // Verify the modified properties.
        for change in changes {
            self.validate_property_name(&change.old_name)?;
            self.validate_property_name(&change.new_name)?;

            // The new and old names cannot be the same.
            if change.old_name == change.new_name {
                return Err(ValidationError::new(
                    format!(
                        "Old name and new name cannot be the same: {}",
                        change.old_name
                    ),
                    ValidationErrorType::SemanticError,
                ));
            }
        }

        // Check for conflicts: The same attribute cannot be added and deleted at the same time.
        for added in additions {
            if deletions.contains(&added.name) {
                return Err(ValidationError::new(
                    format!(
                        "Cannot add and delete property '{}' at the same time",
                        added.name
                    ),
                    ValidationErrorType::SemanticError,
                ));
            }
        }

        // Check for conflicts: It is not possible to delete and modify the same attribute at the same time.
        for deleted in deletions {
            for changed in changes {
                if deleted == &changed.old_name {
                    return Err(ValidationError::new(
                        format!(
                            "Cannot delete and change property '{}' at the same time",
                            deleted
                        ),
                        ValidationErrorType::SemanticError,
                    ));
                }
            }
        }

        Ok(())
    }

    fn validate_property_name(&self, name: &str) -> Result<(), ValidationError> {
        if name.is_empty() {
            return Err(ValidationError::new(
                "Property name cannot be empty".to_string(),
                ValidationErrorType::SemanticError,
            ));
        }

        // The attribute name must start with a letter or an underscore (_).
        let first_char = name
            .chars()
            .next()
            .expect("Attribute name is verified to be non-null");
        if !first_char.is_ascii_alphabetic() && first_char != '_' {
            return Err(ValidationError::new(
                format!(
                    "Property name '{}' must start with a letter or underscore",
                    name
                ),
                ValidationErrorType::SemanticError,
            ));
        }

        // Property names can only contain letters, digits, and underscores (_).
        for (i, c) in name.chars().enumerate() {
            if i > 0 && !c.is_ascii_alphanumeric() && c != '_' {
                return Err(ValidationError::new(
                    format!(
                        "Property name '{}' contains invalid character '{}'",
                        name, c
                    ),
                    ValidationErrorType::SemanticError,
                ));
            }
        }

        Ok(())
    }

    /// Obtain the target type
    pub fn target_type(&self) -> &AlterTargetType {
        &self.target_type
    }

    /// Obtain the target name
    pub fn target_name(&self) -> &str {
        &self.target_name
    }

    /// Obtain the space name
    pub fn space_name(&self) -> Option<&String> {
        self.space_name.as_ref()
    }

    /// Obtain the added attributes
    pub fn additions(&self) -> &[PropertyDef] {
        &self.additions
    }

    /// Retrieve the deleted attributes
    pub fn deletions(&self) -> &[String] {
        &self.deletions
    }

    /// Obtain the modified properties.
    pub fn changes(&self) -> &[PropertyChange] {
        &self.changes
    }

    pub fn validated_result(&self) -> ValidatedAlter {
        ValidatedAlter {
            target_type: self.target_type.clone(),
            target_name: self.target_name.clone(),
            space_name: self.space_name.clone(),
            additions: self.additions.clone(),
            deletions: self.deletions.clone(),
            changes: self.changes.clone(),
        }
    }
}

/// Implementing the StatementValidator trait
///
/// # Refactoring changes
/// The `validate` method accepts `Arc<Ast>` and `Arc<QueryContext>` as arguments.
impl StatementValidator for AlterValidator {
    fn validate(
        &mut self,
        ast: Arc<Ast>,
        qctx: Arc<QueryContext>,
    ) -> Result<ValidationResult, ValidationError> {
        let alter_stmt = match &ast.stmt {
            crate::query::parser::ast::Stmt::Alter(alter_stmt) => alter_stmt,
            _ => {
                return Err(ValidationError::new(
                    "Expected ALTER statement".to_string(),
                    ValidationErrorType::SemanticError,
                ));
            }
        };

        self.validate_impl_with_context(alter_stmt, &qctx)?;

        let mut info = ValidationInfo::new();
        info.semantic_info.space_name = self.space_name.clone();

        Ok(ValidationResult::success_with_info(info))
    }

    fn statement_type(&self) -> StatementType {
        StatementType::Alter
    }

    fn inputs(&self) -> &[ColumnDef] {
        &self.inputs
    }

    fn outputs(&self) -> &[ColumnDef] {
        &self.outputs
    }

    fn is_global_statement(&self) -> bool {
        // The `ALTER SPACE` statement is a global statement; the other `ALTER` statements are not.
        matches!(self.target_type, AlterTargetType::Space)
    }

    fn expression_props(&self) -> &ExpressionProps {
        &self.expr_props
    }

    fn user_defined_vars(&self) -> &[String] {
        &self.user_defined_vars
    }
}

impl Default for AlterValidator {
    fn default() -> Self {
        Self::new()
    }
}
