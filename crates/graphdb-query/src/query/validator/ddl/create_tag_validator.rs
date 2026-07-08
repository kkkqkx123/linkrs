//! Create Tag statement validator
//! Verify the CREATE TAG statements.
//!
//! Design principles:
//! 1. The StatementValidator trait has been implemented to unify the interface.
//! 2. It is necessary to pre-select the space (either obtain it from the statement or use the default value).
//! 3. Verify that the tag name is valid and properties are well-defined.

use crate::core::types::PropertyDef;
use crate::query::parser::ast::stmt::{Ast, CreateStmt, CreateTarget, Stmt};
use crate::query::validator::error::{ValidationError, ValidationErrorType};
use crate::query::validator::structs::validation_info::ValidationInfo;
use crate::query::validator::validator_trait::{
    ColumnDef, ExpressionProps, StatementType, StatementValidator, ValidationResult, ValueType,
};
use crate::query::QueryContext;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct ValidatedCreateTag {
    pub tag_name: String,
    pub properties: Vec<PropertyDef>,
    pub space_name: String,
    pub ttl_duration: Option<i64>,
    pub ttl_col: Option<String>,
    pub if_not_exists: bool,
}

#[derive(Debug)]
pub struct CreateTagValidator {
    tag_name: String,
    properties: Vec<PropertyDef>,
    space_name: String,
    ttl_duration: Option<i64>,
    ttl_col: Option<String>,
    if_not_exists: bool,
    inputs: Vec<ColumnDef>,
    outputs: Vec<ColumnDef>,
    expr_props: ExpressionProps,
    user_defined_vars: Vec<String>,
}

impl CreateTagValidator {
    pub fn new() -> Self {
        Self {
            tag_name: String::new(),
            properties: Vec::new(),
            space_name: String::new(),
            ttl_duration: None,
            ttl_col: None,
            if_not_exists: false,
            inputs: Vec::new(),
            outputs: vec![ColumnDef {
                name: "Result".to_string(),
                type_: ValueType::String,
            }],
            expr_props: ExpressionProps::default(),
            user_defined_vars: Vec::new(),
        }
    }

    fn validate_impl(
        &mut self,
        stmt: &CreateStmt,
        qctx: &QueryContext,
    ) -> Result<(), ValidationError> {
        match &stmt.target {
            CreateTarget::Tag {
                name,
                properties,
                ttl_duration,
                ttl_col,
            } => {
                if name.is_empty() {
                    return Err(ValidationError::new(
                        "Tag name cannot be empty".to_string(),
                        ValidationErrorType::SemanticError,
                    ));
                }

                self.tag_name = name.clone();
                self.properties = properties.clone();
                self.ttl_duration = *ttl_duration;
                self.ttl_col = ttl_col.clone();
                self.if_not_exists = stmt.if_not_exists;

                for prop in &self.properties {
                    if prop.name.is_empty() {
                        return Err(ValidationError::new(
                            "Property name cannot be empty".to_string(),
                            ValidationErrorType::SemanticError,
                        ));
                    }
                }

                let mut seen_names = std::collections::HashSet::new();
                for prop in &self.properties {
                    if !seen_names.insert(&prop.name) {
                        return Err(ValidationError::new(
                            format!(
                                "Duplicate property name '{}' in tag '{}'",
                                prop.name, self.tag_name
                            ),
                            ValidationErrorType::SemanticError,
                        ));
                    }
                }
            }
            _ => {
                return Err(ValidationError::new(
                    "Expected CREATE TAG statement".to_string(),
                    ValidationErrorType::SemanticError,
                ));
            }
        }

        // Get space name from query context
        if let Some(space_info) = qctx.space_info() {
            self.space_name = space_info.space_name.clone();
        } else {
            return Err(ValidationError::new(
                "No graph space selected, please execute USE <space> first".to_string(),
                ValidationErrorType::SemanticError,
            ));
        }

        Ok(())
    }
}

impl StatementValidator for CreateTagValidator {
    fn validate(
        &mut self,
        ast: Arc<Ast>,
        qctx: Arc<QueryContext>,
    ) -> Result<ValidationResult, ValidationError> {
        let create_stmt = match &ast.stmt {
            Stmt::Create(create_stmt) => create_stmt,
            _ => {
                return Err(ValidationError::new(
                    "Expected CREATE statement".to_string(),
                    ValidationErrorType::SemanticError,
                ));
            }
        };

        self.validate_impl(create_stmt, &qctx)?;

        let mut info = ValidationInfo::new();
        info.semantic_info.query_type = Some("CreateTag".to_string());
        info.semantic_info.referenced_schemas = vec![self.tag_name.clone()];
        info.semantic_info.space_name = Some(self.space_name.clone());

        Ok(ValidationResult::success_with_info(info))
    }

    fn statement_type(&self) -> StatementType {
        StatementType::CreateTag
    }

    fn inputs(&self) -> &[ColumnDef] {
        &self.inputs
    }

    fn outputs(&self) -> &[ColumnDef] {
        &self.outputs
    }

    fn expression_props(&self) -> &ExpressionProps {
        &self.expr_props
    }

    fn user_defined_vars(&self) -> &[String] {
        &self.user_defined_vars
    }

    fn is_global_statement(&self) -> bool {
        false
    }
}

impl Default for CreateTagValidator {
    fn default() -> Self {
        Self::new()
    }
}
