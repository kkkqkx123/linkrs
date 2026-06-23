//! Full-Text Search Validator
//!
//! This module implements the validator for full-text search statements,
//! ensuring semantic correctness before plan generation.

use std::fmt;
use std::sync::Arc;

use crate::query::parser::ast::{
    AlterFulltextIndex, CreateFulltextIndex, DescribeFulltextIndex, DropFulltextIndex,
    FulltextQueryExpr, LookupFulltext, MatchFulltext, SearchStatement,
};
use crate::query::validator::error::{ValidationError, ValidationErrorType};
use crate::query::validator::validator_trait::{
    ExpressionProps, StatementValidator, ValidationResult,
};
use crate::query::validator::{ColumnDef, StatementType, ValidationInfo};
use crate::query::QueryContext;

pub struct FulltextValidator {
    inputs: Vec<ColumnDef>,
    outputs: Vec<ColumnDef>,
    expression_props: ExpressionProps,
}

impl Default for FulltextValidator {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for FulltextValidator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FulltextValidator").finish()
    }
}

impl FulltextValidator {
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
            crate::query::parser::ast::Stmt::CreateFulltextIndex(create) => {
                self.validate_create_index(create)
            }
            crate::query::parser::ast::Stmt::DropFulltextIndex(drop) => {
                self.validate_drop_index(drop)
            }
            crate::query::parser::ast::Stmt::AlterFulltextIndex(alter) => {
                self.validate_alter_index(alter)
            }
            crate::query::parser::ast::Stmt::ShowFulltextIndex(_show) => self.validate_show_index(),
            crate::query::parser::ast::Stmt::DescribeFulltextIndex(describe) => {
                self.validate_describe_index(describe)
            }
            crate::query::parser::ast::Stmt::Search(search) => self.validate_search(search),
            crate::query::parser::ast::Stmt::LookupFulltext(lookup) => self.validate_lookup(lookup),
            crate::query::parser::ast::Stmt::MatchFulltext(match_stmt) => {
                self.validate_match(match_stmt)
            }
            _ => Err(ValidationError::new(
                "Not a full-text search statement",
                ValidationErrorType::SemanticError,
            )),
        }
    }

    fn validate_create_index(
        &self,
        create: &CreateFulltextIndex,
    ) -> Result<ValidationInfo, ValidationError> {
        if create.index_name.is_empty() {
            return Err(ValidationError::new(
                "Index name cannot be empty",
                ValidationErrorType::SemanticError,
            ));
        }

        if create.fields.is_empty() {
            return Err(ValidationError::new(
                "Full-text index must have at least one field",
                ValidationErrorType::SemanticError,
            ));
        }

        if let Some(ref config) = create.options.bm25_config {
            if let Some(k1) = config.k1 {
                if k1 < 0.0 {
                    return Err(ValidationError::new(
                        "BM25 k1 parameter must be non-negative",
                        ValidationErrorType::SemanticError,
                    ));
                }
            }
            if let Some(b) = config.b {
                if !(0.0..=1.0).contains(&b) {
                    return Err(ValidationError::new(
                        "BM25 b parameter must be between 0 and 1",
                        ValidationErrorType::SemanticError,
                    ));
                }
            }
        }

        Ok(ValidationInfo::new())
    }

    fn validate_drop_index(
        &self,
        drop: &DropFulltextIndex,
    ) -> Result<ValidationInfo, ValidationError> {
        if drop.index_name.is_empty() {
            return Err(ValidationError::new(
                "Index name cannot be empty",
                ValidationErrorType::SemanticError,
            ));
        }

        Ok(ValidationInfo::new())
    }

    fn validate_alter_index(
        &self,
        alter: &AlterFulltextIndex,
    ) -> Result<ValidationInfo, ValidationError> {
        if alter.index_name.is_empty() {
            return Err(ValidationError::new(
                "Index name cannot be empty",
                ValidationErrorType::SemanticError,
            ));
        }

        if alter.actions.is_empty() {
            return Err(ValidationError::new(
                "ALTER INDEX must have at least one action",
                ValidationErrorType::SemanticError,
            ));
        }

        for action in &alter.actions {
            match action {
                crate::query::parser::ast::AlterIndexAction::AddField(field)
                    if field.field_name.is_empty() =>
                {
                    return Err(ValidationError::new(
                        "Field name cannot be empty",
                        ValidationErrorType::SemanticError,
                    ));
                }
                crate::query::parser::ast::AlterIndexAction::DropField(field_name)
                    if field_name.is_empty() =>
                {
                    return Err(ValidationError::new(
                        "Field name cannot be empty",
                        ValidationErrorType::SemanticError,
                    ));
                }
                _ => {}
            }
        }

        Ok(ValidationInfo::new())
    }

    fn validate_show_index(&self) -> Result<ValidationInfo, ValidationError> {
        Ok(ValidationInfo::new())
    }

    fn validate_describe_index(
        &self,
        describe: &DescribeFulltextIndex,
    ) -> Result<ValidationInfo, ValidationError> {
        if describe.index_name.is_empty() {
            return Err(ValidationError::new(
                "Index name cannot be empty",
                ValidationErrorType::SemanticError,
            ));
        }

        Ok(ValidationInfo::new())
    }

    fn validate_search(&self, search: &SearchStatement) -> Result<ValidationInfo, ValidationError> {
        if search.index_name.is_empty() {
            return Err(ValidationError::new(
                "Index name cannot be empty",
                ValidationErrorType::SemanticError,
            ));
        }

        self.validate_query_expr(&search.query)?;

        if let Some(limit) = search.limit {
            if limit == 0 {
                return Err(ValidationError::new(
                    "LIMIT must be positive",
                    ValidationErrorType::SemanticError,
                ));
            }
        }

        if let Some(offset) = search.offset {
            if offset == 0 {
                return Err(ValidationError::new(
                    "OFFSET must be non-negative",
                    ValidationErrorType::SemanticError,
                ));
            }
        }

        Ok(ValidationInfo::new())
    }

    fn validate_query_expr(&self, expr: &FulltextQueryExpr) -> Result<(), ValidationError> {
        match expr {
            FulltextQueryExpr::Simple(text) => {
                if text.is_empty() {
                    return Err(ValidationError::new(
                        "Query text cannot be empty",
                        ValidationErrorType::SemanticError,
                    ));
                }
            }
            FulltextQueryExpr::Field(field, query) => {
                if field.is_empty() || query.is_empty() {
                    return Err(ValidationError::new(
                        "Field name and query text cannot be empty",
                        ValidationErrorType::SemanticError,
                    ));
                }
            }
            FulltextQueryExpr::MultiField(fields) => {
                if fields.is_empty() {
                    return Err(ValidationError::new(
                        "Multi-field query must have at least one field",
                        ValidationErrorType::SemanticError,
                    ));
                }
                for (field, query) in fields {
                    if field.is_empty() || query.is_empty() {
                        return Err(ValidationError::new(
                            "Field name and query text cannot be empty",
                            ValidationErrorType::SemanticError,
                        ));
                    }
                }
            }
            FulltextQueryExpr::Boolean {
                must,
                should,
                must_not,
            } => {
                if must.is_empty() && should.is_empty() {
                    return Err(ValidationError::new(
                        "Boolean query must have at least one must or should clause",
                        ValidationErrorType::SemanticError,
                    ));
                }
                for q in must.iter().chain(should.iter()).chain(must_not.iter()) {
                    self.validate_query_expr(q)?;
                }
            }
            FulltextQueryExpr::Phrase(text) => {
                if text.is_empty() {
                    return Err(ValidationError::new(
                        "Phrase query text cannot be empty",
                        ValidationErrorType::SemanticError,
                    ));
                }
            }
            FulltextQueryExpr::Prefix(prefix) => {
                if prefix.is_empty() {
                    return Err(ValidationError::new(
                        "Prefix cannot be empty",
                        ValidationErrorType::SemanticError,
                    ));
                }
            }
            FulltextQueryExpr::Fuzzy(text, distance) => {
                if text.is_empty() {
                    return Err(ValidationError::new(
                        "Fuzzy query text cannot be empty",
                        ValidationErrorType::SemanticError,
                    ));
                }
                if let Some(d) = distance {
                    if *d > 5 {
                        return Err(ValidationError::new(
                            "Fuzzy distance must be between 0 and 5",
                            ValidationErrorType::SemanticError,
                        ));
                    }
                }
            }
            FulltextQueryExpr::Range {
                field,
                lower,
                upper,
                ..
            } => {
                if field.is_empty() {
                    return Err(ValidationError::new(
                        "Range field cannot be empty",
                        ValidationErrorType::SemanticError,
                    ));
                }
                if lower.is_none() && upper.is_none() {
                    return Err(ValidationError::new(
                        "Range query must have at least one bound",
                        ValidationErrorType::SemanticError,
                    ));
                }
            }
            FulltextQueryExpr::Wildcard(pattern) => {
                if pattern.is_empty() {
                    return Err(ValidationError::new(
                        "Wildcard pattern cannot be empty",
                        ValidationErrorType::SemanticError,
                    ));
                }
            }
        }
        Ok(())
    }

    fn validate_lookup(&self, lookup: &LookupFulltext) -> Result<ValidationInfo, ValidationError> {
        if lookup.index_name.is_empty() {
            return Err(ValidationError::new(
                "Index name cannot be empty",
                ValidationErrorType::SemanticError,
            ));
        }

        if lookup.schema_name.is_empty() {
            return Err(ValidationError::new(
                "Schema name cannot be empty",
                ValidationErrorType::SemanticError,
            ));
        }

        if lookup.query.is_empty() {
            return Err(ValidationError::new(
                "Query cannot be empty",
                ValidationErrorType::SemanticError,
            ));
        }

        Ok(ValidationInfo::new())
    }

    fn validate_match(
        &self,
        match_stmt: &MatchFulltext,
    ) -> Result<ValidationInfo, ValidationError> {
        if match_stmt.pattern.is_empty() {
            return Err(ValidationError::new(
                "Match pattern cannot be empty",
                ValidationErrorType::SemanticError,
            ));
        }

        Ok(ValidationInfo::new())
    }
}

impl StatementValidator for FulltextValidator {
    fn validate(
        &mut self,
        ast: Arc<crate::query::parser::ast::Ast>,
        qctx: Arc<QueryContext>,
    ) -> Result<ValidationResult, ValidationError> {
        let stmt = ast.stmt();
        let info = self.validate_impl(stmt, &qctx)?;
        Ok(ValidationResult::success_with_info(info))
    }

    fn statement_type(&self) -> StatementType {
        StatementType::Show
    }

    fn inputs(&self) -> &[ColumnDef] {
        &self.inputs
    }

    fn outputs(&self) -> &[ColumnDef] {
        &self.outputs
    }

    fn is_global_statement(&self) -> bool {
        true
    }

    fn expression_props(&self) -> &ExpressionProps {
        &self.expression_props
    }

    fn user_defined_vars(&self) -> &[String] {
        &[]
    }
}
