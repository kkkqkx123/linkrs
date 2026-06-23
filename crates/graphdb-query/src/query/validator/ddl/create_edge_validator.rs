//! Create Edge statement validator
//! Verify the CREATE EDGE statements.
//!
//! Design principles:
//! 1. The StatementValidator trait has been implemented to unify the interface.
//! 2. It is necessary to pre-select the space (either obtain it from the statement or use the default value).
//! 3. Verify that the edge type name is valid and properties are well-defined.

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
pub struct ValidatedCreateEdge {
    pub edge_name: String,
    pub properties: Vec<PropertyDef>,
    pub space_name: String,
    pub ttl_duration: Option<i64>,
    pub ttl_col: Option<String>,
    pub if_not_exists: bool,
}

#[derive(Debug)]
pub struct CreateEdgeValidator {
    edge_name: String,
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

impl CreateEdgeValidator {
    pub fn new() -> Self {
        Self {
            edge_name: String::new(),
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
            CreateTarget::EdgeType {
                name,
                properties,
                ttl_duration,
                ttl_col,
                src_tag: _,
                dst_tag: _,
            } => {
                if name.is_empty() {
                    return Err(ValidationError::new(
                        "Edge type name cannot be empty".to_string(),
                        ValidationErrorType::SemanticError,
                    ));
                }

                self.edge_name = name.clone();
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
            }
            _ => {
                return Err(ValidationError::new(
                    "Expected CREATE EDGE statement".to_string(),
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

impl StatementValidator for CreateEdgeValidator {
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
        info.semantic_info.query_type = Some("CreateEdge".to_string());
        info.semantic_info.referenced_schemas = vec![self.edge_name.clone()];
        info.semantic_info.space_name = Some(self.space_name.clone());

        Ok(ValidationResult::success_with_info(info))
    }

    fn statement_type(&self) -> StatementType {
        StatementType::CreateEdge
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

impl Default for CreateEdgeValidator {
    fn default() -> Self {
        Self::new()
    }
}
