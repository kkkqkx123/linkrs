//! Vector Search Validator
//!
//! This module implements the validator for vector search statements,
//! ensuring semantic correctness before plan generation.

use std::fmt;
use std::sync::Arc;

use crate::query::parser::ast::{
    Ast, CreateVectorIndex, DropVectorIndex, LookupVector, MatchVector, SearchVectorStatement,
    VectorQueryType,
};
use crate::query::validator::error::{ValidationError, ValidationErrorType};
use crate::query::validator::validator_trait::{
    ExpressionProps, StatementValidator, ValidationResult,
};
use crate::query::validator::{ColumnDef, StatementType, ValidationInfo};
use crate::query::QueryContext;

pub struct VectorValidator {
    inputs: Vec<ColumnDef>,
    outputs: Vec<ColumnDef>,
    expression_props: ExpressionProps,
}

impl Default for VectorValidator {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for VectorValidator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VectorValidator").finish()
    }
}

impl VectorValidator {
    pub fn new() -> Self {
        Self {
            inputs: Vec::new(),
            outputs: Vec::new(),
            expression_props: ExpressionProps::default(),
        }
    }

    fn validate_impl(
        &self,
        stmt: &crate::query::parser::ast::Stmt,
        _qctx: &QueryContext,
    ) -> Result<ValidationInfo, ValidationError> {
        match stmt {
            crate::query::parser::ast::Stmt::CreateVectorIndex(create) => {
                self.validate_create_index(create)
            }
            crate::query::parser::ast::Stmt::DropVectorIndex(drop) => {
                self.validate_drop_index(drop)
            }
            crate::query::parser::ast::Stmt::SearchVector(search) => self.validate_search(search),
            crate::query::parser::ast::Stmt::LookupVector(lookup) => self.validate_lookup(lookup),
            crate::query::parser::ast::Stmt::MatchVector(match_stmt) => {
                self.validate_match(match_stmt)
            }
            _ => Err(ValidationError::new(
                "Not a vector search statement",
                ValidationErrorType::SemanticError,
            )),
        }
    }

    fn validate_create_index(
        &self,
        create: &CreateVectorIndex,
    ) -> Result<ValidationInfo, ValidationError> {
        if create.index_name.is_empty() {
            return Err(ValidationError::new(
                "Index name cannot be empty",
                ValidationErrorType::SemanticError,
            ));
        }

        if create.field_name.is_empty() {
            return Err(ValidationError::new(
                "Field name cannot be empty",
                ValidationErrorType::SemanticError,
            ));
        }

        if create.config.vector_size == 0 || create.config.vector_size > 65536 {
            return Err(ValidationError::new(
                format!(
                    "Vector size must be between 1 and 65536, got {}",
                    create.config.vector_size
                ),
                ValidationErrorType::SemanticError,
            ));
        }

        Ok(ValidationInfo::new())
    }

    fn validate_drop_index(
        &self,
        drop: &DropVectorIndex,
    ) -> Result<ValidationInfo, ValidationError> {
        if drop.index_name.is_empty() {
            return Err(ValidationError::new(
                "Index name cannot be empty",
                ValidationErrorType::SemanticError,
            ));
        }

        Ok(ValidationInfo::new())
    }

    fn validate_search(
        &self,
        search: &SearchVectorStatement,
    ) -> Result<ValidationInfo, ValidationError> {
        if search.index_name.is_empty() {
            return Err(ValidationError::new(
                "Index name cannot be empty",
                ValidationErrorType::SemanticError,
            ));
        }

        self.validate_vector_query_expr(&search.query)?;

        if let Some(threshold) = search.threshold {
            if !(0.0..=1.0).contains(&threshold) {
                return Err(ValidationError::new(
                    format!("Threshold must be between 0 and 1, got {}", threshold),
                    ValidationErrorType::SemanticError,
                ));
            }
        }

        if let Some(limit) = search.limit {
            if limit == 0 || limit > 10000 {
                return Err(ValidationError::new(
                    format!("Limit must be between 1 and 10000, got {}", limit),
                    ValidationErrorType::SemanticError,
                ));
            }
        }

        Ok(ValidationInfo::new())
    }

    fn validate_vector_query_expr(
        &self,
        query: &crate::query::parser::ast::VectorQueryExpr,
    ) -> Result<(), ValidationError> {
        match query.query_type {
            VectorQueryType::Vector => {
                // Validate vector format
                // Parse vector string and validate dimensions
                let vector_str = &query.query_data;
                if !vector_str.starts_with('[') || !vector_str.ends_with(']') {
                    return Err(ValidationError::new(
                        "Vector must be in format [x1, x2, ...]",
                        ValidationErrorType::SemanticError,
                    ));
                }

                // Extract numbers from vector string
                let inner = &vector_str[1..vector_str.len() - 1];
                if inner.trim().is_empty() {
                    return Err(ValidationError::new(
                        "Vector cannot be empty",
                        ValidationErrorType::SemanticError,
                    ));
                }

                // Validate each element is a valid number
                for part in inner.split(',') {
                    let trimmed = part.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    if trimmed.parse::<f64>().is_err() {
                        return Err(ValidationError::new(
                            format!("Invalid vector element: {}", trimmed),
                            ValidationErrorType::SemanticError,
                        ));
                    }
                }
            }
            VectorQueryType::Text => {
                // Text query requires embedding service
                // This will be handled at execution time
                if query.query_data.is_empty() {
                    return Err(ValidationError::new(
                        "Text query cannot be empty",
                        ValidationErrorType::SemanticError,
                    ));
                }
            }
            VectorQueryType::Parameter => {
                // Parameter query, validate at execution time
                if !query.query_data.starts_with('$') {
                    return Err(ValidationError::new(
                        "Parameter must start with $",
                        ValidationErrorType::SemanticError,
                    ));
                }
            }
        }

        Ok(())
    }

    fn validate_lookup(&self, lookup: &LookupVector) -> Result<ValidationInfo, ValidationError> {
        if lookup.schema_name.is_empty() {
            return Err(ValidationError::new(
                "Schema name cannot be empty",
                ValidationErrorType::SemanticError,
            ));
        }

        if lookup.index_name.is_empty() {
            return Err(ValidationError::new(
                "Index name cannot be empty",
                ValidationErrorType::SemanticError,
            ));
        }

        self.validate_vector_query_expr(&lookup.query)?;

        if let Some(limit) = lookup.limit {
            if limit == 0 || limit > 10000 {
                return Err(ValidationError::new(
                    format!("Limit must be between 1 and 10000, got {}", limit),
                    ValidationErrorType::SemanticError,
                ));
            }
        }

        Ok(ValidationInfo::new())
    }

    fn validate_match(&self, match_stmt: &MatchVector) -> Result<ValidationInfo, ValidationError> {
        if match_stmt.pattern.is_empty() {
            return Err(ValidationError::new(
                "Match pattern cannot be empty",
                ValidationErrorType::SemanticError,
            ));
        }

        if match_stmt.vector_condition.field.is_empty() {
            return Err(ValidationError::new(
                "Vector field name cannot be empty",
                ValidationErrorType::SemanticError,
            ));
        }

        self.validate_vector_query_expr(&match_stmt.vector_condition.query)?;

        if let Some(threshold) = match_stmt.vector_condition.threshold {
            if !(0.0..=1.0).contains(&threshold) {
                return Err(ValidationError::new(
                    format!("Threshold must be between 0 and 1, got {}", threshold),
                    ValidationErrorType::SemanticError,
                ));
            }
        }

        Ok(ValidationInfo::new())
    }
}

impl StatementValidator for VectorValidator {
    fn validate(
        &mut self,
        ast: Arc<Ast>,
        qctx: Arc<QueryContext>,
    ) -> Result<ValidationResult, ValidationError> {
        let stmt = ast.stmt.clone();
        let info = self.validate_impl(&stmt, &qctx)?;
        Ok(ValidationResult::success_with_info(info))
    }

    fn inputs(&self) -> &[ColumnDef] {
        &self.inputs
    }

    fn outputs(&self) -> &[ColumnDef] {
        &self.outputs
    }

    fn expression_props(&self) -> &ExpressionProps {
        &self.expression_props
    }

    fn statement_type(&self) -> StatementType {
        StatementType::SearchVector
    }

    fn is_global_statement(&self) -> bool {
        false
    }

    fn user_defined_vars(&self) -> &[String] {
        &[]
    }
}
