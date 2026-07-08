//! Query Validator Module
//! Corresponds to the functionality of NebulaGraph's src/graph/validator.
//! Used to verify the legitimacy of the AST (Abstract Syntax Tree).
//!
//! Design Description:
//! Using the trait + enum pattern to manage validators
//! The "trait" defines a unified interface.
//! Implementation of static distribution using enumeration
//! The Factory Pattern is used to create validators.

// Error types
pub mod error;

// Context module
pub mod context;

// Data Structures Module
pub mod structs;

// Validation Policy Submodule
pub mod strategies;

// Statement-level validator
pub mod statements;

// Sentence-level validator
pub mod clauses;

// DDL Validator
pub mod ddl;

// DML Validator
pub mod dml;

// Full-text search validator
pub mod fulltext_validator;

// Vector search validator
pub mod vector_validator;

// Tool Validator
pub mod utility;

// Auxiliary tools
pub mod helpers;

// Definition of the `Validator` trait
pub mod validator_trait;

// Validator Enumeration
pub mod validator_enum;

// Assignment Validator
pub mod assignment_validator;

// Expression Analyzer
pub mod expression_analyzer;

// Export data structure
pub use structs::{
    AggregateCallInfo, AliasType, ClauseKind, HintSeverity, IndexHint, MatchClauseContext,
    MatchStepRange, OptimizationHint, PaginationContext, Path, PathAnalysis, QueryPart,
    ReturnClauseContext, SemanticInfo, UnwindClauseContext, ValidatedStatement, ValidationInfo,
    WhereClauseContext, WithClauseContext, YieldClauseContext,
};

// Re-export the YieldColumn from the core.
pub use crate::core::YieldColumn;

// Exporting a new verifier system (trait + enumeration)
pub use validator_enum::{Validator, ValidatorCollection};
pub use validator_trait::{
    is_global_statement_type, ColumnDef, EdgeProperty, ExpressionProps, InputProperty,
    StatementType, StatementValidator, TagProperty, ValidationResult, ValueType, VarProperty,
};

// Export a statement-level verifier
pub use statements::{
    CreateValidator, DeleteValidator, FetchEdgesValidator, FetchVerticesValidator,
    FindPathValidator, GetSubgraphValidator, GoValidator, InsertEdgesValidator,
    InsertVerticesValidator, LookupValidator, MatchValidator, MergeValidator, RemoveValidator,
    SetItem, SetStatementType, SetValidator, UnwindValidator, UpdateValidator, ValidatedSet,
    ValidatedSetItem, ValidatedUnwind,
};

// Export the clause-level validator
pub use clauses::{
    GroupByValidator, LimitValidator, OrderByValidator, OrderColumn, ReturnValidator,
    SequentialStatement, SequentialValidator, ValidatedGroupBy, ValidatedYield, WithValidator,
    YieldValidator,
};

// Export the DDL validator.
pub use ddl::{
    AlterTargetType, AlterValidator, ClearSpaceValidator, CreateEdgeValidator, CreateTagValidator,
    DescTargetType, DescValidator, DropTargetType, DropValidator, KillQueryValidator,
    ShowConfigsValidator, ShowCreateValidator, ShowQueriesValidator, ShowSessionsValidator,
    ShowTargetType, ShowValidator, ValidatedAlter, ValidatedCreateEdge, ValidatedCreateTag,
    ValidatedDesc, ValidatedDrop, ValidatedShow,
};

// Export the DML validator
pub use dml::{
    ColumnInfo, PipeValidator, QueryValidator, SetOperationValidator, UseValidator,
    ValidatedSetOperation, ValidatedUse,
};

// Export Tool Validator
pub use utility::{
    AlterUserValidator, ChangePasswordValidator, CreateUserValidator, DescribeUserValidator,
    DropUserValidator, ExplainValidator, GrantValidator, ProfileValidator, RevokeValidator,
    ShowRolesValidator, ShowUsersValidator, UpdateConfigsValidator, ValidatedExplain,
    ValidatedGrant, ValidatedUser,
};

// Export assistance tools
pub use helpers::SchemaValidator;

// Export the assignment validator.
pub use assignment_validator::{AssignmentValidator, ValidatedAssignment};

// Export Expression Analyzer
pub use expression_analyzer::{ExpressionAnalysisResult, ExpressionAnalyzer};
