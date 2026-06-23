//! Vertices Insert Statement Validator
//! Corresponding to the functionality of NebulaGraph InsertVerticesValidator
//! Verify the semantic correctness of the INSERT VERTICES statement; multiple tags can be inserted.

use std::collections::HashSet;
use std::sync::Arc;

use crate::core::metadata::SchemaManager;
use crate::core::types::expr::contextual::ContextualExpression;
use crate::core::types::expr::Expression;
use crate::core::types::operators::UnaryOperator;
use crate::core::Value;
use crate::query::parser::ast::stmt::{Ast, InsertTarget, TagInsertSpec, VertexRow};
use crate::query::parser::ast::Stmt;
use crate::query::validator::error::{ValidationError, ValidationErrorType};
use crate::query::validator::structs::validation_info::ValidationInfo;
use crate::query::validator::validator_trait::{
    ColumnDef, ExpressionProps, StatementType, StatementValidator, ValidationResult, ValueType,
};
use crate::query::QueryContext;

/// Parameters for vector dimension validation
struct VectorValidationContext<'a> {
    schema_manager: &'a Arc<SchemaManager>,
    space_name: &'a str,
    tag_name: &'a str,
    prop_names: &'a [String],
    values: &'a [ContextualExpression],
    row_idx: usize,
    tag_idx: usize,
}

/// Verified vertex insertion information
#[derive(Debug, Clone)]
pub struct ValidatedInsertVertices {
    pub space_id: u64,
    pub tags: Vec<ValidatedTagInsert>,
    pub vertices: Vec<ValidatedVertex>,
    pub if_not_exists: bool,
}

/// Verified Tag insertion specifications
#[derive(Debug, Clone)]
pub struct ValidatedTagInsert {
    pub tag_id: i32,
    pub tag_name: String,
    pub prop_names: Vec<String>,
}

/// Verified individual vertex
#[derive(Debug, Clone)]
pub struct ValidatedVertex {
    pub vid: Value,
    pub tag_values: Vec<Vec<Value>>,
}

#[derive(Debug)]
pub struct InsertVerticesValidator {
    inputs: Vec<ColumnDef>,
    outputs: Vec<ColumnDef>,
    expression_props: ExpressionProps,
    user_defined_vars: Vec<String>,
    validated_result: Option<ValidatedInsertVertices>,
    schema_manager: Option<Arc<SchemaManager>>,
}

impl InsertVerticesValidator {
    pub fn new() -> Self {
        Self {
            inputs: Vec::new(),
            outputs: Vec::new(),
            expression_props: ExpressionProps::default(),
            user_defined_vars: Vec::new(),
            validated_result: None,
            schema_manager: None,
        }
    }

    pub fn with_schema_manager(mut self, schema_manager: Arc<SchemaManager>) -> Self {
        self.schema_manager = Some(schema_manager);
        self
    }

    pub fn set_schema_manager(&mut self, schema_manager: Arc<SchemaManager>) {
        self.schema_manager = Some(schema_manager);
    }

    /// Verify the Tag name
    fn validate_tag_name(&self, tag_name: &str) -> Result<(), ValidationError> {
        if tag_name.is_empty() {
            return Err(ValidationError::new(
                "Tag name cannot be empty".to_string(),
                ValidationErrorType::SemanticError,
            ));
        }
        Ok(())
    }

    /// Verify the attribute name
    fn validate_property_names(&self, prop_names: &[String]) -> Result<(), ValidationError> {
        let mut seen = HashSet::new();
        for prop_name in prop_names {
            if !seen.insert(prop_name) {
                return Err(ValidationError::new(
                    format!("Duplicate property name '{}' in INSERT VERTICES", prop_name),
                    ValidationErrorType::SemanticError,
                ));
            }
        }
        Ok(())
    }

    /// Verify the data in the vertex row.
    fn validate_vertex_rows(
        &self,
        tags: &[TagInsertSpec],
        rows: &[VertexRow],
        schema_manager: Option<&Arc<SchemaManager>>,
        space_name: &str,
    ) -> Result<(), ValidationError> {
        for (row_idx, row) in rows.iter().enumerate() {
            // Verify the VID format.
            self.validate_vid_expression(&row.vid, row_idx)?;

            // The number of verification values matches the number of tags.
            if row.tag_values.len() != tags.len() {
                return Err(ValidationError::new(
                    format!(
                        "Value count mismatch for vertex {}: expected {} tag value groups, got {}",
                        row_idx + 1,
                        tags.len(),
                        row.tag_values.len()
                    ),
                    ValidationErrorType::SemanticError,
                ));
            }

            // Verify the number of values for each Tag.
            for (tag_idx, (tag_spec, values)) in tags.iter().zip(row.tag_values.iter()).enumerate()
            {
                if values.len() != tag_spec.prop_names.len() {
                    return Err(ValidationError::new(
                        format!(
                            "Value count mismatch for vertex {}, tag {}: expected {} values, got {}",
                            row_idx + 1,
                            tag_idx + 1,
                            tag_spec.prop_names.len(),
                            values.len()
                        ),
                        ValidationErrorType::SemanticError,
                    ));
                }

                // Validate property constraints if schema manager is available
                if let Some(schema_mgr) = schema_manager {
                    // Check that all NOT NULL properties are present in the insert (or have a DEFAULT)
                    if let Ok(Some(tag_info)) = schema_mgr.get_tag(space_name, &tag_spec.tag_name) {
                        for prop_def in &tag_info.properties {
                            // NOT NULL violation only if property is omitted AND has no default value
                            if !prop_def.nullable
                                && !tag_spec.prop_names.contains(&prop_def.name)
                                && prop_def.default.is_none()
                            {
                                return Err(ValidationError::new(
                                    format!(
                                        "NOT NULL constraint violation for property '{}' in tag '{}': property is required and cannot be omitted",
                                        prop_def.name,
                                        tag_spec.tag_name,
                                    ),
                                    ValidationErrorType::SemanticError,
                                ));
                            }
                        }
                    }

                    self.validate_vector_dimensions(VectorValidationContext {
                        schema_manager: schema_mgr,
                        space_name,
                        tag_name: &tag_spec.tag_name,
                        prop_names: &tag_spec.prop_names,
                        values,
                        row_idx,
                        tag_idx,
                    })?;

                    self.validate_property_constraints(VectorValidationContext {
                        schema_manager: schema_mgr,
                        space_name,
                        tag_name: &tag_spec.tag_name,
                        prop_names: &tag_spec.prop_names,
                        values,
                        row_idx,
                        tag_idx,
                    })?;
                }
            }
        }
        Ok(())
    }

    /// Validate vector dimensions match the schema definition
    fn validate_vector_dimensions(
        &self,
        ctx: VectorValidationContext<'_>,
    ) -> Result<(), ValidationError> {
        use crate::core::types::expr::Expression;
        use crate::core::DataType;
        use crate::core::Value;

        // Get tag schema to check property types
        let tag_info = ctx
            .schema_manager
            .get_tag(ctx.space_name, ctx.tag_name)
            .map_err(|e| {
                ValidationError::new(
                    format!("Failed to get tag schema for '{}': {}", ctx.tag_name, e),
                    ValidationErrorType::SemanticError,
                )
            })?;

        let tag_info = tag_info.ok_or_else(|| {
            ValidationError::new(
                format!(
                    "Tag '{}' does not exist in space '{}'",
                    ctx.tag_name, ctx.space_name
                ),
                ValidationErrorType::SemanticError,
            )
        })?;

        // Check each property value
        for (prop_name, value_expr) in ctx.prop_names.iter().zip(ctx.values.iter()) {
            // Find property definition in schema
            if let Some(prop_def) = tag_info.properties.iter().find(|p| &p.name == prop_name) {
                // Check if property is a vector type and extract expected dimension
                let expected_dim = match &prop_def.data_type {
                    DataType::VectorDense(dim) => Some(dim),
                    DataType::VectorSparse(dim) => Some(dim),
                    DataType::Vector => None, // Generic vector type without dimension constraint
                    _ => None,
                };

                if let Some(expected_dim) = expected_dim {
                    // Evaluate the expression to get the actual value and check dimension
                    if let Some(expr) = value_expr.get_expression() {
                        if let Expression::Literal(Value::Vector(vector_val)) = &expr {
                            let actual_dim = vector_val.dimension();

                            if actual_dim != *expected_dim {
                                return Err(ValidationError::new(
                                    format!(
                                        "Vector dimension mismatch for property '{}' in vertex {}, tag {}: expected {}D vector, got {} dimensions",
                                        prop_name,
                                        ctx.row_idx + 1,
                                        ctx.tag_idx + 1,
                                        expected_dim,
                                        actual_dim
                                    ),
                                    ValidationErrorType::SemanticError,
                                ));
                            }
                        } else {
                            // Expression is not a vector literal, skip dimension check
                            continue;
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// Validate property constraints (NOT NULL, type compatibility)
    fn validate_property_constraints(
        &self,
        ctx: VectorValidationContext<'_>,
    ) -> Result<(), ValidationError> {
        let tag_info = ctx
            .schema_manager
            .get_tag(ctx.space_name, ctx.tag_name)
            .map_err(|e| {
                ValidationError::new(
                    format!("Failed to get tag schema for '{}': {}", ctx.tag_name, e),
                    ValidationErrorType::SemanticError,
                )
            })?;

        let tag_info = tag_info.ok_or_else(|| {
            ValidationError::new(
                format!(
                    "Tag '{}' does not exist in space '{}'",
                    ctx.tag_name, ctx.space_name
                ),
                ValidationErrorType::SemanticError,
            )
        })?;

        for (prop_name, value_expr) in ctx.prop_names.iter().zip(ctx.values.iter()) {
            if let Some(prop_def) = tag_info.properties.iter().find(|p| &p.name == prop_name) {
                let value = self.evaluate_expression(value_expr)?;

                // Check NOT NULL constraint
                if !prop_def.nullable && value.is_null() {
                    return Err(ValidationError::new(
                        format!(
                            "NOT NULL constraint violation for property '{}' in vertex {}, tag {}: NULL value is not allowed",
                            prop_name,
                            ctx.row_idx + 1,
                            ctx.tag_idx + 1,
                        ),
                        ValidationErrorType::SemanticError,
                    ));
                }

                // Check type compatibility for literal values
                if !value.is_null() && !value.is_empty() {
                    let value_type = value.get_type();
                    let schema_type = &prop_def.data_type;

                    if !Self::is_type_compatible_for_insert(&value_type, schema_type) {
                        return Err(ValidationError::new(
                            format!(
                                "Type mismatch for property '{}' in vertex {}, tag {}: expected type {}, got value type {}",
                                prop_name,
                                ctx.row_idx + 1,
                                ctx.tag_idx + 1,
                                schema_type,
                                value_type,
                            ),
                            ValidationErrorType::SemanticError,
                        ));
                    }
                }
            }
        }

        Ok(())
    }

    /// Check if a value type is compatible with a schema type for INSERT operations.
    /// Uses strict checking: the value type should match or be implicitly castable.
    fn is_type_compatible_for_insert(
        value_type: &crate::core::DataType,
        schema_type: &crate::core::DataType,
    ) -> bool {
        use crate::core::DataType;

        // Same type is always compatible
        if value_type == schema_type {
            return true;
        }

        // Null is compatible with any type (NOT NULL is checked separately)
        if value_type == &DataType::Null {
            return true;
        }

        // Numeric types are compatible with each other
        let is_numeric = |dt: &DataType| -> bool {
            matches!(
                dt,
                DataType::SmallInt
                    | DataType::Int
                    | DataType::BigInt
                    | DataType::Float
                    | DataType::Double
                    | DataType::Decimal128
            )
        };

        if is_numeric(value_type) && is_numeric(schema_type) {
            return true;
        }

        // String types are compatible
        if value_type == &DataType::String
            && matches!(schema_type, DataType::String | DataType::FixedString(_))
        {
            return true;
        }

        if matches!(value_type, DataType::FixedString(_)) && schema_type == &DataType::String {
            return true;
        }

        // String values are accepted for Date/DateTime/Time/Timestamp types
        // (conversion happens at runtime)
        if value_type == &DataType::String
            && matches!(
                schema_type,
                DataType::Date | DataType::DateTime | DataType::Time | DataType::Timestamp
            )
        {
            return true;
        }

        // Bool is compatible with Bool
        if value_type == &DataType::Bool && schema_type == &DataType::Bool {
            return true;
        }

        // Date/DateTime/Time/Timestamp types
        if value_type == &DataType::Date && schema_type == &DataType::Date {
            return true;
        }
        if value_type == &DataType::DateTime && schema_type == &DataType::DateTime {
            return true;
        }
        if value_type == &DataType::Time && schema_type == &DataType::Time {
            return true;
        }

        // Geography
        if value_type == &DataType::Geography && schema_type == &DataType::Geography {
            return true;
        }

        false
    }

    /// Verify the VID expression
    fn validate_vid_expression(
        &self,
        vid_expr: &ContextualExpression,
        idx: usize,
    ) -> Result<(), ValidationError> {
        self.validate_vid_expression_internal(vid_expr, idx)
    }

    /// Internal method: Verification of the VID expression
    fn validate_vid_expression_internal(
        &self,
        vid_expr: &ContextualExpression,
        idx: usize,
    ) -> Result<(), ValidationError> {
        let expr_meta = match vid_expr.expression() {
            Some(m) => m,
            None => {
                return Err(ValidationError::new(
                    format!("Vertex ID expression is invalid for vertex {}", idx + 1),
                    ValidationErrorType::SemanticError,
                ))
            }
        };
        let expr = expr_meta.inner();

        match expr {
            Expression::Literal(Value::String(s)) => {
                if s.is_empty() {
                    return Err(ValidationError::new(
                        format!("Vertex ID cannot be empty for vertex {}", idx + 1),
                        ValidationErrorType::SemanticError,
                    ));
                }
                Ok(())
            }
            Expression::Literal(Value::Int(_)) => Ok(()),
            Expression::Literal(Value::BigInt(_)) => Ok(()),
            Expression::Variable(_) => Ok(()),
            Expression::Unary {
                op: UnaryOperator::Minus,
                operand,
            } => {
                // Accept Unary(Minus, Literal(Int)) and Unary(Minus, Literal(BigInt))
                match operand.as_ref() {
                    Expression::Literal(Value::Int(_)) => Ok(()),
                    Expression::Literal(Value::BigInt(_)) => Ok(()),
                    _ => Err(ValidationError::new(
                        format!("Invalid vertex ID expression type for vertex {}", idx + 1),
                        ValidationErrorType::SemanticError,
                    )),
                }
            }
            _ => Err(ValidationError::new(
                format!("Invalid vertex ID expression type for vertex {}", idx + 1),
                ValidationErrorType::SemanticError,
            )),
        }
    }

    /// Evaluating an expression to obtain a value
    fn evaluate_expression(&self, expr: &ContextualExpression) -> Result<Value, ValidationError> {
        if let Some(e) = expr.get_expression() {
            self.evaluate_expression_internal(&e)
        } else {
            Ok(Value::Null(crate::core::NullType::Null))
        }
    }

    /// Internal method: Evaluating an expression to determine its value
    fn evaluate_expression_internal(
        &self,
        expr: &crate::core::types::expr::Expression,
    ) -> Result<Value, ValidationError> {
        use crate::core::types::expr::Expression;

        match expr {
            Expression::Literal(val) => Ok(val.clone()),
            Expression::Variable(name) => {
                // Variables are parsed at runtime.
                Ok(Value::String(format!("${}", name)))
            }
            Expression::Unary {
                op: UnaryOperator::Minus,
                operand,
            } => {
                // Evaluate Unary(Minus, Literal(val)) for integer types
                match operand.as_ref() {
                    Expression::Literal(Value::Int(n)) => Ok(Value::Int(-n)),
                    Expression::Literal(Value::BigInt(n)) => Ok(Value::BigInt(-n)),
                    _ => Ok(Value::Null(crate::core::NullType::Null)),
                }
            }
            _ => Ok(Value::Null(crate::core::NullType::Null)),
        }
    }

    /// Generate a column of outputs.
    fn generate_output_columns(&mut self) {
        self.outputs.clear();
        self.outputs.push(ColumnDef {
            name: "INSERTED_VERTICES".to_string(),
            type_: ValueType::List,
        });
    }
}

impl Default for InsertVerticesValidator {
    fn default() -> Self {
        Self::new()
    }
}

/// Implementing the StatementValidator trait
///
/// # Refactoring Changes
/// The `validate` method accepts `Arc<Ast>` and `Arc<QueryContext>` as arguments.
impl StatementValidator for InsertVerticesValidator {
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

        // 2. Obtain the INSERT statement
        let insert_stmt = match &ast.stmt {
            Stmt::Insert(insert_stmt) => insert_stmt,
            _ => {
                return Err(ValidationError::new(
                    "Expected INSERT statement".to_string(),
                    ValidationErrorType::SemanticError,
                ));
            }
        };

        // 3. Verify the type of the statement
        let (tags, values) = match &insert_stmt.target {
            InsertTarget::Vertices { tags, values } => {
                if tags.is_empty() {
                    return Err(ValidationError::new(
                        "INSERT VERTEX must specify at least one tag".to_string(),
                        ValidationErrorType::SemanticError,
                    ));
                }
                (tags.clone(), values.clone())
            }
            InsertTarget::Edge { .. } => {
                return Err(ValidationError::new(
                    "Expected INSERT VERTICES but got INSERT EDGES".to_string(),
                    ValidationErrorType::SemanticError,
                ));
            }
        };

        // 4. Verify all tags.
        for tag_spec in &tags {
            self.validate_tag_name(&tag_spec.tag_name)?;
            self.validate_property_names(&tag_spec.prop_names)?;
        }

        // 5. Verify the data in the vertex row.
        let space_name = qctx.space_name().unwrap_or_else(|| "default".to_string());
        self.validate_vertex_rows(&tags, &values, self.schema_manager.as_ref(), &space_name)?;

        // 6. Convert the verified data
        let mut validated_tags = Vec::new();
        for tag_spec in &tags {
            validated_tags.push(ValidatedTagInsert {
                tag_id: 0, // Obtaining data from the schema at runtime
                tag_name: tag_spec.tag_name.clone(),
                prop_names: tag_spec.prop_names.clone(),
            });
        }

        let mut validated_vertices = Vec::new();
        for row in &values {
            let vid = self.evaluate_expression(&row.vid)?;
            let mut tag_values = Vec::new();
            for tag_vals in &row.tag_values {
                let mut values = Vec::new();
                for v in tag_vals {
                    values.push(self.evaluate_expression(v)?);
                }
                tag_values.push(values);
            }
            validated_vertices.push(ValidatedVertex { vid, tag_values });
        }

        // 7. Obtain the space_id
        let space_id = qctx.space_id().unwrap_or(0);

        // 8. Create the verification results
        let validated = ValidatedInsertVertices {
            space_id,
            tags: validated_tags.clone(),
            vertices: validated_vertices,
            if_not_exists: insert_stmt.if_not_exists,
        };

        self.validated_result = Some(validated);

        // 9. Generate the output column
        self.generate_output_columns();

        // 10. Constructing detailed ValidationInfo
        let mut info = ValidationInfo::new();

        // Add semantic information
        for tag in &validated_tags {
            if !info.semantic_info.referenced_tags.contains(&tag.tag_name) {
                info.semantic_info
                    .referenced_tags
                    .push(tag.tag_name.clone());
            }
        }

        // 11. Return the verification results containing detailed information.
        Ok(ValidationResult::success_with_info(info))
    }

    fn statement_type(&self) -> StatementType {
        StatementType::InsertVertices
    }

    fn inputs(&self) -> &[ColumnDef] {
        &self.inputs
    }

    fn outputs(&self) -> &[ColumnDef] {
        &self.outputs
    }

    fn is_global_statement(&self) -> bool {
        // The `INSERT VERTICES` command is not a global statement; therefore, the space (the database or table in which the vertices are to be inserted) must be selected in advance.
        false
    }

    fn expression_props(&self) -> &ExpressionProps {
        &self.expression_props
    }

    fn user_defined_vars(&self) -> &[String] {
        &self.user_defined_vars
    }
}

#[cfg(test)]
#[allow(clippy::arc_with_non_send_sync)]
mod tests {
    use super::*;
    use crate::core::types::expr::contextual::ContextualExpression;
    use crate::core::Expression;
    use crate::core::Value;
    use crate::query::parser::ast::stmt::InsertStmt;
    use crate::query::parser::ast::Span;
    use crate::query::validator::context::expression_context::ExpressionAnalysisContext;
    use crate::query::QueryRequestContext;
    use std::sync::Arc;

    fn create_contextual_expr(expr: Expression) -> ContextualExpression {
        let ctx = std::sync::Arc::new(ExpressionAnalysisContext::new());
        let meta = crate::core::types::expr::ExpressionMeta::new(expr);
        let id = ctx.register_expression(meta);
        ContextualExpression::new(id, ctx)
    }

    /// Create a QueryContext for testing purposes, which should contain a valid space_id.
    fn create_test_query_context() -> Arc<QueryContext> {
        let rctx = Arc::new(QueryRequestContext::new("TEST".to_string()));
        let mut qctx = QueryContext::new(rctx);
        let space_info = crate::core::types::SpaceInfo::new("test_space".to_string());
        qctx.set_space_info(space_info);
        Arc::new(qctx)
    }

    fn create_test_ast(stmt: Stmt) -> Arc<Ast> {
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        Arc::new(Ast::new(stmt, ctx))
    }

    fn create_insert_vertices_stmt(
        tags: Vec<TagInsertSpec>,
        values: Vec<VertexRow>,
        if_not_exists: bool,
    ) -> InsertStmt {
        InsertStmt {
            span: Span::default(),
            target: InsertTarget::Vertices { tags, values },
            if_not_exists,
        }
    }

    fn create_tag_spec(tag_name: &str, prop_names: Vec<&str>) -> TagInsertSpec {
        TagInsertSpec {
            tag_name: tag_name.to_string(),
            prop_names: prop_names.iter().map(|s| s.to_string()).collect(),
            is_default_props: false,
        }
    }

    fn create_vertex_row(
        vid: ContextualExpression,
        tag_values: Vec<Vec<ContextualExpression>>,
    ) -> VertexRow {
        VertexRow { vid, tag_values }
    }

    #[test]
    fn test_validate_empty_tags() {
        let mut validator = InsertVerticesValidator::new();
        let stmt = create_insert_vertices_stmt(vec![], vec![], false);

        let qctx = create_test_query_context();
        let result = validator.validate(create_test_ast(Stmt::Insert(stmt)), qctx);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err
            .message
            .contains("INSERT VERTEX must specify at least one tag"));
    }

    #[test]
    fn test_validate_empty_tag_name() {
        let mut validator = InsertVerticesValidator::new();
        let stmt = create_insert_vertices_stmt(
            vec![create_tag_spec("", vec!["name"])],
            vec![create_vertex_row(
                create_contextual_expr(Expression::Literal(Value::String("vid1".to_string()))),
                vec![vec![create_contextual_expr(Expression::Literal(
                    Value::String("Alice".to_string()),
                ))]],
            )],
            false,
        );

        let qctx = create_test_query_context();
        let result = validator.validate(create_test_ast(Stmt::Insert(stmt)), qctx);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.message.contains("Tag name cannot be empty"));
    }

    #[test]
    fn test_validate_duplicate_property_names() {
        let mut validator = InsertVerticesValidator::new();
        let stmt = create_insert_vertices_stmt(
            vec![create_tag_spec("person", vec!["name", "name"])],
            vec![create_vertex_row(
                create_contextual_expr(Expression::Literal(Value::String("vid1".to_string()))),
                vec![vec![
                    create_contextual_expr(Expression::Literal(Value::String("Alice".to_string()))),
                    create_contextual_expr(Expression::Literal(Value::String("Bob".to_string()))),
                ]],
            )],
            false,
        );

        let qctx = create_test_query_context();
        let result = validator.validate(create_test_ast(Stmt::Insert(stmt)), qctx);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.message.contains("Duplicate property name"));
    }

    #[test]
    fn test_validate_value_count_mismatch() {
        let mut validator = InsertVerticesValidator::new();
        let stmt = create_insert_vertices_stmt(
            vec![create_tag_spec("person", vec!["name", "age"])],
            vec![create_vertex_row(
                create_contextual_expr(Expression::Literal(Value::String("vid1".to_string()))),
                vec![vec![create_contextual_expr(Expression::Literal(
                    Value::String("Alice".to_string()),
                ))]],
            )],
            false,
        );

        let qctx = create_test_query_context();
        let result = validator.validate(create_test_ast(Stmt::Insert(stmt)), qctx);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.message.contains("Value count mismatch"));
    }

    #[test]
    fn test_validate_empty_vid() {
        let mut validator = InsertVerticesValidator::new();
        let stmt = create_insert_vertices_stmt(
            vec![create_tag_spec("person", vec!["name"])],
            vec![create_vertex_row(
                create_contextual_expr(Expression::Literal(Value::String("".to_string()))),
                vec![vec![create_contextual_expr(Expression::Literal(
                    Value::String("Alice".to_string()),
                ))]],
            )],
            false,
        );

        let qctx = create_test_query_context();
        let result = validator.validate(create_test_ast(Stmt::Insert(stmt)), qctx);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.message.contains("Vertex ID cannot be empty"));
    }

    #[test]
    fn test_validate_valid_single_tag() {
        let mut validator = InsertVerticesValidator::new();
        let stmt = create_insert_vertices_stmt(
            vec![create_tag_spec("person", vec!["name", "age"])],
            vec![create_vertex_row(
                create_contextual_expr(Expression::Literal(Value::String("vid1".to_string()))),
                vec![vec![
                    create_contextual_expr(Expression::Literal(Value::String("Alice".to_string()))),
                    create_contextual_expr(Expression::Literal(Value::Int(30))),
                ]],
            )],
            false,
        );

        let qctx = create_test_query_context();
        let result = validator.validate(create_test_ast(Stmt::Insert(stmt)), qctx);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_valid_multiple_tags() {
        let mut validator = InsertVerticesValidator::new();
        let stmt = create_insert_vertices_stmt(
            vec![
                create_tag_spec("person", vec!["name"]),
                create_tag_spec("employee", vec!["department", "salary"]),
            ],
            vec![create_vertex_row(
                create_contextual_expr(Expression::Literal(Value::String("vid1".to_string()))),
                vec![
                    vec![create_contextual_expr(Expression::Literal(Value::String(
                        "Alice".to_string(),
                    )))],
                    vec![
                        create_contextual_expr(Expression::Literal(Value::String(
                            "Engineering".to_string(),
                        ))),
                        create_contextual_expr(Expression::Literal(Value::Int(50000))),
                    ],
                ],
            )],
            false,
        );

        let qctx = create_test_query_context();
        let result = validator.validate(create_test_ast(Stmt::Insert(stmt)), qctx);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_multiple_vertices() {
        let mut validator = InsertVerticesValidator::new();
        let stmt = create_insert_vertices_stmt(
            vec![create_tag_spec("person", vec!["name"])],
            vec![
                create_vertex_row(
                    create_contextual_expr(Expression::Literal(Value::String("vid1".to_string()))),
                    vec![vec![create_contextual_expr(Expression::Literal(
                        Value::String("Alice".to_string()),
                    ))]],
                ),
                create_vertex_row(
                    create_contextual_expr(Expression::Literal(Value::String("vid2".to_string()))),
                    vec![vec![create_contextual_expr(Expression::Literal(
                        Value::String("Bob".to_string()),
                    ))]],
                ),
            ],
            false,
        );

        let qctx = create_test_query_context();
        let result = validator.validate(create_test_ast(Stmt::Insert(stmt)), qctx);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_variable_vid() {
        let mut validator = InsertVerticesValidator::new();
        let stmt = create_insert_vertices_stmt(
            vec![create_tag_spec("person", vec!["name"])],
            vec![create_vertex_row(
                create_contextual_expr(Expression::Variable("$vid".to_string())),
                vec![vec![create_contextual_expr(Expression::Literal(
                    Value::String("Alice".to_string()),
                ))]],
            )],
            false,
        );

        let qctx = create_test_query_context();
        let result = validator.validate(create_test_ast(Stmt::Insert(stmt)), qctx);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_integer_vid() {
        let mut validator = InsertVerticesValidator::new();
        let stmt = create_insert_vertices_stmt(
            vec![create_tag_spec("person", vec!["name"])],
            vec![create_vertex_row(
                create_contextual_expr(Expression::Literal(Value::Int(123))),
                vec![vec![create_contextual_expr(Expression::Literal(
                    Value::String("Alice".to_string()),
                ))]],
            )],
            false,
        );

        let qctx = create_test_query_context();
        let result = validator.validate(create_test_ast(Stmt::Insert(stmt)), qctx);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_wrong_target_type() {
        let mut validator = InsertVerticesValidator::new();
        let stmt = InsertStmt {
            span: Span::default(),
            target: InsertTarget::Edge {
                edge_name: "friend".to_string(),
                prop_names: vec![],
                edges: vec![],
            },
            if_not_exists: false,
        };

        let qctx = create_test_query_context();
        let result = validator.validate(create_test_ast(Stmt::Insert(stmt)), qctx);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.message, "Expected INSERT VERTICES but got INSERT EDGES");
    }

    #[test]
    fn test_insert_vertices_validator_trait_interface() {
        let validator = InsertVerticesValidator::new();

        assert_eq!(validator.statement_type(), StatementType::InsertVertices);
        assert!(validator.inputs().is_empty());
        assert!(validator.user_defined_vars().is_empty());
    }

    #[test]
    fn test_validate_if_not_exists() {
        let mut validator = InsertVerticesValidator::new();
        let stmt = create_insert_vertices_stmt(
            vec![create_tag_spec("person", vec!["name"])],
            vec![create_vertex_row(
                create_contextual_expr(Expression::Literal(Value::String("vid1".to_string()))),
                vec![vec![create_contextual_expr(Expression::Literal(
                    Value::String("Alice".to_string()),
                ))]],
            )],
            true, // if_not_exists = true
        );

        let qctx = create_test_query_context();
        let result = validator.validate(create_test_ast(Stmt::Insert(stmt)), qctx);
        assert!(result.is_ok());

        // Verify whether `if_not_exists` has been saved correctly.
        assert!(
            validator
                .validated_result
                .as_ref()
                .expect("Failed to get validated result")
                .if_not_exists
        );
    }
}
