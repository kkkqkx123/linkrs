//! CREATE Statement Validator (Cypher style) – New version of the system
//! Validation against the Cypher CREATE (n:Label {prop: value}) syntax
//! Supports automatic schema inference and creation.
//!
//! This document has been restructured in accordance with the new trait + enumeration validator framework.
//! The StatementValidator trait has been implemented to unify the interface.
//! 2. The full functionality of base_validator.rs is preserved:
//! Verify Lifecycle Management
//! Management of input/output columns
//! Expression property tracking
//! User-defined variable management
//! Permission check
//! Execution plan generation
//! 3. The lifecycle parameters have been removed, and the SchemaManager is now managed using Arc.
//! 4. Use QueryContext to manage the context in a unified manner.

use std::sync::Arc;

use crate::core::metadata::SchemaManager;
use crate::core::types::expr::contextual::ContextualExpression;
use crate::core::types::EdgeDirection;
use crate::core::Expression;
use crate::core::Value;
use crate::query::parser::ast::pattern::{
    EdgePattern, NodePattern, PathElement, PathPattern, Pattern,
};
use crate::query::parser::ast::stmt::{Ast, CreateStmt, CreateTarget};
use crate::query::validator::error::{ValidationError, ValidationErrorType};
use crate::query::validator::structs::validation_info::ValidationInfo;
use crate::query::validator::structs::AliasType;
use crate::query::validator::validator_trait::{
    ColumnDef, ExpressionProps, StatementType, StatementValidator, ValidationResult, ValueType,
};
use crate::query::QueryContext;

/// Verified creation information
#[derive(Debug, Clone)]
pub struct ValidatedCreate {
    pub space_id: u64,
    pub space_name: String,
    pub patterns: Vec<ValidatedPattern>,
    pub auto_create_schema: bool,
    pub missing_tags: Vec<String>,
    pub missing_edge_types: Vec<String>,
}

/// Verified pattern
#[derive(Debug, Clone)]
pub enum ValidatedPattern {
    Node(ValidatedNodeCreate),
    Edge(Box<ValidatedEdgeCreate>),
    Path(Box<ValidatedPathCreate>),
}

/// Attribute entry
#[derive(Debug, Clone)]
pub struct PropertyEntry {
    pub name: String,
    pub value: Value,
}

/// Creation of verified nodes
#[derive(Debug, Clone)]
pub struct ValidatedNodeCreate {
    pub variable: Option<String>,
    pub labels: Vec<String>,
    pub properties: Vec<PropertyEntry>,
}

/// Verified edge creation
#[derive(Debug, Clone)]
pub struct ValidatedEdgeCreate {
    pub variable: Option<String>,
    pub edge_type: String,
    pub src: Value,
    pub dst: Value,
    pub properties: Vec<PropertyEntry>,
    pub direction: EdgeDirection,
}

/// The verified path has been created.
#[derive(Debug, Clone)]
pub struct ValidatedPathCreate {
    pub nodes: Vec<ValidatedNodeCreate>,
    pub edges: Vec<ValidatedEdgeCreate>,
}

/// While verifying the context…
#[derive(Debug)]
pub struct EdgeValidationContext<'a> {
    pub space_name: &'a str,
    pub schema_manager: &'a SchemaManager,
    pub missing_edge_types: &'a mut Vec<String>,
}

/// Side definition
#[derive(Debug)]
pub struct EdgeDefinition<'a> {
    pub variable: &'a Option<String>,
    pub edge_type: &'a str,
    pub src: &'a ContextualExpression,
    pub dst: &'a ContextualExpression,
    pub properties: &'a Option<ContextualExpression>,
    pub direction: &'a EdgeDirection,
}

/// CREATE Statement Validator – New Implementation
///
/// Functionality integrity assurance:
/// 1. Complete validation lifecycle (refer to base_validator.rs)
/// 2. Management of input/output columns
/// 3. Expression property tracking
/// 4. Management of user-defined variables
/// 5. Permission checking (scalable)
/// 6. Generation of execution plans (scalable)
#[derive(Debug)]
pub struct CreateValidator {
    // Schema management
    schema_manager: Option<Arc<SchemaManager>>,
    // Should the Schema be created automatically?
    auto_create_schema: bool,
    // Input column definition
    inputs: Vec<ColumnDef>,
    // Column definition
    outputs: Vec<ColumnDef>,
    // Expression properties
    expr_props: ExpressionProps,
    // User-defined variables
    user_defined_vars: Vec<String>,
    // Cache validation results
    validated_result: Option<ValidatedCreate>,
    // List of verification errors
    validation_errors: Vec<ValidationError>,
    // Is it not necessary to have any space available (for the “CREATE SPACE” operation)?
    no_space_required: bool,
}

impl CreateValidator {
    /// Create a new instance of the validator.
    pub fn new() -> Self {
        Self {
            schema_manager: None,
            auto_create_schema: true,
            inputs: Vec::new(),
            outputs: Vec::new(),
            expr_props: ExpressionProps::default(),
            user_defined_vars: Vec::new(),
            validated_result: None,
            validation_errors: Vec::new(),
            no_space_required: false,
        }
    }

    /// Setting up SchemaManager (builder pattern)
    pub fn with_schema_manager(mut self, schema_manager: Arc<SchemaManager>) -> Self {
        self.schema_manager = Some(schema_manager);
        self
    }

    /// Set schema manager (mutable reference)
    pub fn set_schema_manager(&mut self, schema_manager: Arc<SchemaManager>) {
        self.schema_manager = Some(schema_manager);
    }

    /// Set whether to automatically create a Schema.
    pub fn with_auto_create_schema(mut self, auto_create: bool) -> Self {
        self.auto_create_schema = auto_create;
        self
    }

    /// Obtain the verification results.
    pub fn validated_result(&self) -> Option<&ValidatedCreate> {
        self.validated_result.as_ref()
    }

    /// Obtain the list of verification errors.
    pub fn validation_errors(&self) -> &[ValidationError] {
        &self.validation_errors
    }

    /// Add verification errors
    fn add_error(&mut self, error: ValidationError) {
        self.validation_errors.push(error);
    }

    /// Clear the verification errors.
    fn clear_errors(&mut self) {
        self.validation_errors.clear();
    }

    /// Check for any validation errors.
    fn has_errors(&self) -> bool {
        !self.validation_errors.is_empty()
    }

    /// Verify the CREATE statement (traditional method, maintaining backward compatibility)
    pub fn validate_create(
        &mut self,
        stmt: &CreateStmt,
        space_name: &str,
    ) -> Result<ValidatedCreate, ValidationError> {
        let schema_manager = self.schema_manager.as_ref().ok_or_else(|| {
            ValidationError::new(
                "Schema manager not initialized".to_string(),
                ValidationErrorType::SemanticError,
            )
        })?;

        // Obtaining spatial information
        let space = schema_manager
            .as_ref()
            .get_space(space_name)
            .map_err(|e| {
                ValidationError::new(
                    format!("Failed to get space '{}': {}", space_name, e),
                    ValidationErrorType::SemanticError,
                )
            })?
            .ok_or_else(|| {
                ValidationError::new(
                    format!("Space '{}' does not exist", space_name),
                    ValidationErrorType::SemanticError,
                )
            })?;

        let space_id = space.space_id;
        let mut missing_tags = Vec::new();
        let mut missing_edge_types = Vec::new();

        // Verify the target.
        let patterns = match &stmt.target {
            CreateTarget::Path { patterns } => self.validate_patterns(
                patterns,
                space_name,
                schema_manager.as_ref(),
                &mut missing_tags,
                &mut missing_edge_types,
            )?,
            CreateTarget::Node {
                variable,
                labels,
                properties,
            } => {
                vec![self.validate_single_node(
                    variable,
                    labels,
                    properties,
                    space_name,
                    schema_manager.as_ref(),
                    &mut missing_tags,
                )?]
            }
            CreateTarget::Edge {
                variable,
                edge_type,
                src,
                dst,
                properties,
                direction,
            } => {
                let edge_def = EdgeDefinition {
                    variable,
                    edge_type,
                    src,
                    dst,
                    properties,
                    direction,
                };
                let mut context = EdgeValidationContext {
                    space_name,
                    schema_manager: schema_manager.as_ref(),
                    missing_edge_types: &mut missing_edge_types,
                };
                vec![self.validate_single_edge(&edge_def, &mut context)?]
            }
            _ => {
                return Err(ValidationError::new(
                    "Unsupported CREATE target type".to_string(),
                    ValidationErrorType::SemanticError,
                ));
            }
        };

        let result = ValidatedCreate {
            space_id,
            space_name: space_name.to_string(),
            patterns,
            auto_create_schema: self.auto_create_schema,
            missing_tags,
            missing_edge_types,
        };

        // Cached results
        self.validated_result = Some(result.clone());

        Ok(result)
    }

    /// List of verification modes
    fn validate_patterns(
        &self,
        patterns: &[Pattern],
        space_name: &str,
        schema_manager: &SchemaManager,
        missing_tags: &mut Vec<String>,
        missing_edge_types: &mut Vec<String>,
    ) -> Result<Vec<ValidatedPattern>, ValidationError> {
        let mut validated = Vec::new();

        for pattern in patterns {
            let validated_pattern = match pattern {
                Pattern::Node(node) => ValidatedPattern::Node(self.validate_node_pattern(
                    node,
                    space_name,
                    schema_manager,
                    missing_tags,
                )?),
                Pattern::Edge(edge) => {
                    ValidatedPattern::Edge(Box::new(self.validate_edge_pattern(
                        edge,
                        space_name,
                        schema_manager,
                        missing_edge_types,
                    )?))
                }
                Pattern::Path(path) => {
                    ValidatedPattern::Path(Box::new(self.validate_path_pattern(
                        path,
                        space_name,
                        schema_manager,
                        missing_tags,
                        missing_edge_types,
                    )?))
                }
                Pattern::Variable(_) => {
                    return Err(ValidationError::new(
                        "Variable pattern is not supported in CREATE statement".to_string(),
                        ValidationErrorType::SemanticError,
                    ));
                }
            };
            validated.push(validated_pattern);
        }

        Ok(validated)
    }

    /// Verify node mode
    fn validate_node_pattern(
        &self,
        node: &NodePattern,
        space_name: &str,
        schema_manager: &SchemaManager,
        missing_tags: &mut Vec<String>,
    ) -> Result<ValidatedNodeCreate, ValidationError> {
        // Verify the tags.
        for label in &node.labels {
            if let Ok(None) = schema_manager.get_tag(space_name, label) {
                if !self.auto_create_schema {
                    return Err(ValidationError::new(
                        format!("Tag '{}' does not exist", label),
                        ValidationErrorType::SemanticError,
                    ));
                }
                if !missing_tags.contains(label) {
                    missing_tags.push(label.clone());
                }
            }
        }

        // Extract attributes
        let props = if let Some(ref props_expr) = node.properties {
            self.extract_properties(props_expr)?
        } else {
            Vec::new()
        };

        Ok(ValidatedNodeCreate {
            variable: node.variable.clone(),
            labels: node.labels.clone(),
            properties: props,
        })
    }

    /// Verify the border mode
    fn validate_edge_pattern(
        &self,
        edge: &EdgePattern,
        space_name: &str,
        schema_manager: &SchemaManager,
        missing_edge_types: &mut Vec<String>,
    ) -> Result<ValidatedEdgeCreate, ValidationError> {
        // Verify the edge type (select the first edge type)
        let edge_type = edge.edge_types.first().ok_or_else(|| {
            ValidationError::new(
                "Edge must specify at least one edge type".to_string(),
                ValidationErrorType::SemanticError,
            )
        })?;

        if let Ok(None) = schema_manager.get_edge_type(space_name, edge_type) {
            if !self.auto_create_schema {
                return Err(ValidationError::new(
                    format!("Edge type '{}' does not exist", edge_type),
                    ValidationErrorType::SemanticError,
                ));
            }
            if !missing_edge_types.contains(edge_type) {
                missing_edge_types.push(edge_type.clone());
            }
        }

        // Extract attributes
        let props = if let Some(ref props_expr) = edge.properties {
            self.extract_properties(props_expr)?
        } else {
            Vec::new()
        };

        Ok(ValidatedEdgeCreate {
            variable: edge.variable.clone(),
            edge_type: edge_type.clone(),
            src: Value::Null(crate::core::NullType::Null),
            dst: Value::Null(crate::core::NullType::Null),
            properties: props,
            direction: edge.direction,
        })
    }

    /// Verify the path pattern.
    fn validate_path_pattern(
        &self,
        path: &PathPattern,
        space_name: &str,
        schema_manager: &SchemaManager,
        missing_tags: &mut Vec<String>,
        missing_edge_types: &mut Vec<String>,
    ) -> Result<ValidatedPathCreate, ValidationError> {
        let mut nodes = Vec::new();
        let mut edges = Vec::new();

        for element in &path.elements {
            match element {
                PathElement::Node(node) => {
                    nodes.push(self.validate_node_pattern(
                        node,
                        space_name,
                        schema_manager,
                        missing_tags,
                    )?);
                }
                PathElement::Edge(edge) => {
                    edges.push(self.validate_edge_pattern(
                        edge,
                        space_name,
                        schema_manager,
                        missing_edge_types,
                    )?);
                }
                PathElement::Alternative(_) => {
                    return Err(ValidationError::new(
                        "Alternative pattern is not supported in CREATE statement".to_string(),
                        ValidationErrorType::SemanticError,
                    ));
                }
                PathElement::Optional(_) => {
                    return Err(ValidationError::new(
                        "Optional pattern is not supported in CREATE statement".to_string(),
                        ValidationErrorType::SemanticError,
                    ));
                }
                PathElement::Repeated(_, _) => {
                    return Err(ValidationError::new(
                        "Repeated pattern is not supported in CREATE statement".to_string(),
                        ValidationErrorType::SemanticError,
                    ));
                }
            }
        }

        Ok(ValidatedPathCreate { nodes, edges })
    }

    /// Verification of the creation of a single node (simplified version)
    fn validate_single_node(
        &self,
        variable: &Option<String>,
        labels: &[String],
        properties: &Option<ContextualExpression>,
        space_name: &str,
        schema_manager: &SchemaManager,
        missing_tags: &mut Vec<String>,
    ) -> Result<ValidatedPattern, ValidationError> {
        // Verify the tags.
        for label in labels {
            if let Ok(None) = schema_manager.get_tag(space_name, label) {
                if !self.auto_create_schema {
                    return Err(ValidationError::new(
                        format!("Tag '{}' does not exist", label),
                        ValidationErrorType::SemanticError,
                    ));
                }
                if !missing_tags.contains(label) {
                    missing_tags.push(label.clone());
                }
            }
        }

        // Extract attributes
        let props = if let Some(ref props_expr) = properties {
            self.extract_properties(props_expr)?
        } else {
            Vec::new()
        };

        Ok(ValidatedPattern::Node(ValidatedNodeCreate {
            variable: variable.clone(),
            labels: labels.to_vec(),
            properties: props,
        }))
    }

    fn validate_single_edge(
        &self,
        edge_def: &EdgeDefinition,
        context: &mut EdgeValidationContext,
    ) -> Result<ValidatedPattern, ValidationError> {
        let EdgeDefinition {
            variable,
            edge_type,
            src,
            dst,
            properties,
            direction,
        } = edge_def;

        let EdgeValidationContext {
            space_name,
            schema_manager,
            missing_edge_types,
        } = context;

        if let Ok(None) = schema_manager.get_edge_type(space_name, edge_type) {
            if !self.auto_create_schema {
                return Err(ValidationError::new(
                    format!("Edge type '{}' does not exist", edge_type),
                    ValidationErrorType::SemanticError,
                ));
            }
            if !missing_edge_types.contains(&edge_type.to_string()) {
                missing_edge_types.push(edge_type.to_string());
            }
        }

        let props = if let Some(ref props_expr) = properties {
            self.extract_properties(props_expr)?
        } else {
            Vec::new()
        };

        let src_value = if let Some(expr) = src.get_expression() {
            self.evaluate_expression_internal(&expr)?
        } else {
            Value::Null(crate::core::NullType::Null)
        };

        let dst_value = if let Some(expr) = dst.get_expression() {
            self.evaluate_expression_internal(&expr)?
        } else {
            Value::Null(crate::core::NullType::Null)
        };

        Ok(ValidatedPattern::Edge(Box::new(ValidatedEdgeCreate {
            variable: variable.as_ref().cloned(),
            edge_type: edge_type.to_string(),
            src: src_value,
            dst: dst_value,
            properties: props,
            direction: **direction,
        })))
    }

    /// Extract attribute key-value pairs from the expression.
    fn extract_properties(
        &self,
        expr: &ContextualExpression,
    ) -> Result<Vec<PropertyEntry>, ValidationError> {
        if let Some(e) = expr.get_expression() {
            self.extract_properties_internal(&e)
        } else {
            Err(ValidationError::new(
                "Invalid attribute expression".to_string(),
                ValidationErrorType::SemanticError,
            ))
        }
    }

    /// Internal method: Extracting attribute key-value pairs from expressions
    fn extract_properties_internal(
        &self,
        expr: &crate::core::types::expr::Expression,
    ) -> Result<Vec<PropertyEntry>, ValidationError> {
        use crate::core::types::expr::Expression;

        match expr {
            Expression::Map(entries) => {
                let mut props = Vec::new();
                for (key, value_expr) in entries {
                    let value = self.evaluate_expression_internal(value_expr)?;
                    props.push(PropertyEntry {
                        name: key.clone(),
                        value,
                    });
                }
                Ok(props)
            }
            _ => Err(ValidationError::new(
                "Expected Map expression for properties".to_string(),
                ValidationErrorType::SemanticError,
            )),
        }
    }

    /// Internal method: Evaluation of expressions (simplified version)
    fn evaluate_expression_internal(&self, expr: &Expression) -> Result<Value, ValidationError> {
        match expr {
            Expression::Literal(value) => Ok(value.clone()),
            _ => Err(ValidationError::new(
                format!("Unsupported expression type in CREATE: {:?}", expr),
                ValidationErrorType::SemanticError,
            )),
        }
    }

    /// Validation specific statements (refer to validate_impl in base_validator.rs)
    fn validate_impl(
        &mut self,
        create_stmt: &CreateStmt,
        space_name: &str,
    ) -> Result<(), ValidationError> {
        // Process according to the type of the CreateTarget class.
        match &create_stmt.target {
            // “CREATE SPACE” is a global statement that does not require any additional space (i.e., no spaces need to be added before or after it).
            CreateTarget::Space {
                name,
                vid_type,
                comment: _,
            } => {
                self.no_space_required = true;

                // Obtain the SchemaManager
                let schema_manager = self.schema_manager.as_ref().ok_or_else(|| {
                    ValidationError::new(
                        "Schema manager not initialized".to_string(),
                        ValidationErrorType::SemanticError,
                    )
                })?;

                // Verify whether the space name already exists.
                let existing_space = schema_manager.get_space(name).map_err(|e| {
                    ValidationError::new(
                        format!("Failed to check space existence: {}", e),
                        ValidationErrorType::SemanticError,
                    )
                })?;

                if existing_space.is_some() && !create_stmt.if_not_exists {
                    return Err(ValidationError::new(
                        format!("Space '{}' already exists", name),
                        ValidationErrorType::SemanticError,
                    ));
                }

                // Verify whether vid_type is valid.
                let valid_vid_types = [
                    "INT64",
                    "INT32",
                    "INT16",
                    "INT8",
                    "FIXEDSTRING",
                    "STRING",
                    "FIXED_STRING",
                ];
                let vid_type_upper = vid_type.to_uppercase();
                let is_valid_vid_type = valid_vid_types
                    .iter()
                    .any(|&t| vid_type_upper.starts_with(t) || vid_type_upper == t);

                if !is_valid_vid_type {
                    return Err(ValidationError::new(
                        format!(
                            "Invalid vid_type '{}'. Supported types: INT64, INT32, INT16, INT8, FIXEDSTRING(N), FIXED_STRING(N), STRING",
                            vid_type
                        ),
                        ValidationErrorType::SemanticError,
                    ));
                }

                // Setting the output columns: The command “CREATE SPACE” returns the execution results.
                self.outputs.clear();
                self.outputs.push(ColumnDef {
                    name: "Execution Result".to_string(),
                    type_: ValueType::String,
                });

                // Cache validation results
                let result = ValidatedCreate {
                    space_id: 0,
                    space_name: name.clone(),
                    patterns: Vec::new(),
                    auto_create_schema: false,
                    missing_tags: Vec::new(),
                    missing_edge_types: Vec::new(),
                };
                self.validated_result = Some(result);

                Ok(())
            }
            // CREATE TAG/EDGE: This operation requires additional space, but the current validator does not support it.
            CreateTarget::Tag { .. } | CreateTarget::EdgeType { .. } => Err(ValidationError::new(
                "CreateValidator does not support CREATE TAG/EDGE, use a DDL validator!"
                    .to_string(),
                ValidationErrorType::SemanticError,
            )),
            // The CREATE INDEX command is now processed by a dedicated component called CreateIndexValidator.
            CreateTarget::Index { .. } => Err(ValidationError::new(
                "CreateIndexValidator does not support this type of CREATE statement.".to_string(),
                ValidationErrorType::SemanticError,
            )),
            // CREATE Node/Edge/Path: This operation requires additional storage space and involves the execution of DML (Data Manipulation Language) validation processes.
            CreateTarget::Node { .. } | CreateTarget::Edge { .. } | CreateTarget::Path { .. } => {
                if space_name.is_empty() {
                    return Err(ValidationError::new(
                        "The CREATE statement requires a pre-selected map space, so run USE <space_name> first.".to_string(),
                        ValidationErrorType::SemanticError,
                    ));
                }

                // Please provide the text you would like to have translated. I will then perform the verification and provide the translated version.
                let result = self.validate_create(create_stmt, space_name)?;

                // Set the output columns – based on the actual type.
                self.outputs.clear();
                for (i, pattern) in result.patterns.iter().enumerate() {
                    let (col_name, col_type) = match pattern {
                        ValidatedPattern::Node(node) => {
                            let name = node
                                .variable
                                .clone()
                                .unwrap_or_else(|| format!("node_{}", i));
                            (name, ValueType::Vertex)
                        }
                        ValidatedPattern::Edge(edge) => {
                            let name = edge
                                .variable
                                .clone()
                                .unwrap_or_else(|| format!("edge_{}", i));
                            (name, ValueType::Edge)
                        }
                        ValidatedPattern::Path(_) => (format!("path_{}", i), ValueType::Path),
                    };
                    self.outputs.push(ColumnDef {
                        name: col_name,
                        type_: col_type,
                    });
                }

                // Cache validation results
                self.validated_result = Some(result);

                Ok(())
            }
        }
    }
}

impl Default for CreateValidator {
    fn default() -> Self {
        Self::new()
    }
}

/// Implementing the StatementValidator trait
///
/// Complete implementation of the validation lifecycle (refer to base_validator.rs):
/// 1. Check whether space is required (is_global_statement)
/// 2. Execute the specific validation logic (validate_impl).
/// 3. Permission check (check_permission)
/// 4. Generate the execution plan (to_plan)
/// 5. Synchronous input/output to AstContext
///
/// # Refactoring changes
/// The `validate` method accepts `Arc<Ast>` and `Arc<QueryContext>` as arguments.
impl StatementValidator for CreateValidator {
    fn validate(
        &mut self,
        ast: Arc<Ast>,
        qctx: Arc<QueryContext>,
    ) -> Result<ValidationResult, ValidationError> {
        // Clear the previous state.
        self.outputs.clear();
        self.inputs.clear();
        self.expr_props = ExpressionProps::default();
        self.user_defined_vars.clear();
        self.clear_errors();
        self.no_space_required = false;

        // Obtaining the CREATE statement
        let create_stmt = match &ast.stmt {
            crate::query::parser::ast::Stmt::Create(create_stmt) => create_stmt,
            _ => {
                return Err(ValidationError::new(
                    "Expected CREATE statement".to_string(),
                    ValidationErrorType::SemanticError,
                ));
            }
        };

        // Step 1: Check whether space is required.
        let is_global = matches!(&create_stmt.target, CreateTarget::Space { .. });

        if !is_global && qctx.space_id().is_none() {
            return Err(ValidationError::new(
                "未选择图空间。请先执行 `USE <space>` 选择图空间。".to_string(),
                ValidationErrorType::SemanticError,
            ));
        }

        // Step 2: Obtain the name of the space.
        let space_name = qctx.space_name().unwrap_or_default();

        // Step 3: Execute the specific validation logic
        if let Err(e) = self.validate_impl(create_stmt, &space_name) {
            self.add_error(e);
        }

        // If there are any validation errors, return a failure result.
        if self.has_errors() {
            let errors = self.validation_errors.clone();
            return Ok(ValidationResult::failure(errors));
        }

        // Constructing detailed ValidationInfo
        let mut info = ValidationInfo::new();

        // Add alias mappings and semantic information.
        if let Some(ref result) = self.validated_result {
            for pattern in &result.patterns {
                match pattern {
                    ValidatedPattern::Node(node) => {
                        if let Some(ref var) = node.variable {
                            info.add_alias(var.clone(), AliasType::Node);
                        }
                        for label in &node.labels {
                            if !info.semantic_info.referenced_tags.contains(label) {
                                info.semantic_info.referenced_tags.push(label.clone());
                            }
                        }
                    }
                    ValidatedPattern::Edge(edge) => {
                        if let Some(ref var) = edge.variable {
                            info.add_alias(var.clone(), AliasType::Edge);
                        }
                        if !info
                            .semantic_info
                            .referenced_edges
                            .contains(&edge.edge_type)
                        {
                            info.semantic_info
                                .referenced_edges
                                .push(edge.edge_type.clone());
                        }
                    }
                    ValidatedPattern::Path(path) => {
                        for node in &path.nodes {
                            if let Some(ref var) = node.variable {
                                info.add_alias(var.clone(), AliasType::Node);
                            }
                            for label in &node.labels {
                                if !info.semantic_info.referenced_tags.contains(label) {
                                    info.semantic_info.referenced_tags.push(label.clone());
                                }
                            }
                        }
                        for edge in &path.edges {
                            if let Some(ref var) = edge.variable {
                                info.add_alias(var.clone(), AliasType::Edge);
                            }
                            if !info
                                .semantic_info
                                .referenced_edges
                                .contains(&edge.edge_type)
                            {
                                info.semantic_info
                                    .referenced_edges
                                    .push(edge.edge_type.clone());
                            }
                        }
                    }
                }
            }
        }

        // Return the verification results including detailed information.
        Ok(ValidationResult::success_with_info(info))
    }

    fn statement_type(&self) -> StatementType {
        StatementType::Create
    }

    fn inputs(&self) -> &[ColumnDef] {
        &self.inputs
    }

    fn outputs(&self) -> &[ColumnDef] {
        &self.outputs
    }

    fn is_global_statement(&self) -> bool {
        // The `CREATE` statement is not a global statement; it is necessary to select a specific space in advance.
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
    use crate::query::validator::validator_trait::StatementValidator;

    #[test]
    fn test_create_validator_new() {
        let validator = CreateValidator::new();
        assert!(validator.inputs().is_empty());
        assert!(validator.outputs().is_empty());
        assert!(validator.validated_result().is_none());
        assert!(validator.validation_errors().is_empty());
    }

    #[test]
    fn test_create_validator_default() {
        let validator: CreateValidator = Default::default();
        assert!(validator.inputs().is_empty());
        assert!(validator.outputs().is_empty());
    }

    #[test]
    fn test_statement_validator_trait() {
        let validator = CreateValidator::new();

        // Testing the trait method
        assert_eq!(validator.statement_type(), StatementType::Create);
        assert_eq!(validator.validator_name(), "CREATEValidator");
    }
}
