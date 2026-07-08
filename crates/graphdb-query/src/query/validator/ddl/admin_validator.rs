//! Management Statement Validator
//! Corresponding to the functionality of NebulaGraph AdminValidator
//! Verify management statements such as SHOW, DESC, SHOW CREATE, and SHOW CONFIGS.
//!
//! Design principles:
//! The StatementValidator trait has been implemented to unify the interface.
//! 2. All management statements are global statements; there is no need to pre-select a specific scope (i.e., no need to specify a particular “space” in which the statements should be applied).
//! 3. Verify whether the target object exists.

use crate::query::parser::ast::stmt::{
    Ast, DescStmt, DescTarget, KillQueryStmt, ShowConfigsStmt, ShowCreateStmt, ShowCreateTarget,
    ShowQueriesStmt, ShowSessionsStmt, ShowStmt, ShowTarget,
};
use crate::query::validator::error::{ValidationError, ValidationErrorType};
use crate::query::validator::structs::validation_info::ValidationInfo;
use crate::query::validator::validator_trait::{
    ColumnDef, ExpressionProps, StatementType, StatementValidator, ValidationResult, ValueType,
};
use crate::query::QueryContext;
use std::sync::Arc;

/// Verified SHOW information
#[derive(Debug, Clone)]
pub struct ValidatedShow {
    pub target_type: ShowTargetType,
    pub target_name: Option<String>,
}

/// SHOW: Target type
#[derive(Debug, Clone)]
pub enum ShowTargetType {
    Spaces,
    Tags,
    Edges,
    Tag,
    Edge,
    Indexes,
    Index,
    Users,
    Roles,
    Sessions,
    Queries,
    Configs,
    Stats,
}

/// SHOW statement validator
#[derive(Debug)]
pub struct ShowValidator {
    target_type: ShowTargetType,
    target_name: Option<String>,
    inputs: Vec<ColumnDef>,
    outputs: Vec<ColumnDef>,
    expr_props: ExpressionProps,
    user_defined_vars: Vec<String>,
}

impl ShowValidator {
    pub fn new() -> Self {
        Self {
            target_type: ShowTargetType::Spaces,
            target_name: None,
            inputs: Vec::new(),
            outputs: Vec::new(),
            expr_props: ExpressionProps::default(),
            user_defined_vars: Vec::new(),
        }
    }

    fn validate_impl(&mut self, stmt: &ShowStmt) -> Result<(), ValidationError> {
        self.target_type = match &stmt.target {
            ShowTarget::Spaces => ShowTargetType::Spaces,
            ShowTarget::Tags => ShowTargetType::Tags,
            ShowTarget::Edges => ShowTargetType::Edges,
            ShowTarget::Tag(name) => {
                self.target_name = Some(name.clone());
                ShowTargetType::Tag
            }
            ShowTarget::Edge(name) => {
                self.target_name = Some(name.clone());
                ShowTargetType::Edge
            }
            ShowTarget::Indexes => ShowTargetType::Indexes,
            ShowTarget::Index(name) => {
                self.target_name = Some(name.clone());
                ShowTargetType::Index
            }
            ShowTarget::Users => ShowTargetType::Users,
            ShowTarget::Roles => ShowTargetType::Roles,
            ShowTarget::Stats => ShowTargetType::Stats,
        };

        self.setup_outputs();
        Ok(())
    }

    fn setup_outputs(&mut self) {
        self.outputs = match self.target_type {
            ShowTargetType::Spaces => vec![
                ColumnDef {
                    name: "Name".to_string(),
                    type_: ValueType::String,
                },
                ColumnDef {
                    name: "ID".to_string(),
                    type_: ValueType::Int,
                },
            ],
            ShowTargetType::Tags | ShowTargetType::Edges => vec![ColumnDef {
                name: "Name".to_string(),
                type_: ValueType::String,
            }],
            ShowTargetType::Users => vec![
                ColumnDef {
                    name: "Account".to_string(),
                    type_: ValueType::String,
                },
                ColumnDef {
                    name: "IP".to_string(),
                    type_: ValueType::String,
                },
            ],
            ShowTargetType::Roles => vec![
                ColumnDef {
                    name: "Account".to_string(),
                    type_: ValueType::String,
                },
                ColumnDef {
                    name: "Role".to_string(),
                    type_: ValueType::String,
                },
            ],
            ShowTargetType::Sessions => vec![
                ColumnDef {
                    name: "SessionId".to_string(),
                    type_: ValueType::Int,
                },
                ColumnDef {
                    name: "UserName".to_string(),
                    type_: ValueType::String,
                },
            ],
            ShowTargetType::Queries => vec![
                ColumnDef {
                    name: "SessionID".to_string(),
                    type_: ValueType::Int,
                },
                ColumnDef {
                    name: "ExecutionPlanID".to_string(),
                    type_: ValueType::Int,
                },
            ],
            ShowTargetType::Configs => vec![
                ColumnDef {
                    name: "Module".to_string(),
                    type_: ValueType::String,
                },
                ColumnDef {
                    name: "Name".to_string(),
                    type_: ValueType::String,
                },
                ColumnDef {
                    name: "Value".to_string(),
                    type_: ValueType::String,
                },
            ],
            _ => vec![ColumnDef {
                name: "Result".to_string(),
                type_: ValueType::String,
            }],
        };
    }

    pub fn validated_result(&self) -> ValidatedShow {
        ValidatedShow {
            target_type: self.target_type.clone(),
            target_name: self.target_name.clone(),
        }
    }
}

/// Implementing the StatementValidator trait
///
/// # Refactoring changes
/// The `validate` method accepts `Arc<Ast>` and `Arc<QueryContext>` as parameters.
impl StatementValidator for ShowValidator {
    fn validate(
        &mut self,
        ast: Arc<Ast>,
        qctx: Arc<QueryContext>,
    ) -> Result<ValidationResult, ValidationError> {
        let show_stmt = match &ast.stmt {
            crate::query::parser::ast::Stmt::Show(show_stmt) => show_stmt,
            _ => {
                return Err(ValidationError::new(
                    "Expected SHOW statement".to_string(),
                    ValidationErrorType::SemanticError,
                ));
            }
        };

        self.validate_impl(show_stmt)?;

        let mut info = ValidationInfo::new();

        info.semantic_info.query_type = Some(format!("{:?}", self.target_type));

        // For SHOW TAGS and SHOW EDGES, set the space name from query context
        match self.target_type {
            ShowTargetType::Tags | ShowTargetType::Edges => {
                if let Some(space_name) = qctx.space_name() {
                    info.semantic_info.space_name = Some(space_name);
                }
            }
            _ => {}
        }

        Ok(ValidationResult::success_with_info(info))
    }

    fn statement_type(&self) -> StatementType {
        StatementType::ShowSpaces
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
        &self.expr_props
    }

    fn user_defined_vars(&self) -> &[String] {
        &self.user_defined_vars
    }
}

impl Default for ShowValidator {
    fn default() -> Self {
        Self::new()
    }
}

/// Verified DESCRIBE information
#[derive(Debug, Clone)]
pub struct ValidatedDesc {
    pub target_type: DescTargetType,
    pub space_name: String,
    pub target_name: String,
}

/// Describe the target type.
#[derive(Debug, Clone)]
pub enum DescTargetType {
    Space,
    Tag,
    Edge,
}

/// The DESCRIBE statement validator
#[derive(Debug)]
pub struct DescValidator {
    target_type: DescTargetType,
    space_name: String,
    target_name: String,
    inputs: Vec<ColumnDef>,
    outputs: Vec<ColumnDef>,
    expr_props: ExpressionProps,
    user_defined_vars: Vec<String>,
}

impl DescValidator {
    pub fn new() -> Self {
        Self {
            target_type: DescTargetType::Space,
            space_name: String::new(),
            target_name: String::new(),
            inputs: Vec::new(),
            outputs: Vec::new(),
            expr_props: ExpressionProps::default(),
            user_defined_vars: Vec::new(),
        }
    }

    fn validate_impl(&mut self, stmt: &DescStmt) -> Result<(), ValidationError> {
        match &stmt.target {
            DescTarget::Space(name) => {
                self.target_type = DescTargetType::Space;
                self.target_name = name.clone();
                self.space_name = name.clone();
                self.outputs = vec![
                    ColumnDef {
                        name: "Field".to_string(),
                        type_: ValueType::String,
                    },
                    ColumnDef {
                        name: "Value".to_string(),
                        type_: ValueType::String,
                    },
                ];
            }
            DescTarget::Tag {
                space_name,
                tag_name,
            } => {
                self.target_type = DescTargetType::Tag;
                self.space_name = space_name.clone();
                self.target_name = tag_name.clone();
                self.outputs = vec![
                    ColumnDef {
                        name: "Field".to_string(),
                        type_: ValueType::String,
                    },
                    ColumnDef {
                        name: "Type".to_string(),
                        type_: ValueType::String,
                    },
                    ColumnDef {
                        name: "Null".to_string(),
                        type_: ValueType::String,
                    },
                    ColumnDef {
                        name: "Default".to_string(),
                        type_: ValueType::String,
                    },
                ];
            }
            DescTarget::Edge {
                space_name,
                edge_name,
            } => {
                self.target_type = DescTargetType::Edge;
                self.space_name = space_name.clone();
                self.target_name = edge_name.clone();
                self.outputs = vec![
                    ColumnDef {
                        name: "Field".to_string(),
                        type_: ValueType::String,
                    },
                    ColumnDef {
                        name: "Type".to_string(),
                        type_: ValueType::String,
                    },
                    ColumnDef {
                        name: "Null".to_string(),
                        type_: ValueType::String,
                    },
                    ColumnDef {
                        name: "Default".to_string(),
                        type_: ValueType::String,
                    },
                ];
            }
        }
        Ok(())
    }

    pub fn validated_result(&self) -> ValidatedDesc {
        ValidatedDesc {
            target_type: self.target_type.clone(),
            space_name: self.space_name.clone(),
            target_name: self.target_name.clone(),
        }
    }
}

/// Implementing the StatementValidator trait
///
/// # Refactoring Changes
/// The `validate` method accepts `Arc<Ast>` and `Arc<QueryContext>`.
impl StatementValidator for DescValidator {
    fn validate(
        &mut self,
        ast: Arc<Ast>,
        qctx: Arc<QueryContext>,
    ) -> Result<ValidationResult, ValidationError> {
        let desc_stmt = match &ast.stmt {
            crate::query::parser::ast::Stmt::Desc(desc_stmt) => desc_stmt,
            _ => {
                return Err(ValidationError::new(
                    "Expected DESCRIBE statement".to_string(),
                    ValidationErrorType::SemanticError,
                ));
            }
        };

        self.validate_impl(desc_stmt)?;

        // If space_name is empty, try to get it from query context
        if self.space_name.is_empty() {
            if let Some(ctx_space_name) = qctx.space_name() {
                self.space_name = ctx_space_name;
            }
        }

        // Create ValidationInfo with space_name for TAG/EDGE operations
        let mut info = crate::query::validator::structs::validation_info::ValidationInfo::new();
        if !self.space_name.is_empty() {
            info.semantic_info.space_name = Some(self.space_name.clone());
        }

        Ok(ValidationResult {
            success: true,
            errors: Vec::new(),
            inputs: self.inputs.clone(),
            outputs: self.outputs.clone(),
            warnings: Vec::new(),
            info: Some(info),
        })
    }

    fn statement_type(&self) -> StatementType {
        StatementType::DescribeSpace
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
        &self.expr_props
    }

    fn user_defined_vars(&self) -> &[String] {
        &self.user_defined_vars
    }
}

impl Default for DescValidator {
    fn default() -> Self {
        Self::new()
    }
}

/// SHOW CREATE statement validator
#[derive(Debug)]
pub struct ShowCreateValidator {
    target_type: ShowCreateTargetType,
    target_name: String,
    inputs: Vec<ColumnDef>,
    outputs: Vec<ColumnDef>,
    expr_props: ExpressionProps,
    user_defined_vars: Vec<String>,
}

#[derive(Debug, Clone)]
pub enum ShowCreateTargetType {
    Space,
    Tag,
    Edge,
    Index,
}

impl ShowCreateValidator {
    pub fn new() -> Self {
        Self {
            target_type: ShowCreateTargetType::Space,
            target_name: String::new(),
            inputs: Vec::new(),
            outputs: Vec::new(),
            expr_props: ExpressionProps::default(),
            user_defined_vars: Vec::new(),
        }
    }

    fn validate_impl(&mut self, stmt: &ShowCreateStmt) -> Result<(), ValidationError> {
        match &stmt.target {
            ShowCreateTarget::Space(name) => {
                self.target_type = ShowCreateTargetType::Space;
                self.target_name = name.clone();
            }
            ShowCreateTarget::Tag(name) => {
                self.target_type = ShowCreateTargetType::Tag;
                self.target_name = name.clone();
            }
            ShowCreateTarget::Edge(name) => {
                self.target_type = ShowCreateTargetType::Edge;
                self.target_name = name.clone();
            }
            ShowCreateTarget::Index(name) => {
                self.target_type = ShowCreateTargetType::Index;
                self.target_name = name.clone();
            }
        }

        self.outputs = vec![
            ColumnDef {
                name: "Target".to_string(),
                type_: ValueType::String,
            },
            ColumnDef {
                name: "CreateStatement".to_string(),
                type_: ValueType::String,
            },
        ];

        Ok(())
    }
}

/// Implementing the StatementValidator trait
///
/// # Refactoring Changes
/// The `validate` method accepts `Arc<Ast>` and `Arc<QueryContext>`.
impl StatementValidator for ShowCreateValidator {
    fn validate(
        &mut self,
        ast: Arc<Ast>,
        qctx: Arc<QueryContext>,
    ) -> Result<ValidationResult, ValidationError> {
        let show_create_stmt = match &ast.stmt {
            crate::query::parser::ast::Stmt::ShowCreate(show_create_stmt) => show_create_stmt,
            _ => {
                return Err(ValidationError::new(
                    "Expected SHOW CREATE statement".to_string(),
                    ValidationErrorType::SemanticError,
                ));
            }
        };

        self.validate_impl(show_create_stmt)?;

        let mut info = ValidationInfo::new();

        info.semantic_info.query_type = Some(format!("{:?}", self.target_type));

        // For TAG and EDGE types, set the current space name
        match self.target_type {
            ShowCreateTargetType::Tag | ShowCreateTargetType::Edge => {
                if let Some(space_name) = qctx.space_name() {
                    info.semantic_info.space_name = Some(space_name);
                }
            }
            _ => {}
        }

        Ok(ValidationResult::success_with_info(info))
    }

    fn statement_type(&self) -> StatementType {
        StatementType::ShowSpaces
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
        &self.expr_props
    }

    fn user_defined_vars(&self) -> &[String] {
        &self.user_defined_vars
    }
}

impl Default for ShowCreateValidator {
    fn default() -> Self {
        Self::new()
    }
}

/// The SHOW CONFIGS statement validator
#[derive(Debug)]
pub struct ShowConfigsValidator {
    module: Option<String>,
    inputs: Vec<ColumnDef>,
    outputs: Vec<ColumnDef>,
    expr_props: ExpressionProps,
    user_defined_vars: Vec<String>,
}

impl ShowConfigsValidator {
    pub fn new() -> Self {
        Self {
            module: None,
            inputs: Vec::new(),
            outputs: Vec::new(),
            expr_props: ExpressionProps::default(),
            user_defined_vars: Vec::new(),
        }
    }

    fn validate_impl(&mut self, stmt: &ShowConfigsStmt) -> Result<(), ValidationError> {
        self.module = stmt.module.clone();

        self.outputs = vec![
            ColumnDef {
                name: "Module".to_string(),
                type_: ValueType::String,
            },
            ColumnDef {
                name: "Name".to_string(),
                type_: ValueType::String,
            },
            ColumnDef {
                name: "Value".to_string(),
                type_: ValueType::String,
            },
        ];

        Ok(())
    }
}

/// Implementing the StatementValidator trait
///
/// # Refactoring Changes
/// The `validate` method accepts `Arc<Ast>` and `Arc<QueryContext>`.
impl StatementValidator for ShowConfigsValidator {
    fn validate(
        &mut self,
        ast: Arc<Ast>,
        _qctx: Arc<QueryContext>,
    ) -> Result<ValidationResult, ValidationError> {
        let show_configs_stmt = match &ast.stmt {
            crate::query::parser::ast::Stmt::ShowConfigs(show_configs_stmt) => show_configs_stmt,
            _ => {
                return Err(ValidationError::new(
                    "Expected SHOW CONFIGS statement".to_string(),
                    ValidationErrorType::SemanticError,
                ));
            }
        };

        self.validate_impl(show_configs_stmt)?;

        let mut info = ValidationInfo::new();

        info.semantic_info.query_type = Some("ShowConfigs".to_string());

        Ok(ValidationResult::success_with_info(info))
    }

    fn statement_type(&self) -> StatementType {
        StatementType::ShowSpaces
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
        &self.expr_props
    }

    fn user_defined_vars(&self) -> &[String] {
        &self.user_defined_vars
    }
}

impl Default for ShowConfigsValidator {
    fn default() -> Self {
        Self::new()
    }
}

/// The SHOW SESSIONS statement validator
#[derive(Debug)]
pub struct ShowSessionsValidator {
    inputs: Vec<ColumnDef>,
    outputs: Vec<ColumnDef>,
    expr_props: ExpressionProps,
    user_defined_vars: Vec<String>,
}

impl ShowSessionsValidator {
    pub fn new() -> Self {
        Self {
            inputs: Vec::new(),
            outputs: vec![
                ColumnDef {
                    name: "SessionId".to_string(),
                    type_: ValueType::Int,
                },
                ColumnDef {
                    name: "UserName".to_string(),
                    type_: ValueType::String,
                },
                ColumnDef {
                    name: "SpaceName".to_string(),
                    type_: ValueType::String,
                },
                ColumnDef {
                    name: "CreateTime".to_string(),
                    type_: ValueType::String,
                },
                ColumnDef {
                    name: "UpdateTime".to_string(),
                    type_: ValueType::String,
                },
            ],
            expr_props: ExpressionProps::default(),
            user_defined_vars: Vec::new(),
        }
    }

    fn validate_impl(&mut self, _stmt: &ShowSessionsStmt) -> Result<(), ValidationError> {
        Ok(())
    }
}

/// Implementing the StatementValidator trait
///
/// # Refactoring Changes
/// The `validate` method accepts `Arc<Ast>` and `Arc<QueryContext>`.
impl StatementValidator for ShowSessionsValidator {
    fn validate(
        &mut self,
        ast: Arc<Ast>,
        _qctx: Arc<QueryContext>,
    ) -> Result<ValidationResult, ValidationError> {
        let show_sessions_stmt = match &ast.stmt {
            crate::query::parser::ast::Stmt::ShowSessions(show_sessions_stmt) => show_sessions_stmt,
            _ => {
                return Err(ValidationError::new(
                    "Expected SHOW SESSIONS statement".to_string(),
                    ValidationErrorType::SemanticError,
                ));
            }
        };

        self.validate_impl(show_sessions_stmt)?;

        let mut info = ValidationInfo::new();

        info.semantic_info.query_type = Some("ShowSessions".to_string());

        Ok(ValidationResult::success_with_info(info))
    }

    fn statement_type(&self) -> StatementType {
        StatementType::ShowSpaces
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
        &self.expr_props
    }

    fn user_defined_vars(&self) -> &[String] {
        &self.user_defined_vars
    }
}

impl Default for ShowSessionsValidator {
    fn default() -> Self {
        Self::new()
    }
}

/// SHOW QUERIES statement validator
#[derive(Debug)]
pub struct ShowQueriesValidator {
    inputs: Vec<ColumnDef>,
    outputs: Vec<ColumnDef>,
    expr_props: ExpressionProps,
    user_defined_vars: Vec<String>,
}

impl ShowQueriesValidator {
    pub fn new() -> Self {
        Self {
            inputs: Vec::new(),
            outputs: vec![
                ColumnDef {
                    name: "SessionID".to_string(),
                    type_: ValueType::Int,
                },
                ColumnDef {
                    name: "ExecutionPlanID".to_string(),
                    type_: ValueType::Int,
                },
                ColumnDef {
                    name: "User".to_string(),
                    type_: ValueType::String,
                },
                ColumnDef {
                    name: "Query".to_string(),
                    type_: ValueType::String,
                },
                ColumnDef {
                    name: "StartTime".to_string(),
                    type_: ValueType::String,
                },
            ],
            expr_props: ExpressionProps::default(),
            user_defined_vars: Vec::new(),
        }
    }

    fn validate_impl(&mut self, _stmt: &ShowQueriesStmt) -> Result<(), ValidationError> {
        Ok(())
    }
}

/// Implementing the StatementValidator trait
///
/// # Refactoring Changes
/// The `validate` method accepts `Arc<Ast>` and `Arc<QueryContext>`.
impl StatementValidator for ShowQueriesValidator {
    fn validate(
        &mut self,
        ast: Arc<Ast>,
        _qctx: Arc<QueryContext>,
    ) -> Result<ValidationResult, ValidationError> {
        let show_queries_stmt = match &ast.stmt {
            crate::query::parser::ast::Stmt::ShowQueries(show_queries_stmt) => show_queries_stmt,
            _ => {
                return Err(ValidationError::new(
                    "Expected SHOW QUERIES statement".to_string(),
                    ValidationErrorType::SemanticError,
                ));
            }
        };

        self.validate_impl(show_queries_stmt)?;

        let mut info = ValidationInfo::new();

        info.semantic_info.query_type = Some("ShowQueries".to_string());

        Ok(ValidationResult::success_with_info(info))
    }

    fn statement_type(&self) -> StatementType {
        StatementType::ShowSpaces
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
        &self.expr_props
    }

    fn user_defined_vars(&self) -> &[String] {
        &self.user_defined_vars
    }
}

impl Default for ShowQueriesValidator {
    fn default() -> Self {
        Self::new()
    }
}

/// KILL QUERY Statement Validator
#[derive(Debug)]
pub struct KillQueryValidator {
    session_id: i64,
    plan_id: i64,
    inputs: Vec<ColumnDef>,
    outputs: Vec<ColumnDef>,
    expr_props: ExpressionProps,
    user_defined_vars: Vec<String>,
}

impl KillQueryValidator {
    pub fn new() -> Self {
        Self {
            session_id: 0,
            plan_id: 0,
            inputs: Vec::new(),
            outputs: vec![ColumnDef {
                name: "Result".to_string(),
                type_: ValueType::String,
            }],
            expr_props: ExpressionProps::default(),
            user_defined_vars: Vec::new(),
        }
    }

    fn validate_impl(&mut self, stmt: &KillQueryStmt) -> Result<(), ValidationError> {
        self.session_id = stmt.session_id;
        self.plan_id = stmt.plan_id;
        Ok(())
    }
}

/// Implementing the StatementValidator trait
///
/// # Refactoring Changes
/// The `validate` method accepts `Arc<Ast>` and `Arc<QueryContext>`.
impl StatementValidator for KillQueryValidator {
    fn validate(
        &mut self,
        ast: Arc<Ast>,
        _qctx: Arc<QueryContext>,
    ) -> Result<ValidationResult, ValidationError> {
        let kill_query_stmt = match &ast.stmt {
            crate::query::parser::ast::Stmt::KillQuery(kill_query_stmt) => kill_query_stmt,
            _ => {
                return Err(ValidationError::new(
                    "Expected KILL QUERY statement".to_string(),
                    ValidationErrorType::SemanticError,
                ));
            }
        };

        self.validate_impl(kill_query_stmt)?;

        let mut info = ValidationInfo::new();

        info.semantic_info.query_type = Some("KillQuery".to_string());

        Ok(ValidationResult::success_with_info(info))
    }

    fn statement_type(&self) -> StatementType {
        StatementType::ShowSpaces
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
        &self.expr_props
    }

    fn user_defined_vars(&self) -> &[String] {
        &self.user_defined_vars
    }
}

impl Default for KillQueryValidator {
    fn default() -> Self {
        Self::new()
    }
}

/// CLEAR SPACE Statement Validator
#[derive(Debug)]
pub struct ClearSpaceValidator {
    space_name: String,
    inputs: Vec<ColumnDef>,
    outputs: Vec<ColumnDef>,
    expr_props: ExpressionProps,
    user_defined_vars: Vec<String>,
}

impl ClearSpaceValidator {
    pub fn new() -> Self {
        Self {
            space_name: String::new(),
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
        stmt: &crate::query::parser::ast::stmt::ClearSpaceStmt,
    ) -> Result<(), ValidationError> {
        self.space_name = stmt.space_name.clone();
        Ok(())
    }
}

/// Implementing the StatementValidator trait
impl StatementValidator for ClearSpaceValidator {
    fn validate(
        &mut self,
        ast: Arc<Ast>,
        _qctx: Arc<QueryContext>,
    ) -> Result<ValidationResult, ValidationError> {
        let clear_space_stmt = match &ast.stmt {
            crate::query::parser::ast::Stmt::ClearSpace(clear_space_stmt) => clear_space_stmt,
            _ => {
                return Err(ValidationError::new(
                    "Expected CLEAR SPACE statement".to_string(),
                    ValidationErrorType::SemanticError,
                ));
            }
        };

        self.validate_impl(clear_space_stmt)?;

        let mut info = ValidationInfo::new();
        info.semantic_info.query_type = Some("ClearSpace".to_string());

        Ok(ValidationResult::success_with_info(info))
    }

    fn statement_type(&self) -> StatementType {
        StatementType::ClearSpace
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
        &self.expr_props
    }

    fn user_defined_vars(&self) -> &[String] {
        &self.user_defined_vars
    }
}

impl Default for ClearSpaceValidator {
    fn default() -> Self {
        Self::new()
    }
}
