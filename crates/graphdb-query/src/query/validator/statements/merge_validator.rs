//! Merge Statement Validator
//! Used to validate MERGE statements (Cypher-style pattern creation/matching)

use std::sync::Arc;

use crate::core::types::expr::contextual::ContextualExpression;
use crate::core::Expression;
use crate::query::parser::ast::stmt::{Ast, MergeStmt, SetClause};
use crate::query::parser::ast::Pattern;
use crate::query::validator::error::{ValidationError, ValidationErrorType};
use crate::query::validator::structs::validation_info::ValidationInfo;
use crate::query::validator::structs::AliasType;
use crate::query::validator::validator_trait::{
    ColumnDef, ExpressionProps, StatementType, StatementValidator, ValidationResult, ValueType,
};
use crate::query::QueryContext;

/// Merge Statement Validator
#[derive(Debug)]
pub struct MergeValidator {
    pattern: Option<Pattern>,
    on_create: Option<SetClause>,
    on_match: Option<SetClause>,
    inputs: Vec<ColumnDef>,
    outputs: Vec<ColumnDef>,
    expr_props: ExpressionProps,
    user_defined_vars: Vec<String>,
}

impl MergeValidator {
    /// Create a new Merge validator.
    pub fn new() -> Self {
        Self {
            pattern: None,
            on_create: None,
            on_match: None,
            inputs: Vec::new(),
            outputs: Vec::new(),
            expr_props: ExpressionProps::default(),
            user_defined_vars: Vec::new(),
        }
    }

    /// Verification mode
    fn validate_pattern(&self, pattern: &Pattern) -> Result<(), ValidationError> {
        use crate::query::parser::ast::Pattern;

        match pattern {
            Pattern::Node(node) => self.validate_node_pattern(node),
            Pattern::Edge(edge) => self.validate_edge_pattern(edge),
            Pattern::Path(path) => self.validate_path_pattern(path),
            Pattern::Variable(var) => self.validate_variable_pattern(var),
        }
    }

    /// Verify node mode
    fn validate_node_pattern(
        &self,
        node: &crate::query::parser::ast::NodePattern,
    ) -> Result<(), ValidationError> {
        // Verify the variable names (if any).
        if let Some(ref var) = node.variable {
            self.validate_variable_name(var)?;
        }

        // Verify the tags (if any).
        for label in &node.labels {
            self.validate_label_name(label)?;
        }

        // Verify the attributes (if any).
        if let Some(ref props) = node.properties {
            self.validate_properties(props)?;
        }

        Ok(())
    }

    /// Verify the border mode
    fn validate_edge_pattern(
        &self,
        edge: &crate::query::parser::ast::EdgePattern,
    ) -> Result<(), ValidationError> {
        // Verify the variable names (if any).
        if let Some(ref var) = edge.variable {
            self.validate_variable_name(var)?;
        }

        // Verify the edge type (if any).
        for type_ in &edge.edge_types {
            self.validate_edge_type(type_)?;
        }

        // Verify the attributes (if any).
        if let Some(ref props) = edge.properties {
            self.validate_properties(props)?;
        }

        Ok(())
    }

    /// Verify the path pattern.
    fn validate_path_pattern(
        &self,
        path: &crate::query::parser::ast::PathPattern,
    ) -> Result<(), ValidationError> {
        // Verify each element in the path.
        for element in &path.elements {
            self.validate_path_element(element)?;
        }
        Ok(())
    }

    /// Verify the path elements
    fn validate_path_element(
        &self,
        element: &crate::query::parser::ast::PathElement,
    ) -> Result<(), ValidationError> {
        use crate::query::parser::ast::PathElement;

        match element {
            PathElement::Node(node) => self.validate_node_pattern(node),
            PathElement::Edge(edge) => self.validate_edge_pattern(edge),
            PathElement::Alternative(patterns) => {
                for pattern in patterns {
                    self.validate_pattern(pattern)?;
                }
                Ok(())
            }
            PathElement::Optional(inner) => self.validate_path_element(inner),
            PathElement::Repeated(inner, _) => self.validate_path_element(inner),
        }
    }

    /// Verify the variable pattern.
    fn validate_variable_pattern(
        &self,
        var: &crate::query::parser::ast::VariablePattern,
    ) -> Result<(), ValidationError> {
        // Verify the variable names.
        self.validate_variable_name(&var.name)
    }

    /// Verify the variable names.
    fn validate_variable_name(&self, name: &str) -> Result<(), ValidationError> {
        if name.is_empty() {
            return Err(ValidationError::new(
                "Variable name cannot be empty".to_string(),
                ValidationErrorType::SemanticError,
            ));
        }

        // Variable names must start with a letter or an underscore.
        let first_char = name
            .chars()
            .next()
            .expect("The variable name is verified to be non-null");
        if !first_char.is_alphabetic() && first_char != '_' {
            return Err(ValidationError::new(
                format!(
                    "Variable name must start with a letter or underscore: {}",
                    name
                ),
                ValidationErrorType::SemanticError,
            ));
        }

        Ok(())
    }

    /// Verify the attribute name.
    fn validate_property_name(&self, name: &str) -> Result<(), ValidationError> {
        if name.is_empty() {
            return Err(ValidationError::new(
                "Property name cannot be empty".to_string(),
                ValidationErrorType::SemanticError,
            ));
        }

        // The attribute name must start with a letter or an underscore.
        let first_char = name
            .chars()
            .next()
            .expect("Attribute name is verified to be non-null");
        if !first_char.is_alphabetic() && first_char != '_' {
            return Err(ValidationError::new(
                format!(
                    "Property name must start with a letter or underscore: {}",
                    name
                ),
                ValidationErrorType::SemanticError,
            ));
        }

        Ok(())
    }

    /// Verify the tag name.
    fn validate_label_name(&self, name: &str) -> Result<(), ValidationError> {
        if name.is_empty() {
            return Err(ValidationError::new(
                "Label name cannot be empty".to_string(),
                ValidationErrorType::SemanticError,
            ));
        }
        Ok(())
    }

    /// Verify the edge type
    fn validate_edge_type(&self, type_: &str) -> Result<(), ValidationError> {
        if type_.is_empty() {
            return Err(ValidationError::new(
                "Edge type cannot be empty".to_string(),
                ValidationErrorType::SemanticError,
            ));
        }
        Ok(())
    }

    /// Verify the attribute expression
    fn validate_properties(&self, props: &ContextualExpression) -> Result<(), ValidationError> {
        if let Some(e) = props.get_expression() {
            self.validate_properties_internal(&e)
        } else {
            Err(ValidationError::new(
                "Invalid attribute expression".to_string(),
                ValidationErrorType::SemanticError,
            ))
        }
    }

    fn validate_properties_internal(&self, props: &Expression) -> Result<(), ValidationError> {
        match props {
            Expression::Map(items) => {
                if items.is_empty() {
                    return Err(ValidationError::new(
                        "Attributes cannot be null".to_string(),
                        ValidationErrorType::SemanticError,
                    ));
                }

                for (key, value) in items {
                    self.validate_property_name(key)?;
                    self.validate_expression_recursive(value)?;
                }
                Ok(())
            }
            _ => Err(ValidationError::new(
                "Attribute must be a mapping type".to_string(),
                ValidationErrorType::SemanticError,
            )),
        }
    }

    /// Verify the attribute values
    fn validate_property_value(&self, value: &ContextualExpression) -> Result<(), ValidationError> {
        if let Some(e) = value.get_expression() {
            self.validate_property_value_internal(&e)
        } else {
            Err(ValidationError::new(
                "Invalid attribute value expression".to_string(),
                ValidationErrorType::SemanticError,
            ))
        }
    }

    fn validate_property_value_internal(&self, value: &Expression) -> Result<(), ValidationError> {
        self.validate_expression_recursive(value)
    }

    /// Recursive verification of expressions
    fn validate_expression_recursive(&self, expr: &Expression) -> Result<(), ValidationError> {
        match expr {
            Expression::Literal(_) => Ok(()),
            Expression::Variable(_) => Ok(()),
            Expression::Function { args, .. } => {
                if args.is_empty() {
                    return Err(ValidationError::new(
                        "Function calls must have arguments".to_string(),
                        ValidationErrorType::SemanticError,
                    ));
                }
                for arg in args.iter() {
                    self.validate_expression_recursive(arg)?;
                }
                Ok(())
            }
            Expression::Binary { left, right, .. } => {
                self.validate_expression_recursive(left)?;
                self.validate_expression_recursive(right)?;
                Ok(())
            }
            Expression::Unary { operand, .. } => {
                self.validate_expression_recursive(operand)?;
                Ok(())
            }
            Expression::List(items) => {
                for item in items.iter() {
                    self.validate_expression_recursive(item)?;
                }
                Ok(())
            }
            Expression::Map(items) => {
                for (_, value) in items.iter() {
                    self.validate_expression_recursive(value)?;
                }
                Ok(())
            }
            Expression::Case {
                test_expr,
                conditions,
                default,
            } => {
                if let Some(test) = test_expr {
                    self.validate_expression_recursive(test)?;
                }
                for (cond, val) in conditions.iter() {
                    self.validate_expression_recursive(cond)?;
                    self.validate_expression_recursive(val)?;
                }
                if let Some(def) = default {
                    self.validate_expression_recursive(def)?;
                }
                Ok(())
            }
            Expression::Property { object, .. } => {
                self.validate_expression_recursive(object)?;
                Ok(())
            }
            Expression::Aggregate { arg, .. } => {
                self.validate_expression_recursive(arg)?;
                Ok(())
            }
            Expression::TypeCast { expression, .. } => {
                self.validate_expression_recursive(expression)?;
                Ok(())
            }
            Expression::Subscript {
                collection, index, ..
            } => {
                self.validate_expression_recursive(collection)?;
                self.validate_expression_recursive(index)?;
                Ok(())
            }
            Expression::Range {
                collection,
                start,
                end,
                ..
            } => {
                self.validate_expression_recursive(collection)?;
                if let Some(s) = start {
                    self.validate_expression_recursive(s)?;
                }
                if let Some(e) = end {
                    self.validate_expression_recursive(e)?;
                }
                Ok(())
            }
            Expression::Path(exprs) => {
                for expr in exprs.iter() {
                    self.validate_expression_recursive(expr)?;
                }
                Ok(())
            }
            Expression::Label(_) => Ok(()),
            Expression::ListComprehension {
                source,
                filter,
                map,
                ..
            } => {
                self.validate_expression_recursive(source)?;
                if let Some(f) = filter {
                    self.validate_expression_recursive(f)?;
                }
                if let Some(m) = map {
                    self.validate_expression_recursive(m)?;
                }
                Ok(())
            }
            Expression::LabelTagProperty { tag, .. } => {
                self.validate_expression_recursive(tag)?;
                Ok(())
            }
            Expression::TagProperty { .. } => Ok(()),
            Expression::EdgeProperty { .. } => Ok(()),
            Expression::Predicate { args, .. } => {
                for arg in args.iter() {
                    self.validate_expression_recursive(arg)?;
                }
                Ok(())
            }
            Expression::Reduce {
                initial,
                source,
                mapping,
                ..
            } => {
                self.validate_expression_recursive(initial)?;
                self.validate_expression_recursive(source)?;
                self.validate_expression_recursive(mapping)?;
                Ok(())
            }
            Expression::PathBuild(exprs) => {
                for expr in exprs.iter() {
                    self.validate_expression_recursive(expr)?;
                }
                Ok(())
            }
            Expression::Parameter(_) => Ok(()),
            Expression::Vector(_) => Ok(()),
        }
    }

    /// Verify the SET statement
    fn validate_set_clause(&self, set_clause: &SetClause) -> Result<(), ValidationError> {
        for assignment in &set_clause.assignments {
            // Verify the attribute name
            if assignment.property.is_empty() {
                return Err(ValidationError::new(
                    "Property name cannot be empty".to_string(),
                    ValidationErrorType::SemanticError,
                ));
            }

            // Verify the assigned value
            self.validate_property_value(&assignment.value)?;
        }

        Ok(())
    }

    fn validate_impl(&mut self, stmt: &MergeStmt) -> Result<(), ValidationError> {
        // Verification mode
        self.validate_pattern(&stmt.pattern)?;

        // Verify the ON CREATE clause
        if let Some(ref on_create) = stmt.on_create {
            self.validate_set_clause(on_create)?;
        }

        // Verify the ON MATCH clause
        if let Some(ref on_match) = stmt.on_match {
            self.validate_set_clause(on_match)?;
        }

        // Save the information.
        self.pattern = Some(stmt.pattern.clone());
        self.on_create = stmt.on_create.clone();
        self.on_match = stmt.on_match.clone();

        // Set the output columns
        self.setup_outputs();

        Ok(())
    }

    fn setup_outputs(&mut self) {
        // The MERGE statement returns the created/matched nodes or edges.
        self.outputs = vec![ColumnDef {
            name: "result".to_string(),
            type_: ValueType::Vertex, // It could be a vertex or an edge.
        }];
    }

    fn extract_pattern_info(&self, pattern: &Pattern, info: &mut ValidationInfo) {
        use crate::query::parser::ast::Pattern;

        match pattern {
            Pattern::Node(node) => {
                if let Some(ref var) = node.variable {
                    info.add_alias(var.clone(), AliasType::Node);
                }
                for label in &node.labels {
                    if !info.semantic_info.referenced_tags.contains(label) {
                        info.semantic_info.referenced_tags.push(label.clone());
                    }
                }
            }
            Pattern::Edge(edge) => {
                if let Some(ref var) = edge.variable {
                    info.add_alias(var.clone(), AliasType::Edge);
                }
                for edge_type in &edge.edge_types {
                    if !info.semantic_info.referenced_edges.contains(edge_type) {
                        info.semantic_info.referenced_edges.push(edge_type.clone());
                    }
                }
            }
            Pattern::Path(path) => {
                for element in &path.elements {
                    self.extract_path_element_info(element, info);
                }
            }
            Pattern::Variable(var) => {
                info.add_alias(var.name.clone(), AliasType::Variable);
            }
        }
    }

    fn extract_path_element_info(
        &self,
        element: &crate::query::parser::ast::PathElement,
        info: &mut ValidationInfo,
    ) {
        use crate::query::parser::ast::PathElement;

        match element {
            PathElement::Node(node) => {
                if let Some(ref var) = node.variable {
                    info.add_alias(var.clone(), AliasType::Node);
                }
                for label in &node.labels {
                    if !info.semantic_info.referenced_tags.contains(label) {
                        info.semantic_info.referenced_tags.push(label.clone());
                    }
                }
            }
            PathElement::Edge(edge) => {
                if let Some(ref var) = edge.variable {
                    info.add_alias(var.clone(), AliasType::Edge);
                }
                for edge_type in &edge.edge_types {
                    if !info.semantic_info.referenced_edges.contains(edge_type) {
                        info.semantic_info.referenced_edges.push(edge_type.clone());
                    }
                }
            }
            PathElement::Alternative(patterns) => {
                for pattern in patterns {
                    self.extract_pattern_info(pattern, info);
                }
            }
            PathElement::Optional(inner) => {
                self.extract_path_element_info(inner, info);
            }
            PathElement::Repeated(inner, _) => {
                self.extract_path_element_info(inner, info);
            }
        }
    }
}

impl Default for MergeValidator {
    fn default() -> Self {
        Self::new()
    }
}

/// Implementing the StatementValidator trait
///
/// # Refactoring changes
/// The `validate` method accepts `Arc<Ast>` and `Arc<QueryContext>` as parameters.
impl StatementValidator for MergeValidator {
    fn validate(
        &mut self,
        ast: Arc<Ast>,
        _qctx: Arc<QueryContext>,
    ) -> Result<ValidationResult, ValidationError> {
        let merge_stmt = match &ast.stmt {
            crate::query::parser::ast::Stmt::Merge(merge_stmt) => merge_stmt,
            _ => {
                return Err(ValidationError::new(
                    "Expected MERGE statement".to_string(),
                    ValidationErrorType::SemanticError,
                ));
            }
        };

        self.validate_impl(merge_stmt)?;

        let mut info = ValidationInfo::new();

        if let Some(ref pattern) = self.pattern {
            self.extract_pattern_info(pattern, &mut info);
        }

        Ok(ValidationResult::success_with_info(info))
    }

    fn statement_type(&self) -> StatementType {
        StatementType::Merge
    }

    fn inputs(&self) -> &[ColumnDef] {
        &self.inputs
    }

    fn outputs(&self) -> &[ColumnDef] {
        &self.outputs
    }

    fn is_global_statement(&self) -> bool {
        // The `MERGE` statement is not a global statement; it needs to be executed in a specific context (i.e., within a particular scope or environment).
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

    #[test]
    fn test_merge_validator_new() {
        let validator = MergeValidator::new();
        assert_eq!(validator.statement_type(), StatementType::Merge);
        assert!(!validator.is_global_statement());
    }

    #[test]
    fn test_validate_variable_name() {
        let validator = MergeValidator::new();

        // Valid variable names
        assert!(validator.validate_variable_name("n").is_ok());
        assert!(validator.validate_variable_name("node1").is_ok());
        assert!(validator.validate_variable_name("_node").is_ok());

        // Invalid variable name
        assert!(validator.validate_variable_name("").is_err());
        assert!(validator.validate_variable_name("1node").is_err());
    }

    #[test]
    fn test_validate_label_name() {
        let validator = MergeValidator::new();

        // Valid tag names
        assert!(validator.validate_label_name("Person").is_ok());

        // Invalid tag name
        assert!(validator.validate_label_name("").is_err());
    }
}
