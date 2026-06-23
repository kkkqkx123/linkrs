//! Schema Validation for Execution Plans
//!
//! This module provides functionality to validate schema compatibility between
//! connected plan nodes. It ensures that output schemas of upstream nodes are
//! compatible with input requirements of downstream nodes.

use crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
use std::collections::{HashMap, HashSet};

/// Schema information for a plan node
#[derive(Debug, Clone, Default)]
pub struct NodeSchema {
    /// Column names produced by this node
    pub columns: Vec<String>,
    /// Column types (optional, for type checking)
    pub column_types: HashMap<String, ColumnType>,
    /// Variables produced by this node
    pub variables: HashSet<String>,
}

impl NodeSchema {
    /// Create a new empty schema
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a schema with given column names
    pub fn with_columns(columns: Vec<String>) -> Self {
        Self {
            columns,
            column_types: HashMap::new(),
            variables: HashSet::new(),
        }
    }

    /// Add a column to the schema
    pub fn add_column(&mut self, name: String, col_type: ColumnType) {
        self.columns.push(name.clone());
        self.column_types.insert(name, col_type);
    }

    /// Add a variable to the schema
    pub fn add_variable(&mut self, var: String) {
        self.variables.insert(var);
    }

    /// Check if a column exists
    pub fn has_column(&self, name: &str) -> bool {
        self.columns.iter().any(|c| c == name)
    }

    /// Check if a variable exists
    pub fn has_variable(&self, var: &str) -> bool {
        self.variables.contains(var)
    }

    /// Get the type of a column
    pub fn column_type(&self, name: &str) -> Option<&ColumnType> {
        self.column_types.get(name)
    }

    /// Merge with another schema (for join operations)
    pub fn merge(&self, other: &NodeSchema) -> NodeSchema {
        let mut merged = NodeSchema::new();
        merged.columns = self.columns.clone();
        merged.columns.extend(other.columns.clone());
        merged.column_types = self.column_types.clone();
        merged.column_types.extend(other.column_types.clone());
        merged.variables = self.variables.clone();
        merged.variables.extend(other.variables.clone());
        merged
    }
}

/// Column types for schema validation
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ColumnType {
    Integer,
    Float,
    String,
    Boolean,
    Vertex,
    Edge,
    Path,
    List(Box<ColumnType>),
    Map,
    Null,
    Any,
}

impl ColumnType {
    /// Check if this type is compatible with another type
    pub fn is_compatible_with(&self, other: &ColumnType) -> bool {
        match (self, other) {
            (ColumnType::Any, _) | (_, ColumnType::Any) => true,
            (ColumnType::Null, _) | (_, ColumnType::Null) => true,
            (a, b) => a == b,
        }
    }
}

/// Schema validation error
#[derive(Debug, Clone)]
pub enum SchemaValidationError {
    /// Missing required column
    MissingColumn {
        node_id: i64,
        column: String,
        available_columns: Vec<String>,
    },
    /// Type mismatch between columns
    TypeMismatch {
        node_id: i64,
        column: String,
        expected: ColumnType,
        actual: ColumnType,
    },
    /// Missing required variable
    MissingVariable {
        node_id: i64,
        variable: String,
        available_variables: Vec<String>,
    },
    /// Incompatible schemas for operation
    IncompatibleSchemas {
        node_id: i64,
        left_columns: Vec<String>,
        right_columns: Vec<String>,
        reason: String,
    },
    /// Generic schema error
    Generic { node_id: i64, message: String },
}

impl SchemaValidationError {
    /// Create a missing column error
    pub fn missing_column(node_id: i64, column: String, available: Vec<String>) -> Self {
        Self::MissingColumn {
            node_id,
            column,
            available_columns: available,
        }
    }

    /// Create a type mismatch error
    pub fn type_mismatch(
        node_id: i64,
        column: String,
        expected: ColumnType,
        actual: ColumnType,
    ) -> Self {
        Self::TypeMismatch {
            node_id,
            column,
            expected,
            actual,
        }
    }

    /// Convert to error message
    pub fn to_error_message(&self) -> String {
        match self {
            Self::MissingColumn {
                node_id,
                column,
                available_columns,
            } => format!(
                "Node {}: Missing required column '{}'. Available columns: {}",
                node_id,
                column,
                available_columns.join(", ")
            ),
            Self::TypeMismatch {
                node_id,
                column,
                expected,
                actual,
            } => format!(
                "Node {}: Type mismatch for column '{}'. Expected {:?}, got {:?}",
                node_id, column, expected, actual
            ),
            Self::MissingVariable {
                node_id,
                variable,
                available_variables,
            } => format!(
                "Node {}: Missing required variable '{}'. Available variables: {}",
                node_id,
                variable,
                available_variables.join(", ")
            ),
            Self::IncompatibleSchemas {
                node_id,
                left_columns,
                right_columns,
                reason,
            } => format!(
                "Node {}: Incompatible schemas. Left: [{}], Right: [{}]. Reason: {}",
                node_id,
                left_columns.join(", "),
                right_columns.join(", "),
                reason
            ),
            Self::Generic { node_id, message } => format!("Node {}: {}", node_id, message),
        }
    }
}

/// Result of schema validation
#[derive(Debug, Clone)]
pub struct SchemaValidationResult {
    /// Whether validation passed
    pub is_valid: bool,
    /// List of errors found
    pub errors: Vec<SchemaValidationError>,
    /// Schema information for each node
    pub schemas: HashMap<i64, NodeSchema>,
}

impl SchemaValidationResult {
    /// Create a successful result
    pub fn valid(schemas: HashMap<i64, NodeSchema>) -> Self {
        Self {
            is_valid: true,
            errors: Vec::new(),
            schemas,
        }
    }

    /// Create a failed result with errors
    pub fn invalid(errors: Vec<SchemaValidationError>) -> Self {
        Self {
            is_valid: false,
            errors,
            schemas: HashMap::new(),
        }
    }

    /// Add an error to the result
    pub fn add_error(&mut self, error: SchemaValidationError) {
        self.is_valid = false;
        self.errors.push(error);
    }
}

/// Schema validator for execution plans
pub struct SchemaValidator {
    /// Whether to perform strict type checking
    strict_type_checking: bool,
    /// Whether to validate variable references
    validate_variables: bool,
}

impl Default for SchemaValidator {
    fn default() -> Self {
        Self::new()
    }
}

impl SchemaValidator {
    /// Create a new schema validator with default settings
    pub fn new() -> Self {
        Self {
            strict_type_checking: false,
            validate_variables: true,
        }
    }

    /// Enable strict type checking
    pub fn with_strict_type_checking(mut self, enabled: bool) -> Self {
        self.strict_type_checking = enabled;
        self
    }

    /// Enable variable validation
    pub fn with_variable_validation(mut self, enabled: bool) -> Self {
        self.validate_variables = enabled;
        self
    }

    /// Validate schema compatibility for an execution plan
    ///
    /// # Arguments
    /// * `root` - The root node of the execution plan
    ///
    /// # Returns
    /// A `SchemaValidationResult` with validation status and any errors
    pub fn validate(&self, root: &PlanNodeEnum) -> SchemaValidationResult {
        let mut schemas: HashMap<i64, NodeSchema> = HashMap::new();
        let mut errors: Vec<SchemaValidationError> = Vec::new();

        self.validate_node(root, &mut schemas, &mut errors);

        if errors.is_empty() {
            SchemaValidationResult::valid(schemas)
        } else {
            SchemaValidationResult::invalid(errors)
        }
    }

    /// Validate a single node and its children
    fn validate_node(
        &self,
        node: &PlanNodeEnum,
        schemas: &mut HashMap<i64, NodeSchema>,
        errors: &mut Vec<SchemaValidationError>,
    ) {
        let node_id = node.id();

        if schemas.contains_key(&node_id) {
            return;
        }

        let children = node.children();
        for child in &children {
            self.validate_node(child, schemas, errors);
        }

        let schema = self.compute_node_schema(node, &children, schemas, errors);
        schemas.insert(node_id, schema);
    }

    /// Compute the output schema for a node based on its type and inputs
    fn compute_node_schema(
        &self,
        node: &PlanNodeEnum,
        children: &[&PlanNodeEnum],
        schemas: &HashMap<i64, NodeSchema>,
        _errors: &mut Vec<SchemaValidationError>,
    ) -> NodeSchema {
        let _node_id = node.id();
        let mut schema = NodeSchema::new();

        match node {
            PlanNodeEnum::Start(_) => {
                schema = NodeSchema::new();
            }
            PlanNodeEnum::Project(_project_node) => {
                if let Some(input_schema) = children.first().and_then(|c| schemas.get(&c.id())) {
                    schema = input_schema.clone();
                }
                if let Some(var) = node.output_var() {
                    schema.add_variable(var.to_string());
                }
            }
            PlanNodeEnum::Filter(_) => {
                if let Some(input_schema) = children.first().and_then(|c| schemas.get(&c.id())) {
                    schema = input_schema.clone();
                }
            }
            PlanNodeEnum::Aggregate(_) => {
                if let Some(var) = node.output_var() {
                    schema.add_variable(var.to_string());
                }
            }
            PlanNodeEnum::InnerJoin(_)
            | PlanNodeEnum::LeftJoin(_)
            | PlanNodeEnum::RightJoin(_)
            | PlanNodeEnum::CrossJoin(_)
            | PlanNodeEnum::HashInnerJoin(_)
            | PlanNodeEnum::HashLeftJoin(_)
            | PlanNodeEnum::SemiJoin(_) => {
                if children.len() >= 2 {
                    let left_schema = schemas.get(&children[0].id());
                    let right_schema = schemas.get(&children[1].id());

                    if let (Some(left), Some(right)) = (left_schema, right_schema) {
                        schema = left.merge(right);
                    }
                }
            }
            PlanNodeEnum::Union(_) => {
                if let Some(input_schema) = children.first().and_then(|c| schemas.get(&c.id())) {
                    schema = input_schema.clone();
                }
            }
            PlanNodeEnum::DataCollect(_) => {
                if let Some(input_schema) = children.first().and_then(|c| schemas.get(&c.id())) {
                    schema = input_schema.clone();
                }
            }
            _ => {
                if let Some(input_schema) = children.first().and_then(|c| schemas.get(&c.id())) {
                    schema = input_schema.clone();
                }
                if let Some(var) = node.output_var() {
                    schema.add_variable(var.to_string());
                }
            }
        }

        for col_name in node.col_names() {
            schema.add_column(col_name.clone(), ColumnType::Any);
        }

        schema
    }

    /// Validate that required columns are present in the input schema
    pub fn validate_columns(
        &self,
        node_id: i64,
        required_columns: &[String],
        input_schema: &NodeSchema,
    ) -> Vec<SchemaValidationError> {
        let mut errors = Vec::new();

        for col in required_columns {
            if !input_schema.has_column(col) {
                errors.push(SchemaValidationError::missing_column(
                    node_id,
                    col.clone(),
                    input_schema.columns.clone(),
                ));
            }
        }

        errors
    }

    /// Validate that required variables are present in the input schema
    pub fn validate_variables(
        &self,
        node_id: i64,
        required_variables: &[String],
        input_schema: &NodeSchema,
    ) -> Vec<SchemaValidationError> {
        if !self.validate_variables {
            return Vec::new();
        }

        let mut errors = Vec::new();

        for var in required_variables {
            if !input_schema.has_variable(var) {
                errors.push(SchemaValidationError::Generic {
                    node_id,
                    message: format!(
                        "Missing required variable '{}'. Available: {}",
                        var,
                        input_schema
                            .variables
                            .iter()
                            .cloned()
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                });
            }
        }

        errors
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_node_schema_creation() {
        let mut schema = NodeSchema::new();
        schema.add_column("id".to_string(), ColumnType::Integer);
        schema.add_column("name".to_string(), ColumnType::String);

        assert!(schema.has_column("id"));
        assert!(schema.has_column("name"));
        assert!(!schema.has_column("age"));
    }

    #[test]
    fn test_column_type_compatibility() {
        assert!(ColumnType::Integer.is_compatible_with(&ColumnType::Integer));
        assert!(ColumnType::Any.is_compatible_with(&ColumnType::String));
        assert!(ColumnType::String.is_compatible_with(&ColumnType::Any));
        assert!(!ColumnType::Integer.is_compatible_with(&ColumnType::String));
    }

    #[test]
    fn test_schema_merge() {
        let mut left = NodeSchema::new();
        left.add_column("a".to_string(), ColumnType::Integer);

        let mut right = NodeSchema::new();
        right.add_column("b".to_string(), ColumnType::String);

        let merged = left.merge(&right);

        assert!(merged.has_column("a"));
        assert!(merged.has_column("b"));
    }

    #[test]
    fn test_schema_validator_start_node() {
        let validator = SchemaValidator::new();
        let start = PlanNodeEnum::Start(
            crate::query::planning::plan::core::nodes::control_flow::start_node::StartNode::new(),
        );
        let result = validator.validate(&start);
        assert!(result.is_valid);
    }
}
