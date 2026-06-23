//! Schema validation tool module
//!
//! Provide a complete Schema validation function that meets the standards of NebulaGraph's SchemaUtil.
//! Schema-level validation for DML statements (INSERT, UPDATE, DELETE)
//!
//! This document has been updated in accordance with the new validator framework.
//! 1. All original functions have been retained.
//! - Attribute existence verification
//! - Attribute type validation
//! - Empty value check
//! - Fill in default values
//! - VID type validation
//! - Expression evaluation
//! - Automatic Schema creation
//! 2. Integration support for the new verification system has been added.
//! 3. Use Arc to manage SchemaManager in order to support the new system.

use std::sync::Arc;

use crate::core::metadata::SchemaManager;
use crate::core::types::expr::contextual::ContextualExpression;
use crate::core::types::operators::UnaryOperator;
use crate::core::types::{DataType, EdgeTypeInfo, PropertyDef, TagInfo};
use crate::core::Value;
use crate::query::validator::error::{ValidationError as CoreValidationError, ValidationErrorType};
use crate::query::validator::validator_trait::ValueType;

/// Edge type definition for auto-creation with source, destination and properties
type EdgeTypeAutoCreateDef = (String, String, String, Vec<(String, Value)>);

/// Schema Validator
/// Encapsulate all the validation logic related to the Schema.
///
/// This is a tool validator; it does not directly implement the StatementValidator trait.
/// It is used by other statement validators (such as InsertVerticesValidator, UpdateValidator, etc.).
#[derive(Debug, Clone)]
pub struct SchemaValidator {
    schema_manager: Arc<SchemaManager>,
}

/// Edge type creation parameters for auto-creation
pub struct AutoCreateEdgeTypeParams<'a> {
    pub space_name: &'a str,
    pub edge_type_name: &'a str,
    pub src_tag_name: &'a str,
    pub dst_tag_name: &'a str,
    pub properties: &'a [(String, Value)],
}

/// Automated creation of missing Edge Types in batches
pub struct AutoCreateMissingEdgeTypesParam<'a> {
    pub space_name: &'a str,
    pub edge_types: &'a [AutoCreateEdgeTypeParams<'a>],
}

impl SchemaValidator {
    /// Create a new Schema validator.
    pub fn new(schema_manager: Arc<SchemaManager>) -> Self {
        Self { schema_manager }
    }

    /// Obtaining the underlying SchemaManager
    pub fn get_schema_manager(&self) -> &SchemaManager {
        self.schema_manager.as_ref()
    }

    /// Obtain Arc<SchemaManager>
    pub fn schema_manager_arc(&self) -> Arc<SchemaManager> {
        self.schema_manager.clone()
    }

    /// Obtaining Tag information
    pub fn get_tag(
        &self,
        space_name: &str,
        tag_name: &str,
    ) -> Result<Option<TagInfo>, CoreValidationError> {
        self.schema_manager
            .as_ref()
            .get_tag(space_name, tag_name)
            .map_err(|e| {
                CoreValidationError::new(
                    format!("Failed to get Tag: {}", e),
                    ValidationErrorType::SemanticError,
                )
            })
    }

    /// Obtaining EdgeType information
    pub fn get_edge_type(
        &self,
        space_name: &str,
        edge_type_name: &str,
    ) -> Result<Option<EdgeTypeInfo>, CoreValidationError> {
        self.schema_manager
            .as_ref()
            .get_edge_type(space_name, edge_type_name)
            .map_err(|e| {
                CoreValidationError::new(
                    format!("Failed to get Edge Type: {}", e),
                    ValidationErrorType::SemanticError,
                )
            })
    }

    /// Retrieve all EdgeTypes of Space
    pub fn get_all_edge_types(
        &self,
        space_name: &str,
    ) -> Result<Vec<EdgeTypeInfo>, CoreValidationError> {
        self.schema_manager
            .as_ref()
            .list_edge_types(space_name)
            .map_err(|e| {
                CoreValidationError::new(
                    format!("Failed to get Edge Type list: {}", e),
                    ValidationErrorType::SemanticError,
                )
            })
    }

    /// Verify whether the attribute name exists in the Schema.
    pub fn validate_property_exists(
        &self,
        prop_name: &str,
        properties: &[PropertyDef],
    ) -> Result<(), CoreValidationError> {
        if !properties.iter().any(|p| p.name == prop_name) {
            return Err(CoreValidationError::new(
                format!("Attribute '{}' not present in Schema", prop_name),
                ValidationErrorType::SemanticError,
            ));
        }
        Ok(())
    }

    /// Retrieve the attribute definition based on the attribute name.
    pub fn get_property_def<'b>(
        &self,
        prop_name: &str,
        properties: &'b [PropertyDef],
    ) -> Option<&'b PropertyDef> {
        properties.iter().find(|p| p.name == prop_name)
    }

    /// Verify whether the type of the attribute value matches the expected type.
    pub fn validate_property_type(
        &self,
        prop_name: &str,
        expected_type: &DataType,
        value: &Value,
    ) -> Result<(), CoreValidationError> {
        // Special handling of NULL values (constrained by the validate_not_null function)
        if matches!(value, Value::Null(_)) {
            return Ok(());
        }

        let actual_type = value.get_type();

        if !Self::is_type_compatible(expected_type, &actual_type) {
            return Err(CoreValidationError::new(
                format!(
                    "Attribute '{}' Desired type {:?} , actual type {:?}",
                    prop_name, expected_type, actual_type
                ),
                ValidationErrorType::TypeMismatch,
            ));
        }
        Ok(())
    }

    /// Check the compatibility of the types.
    /// Supports some implicit type conversions.
    pub fn is_type_compatible(expected: &DataType, actual: &DataType) -> bool {
        match (expected, actual) {
            // Exact match
            (a, b) if a == b => true,

            // The integer type is compatible.
            (DataType::SmallInt, DataType::Int) => true,
            (DataType::SmallInt, DataType::BigInt) => true,
            (DataType::Int, DataType::SmallInt) => true,
            (DataType::Int, DataType::BigInt) => true,
            (DataType::BigInt, DataType::SmallInt) => true,
            (DataType::BigInt, DataType::Int) => true,

            // Floating-point number compatibility
            (DataType::Float, DataType::Double) => true,
            (DataType::Double, DataType::Float) => true,

            // VID is compatible with various types.
            (DataType::VID, DataType::String) => true,
            (DataType::VID, DataType::SmallInt) => true,
            (DataType::VID, DataType::Int) => true,
            (DataType::VID, DataType::BigInt) => true,
            (DataType::VID, DataType::FixedString(_)) => true,

            // FixedString is compatible with String.
            (DataType::FixedString(_), DataType::String) => true,
            (DataType::String, DataType::FixedString(_)) => true,

            // The value NULL can be assigned to any data type (before verifying that the value is not empty).
            (_, DataType::Null) => true,

            // The other conditions do not match.
            _ => false,
        }
    }

    /// Convert DataType to ValueType (for the new validator framework)
    pub fn data_type_to_value_type(data_type: &DataType) -> ValueType {
        match data_type {
            DataType::Bool => ValueType::Bool,
            DataType::SmallInt | DataType::Int | DataType::BigInt => ValueType::Int,
            DataType::Float | DataType::Double => ValueType::Float,
            DataType::String | DataType::FixedString(_) => ValueType::String,
            DataType::Date => ValueType::Date,
            DataType::Time => ValueType::Time,
            DataType::DateTime => ValueType::DateTime,
            DataType::Null => ValueType::Null,
            DataType::Vertex => ValueType::Vertex,
            DataType::Edge => ValueType::Edge,
            DataType::Path => ValueType::Path,
            DataType::List => ValueType::List,
            DataType::Map => ValueType::Map,
            DataType::Set => ValueType::Set,
            _ => ValueType::Unknown,
        }
    }

    /// Verify the non-null constraint
    pub fn validate_not_null(
        &self,
        prop_name: &str,
        prop_def: &PropertyDef,
        value: &Value,
    ) -> Result<(), CoreValidationError> {
        if !prop_def.nullable && matches!(value, Value::Null(_)) {
            return Err(CoreValidationError::new(
                format!("The non-null attribute '{}' cannot be NULL.", prop_name),
                ValidationErrorType::ConstraintViolation,
            ));
        }
        Ok(())
    }

    /// Get the default value of the attribute
    pub fn get_default_value(&self, prop_def: &PropertyDef) -> Option<Value> {
        prop_def.default.clone()
    }

    /// Fill in the default values
    /// Fill in default values or NULL for the attributes that have not been provided.
    pub fn fill_default_values(
        &self,
        properties: &[PropertyDef],
        provided_props: &[(String, Value)],
    ) -> Result<Vec<(String, Value)>, CoreValidationError> {
        let mut result = provided_props.to_vec();

        for prop_def in properties {
            if !result.iter().any(|(name, _)| name == &prop_def.name) {
                // The attribute was not provided; attempting to use the default value.
                if let Some(default) = &prop_def.default {
                    result.push((prop_def.name.clone(), default.clone()));
                } else if !prop_def.nullable {
                    return Err(CoreValidationError::new(
                        format!(
                            "Attribute '{}' is not provided and has no default value, and is not allowed to be NULL.",
                            prop_def.name
                        ),
                        ValidationErrorType::ConstraintViolation,
                    ));
                } else {
                    // The property is `nullable` and has no default value; therefore, it should be set to `NULL`.
                    result.push((
                        prop_def.name.clone(),
                        Value::Null(crate::core::NullType::default()),
                    ));
                }
            }
        }

        Ok(result)
    }

    /// Verify the VID type
    pub fn validate_vid(
        &self,
        vid: &Value,
        expected_type: &DataType,
    ) -> Result<(), CoreValidationError> {
        match expected_type {
            DataType::String | DataType::FixedString(_) => {
                if !matches!(vid, Value::String(_)) {
                    return Err(CoreValidationError::new(
                        format!("VID Expected string type, actually {:?}", vid.get_type()),
                        ValidationErrorType::TypeMismatch,
                    ));
                }
            }
            DataType::SmallInt | DataType::Int | DataType::BigInt => {
                if !matches!(vid, Value::SmallInt(_) | Value::Int(_) | Value::BigInt(_)) {
                    return Err(CoreValidationError::new(
                        format!("VID Expected integer type, actually {:?}", vid.get_type()),
                        ValidationErrorType::TypeMismatch,
                    ));
                }
            }
            DataType::VID => {
                // The VID type accepts a variety of formats.
                if !matches!(
                    vid,
                    Value::String(_) | Value::SmallInt(_) | Value::Int(_) | Value::BigInt(_)
                ) {
                    return Err(CoreValidationError::new(
                        format!("VID type incompatibility: {:?}", vid.get_type()),
                        ValidationErrorType::TypeMismatch,
                    ));
                }
            }
            _ => {
                return Err(CoreValidationError::new(
                    format!("Unsupported VID types: {:?}", expected_type),
                    ValidationErrorType::TypeMismatch,
                ));
            }
        }
        Ok(())
    }

    /// Unified verification of VID expressions
    /// Verify the expression based on the `vid_type` of `Space` to ensure that the types match.
    ///
    /// Parameters:
    /// - expr: The VID expression
    /// - `vid_type`: The VID type defined by the Space standard.
    /// - Role: Description of the VID role (e.g., "source", "destination", "vertex")
    ///
    /// Please provide the text you would like to have translated.
    /// - Ok(()) 验证通过
    /// - Err(ValidationError) 验证失败
    pub fn validate_vid_expr(
        &self,
        expr: &ContextualExpression,
        vid_type: &DataType,
        role: &str,
    ) -> Result<(), CoreValidationError> {
        if let Some(e) = expr.get_expression() {
            self.validate_vid_expr_internal(&e, vid_type, role)
        } else {
            Err(CoreValidationError::new(
                format!("{} vertex ID expression is invalid", role),
                ValidationErrorType::SemanticError,
            ))
        }
    }

    /// Internal method: Validation of the VID expression
    fn validate_vid_expr_internal(
        &self,
        expr: &crate::core::types::expr::Expression,
        vid_type: &DataType,
        role: &str,
    ) -> Result<(), CoreValidationError> {
        use crate::core::types::expr::Expression;

        match expr {
            Expression::Literal(value) => {
                // Literal values need to be checked for null values and type mismatches.
                match value {
                    Value::String(s) => {
                        if s.is_empty() {
                            return Err(CoreValidationError::new(
                                format!("{} vertex ID cannot be an empty string.", role),
                                ValidationErrorType::SemanticError,
                            ));
                        }
                        // Check whether the type of the data matches the required format.
                        if !matches!(
                            vid_type,
                            DataType::String | DataType::FixedString(_) | DataType::VID
                        ) {
                            return Err(CoreValidationError::new(
                                format!(
                                    "{} vertex ID expects {:?} type, actually a string",
                                    role, vid_type
                                ),
                                ValidationErrorType::TypeMismatch,
                            ));
                        }
                    }
                    Value::SmallInt(_) | Value::Int(_) | Value::BigInt(_) => {
                        // Check whether the types of the data match.
                        if !matches!(
                            vid_type,
                            DataType::SmallInt | DataType::Int | DataType::BigInt | DataType::VID
                        ) {
                            return Err(CoreValidationError::new(
                                format!(
                                    "{} vertex ID expectation {:?} type, actually an integer",
                                    role, vid_type
                                ),
                                ValidationErrorType::TypeMismatch,
                            ));
                        }
                    }
                    _ => {
                        return Err(CoreValidationError::new(
                            format!("{} vertex ID must be a string or integer constant.", role),
                            ValidationErrorType::TypeMismatch,
                        ));
                    }
                }
                Ok(())
            }
            Expression::Variable(_) => {
                // The specific value of the variable cannot be determined during the validation phase; it is assumed to be valid.
                Ok(())
            }
            Expression::Unary {
                op: UnaryOperator::Minus,
                operand,
            } => {
                // Accept Unary(Minus, Literal(Int)) and Unary(Minus, Literal(BigInt))
                match operand.as_ref() {
                    Expression::Literal(Value::SmallInt(_))
                    | Expression::Literal(Value::Int(_))
                    | Expression::Literal(Value::BigInt(_)) => {
                        if !matches!(
                            vid_type,
                            DataType::SmallInt | DataType::Int | DataType::BigInt | DataType::VID
                        ) {
                            return Err(CoreValidationError::new(
                                format!(
                                    "{} vertex ID expectation {:?} type, actually a negative integer",
                                    role, vid_type
                                ),
                                ValidationErrorType::TypeMismatch,
                            ));
                        }
                        Ok(())
                    }
                    _ => Err(CoreValidationError::new(
                        format!("{} vertex ID must be a constant or variable.", role),
                        ValidationErrorType::SemanticError,
                    )),
                }
            }
            _ => Err(CoreValidationError::new(
                format!("{} vertex ID must be a constant or variable.", role),
                ValidationErrorType::SemanticError,
            )),
        }
    }

    /// Verify the list of attribute values.
    /// Verify that all attributes exist, their types match, and that the values are not empty.
    pub fn validate_properties(
        &self,
        properties: &[PropertyDef],
        prop_values: &[(String, Value)],
    ) -> Result<Vec<(String, Value)>, CoreValidationError> {
        let mut result = Vec::new();

        for (prop_name, value) in prop_values {
            // Verify that the attribute exists.
            let prop_def = self
                .get_property_def(prop_name, properties)
                .ok_or_else(|| {
                    CoreValidationError::new(
                        format!("Attribute '{}' does not exist", prop_name),
                        ValidationErrorType::SemanticError,
                    )
                })?;

            // Verify the non-null constraint
            self.validate_not_null(prop_name, prop_def, value)?;

            // Verification type
            self.validate_property_type(prop_name, &prop_def.data_type, value)?;

            result.push((prop_name.clone(), value.clone()));
        }

        // Fill in the default values
        self.fill_default_values(properties, &result)
    }

    /// Verify whether the expression represents a computable value.
    /// Used to check VID and attribute value expressions.
    pub fn is_evaluable_expr(&self, expr: &ContextualExpression) -> bool {
        if let Some(e) = expr.get_expression() {
            self.is_evaluable_expr_internal(&e)
        } else {
            false
        }
    }

    /// Internal method: Verifying whether an expression represents a computable value
    fn is_evaluable_expr_internal(&self, expr: &crate::core::types::expr::Expression) -> bool {
        use crate::core::types::expr::Expression;
        match expr {
            Expression::Literal(_) => true,
            Expression::Variable(_) => true,
            Expression::List(list) => list.iter().all(|e| self.is_evaluable_expr_internal(e)),
            Expression::Map(map) => map.iter().all(|(_, e)| self.is_evaluable_expr_internal(e)),
            // Function calls that are deterministic can also be accepted.
            Expression::Function { .. } => true,
            _ => false,
        }
    }

    /// Evaluating an expression to obtain a value
    /// Only constant expressions are allowed.
    pub fn evaluate_expression(
        &self,
        expr: &ContextualExpression,
    ) -> Result<Value, CoreValidationError> {
        if let Some(e) = expr.get_expression() {
            self.evaluate_expression_internal(&e)
        } else {
            Err(CoreValidationError::new(
                "Invalid expression".to_string(),
                ValidationErrorType::SemanticError,
            ))
        }
    }

    /// Internal method: Evaluating an expression to obtain a value
    fn evaluate_expression_internal(
        &self,
        expr: &crate::core::types::expr::Expression,
    ) -> Result<Value, CoreValidationError> {
        use crate::core::types::expr::Expression;
        match expr {
            Expression::Literal(value) => Ok(value.clone()),
            Expression::Variable(name) => {
                // Variables cannot be evaluated during the validation phase; a special marker is returned instead.
                Ok(Value::String(format!("${}", name)))
            }
            Expression::List(list) => {
                let values: Result<Vec<_>, _> = list
                    .iter()
                    .map(|e| self.evaluate_expression_internal(e))
                    .collect();
                Ok(Value::list(crate::core::value::List { values: values? }))
            }
            Expression::Map(map) => {
                let mut result = std::collections::HashMap::new();
                for (k, v) in map {
                    result.insert(k.clone(), self.evaluate_expression_internal(v)?);
                }
                Ok(Value::map(result))
            }
            _ => Err(CoreValidationError::new(
                format!("Unable to evaluate expression: {:?}", expr),
                ValidationErrorType::SemanticError,
            )),
        }
    }

    /// Automatically create a Tag (if it does not exist).
    /// Infer the Schema of the Tag based on the provided attributes.
    pub fn auto_create_tag(
        &self,
        space_name: &str,
        tag_name: &str,
        properties: &[(String, Value)],
    ) -> Result<TagInfo, CoreValidationError> {
        // Check whether the tag already exists.
        if let Some(existing) = self
            .schema_manager
            .as_ref()
            .get_tag(space_name, tag_name)
            .map_err(|e| {
                CoreValidationError::new(
                    format!("Failed to get Tag: {}", e),
                    ValidationErrorType::SemanticError,
                )
            })?
        {
            return Ok(existing);
        }

        // Infer the attribute type based on the attribute value.
        let mut prop_defs = Vec::new();
        for (prop_name, value) in properties {
            let data_type = Self::infer_data_type(value);
            let prop_def = PropertyDef::new(prop_name.clone(), data_type).with_nullable(true); // The automatically generated attributes can be left empty by default.
            prop_defs.push(prop_def);
        }

        // Create TagInfo
        let tag_info = TagInfo {
            tag_id: 0, // Allocated by the storage layer
            tag_name: tag_name.to_string(),
            properties: prop_defs,
            comment: Some("Auto-created for Cypher CREATE".to_string()),
            ttl_duration: None,
            ttl_col: None,
        };

        // Create a Tag
        self.schema_manager
            .as_ref()
            .create_tag(space_name, &tag_info)
            .map_err(|e| {
                CoreValidationError::new(
                    format!("Create Tag '{}' failed: {}", tag_name, e),
                    ValidationErrorType::SemanticError,
                )
            })?;

        Ok(tag_info)
    }

    /// Automatically create an Edge Type (if it does not exist).
    /// Infer the Schema of the Edge Type based on the provided attributes.
    pub fn auto_create_edge_type(
        &self,
        space_name: &str,
        edge_type_name: &str,
        src_tag_name: &str,
        dst_tag_name: &str,
        properties: &[(String, Value)],
    ) -> Result<EdgeTypeInfo, CoreValidationError> {
        if let Some(existing) = self
            .schema_manager
            .as_ref()
            .get_edge_type(space_name, edge_type_name)
            .map_err(|e| {
                CoreValidationError::new(
                    format!("Failed to get Edge Type: {}", e),
                    ValidationErrorType::SemanticError,
                )
            })?
        {
            return Ok(existing);
        }

        let mut prop_defs = Vec::new();
        for (prop_name, value) in properties {
            let data_type = Self::infer_data_type(value);
            let prop_def = PropertyDef::new(prop_name.clone(), data_type).with_nullable(true);
            prop_defs.push(prop_def);
        }

        let edge_info = EdgeTypeInfo {
            edge_type_id: 0,
            edge_type_name: edge_type_name.to_string(),
            src_tag_name: src_tag_name.to_string(),
            dst_tag_name: dst_tag_name.to_string(),
            properties: prop_defs,
            comment: Some("Auto-created for Cypher CREATE".to_string()),
            ttl_duration: None,
            ttl_col: None,
            oe_strategy: crate::core::types::EdgeStrategy::Multiple,
            ie_strategy: crate::core::types::EdgeStrategy::Multiple,
        };

        // Create an Edge Type
        self.schema_manager
            .as_ref()
            .create_edge_type(space_name, &edge_info)
            .map_err(|e| {
                CoreValidationError::new(
                    format!("Create Edge Type '{}' failed: {}", edge_type_name, e),
                    ValidationErrorType::SemanticError,
                )
            })?;

        Ok(edge_info)
    }

    /// Determine the DataType based on the Value.
    fn infer_data_type(value: &Value) -> DataType {
        match value {
            Value::Null(_) => DataType::String, // The text to be translated is: “By default, it is of the string type.”
            Value::Bool(_) => DataType::Bool,
            Value::SmallInt(_) => DataType::SmallInt,
            Value::Int(_) => DataType::Int,
            Value::BigInt(_) => DataType::BigInt,
            Value::Float(_) => DataType::Float,
            Value::Double(_) => DataType::Double,
            Value::String(s) => {
                // Select either FixedString or String depending on the length of the string.
                if s.len() <= 256 {
                    DataType::FixedString(s.len().max(32))
                } else {
                    DataType::String
                }
            }
            Value::List(_) => DataType::List,
            Value::Map(_) => DataType::Map,
            Value::Date(_) => DataType::Date,
            Value::DateTime(_) => DataType::DateTime,
            _ => DataType::String, // The text “默认为字符串类型” translates to “By default, it is of the string type.”
        }
    }

    /// Automated creation of missing Tags in batches
    pub fn auto_create_missing_tags(
        &self,
        space_name: &str,
        tags: &[(String, Vec<(String, Value)>)],
    ) -> Result<Vec<TagInfo>, CoreValidationError> {
        let mut created = Vec::new();
        for (tag_name, properties) in tags {
            let tag_info = self.auto_create_tag(space_name, tag_name, properties)?;
            created.push(tag_info);
        }
        Ok(created)
    }

    /// Automated creation of missing Edge Types in batches
    pub fn auto_create_missing_edge_types(
        &self,
        space_name: &str,
        edge_types: &[EdgeTypeAutoCreateDef],
    ) -> Result<Vec<EdgeTypeInfo>, CoreValidationError> {
        let mut created = Vec::new();
        for (edge_type_name, src_tag_name, dst_tag_name, properties) in edge_types {
            let edge_info = self.auto_create_edge_type(
                space_name,
                edge_type_name,
                src_tag_name,
                dst_tag_name,
                properties,
            )?;
            created.push(edge_info);
        }
        Ok(created)
    }

    /// Validate property references in expressions
    ///
    /// Recursively checks all property references in an expression to ensure they exist
    /// in the corresponding tag or edge type schema.
    ///
    /// # Arguments
    /// * `expr` - The expression to validate
    /// * `space_name` - The space name for schema lookup
    /// * `available_vars` - Map of variable names to their types (tag name or "edge" or "vertex")
    ///
    /// # Returns
    /// * `Ok(())` if all property references are valid
    /// * `Err(ValidationError)` if any property reference is invalid
    pub fn validate_expression_properties(
        &self,
        expr: &crate::core::types::expr::Expression,
        space_name: &str,
        available_vars: &std::collections::HashMap<String, String>,
    ) -> Result<(), CoreValidationError> {
        use crate::core::types::expr::Expression;

        match expr {
            Expression::Property { object, property } => {
                self.validate_property_reference(object, property, space_name, available_vars)
            }
            Expression::Binary { left, right, .. } => {
                self.validate_expression_properties(left, space_name, available_vars)?;
                self.validate_expression_properties(right, space_name, available_vars)
            }
            Expression::Unary { operand, .. } => {
                self.validate_expression_properties(operand, space_name, available_vars)
            }
            Expression::Function { args, .. } => {
                for arg in args {
                    self.validate_expression_properties(arg, space_name, available_vars)?;
                }
                Ok(())
            }
            Expression::List(items) => {
                for item in items {
                    self.validate_expression_properties(item, space_name, available_vars)?;
                }
                Ok(())
            }
            Expression::Map(map) => {
                for (_, value) in map {
                    self.validate_expression_properties(value, space_name, available_vars)?;
                }
                Ok(())
            }
            Expression::Case {
                test_expr,
                conditions,
                default,
            } => {
                if let Some(test) = test_expr {
                    self.validate_expression_properties(test, space_name, available_vars)?;
                }
                for (condition, result) in conditions {
                    self.validate_expression_properties(condition, space_name, available_vars)?;
                    self.validate_expression_properties(result, space_name, available_vars)?;
                }
                if let Some(def) = default {
                    self.validate_expression_properties(def, space_name, available_vars)?;
                }
                Ok(())
            }
            Expression::Aggregate { arg, .. } => {
                self.validate_expression_properties(arg, space_name, available_vars)?;
                Ok(())
            }
            Expression::Predicate { args, .. } => {
                for arg in args {
                    self.validate_expression_properties(arg, space_name, available_vars)?;
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    /// Validate a single property reference
    ///
    /// Checks if the property exists in the schema of the referenced object (tag or edge type)
    fn validate_property_reference(
        &self,
        object: &crate::core::types::expr::Expression,
        property: &str,
        space_name: &str,
        available_vars: &std::collections::HashMap<String, String>,
    ) -> Result<(), CoreValidationError> {
        use crate::core::types::expr::Expression;

        let schema_name = match object {
            Expression::Variable(var_name) => {
                available_vars.get(var_name).cloned().unwrap_or_default()
            }
            Expression::Label(label_name) => label_name.clone(),
            _ => {
                return Err(CoreValidationError::new(
                    format!(
                        "Invalid property access: property '{}' on non-variable object",
                        property
                    ),
                    ValidationErrorType::SemanticError,
                ));
            }
        };

        if schema_name.is_empty() {
            return Err(CoreValidationError::new(
                format!("Cannot determine schema for property access '{}'", property),
                ValidationErrorType::SemanticError,
            ));
        }

        if schema_name == "vertex" || schema_name == "Vertex" {
            return Ok(());
        }

        if schema_name == "edge" || schema_name == "Edge" {
            return Ok(());
        }

        let properties =
            if let Ok(Some(tag_info)) = self.schema_manager.get_tag(space_name, &schema_name) {
                tag_info.properties
            } else if let Ok(Some(edge_info)) =
                self.schema_manager.get_edge_type(space_name, &schema_name)
            {
                edge_info.properties
            } else {
                return Err(CoreValidationError::new(
                    format!(
                        "Schema '{}' not found in space '{}'",
                        schema_name, space_name
                    ),
                    ValidationErrorType::SemanticError,
                ));
            };

        if !properties.iter().any(|p| p.name == property) {
            return Err(CoreValidationError::new(
                format!(
                    "Property '{}' not found in schema '{}'",
                    property, schema_name
                ),
                ValidationErrorType::SemanticError,
            ));
        }

        Ok(())
    }

    /// Infer the type of an expression using schema information
    ///
    /// # Arguments
    /// * `expr` - The expression to infer type for
    /// * `space_name` - The space name for schema lookup
    /// * `available_vars` - Map of variable names to their types
    /// * `input_columns` - Map of column names to their types from input
    ///
    /// # Returns
    /// The inferred ValueType
    pub fn infer_expression_type(
        &self,
        expr: &crate::core::types::expr::Expression,
        space_name: &str,
        available_vars: &std::collections::HashMap<String, String>,
        input_columns: &std::collections::HashMap<String, ValueType>,
    ) -> ValueType {
        use crate::core::types::expr::Expression;

        match expr {
            Expression::Literal(value) => Self::value_to_value_type(value),
            Expression::Variable(name) => input_columns
                .get(name)
                .cloned()
                .unwrap_or(ValueType::Unknown),
            Expression::Property { object, property } => {
                self.infer_property_type(object, property, space_name, available_vars)
            }
            Expression::Binary { op, .. } => match op {
                crate::core::types::operators::BinaryOperator::Add
                | crate::core::types::operators::BinaryOperator::Subtract
                | crate::core::types::operators::BinaryOperator::Multiply
                | crate::core::types::operators::BinaryOperator::Divide => ValueType::Float,
                crate::core::types::operators::BinaryOperator::Equal
                | crate::core::types::operators::BinaryOperator::NotEqual
                | crate::core::types::operators::BinaryOperator::LessThan
                | crate::core::types::operators::BinaryOperator::LessThanOrEqual
                | crate::core::types::operators::BinaryOperator::GreaterThan
                | crate::core::types::operators::BinaryOperator::GreaterThanOrEqual
                | crate::core::types::operators::BinaryOperator::And
                | crate::core::types::operators::BinaryOperator::Or => ValueType::Bool,
                _ => ValueType::Unknown,
            },
            Expression::Unary { op, .. } => match op {
                crate::core::types::operators::UnaryOperator::Not => ValueType::Bool,
                crate::core::types::operators::UnaryOperator::Minus => ValueType::Float,
                _ => ValueType::Unknown,
            },
            Expression::Function { name, .. } => Self::infer_function_return_type(name),
            Expression::List(_) => ValueType::List,
            Expression::Map(_) => ValueType::Map,
            Expression::Case {
                conditions,
                default,
                ..
            } => {
                if !conditions.is_empty() {
                    let (_, result) = &conditions[0];
                    self.infer_expression_type(result, space_name, available_vars, input_columns)
                } else if let Some(def) = default {
                    self.infer_expression_type(def, space_name, available_vars, input_columns)
                } else {
                    ValueType::Unknown
                }
            }
            _ => ValueType::Unknown,
        }
    }

    /// Infer the type of a property access expression
    fn infer_property_type(
        &self,
        object: &crate::core::types::expr::Expression,
        property: &str,
        space_name: &str,
        available_vars: &std::collections::HashMap<String, String>,
    ) -> ValueType {
        use crate::core::types::expr::Expression;

        let schema_name = match object {
            Expression::Variable(var_name) => {
                available_vars.get(var_name).cloned().unwrap_or_default()
            }
            Expression::Label(label_name) => label_name.clone(),
            _ => return ValueType::Unknown,
        };

        if schema_name.is_empty() {
            return ValueType::Unknown;
        }

        let properties =
            if let Ok(Some(tag_info)) = self.schema_manager.get_tag(space_name, &schema_name) {
                tag_info.properties
            } else if let Ok(Some(edge_info)) =
                self.schema_manager.get_edge_type(space_name, &schema_name)
            {
                edge_info.properties
            } else {
                return ValueType::Unknown;
            };

        for prop in &properties {
            if prop.name == property {
                return Self::data_type_to_value_type(&prop.data_type);
            }
        }

        ValueType::Unknown
    }

    /// Convert a Value to ValueType
    fn value_to_value_type(value: &Value) -> ValueType {
        match value {
            Value::Null(_) => ValueType::Null,
            Value::Bool(_) => ValueType::Bool,
            Value::SmallInt(_) | Value::Int(_) | Value::BigInt(_) => ValueType::Int,
            Value::Float(_) | Value::Double(_) => ValueType::Float,
            Value::String(_) => ValueType::String,
            Value::Date(_) => ValueType::Date,
            Value::Time(_) => ValueType::Time,
            Value::DateTime(_) => ValueType::DateTime,
            Value::Vertex(_) => ValueType::Vertex,
            Value::Edge(_) => ValueType::Edge,
            Value::Path(_) => ValueType::Path,
            Value::List(_) => ValueType::List,
            Value::Map(_) => ValueType::Map,
            Value::Set(_) => ValueType::Set,
            _ => ValueType::Unknown,
        }
    }

    /// Infer the return type of a function
    fn infer_function_return_type(function_name: &str) -> ValueType {
        match function_name.to_lowercase().as_str() {
            "count" | "sum" | "avg" | "min" | "max" => ValueType::Int,
            "size" | "length" => ValueType::Int,
            "contains" | "startswith" | "endswith" | "haskey" => ValueType::Bool,
            "substr" | "lower" | "upper" | "trim" | "ltrim" | "rtrim" | "replace" => {
                ValueType::String
            }
            "abs" | "round" | "floor" | "ceil" | "sqrt" | "log" | "exp" | "pow" => ValueType::Float,
            "type" | "label" => ValueType::String,
            "id" => ValueType::Int,
            "head" | "last" => ValueType::Unknown,
            "keys" | "labels" | "properties" => ValueType::List,
            "coalesce" => ValueType::Unknown,
            "nullif" => ValueType::Unknown,
            _ => ValueType::Unknown,
        }
    }

    /// Validate a ContextualExpression's property references
    pub fn validate_contextual_expression_properties(
        &self,
        expr: &ContextualExpression,
        space_name: &str,
        available_vars: &std::collections::HashMap<String, String>,
    ) -> Result<(), CoreValidationError> {
        if let Some(inner_expr) = expr.get_expression() {
            self.validate_expression_properties(&inner_expr, space_name, available_vars)
        } else {
            Err(CoreValidationError::new(
                "Invalid expression: unable to get expression content".to_string(),
                ValidationErrorType::SemanticError,
            ))
        }
    }

    /// Infer type for a ContextualExpression
    pub fn infer_contextual_expression_type(
        &self,
        expr: &ContextualExpression,
        space_name: &str,
        available_vars: &std::collections::HashMap<String, String>,
        input_columns: &std::collections::HashMap<String, ValueType>,
    ) -> ValueType {
        if let Some(inner_expr) = expr.get_expression() {
            self.infer_expression_type(&inner_expr, space_name, available_vars, input_columns)
        } else {
            ValueType::Unknown
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::PropertyDef;
    use std::sync::Arc;

    fn create_test_validator() -> SchemaValidator {
        let schema_manager = Arc::new(SchemaManager::new());
        SchemaValidator::new(schema_manager)
    }

    #[test]
    fn test_validate_property_exists_success() {
        let validator = create_test_validator();
        let properties = vec![
            PropertyDef::new("name".to_string(), DataType::String),
            PropertyDef::new("age".to_string(), DataType::Int),
        ];

        assert!(validator
            .validate_property_exists("name", &properties)
            .is_ok());
        assert!(validator
            .validate_property_exists("age", &properties)
            .is_ok());
    }

    #[test]
    fn test_validate_property_exists_failure() {
        let validator = create_test_validator();
        let properties = vec![PropertyDef::new("name".to_string(), DataType::String)];

        let result = validator.validate_property_exists("age", &properties);
        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("not present"));
    }

    #[test]
    fn test_validate_property_type_success() {
        let validator = create_test_validator();

        assert!(validator
            .validate_property_type(
                "name",
                &DataType::String,
                &Value::String("test".to_string())
            )
            .is_ok());
        assert!(validator
            .validate_property_type("age", &DataType::Int, &Value::Int(25))
            .is_ok());
    }

    #[test]
    fn test_validate_property_type_failure() {
        let validator = create_test_validator();

        let result = validator.validate_property_type(
            "age",
            &DataType::Int,
            &Value::String("test".to_string()),
        );
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.message.contains("Desired type") || err.message.contains("type"));
    }

    #[test]
    fn test_validate_not_null_success() {
        let validator = create_test_validator();
        let prop_def = PropertyDef::new("name".to_string(), DataType::String).with_nullable(false);

        assert!(validator
            .validate_not_null("name", &prop_def, &Value::String("test".to_string()))
            .is_ok());
    }

    #[test]
    fn test_validate_not_null_failure() {
        let validator = create_test_validator();
        let prop_def = PropertyDef::new("name".to_string(), DataType::String).with_nullable(false);

        let result = validator.validate_not_null(
            "name",
            &prop_def,
            &Value::Null(crate::core::NullType::default()),
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("cannot be NULL"));
    }

    #[test]
    fn test_fill_default_values() {
        let validator = create_test_validator();
        let properties = vec![
            PropertyDef::new("name".to_string(), DataType::String).with_nullable(false),
            PropertyDef::new("email".to_string(), DataType::String)
                .with_nullable(true)
                .with_default(Some(Value::String("default@example.com".to_string()))),
            PropertyDef::new("age".to_string(), DataType::Int).with_nullable(true),
        ];

        let provided = vec![("name".to_string(), Value::String("John".to_string()))];
        let result = validator
            .fill_default_values(&properties, &provided)
            .expect("Failed to fill default values");

        assert_eq!(result.len(), 3);
        assert_eq!(result[0].0, "name");
        assert_eq!(result[1].0, "email");
        assert_eq!(
            result[1].1,
            Value::String("default@example.com".to_string())
        );
        assert_eq!(result[2].0, "age");
        assert!(matches!(result[2].1, Value::Null(_)));
    }

    #[test]
    fn test_validate_vid_string() {
        let validator = create_test_validator();

        assert!(validator
            .validate_vid(&Value::String("vid1".to_string()), &DataType::String)
            .is_ok());
    }

    #[test]
    fn test_validate_vid_int() {
        let validator = create_test_validator();

        assert!(validator
            .validate_vid(&Value::Int(123), &DataType::Int)
            .is_ok());
    }

    #[test]
    fn test_is_type_compatible() {
        // Integer compatibility
        assert!(SchemaValidator::is_type_compatible(
            &DataType::Int,
            &DataType::BigInt
        ));
        assert!(SchemaValidator::is_type_compatible(
            &DataType::BigInt,
            &DataType::Int
        ));

        // Floating-point number compatibility
        assert!(SchemaValidator::is_type_compatible(
            &DataType::Float,
            &DataType::Double
        ));

        // VID is compatible.
        assert!(SchemaValidator::is_type_compatible(
            &DataType::VID,
            &DataType::String
        ));
        assert!(SchemaValidator::is_type_compatible(
            &DataType::VID,
            &DataType::Int
        ));

        // Incompatible
        assert!(!SchemaValidator::is_type_compatible(
            &DataType::Int,
            &DataType::String
        ));
        assert!(!SchemaValidator::is_type_compatible(
            &DataType::Bool,
            &DataType::Int
        ));
    }

    #[test]
    fn test_data_type_to_value_type() {
        assert!(matches!(
            SchemaValidator::data_type_to_value_type(&DataType::Bool),
            ValueType::Bool
        ));
        assert!(matches!(
            SchemaValidator::data_type_to_value_type(&DataType::Int),
            ValueType::Int
        ));
        assert!(matches!(
            SchemaValidator::data_type_to_value_type(&DataType::String),
            ValueType::String
        ));
    }
}
