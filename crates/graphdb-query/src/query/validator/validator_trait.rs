//! The “Validator Unified Trait” definition
//! Define the standard interface for all statement validators.
//! This is the core of the new verifier system, which replaces the previous decentralized design.
//!
//! Design principles:
//! 1. Maintain all functions (lifecycle validation, context management, permission checks, etc.).
//! 2. Use traits to unify interfaces, which facilitates extension.
//! 3. Use enumerations to manage different types of validators, in order to avoid dynamic distribution.
//!
//! # Refactoring changes
//! Replace `&mut AstContext` with `Arc<QueryContext>`.
//! Replace `Arc<Stmt>` with `Arc<Ast>` to unify the way AST (Abstract Syntax Tree) is passed.
//! The verifier has access to the context of the expression.

use std::sync::Arc;

use crate::query::parser::ast::stmt::Ast;
use crate::query::validator::error::ValidationError;
use crate::query::validator::structs::validation_info::ValidationInfo;
use crate::query::validator::structs::AliasType;
use crate::query::QueryContext;

/// Column definition
#[derive(Debug, Clone)]
pub struct ColumnDef {
    pub name: String,
    pub type_: ValueType,
}

/// Value type enumeration
#[derive(Debug, Clone, PartialEq)]
pub enum ValueType {
    Empty,
    Unknown,
    Bool,
    Int,
    Float,
    String,
    Date,
    Time,
    DateTime,
    Vertex,
    Edge,
    Path,
    List,
    Map,
    Set,
    Null,
}

impl ValueType {
    /// Convert from DataType to ValueType
    pub fn from_data_type(data_type: &crate::core::DataType) -> Self {
        use crate::core::DataType;
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

    /// Convert to DataType
    pub fn to_data_type(&self) -> crate::core::DataType {
        use crate::core::DataType;
        match self {
            ValueType::Empty => DataType::Empty,
            ValueType::Unknown => DataType::Empty,
            ValueType::Bool => DataType::Bool,
            ValueType::Int => DataType::Int,
            ValueType::Float => DataType::Float,
            ValueType::String => DataType::String,
            ValueType::Date => DataType::Date,
            ValueType::Time => DataType::Time,
            ValueType::DateTime => DataType::DateTime,
            ValueType::Vertex => DataType::Vertex,
            ValueType::Edge => DataType::Edge,
            ValueType::Path => DataType::Path,
            ValueType::List => DataType::List,
            ValueType::Map => DataType::Map,
            ValueType::Set => DataType::Set,
            ValueType::Null => DataType::Null,
        }
    }
}

/// Expression properties
#[derive(Debug, Clone, Default)]
pub struct ExpressionProps {
    pub input_props: Vec<InputProperty>,
    pub var_props: Vec<VarProperty>,
    pub tag_props: Vec<TagProperty>,
    pub edge_props: Vec<EdgeProperty>,
}

#[derive(Debug, Clone)]
pub struct InputProperty {
    pub prop_name: String,
    pub type_: ValueType,
}

#[derive(Debug, Clone)]
pub struct VarProperty {
    pub var_name: String,
    pub prop_name: String,
    pub type_: ValueType,
}

#[derive(Debug, Clone)]
pub struct TagProperty {
    pub tag_name: String,
    pub prop_name: String,
    pub type_: ValueType,
}

#[derive(Debug, Clone)]
pub struct EdgeProperty {
    pub edge_type: i32,
    pub prop_name: String,
    pub type_: ValueType,
}

/// Statement Type Enumeration
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StatementType {
    // Query class
    Match,
    Go,
    FetchVertices,
    FetchEdges,
    Lookup,
    FindPath,
    GetSubgraph,

    // Data Manipulation Language (DML)
    InsertVertices,
    InsertEdges,
    Update,
    Delete,

    // Data Definition Language (DDL)
    Create,
    CreateSpace,
    CreateTag,
    CreateEdge,
    CreateTagIndex,
    CreateEdgeIndex,
    Drop,
    DropSpace,
    DropTag,
    DropEdge,
    DropTagIndex,
    DropEdgeIndex,
    Alter,
    AlterTag,
    AlterEdge,

    // Session Management
    Use,

    // Pipelines and clauses
    Pipe,
    Yield,
    OrderBy,
    Limit,
    Unwind,
    Set,
    Sequential,

    // Management statements
    Show,
    ShowSpaces,
    ShowTags,
    ShowEdges,
    Desc,
    DescribeSpace,
    DescribeTag,
    DescribeEdge,
    ShowCreate,
    ShowConfigs,
    ShowSessions,
    ShowQueries,
    KillQuery,

    // Permission class statements
    CreateUser,
    DropUser,
    AlterUser,
    ChangePassword,
    Grant,
    Revoke,
    DescribeUser,
    ShowUsers,
    ShowRoles,

    // Other sentences
    GroupBy,
    Assignment,
    Explain,
    Profile,
    SetOperation,

    // New sentence type added
    Query,
    Merge,
    Return,
    With,
    Remove,
    UpdateConfigs,
    ClearSpace,

    // Full-text Search statements
    CreateFulltextIndex,
    DropFulltextIndex,
    AlterFulltextIndex,
    ShowFulltextIndex,
    DescribeFulltextIndex,
    Search,
    LookupFulltext,
    MatchFulltext,

    // Vector Search statements
    CreateVectorIndex,
    DropVectorIndex,
    SearchVector,
    LookupVector,
    MatchVector,

    // Transaction statements
    BeginTransaction,
    CommitTransaction,
    RollbackTransaction,
}

impl StatementType {
    pub fn as_str(&self) -> &'static str {
        match self {
            // Query class
            StatementType::Match => "MATCH",
            StatementType::Go => "GO",
            StatementType::FetchVertices => "FETCH_VERTICES",
            StatementType::FetchEdges => "FETCH_EDGES",
            StatementType::Lookup => "LOOKUP",
            StatementType::FindPath => "FIND_PATH",
            StatementType::GetSubgraph => "GET_SUBGRAPH",

            // Data Manipulation Language (DML)
            StatementType::InsertVertices => "INSERT_VERTICES",
            StatementType::InsertEdges => "INSERT_EDGES",
            StatementType::Update => "UPDATE",
            StatementType::Delete => "DELETE",

            // Data Definition Language (DDL)
            StatementType::Create => "CREATE",
            StatementType::CreateSpace => "CREATE_SPACE",
            StatementType::CreateTag => "CREATE_TAG",
            StatementType::CreateEdge => "CREATE_EDGE",
            StatementType::CreateTagIndex => "CREATE_TAG_INDEX",
            StatementType::CreateEdgeIndex => "CREATE_EDGE_INDEX",
            StatementType::Drop => "DROP",
            StatementType::DropSpace => "DROP_SPACE",
            StatementType::DropTag => "DROP_TAG",
            StatementType::DropEdge => "DROP_EDGE",
            StatementType::DropTagIndex => "DROP_TAG_INDEX",
            StatementType::DropEdgeIndex => "DROP_EDGE_INDEX",
            StatementType::Alter => "ALTER",
            StatementType::AlterTag => "ALTER_TAG",
            StatementType::AlterEdge => "ALTER_EDGE",

            // Session Management
            StatementType::Use => "USE",

            // Pipelines and clauses
            StatementType::Pipe => "PIPE",
            StatementType::Yield => "YIELD",
            StatementType::OrderBy => "ORDER_BY",
            StatementType::Limit => "LIMIT",
            StatementType::Unwind => "UNWIND",
            StatementType::Set => "SET",
            StatementType::Sequential => "SEQUENTIAL",

            // Management statements
            StatementType::Show => "SHOW",
            StatementType::ShowSpaces => "SHOW_SPACES",
            StatementType::ShowTags => "SHOW_TAGS",
            StatementType::ShowEdges => "SHOW_EDGES",
            StatementType::Desc => "DESC",
            StatementType::DescribeSpace => "DESCRIBE_SPACE",
            StatementType::DescribeTag => "DESCRIBE_TAG",
            StatementType::DescribeEdge => "DESCRIBE_EDGE",
            StatementType::ShowCreate => "SHOW_CREATE",
            StatementType::ShowConfigs => "SHOW_CONFIGS",
            StatementType::ShowSessions => "SHOW_SESSIONS",
            StatementType::ShowQueries => "SHOW_QUERIES",
            StatementType::KillQuery => "KILL_QUERY",

            // Permission class statements
            StatementType::CreateUser => "CREATE_USER",
            StatementType::DropUser => "DROP_USER",
            StatementType::AlterUser => "ALTER_USER",
            StatementType::ChangePassword => "CHANGE_PASSWORD",
            StatementType::Grant => "GRANT",
            StatementType::Revoke => "REVOKE",
            StatementType::DescribeUser => "DESCRIBE_USER",
            StatementType::ShowUsers => "SHOW_USERS",
            StatementType::ShowRoles => "SHOW_ROLES",

            // Other sentences
            StatementType::GroupBy => "GROUP_BY",
            StatementType::Assignment => "ASSIGNMENT",
            StatementType::Explain => "EXPLAIN",
            StatementType::Profile => "PROFILE",
            StatementType::SetOperation => "SET_OPERATION",

            // New sentence type added
            StatementType::Query => "QUERY",
            StatementType::Merge => "MERGE",
            StatementType::Return => "RETURN",
            StatementType::With => "WITH",
            StatementType::Remove => "REMOVE",
            StatementType::UpdateConfigs => "UPDATE_CONFIGS",
            StatementType::ClearSpace => "CLEAR_SPACE",

            // Full-text Search statements
            StatementType::CreateFulltextIndex => "CREATE_FULLTEXT_INDEX",
            StatementType::DropFulltextIndex => "DROP_FULLTEXT_INDEX",
            StatementType::AlterFulltextIndex => "ALTER_FULLTEXT_INDEX",
            StatementType::ShowFulltextIndex => "SHOW_FULLTEXT_INDEX",
            StatementType::DescribeFulltextIndex => "DESCRIBE_FULLTEXT_INDEX",
            StatementType::Search => "SEARCH",
            StatementType::LookupFulltext => "LOOKUP_FULLTEXT",
            StatementType::MatchFulltext => "MATCH_FULLTEXT",

            // Vector Search statements
            StatementType::CreateVectorIndex => "CREATE_VECTOR_INDEX",
            StatementType::DropVectorIndex => "DROP_VECTOR_INDEX",
            StatementType::SearchVector => "SEARCH_VECTOR",
            StatementType::LookupVector => "LOOKUP_VECTOR",
            StatementType::MatchVector => "MATCH_VECTOR",

            // Transaction statements
            StatementType::BeginTransaction => "BEGIN_TRANSACTION",
            StatementType::CommitTransaction => "COMMIT_TRANSACTION",
            StatementType::RollbackTransaction => "ROLLBACK_TRANSACTION",
        }
    }

    pub fn is_global_statement(&self) -> bool {
        is_global_statement_type(*self)
    }

    pub fn is_ddl(&self) -> bool {
        matches!(
            self,
            StatementType::Create
                | StatementType::CreateSpace
                | StatementType::CreateTag
                | StatementType::CreateEdge
                | StatementType::Drop
                | StatementType::DropSpace
                | StatementType::DropTag
                | StatementType::DropEdge
                | StatementType::Alter
                | StatementType::AlterTag
                | StatementType::AlterEdge
        )
    }

    pub fn is_dml(&self) -> bool {
        matches!(
            self,
            StatementType::InsertVertices
                | StatementType::InsertEdges
                | StatementType::Update
                | StatementType::Delete
        )
    }
}

/// Validation results
/// All information from the verification phase, to be provided to the planning phase.
#[derive(Debug, Clone)]
pub struct ValidationResult {
    pub success: bool,
    pub errors: Vec<ValidationError>,
    pub inputs: Vec<ColumnDef>,
    pub outputs: Vec<ColumnDef>,
    pub warnings: Vec<String>,
    /// Detailed verification information (optional, for use with new interfaces)
    pub info: Option<ValidationInfo>,
}

impl ValidationResult {
    /// Created a successful verification result (compatible with the old interface).
    pub fn success(inputs: Vec<ColumnDef>, outputs: Vec<ColumnDef>) -> Self {
        Self {
            success: true,
            errors: Vec::new(),
            inputs,
            outputs,
            warnings: Vec::new(),
            info: None,
        }
    }

    /// Created a successful verification result (new interface, including detailed verification information).
    pub fn success_with_info(info: ValidationInfo) -> Self {
        Self {
            success: true,
            errors: Vec::new(),
            inputs: Vec::new(),
            outputs: Vec::new(),
            warnings: Vec::new(),
            info: Some(info),
        }
    }

    pub fn failure(errors: Vec<ValidationError>) -> Self {
        Self {
            success: false,
            errors,
            inputs: Vec::new(),
            outputs: Vec::new(),
            warnings: Vec::new(),
            info: None,
        }
    }

    pub fn add_warning(&mut self, warning: String) {
        self.warnings.push(warning);
    }

    pub fn merge(&mut self, other: ValidationResult) {
        self.errors.extend(other.errors);
        self.warnings.extend(other.warnings);
        if !other.success {
            self.success = false;
        }
    }

    /// Check whether it contains verification information.
    pub fn has_info(&self) -> bool {
        self.info.is_some()
    }

    /// Obtain verification information
    pub fn info(&self) -> Option<&ValidationInfo> {
        self.info.as_ref()
    }
}

/// A unified interface for all statement validators
///
/// Design principles:
/// Simplify the interface by only retaining the core methods.
/// The validation of the lifecycle is uniformly managed by the Validator enumeration.
/// 3. Use static distribution in place of dynamic distribution.
/// 4. Use Arc<QueryContext> as the validation context.
///
/// # Refactoring changes
/// The `validate` method now accepts `Arc<Ast>` and `Arc<QueryContext>` as parameters.
/// The validator no longer modifies the context directly; instead, it passes the result through the return value.
/// The `validate` method returns detailed validation results that include `ValidationInfo`.
/// Use Arc<Ast> to share ownership of the AST (Abstract Syntax Tree), thereby avoiding unnecessary cloning.
/// The verifier has access to the context of the expression.
pub trait StatementValidator {
    /// Execute the validation logic.
    /// Return the verification results that include detailed verification information.
    ///
    /// # Parameters
    /// The AST (Abstract Syntax Tree) to be verified, which contains the context of statements and expressions.
    /// `qctx`: Query context, which contains the symbol table, space information, and other data.
    fn validate(
        &mut self,
        _ast: Arc<Ast>,
        _qctx: Arc<QueryContext>,
    ) -> Result<ValidationResult, ValidationError> {
        // Default implementation: Building the basic ValidationInfo
        let mut info = ValidationInfo::new();

        // Convert the information in the input and output columns into validation information.
        for input in self.inputs() {
            info.add_alias(input.name.clone(), AliasType::Variable);
        }
        for output in self.outputs() {
            info.add_alias(output.name.clone(), AliasType::Variable);
        }

        Ok(ValidationResult::success_with_info(info))
    }

    /// Determine the type of the sentence.
    fn statement_type(&self) -> StatementType;

    /// Obtain the definition of the input column.
    fn inputs(&self) -> &[ColumnDef];

    /// Obtain the definitions of the output columns.
    fn outputs(&self) -> &[ColumnDef];

    /// Determine whether it is a global statement (no need to pre-select a scope).
    fn is_global_statement(&self) -> bool;

    /// Obtain the name of the validator.
    fn validator_name(&self) -> String {
        format!("{}Validator", self.statement_type().as_str())
    }

    /// Obtain the properties of the expression
    fn expression_props(&self) -> &ExpressionProps;

    /// Obtain a list of user-defined variables
    fn user_defined_vars(&self) -> &[String];
}

/// Determine whether the type of the statement is a global statement.
pub fn is_global_statement_type(stmt_type: StatementType) -> bool {
    matches!(
        stmt_type,
        StatementType::CreateSpace
            | StatementType::DropSpace
            | StatementType::ShowSpaces
            | StatementType::DescribeSpace
            | StatementType::Use
            // Management statements
            | StatementType::Show
            | StatementType::ShowTags
            | StatementType::ShowEdges
            | StatementType::Desc
            | StatementType::ShowCreate
            | StatementType::ShowConfigs
            | StatementType::ShowSessions
            | StatementType::ShowQueries
            | StatementType::KillQuery
            // Permission class statements
            | StatementType::CreateUser
            | StatementType::DropUser
            | StatementType::AlterUser
            | StatementType::ChangePassword
            | StatementType::Grant
            | StatementType::Revoke
            | StatementType::DescribeUser
            | StatementType::ShowUsers
            | StatementType::ShowRoles
    )
}
