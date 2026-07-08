//! Index creation statement validator
//! Verify the CREATE TAG INDEX and CREATE EDGE INDEX statements.
//!
//! Design principles:
//! The StatementValidator trait has been implemented to unify the interface.
//! 2. It is necessary to pre-select the space (either obtain it from the statement or use the default value).
//! 3. Verify that the index attribute is not empty.

use crate::query::parser::ast::stmt::{Ast, CreateStmt, CreateTarget, IndexType};
use crate::query::validator::error::{ValidationError, ValidationErrorType};
use crate::query::validator::structs::validation_info::ValidationInfo;
use crate::query::validator::validator_trait::{
    ColumnDef, ExpressionProps, StatementType, StatementValidator, ValidationResult, ValueType,
};
use crate::query::QueryContext;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct ValidatedIndexCreate {
    pub index_type: IndexType,
    pub index_name: String,
    pub schema_name: String,
    pub properties: Vec<String>,
    pub space_name: String,
    pub if_not_exists: bool,
}

#[derive(Debug, Clone)]
pub enum IndexCreateTarget {
    Tag,
    Edge,
}

#[derive(Debug)]
pub struct CreateIndexValidator {
    index_type: IndexCreateTarget,
    index_name: String,
    schema_name: String,
    properties: Vec<String>,
    space_name: String,
    if_not_exists: bool,
    inputs: Vec<ColumnDef>,
    outputs: Vec<ColumnDef>,
    expr_props: ExpressionProps,
    user_defined_vars: Vec<String>,
}

impl CreateIndexValidator {
    pub fn new() -> Self {
        Self {
            index_type: IndexCreateTarget::Tag,
            index_name: String::new(),
            schema_name: String::new(),
            properties: Vec::new(),
            space_name: String::new(),
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

    fn validate_impl(&mut self, stmt: &CreateStmt) -> Result<(), ValidationError> {
        let target = match &stmt.target {
            CreateTarget::Index { index_type, .. } => {
                self.index_type = match index_type {
                    IndexType::Tag => IndexCreateTarget::Tag,
                    IndexType::Edge => IndexCreateTarget::Edge,
                };
                &stmt.target
            }
            _ => {
                return Err(ValidationError::new(
                    "Expected CREATE TAG INDEX or CREATE EDGE INDEX statement".to_string(),
                    ValidationErrorType::SemanticError,
                ));
            }
        };

        if let CreateTarget::Index {
            index_type: _,
            name,
            on,
            properties,
        } = target
        {
            self.index_name = name.clone();
            self.schema_name = on.clone();
            self.properties = properties.clone();
            self.if_not_exists = stmt.if_not_exists;
        }

        if self.index_name.is_empty() {
            return Err(ValidationError::new(
                "Index name cannot be empty".to_string(),
                ValidationErrorType::SemanticError,
            ));
        }

        if self.schema_name.is_empty() {
            return Err(ValidationError::new(
                "Target (tag/edge type) name cannot be empty".to_string(),
                ValidationErrorType::SemanticError,
            ));
        }

        if self.properties.is_empty() {
            return Err(ValidationError::new(
                "At least one property must be specified for index".to_string(),
                ValidationErrorType::SemanticError,
            ));
        }

        Ok(())
    }

    fn validate_impl_with_context(
        &mut self,
        stmt: &CreateStmt,
        qctx: &QueryContext,
    ) -> Result<(), ValidationError> {
        self.validate_impl(stmt)?;

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

impl StatementValidator for CreateIndexValidator {
    fn validate(
        &mut self,
        ast: Arc<Ast>,
        qctx: Arc<QueryContext>,
    ) -> Result<ValidationResult, ValidationError> {
        let create_stmt = match &ast.stmt {
            crate::query::parser::ast::Stmt::Create(create_stmt) => create_stmt,
            _ => {
                return Err(ValidationError::new(
                    "Expected CREATE statement".to_string(),
                    ValidationErrorType::SemanticError,
                ));
            }
        };

        self.validate_impl_with_context(create_stmt, &qctx)?;

        let mut info = ValidationInfo::new();
        info.semantic_info.query_type = Some(format!(
            "Create{}Index",
            match self.index_type {
                IndexCreateTarget::Tag => "Tag",
                IndexCreateTarget::Edge => "Edge",
            }
        ));

        info.semantic_info.referenced_schemas = vec![self.schema_name.clone()];
        info.semantic_info.space_name = Some(self.space_name.clone());

        Ok(ValidationResult::success_with_info(info))
    }

    fn statement_type(&self) -> StatementType {
        match self.index_type {
            IndexCreateTarget::Tag => StatementType::CreateTagIndex,
            IndexCreateTarget::Edge => StatementType::CreateEdgeIndex,
        }
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

impl Default for CreateIndexValidator {
    fn default() -> Self {
        Self::new()
    }
}
