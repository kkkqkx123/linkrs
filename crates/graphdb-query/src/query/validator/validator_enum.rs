//! Validator Enumeration
//! Use an enumeration to uniformly manage all types of validators.

use std::sync::Arc;

use crate::core::metadata::SchemaManager;
use crate::query::parser::ast::stmt::Ast;
use crate::query::parser::ast::{CreateTarget, FetchTarget, Stmt};
use crate::query::validator::validator_trait::{
    ColumnDef, ExpressionProps, StatementType, StatementValidator, ValidationResult,
};
use crate::query::QueryContext;

// Import the specific validator.
use crate::query::validator::assignment_validator::AssignmentValidator;
use crate::query::validator::clauses::group_by_validator::GroupByValidator;
use crate::query::validator::clauses::limit_validator::LimitValidator;
use crate::query::validator::clauses::order_by_validator::OrderByValidator;
use crate::query::validator::clauses::return_validator::ReturnValidator;
use crate::query::validator::clauses::sequential_validator::SequentialValidator;
use crate::query::validator::clauses::with_validator::WithValidator;
use crate::query::validator::clauses::yield_validator::YieldValidator;
use crate::query::validator::ddl::admin_validator::{
    ClearSpaceValidator, DescValidator, KillQueryValidator, ShowConfigsValidator,
    ShowCreateValidator, ShowQueriesValidator, ShowSessionsValidator, ShowValidator,
};
use crate::query::validator::ddl::alter_validator::AlterValidator;
use crate::query::validator::ddl::create_edge_validator::CreateEdgeValidator;
use crate::query::validator::ddl::create_tag_validator::CreateTagValidator;
use crate::query::validator::ddl::drop_validator::DropValidator;
use crate::query::validator::ddl::index_validator::CreateIndexValidator;
use crate::query::validator::dml::pipe_validator::PipeValidator;
use crate::query::validator::dml::query_validator::QueryValidator;
use crate::query::validator::dml::set_operation_validator::SetOperationValidator;
use crate::query::validator::dml::use_validator::UseValidator;
use crate::query::validator::fulltext_validator::FulltextValidator;
use crate::query::validator::statements::create_validator::CreateValidator;
use crate::query::validator::statements::delete_validator::DeleteValidator;
use crate::query::validator::statements::fetch_edges_validator::FetchEdgesValidator;
use crate::query::validator::statements::fetch_vertices_validator::FetchVerticesValidator;
use crate::query::validator::statements::find_path_validator::FindPathValidator;
use crate::query::validator::statements::get_subgraph_validator::GetSubgraphValidator;
use crate::query::validator::statements::go_validator::GoValidator;
use crate::query::validator::statements::insert_edges_validator::InsertEdgesValidator;
use crate::query::validator::statements::insert_vertices_validator::InsertVerticesValidator;
use crate::query::validator::statements::lookup_validator::LookupValidator;
use crate::query::validator::statements::match_validator::MatchValidator;
use crate::query::validator::statements::merge_validator::MergeValidator;
use crate::query::validator::statements::remove_validator::RemoveValidator;
use crate::query::validator::statements::set_validator::SetValidator;
use crate::query::validator::statements::transaction_validator::TransactionValidator;
use crate::query::validator::statements::unwind_validator::UnwindValidator;
use crate::query::validator::statements::update_validator::UpdateValidator;
use crate::query::validator::utility::acl_validator::{
    AlterUserValidator, ChangePasswordValidator, CreateUserValidator, DescribeUserValidator,
    DropUserValidator, GrantValidator, RevokeValidator, ShowRolesValidator, ShowUsersValidator,
};
use crate::query::validator::utility::explain_validator::{ExplainValidator, ProfileValidator};
use crate::query::validator::utility::update_config_validator::UpdateConfigsValidator;
use crate::query::validator::vector_validator::VectorValidator;

/// Unified Validator Enumeration
///
/// Design advantages:
/// Determine the type during the compilation phase to avoid the overhead associated with dynamic distribution.
/// 2. Unified interfaces for easier management and expansion.
/// 3. Mode matching is supported, which facilitates the processing of specific validators.
/// 4. Maintain the full functionality of the validation lifecycle.
#[derive(Debug)]
pub enum Validator {
    // Management class validators
    /// SHOW statement validator
    Show(ShowValidator),
    /// The DESCRIBE statement validator
    Desc(DescValidator),
    /// SHOW CREATE statement validator
    ShowCreate(ShowCreateValidator),
    /// The SHOW CONFIGS statement validator
    ShowConfigs(ShowConfigsValidator),
    /// The SHOW SESSIONS statement validator
    ShowSessions(ShowSessionsValidator),
    /// SHOW QUERIES statement validator
    ShowQueries(ShowQueriesValidator),
    /// KILL QUERY Statement Validator
    KillQuery(KillQueryValidator),

    // Permission class validator
    /// CREATE USER statement validator
    CreateUser(CreateUserValidator),
    /// DROP USER statement validator
    DropUser(DropUserValidator),
    /// ALTER USER statement validator
    AlterUser(AlterUserValidator),
    /// CHANGE PASSWORD statement validator
    ChangePassword(ChangePasswordValidator),
    /// GRANT statement validator
    Grant(GrantValidator),
    /// REVOKE statement validator
    Revoke(RevokeValidator),
    /// The “DESCRIBE USER” statement validator ensures that the provided user information is valid and meets the required criteria. It performs various checks to verify the accuracy, completeness, and consistency of the user data, such as checking the username, password, email address, and other relevant fields. If the user data is invalid or does not meet the specified requirements, the validator generates an error message indicating the issues with the data. This validation process helps to maintain the security and integrity of the system by preventing unauthorized access to user accounts.
    DescribeUser(DescribeUserValidator),
    /// SHOW USERS Statement Validator
    ShowUsers(ShowUsersValidator),
    /// SHOW ROLES statement validator
    ShowRoles(ShowRolesValidator),

    // DDL Validator
    /// ALTER statement validator
    Alter(AlterValidator),
    /// DROP statement validator
    Drop(DropValidator),
    /// CREATE TAG INDEX statement validator
    CreateTagIndex(CreateIndexValidator),
    /// CREATE EDGE INDEX statement validator
    CreateEdgeIndex(CreateIndexValidator),
    /// CREATE TAG statement validator
    CreateTag(CreateTagValidator),
    /// CREATE EDGE statement validator
    CreateEdge(CreateEdgeValidator),
    /// CREATE statement validator
    Create(CreateValidator),

    // DML Validator
    /// USE statement validator
    Use(UseValidator),
    /// SET statement validator
    Set(SetValidator),
    /// ASSIGNMENT Statement Validator
    Assignment(AssignmentValidator),
    /// PIPE statement validator
    Pipe(PipeValidator),
    /// QUERY Statement Validator
    Query(QueryValidator),
    /// SET OPERATION statement validator
    SetOperation(SetOperationValidator),

    // Query class validator
    /// MATCH statement validator
    Match(MatchValidator),
    /// LOOKUP Statement Validator
    Lookup(LookupValidator),
    /// GO Statement Validator
    Go(GoValidator),
    /// FIND PATH Statement Validator
    FindPath(FindPathValidator),
    /// GET SUBGRAPH statement validator
    GetSubgraph(GetSubgraphValidator),
    /// The FETCH VERTICES statement validator
    FetchVertices(FetchVerticesValidator),
    /// The FETCH EDGES statement validator
    FetchEdges(FetchEdgesValidator),
    /// INSERT VERTICES statement validator
    InsertVertices(InsertVerticesValidator),
    /// INSERT EDGES statement validator
    InsertEdges(InsertEdgesValidator),
    /// UPDATE Statement Validator
    Update(UpdateValidator),
    /// DELETE Statement Validator
    Delete(DeleteValidator),
    /// MERGE statement validator
    Merge(MergeValidator),
    /// REMOVE Statement Validator
    Remove(RemoveValidator),
    /// UNWIND Statement Validator
    Unwind(UnwindValidator),

    // Clause type validator
    /// ORDER BY statement validator
    OrderBy(OrderByValidator),
    /// GROUP BY statement validator
    GroupBy(GroupByValidator),
    /// YIELD statement validator
    Yield(YieldValidator),
    /// RETURN statement validator
    Return(ReturnValidator),
    /// WITH statement validator
    With(WithValidator),
    /// LIMIT statement validator
    Limit(LimitValidator),
    /// SEQUENTIAL Statement Validator
    Sequential(SequentialValidator),

    // Utility class validator
    /// EXPLAIN Statement Validator
    Explain(ExplainValidator),
    /// PROFILE Statement Validator
    Profile(ProfileValidator),
    /// UPDATE CONFIG statement validator
    UpdateConfig(UpdateConfigsValidator),
    /// CLEAR SPACE Statement Validator
    ClearSpace(ClearSpaceValidator),

    // Full-text Search validators
    /// Full-text search statement validator
    Fulltext(FulltextValidator),
    /// Vector search statement validator
    Vector(VectorValidator),
    /// Transaction statement validator
    Transaction(TransactionValidator),
}

impl Validator {
    /// Obtain the type of the validator.
    pub fn get_type(&self) -> StatementType {
        match self {
            Validator::Show(v) => v.statement_type(),
            Validator::Desc(v) => v.statement_type(),
            Validator::ShowCreate(v) => v.statement_type(),
            Validator::ShowConfigs(v) => v.statement_type(),
            Validator::ShowSessions(v) => v.statement_type(),
            Validator::ShowQueries(v) => v.statement_type(),
            Validator::KillQuery(v) => v.statement_type(),
            Validator::CreateUser(v) => v.statement_type(),
            Validator::DropUser(v) => v.statement_type(),
            Validator::AlterUser(v) => v.statement_type(),
            Validator::ChangePassword(v) => v.statement_type(),
            Validator::Grant(v) => v.statement_type(),
            Validator::Revoke(v) => v.statement_type(),
            Validator::DescribeUser(v) => v.statement_type(),
            Validator::ShowUsers(v) => v.statement_type(),
            Validator::ShowRoles(v) => v.statement_type(),
            Validator::Alter(v) => v.statement_type(),
            Validator::Drop(v) => v.statement_type(),
            Validator::CreateTagIndex(v) => v.statement_type(),
            Validator::CreateEdgeIndex(v) => v.statement_type(),
            Validator::CreateTag(v) => v.statement_type(),
            Validator::CreateEdge(v) => v.statement_type(),
            Validator::Create(v) => v.statement_type(),
            Validator::Use(v) => v.statement_type(),
            Validator::Set(v) => v.statement_type(),
            Validator::Assignment(v) => v.statement_type(),
            Validator::Pipe(v) => v.statement_type(),
            Validator::Query(v) => v.statement_type(),
            Validator::SetOperation(v) => v.statement_type(),
            Validator::Match(v) => v.statement_type(),
            Validator::Lookup(v) => v.statement_type(),
            Validator::Go(v) => v.statement_type(),
            Validator::FindPath(v) => v.statement_type(),
            Validator::GetSubgraph(v) => v.statement_type(),
            Validator::FetchVertices(v) => v.statement_type(),
            Validator::FetchEdges(v) => v.statement_type(),
            Validator::InsertVertices(v) => v.statement_type(),
            Validator::InsertEdges(v) => v.statement_type(),
            Validator::Update(v) => v.statement_type(),
            Validator::Delete(v) => v.statement_type(),
            Validator::Merge(v) => v.statement_type(),
            Validator::Remove(v) => v.statement_type(),
            Validator::Unwind(v) => v.statement_type(),
            Validator::OrderBy(v) => v.statement_type(),
            Validator::GroupBy(v) => v.statement_type(),
            Validator::Yield(v) => v.statement_type(),
            Validator::Return(v) => v.statement_type(),
            Validator::With(v) => v.statement_type(),
            Validator::Limit(v) => v.statement_type(),
            Validator::Sequential(v) => v.statement_type(),
            Validator::Explain(v) => v.statement_type(),
            Validator::Profile(v) => v.statement_type(),
            Validator::UpdateConfig(v) => v.statement_type(),
            Validator::ClearSpace(v) => v.statement_type(),
            Validator::Fulltext(v) => v.statement_type(),
            Validator::Vector(v) => v.statement_type(),
            Validator::Transaction(v) => v.statement_type(),
        }
    }

    /// Verify the statement.
    pub fn validate(&mut self, ast: Arc<Ast>, qctx: Arc<QueryContext>) -> ValidationResult {
        match self {
            Validator::Show(v) => v
                .validate(ast, qctx)
                .unwrap_or_else(|e| ValidationResult::failure(vec![e])),
            Validator::Desc(v) => v
                .validate(ast, qctx)
                .unwrap_or_else(|e| ValidationResult::failure(vec![e])),
            Validator::ShowCreate(v) => v
                .validate(ast, qctx)
                .unwrap_or_else(|e| ValidationResult::failure(vec![e])),
            Validator::ShowConfigs(v) => v
                .validate(ast, qctx)
                .unwrap_or_else(|e| ValidationResult::failure(vec![e])),
            Validator::ShowSessions(v) => v
                .validate(ast, qctx)
                .unwrap_or_else(|e| ValidationResult::failure(vec![e])),
            Validator::ShowQueries(v) => v
                .validate(ast, qctx)
                .unwrap_or_else(|e| ValidationResult::failure(vec![e])),
            Validator::KillQuery(v) => v
                .validate(ast, qctx)
                .unwrap_or_else(|e| ValidationResult::failure(vec![e])),
            Validator::CreateUser(v) => v
                .validate(ast, qctx)
                .unwrap_or_else(|e| ValidationResult::failure(vec![e])),
            Validator::DropUser(v) => v
                .validate(ast, qctx)
                .unwrap_or_else(|e| ValidationResult::failure(vec![e])),
            Validator::AlterUser(v) => v
                .validate(ast, qctx)
                .unwrap_or_else(|e| ValidationResult::failure(vec![e])),
            Validator::ChangePassword(v) => v
                .validate(ast, qctx)
                .unwrap_or_else(|e| ValidationResult::failure(vec![e])),
            Validator::Grant(v) => v
                .validate(ast, qctx)
                .unwrap_or_else(|e| ValidationResult::failure(vec![e])),
            Validator::Revoke(v) => v
                .validate(ast, qctx)
                .unwrap_or_else(|e| ValidationResult::failure(vec![e])),
            Validator::DescribeUser(v) => v
                .validate(ast, qctx)
                .unwrap_or_else(|e| ValidationResult::failure(vec![e])),
            Validator::ShowUsers(v) => v
                .validate(ast, qctx)
                .unwrap_or_else(|e| ValidationResult::failure(vec![e])),
            Validator::ShowRoles(v) => v
                .validate(ast, qctx)
                .unwrap_or_else(|e| ValidationResult::failure(vec![e])),
            Validator::Alter(v) => v
                .validate(ast, qctx)
                .unwrap_or_else(|e| ValidationResult::failure(vec![e])),
            Validator::Drop(v) => v
                .validate(ast, qctx)
                .unwrap_or_else(|e| ValidationResult::failure(vec![e])),
            Validator::CreateTagIndex(v) => v
                .validate(ast, qctx)
                .unwrap_or_else(|e| ValidationResult::failure(vec![e])),
            Validator::CreateEdgeIndex(v) => v
                .validate(ast, qctx)
                .unwrap_or_else(|e| ValidationResult::failure(vec![e])),
            Validator::CreateTag(v) => v
                .validate(ast, qctx)
                .unwrap_or_else(|e| ValidationResult::failure(vec![e])),
            Validator::CreateEdge(v) => v
                .validate(ast, qctx)
                .unwrap_or_else(|e| ValidationResult::failure(vec![e])),
            Validator::Create(v) => v
                .validate(ast, qctx)
                .unwrap_or_else(|e| ValidationResult::failure(vec![e])),
            Validator::Use(v) => v
                .validate(ast, qctx)
                .unwrap_or_else(|e| ValidationResult::failure(vec![e])),
            Validator::Set(v) => v
                .validate(ast, qctx)
                .unwrap_or_else(|e| ValidationResult::failure(vec![e])),
            Validator::Assignment(v) => v
                .validate(ast, qctx)
                .unwrap_or_else(|e| ValidationResult::failure(vec![e])),
            Validator::Pipe(v) => v
                .validate(ast, qctx)
                .unwrap_or_else(|e| ValidationResult::failure(vec![e])),
            Validator::Query(v) => v
                .validate(ast, qctx)
                .unwrap_or_else(|e| ValidationResult::failure(vec![e])),
            Validator::SetOperation(v) => v
                .validate(ast, qctx)
                .unwrap_or_else(|e| ValidationResult::failure(vec![e])),
            Validator::Match(v) => v
                .validate(ast, qctx)
                .unwrap_or_else(|e| ValidationResult::failure(vec![e])),
            Validator::Lookup(v) => v
                .validate(ast, qctx)
                .unwrap_or_else(|e| ValidationResult::failure(vec![e])),
            Validator::Go(v) => v
                .validate(ast, qctx)
                .unwrap_or_else(|e| ValidationResult::failure(vec![e])),
            Validator::FindPath(v) => v
                .validate(ast, qctx)
                .unwrap_or_else(|e| ValidationResult::failure(vec![e])),
            Validator::GetSubgraph(v) => v
                .validate(ast, qctx)
                .unwrap_or_else(|e| ValidationResult::failure(vec![e])),
            Validator::FetchVertices(v) => v
                .validate(ast, qctx)
                .unwrap_or_else(|e| ValidationResult::failure(vec![e])),
            Validator::FetchEdges(v) => v
                .validate(ast, qctx)
                .unwrap_or_else(|e| ValidationResult::failure(vec![e])),
            Validator::InsertVertices(v) => v
                .validate(ast, qctx)
                .unwrap_or_else(|e| ValidationResult::failure(vec![e])),
            Validator::InsertEdges(v) => v
                .validate(ast, qctx)
                .unwrap_or_else(|e| ValidationResult::failure(vec![e])),
            Validator::Update(v) => v
                .validate(ast, qctx)
                .unwrap_or_else(|e| ValidationResult::failure(vec![e])),
            Validator::Delete(v) => v
                .validate(ast, qctx)
                .unwrap_or_else(|e| ValidationResult::failure(vec![e])),
            Validator::Merge(v) => v
                .validate(ast, qctx)
                .unwrap_or_else(|e| ValidationResult::failure(vec![e])),
            Validator::Remove(v) => v
                .validate(ast, qctx)
                .unwrap_or_else(|e| ValidationResult::failure(vec![e])),
            Validator::Unwind(v) => v
                .validate(ast, qctx)
                .unwrap_or_else(|e| ValidationResult::failure(vec![e])),
            Validator::OrderBy(v) => v
                .validate(ast, qctx)
                .unwrap_or_else(|e| ValidationResult::failure(vec![e])),
            Validator::GroupBy(v) => v
                .validate(ast, qctx)
                .unwrap_or_else(|e| ValidationResult::failure(vec![e])),
            Validator::Yield(v) => v
                .validate(ast, qctx)
                .unwrap_or_else(|e| ValidationResult::failure(vec![e])),
            Validator::Return(v) => v
                .validate(ast, qctx)
                .unwrap_or_else(|e| ValidationResult::failure(vec![e])),
            Validator::With(v) => v
                .validate(ast, qctx)
                .unwrap_or_else(|e| ValidationResult::failure(vec![e])),
            Validator::Limit(v) => v
                .validate(ast, qctx)
                .unwrap_or_else(|e| ValidationResult::failure(vec![e])),
            Validator::Sequential(v) => v
                .validate(ast, qctx)
                .unwrap_or_else(|e| ValidationResult::failure(vec![e])),
            Validator::Explain(v) => v
                .validate(ast, qctx)
                .unwrap_or_else(|e| ValidationResult::failure(vec![e])),
            Validator::Profile(v) => v
                .validate(ast, qctx)
                .unwrap_or_else(|e| ValidationResult::failure(vec![e])),
            Validator::UpdateConfig(v) => v
                .validate(ast, qctx)
                .unwrap_or_else(|e| ValidationResult::failure(vec![e])),
            Validator::ClearSpace(v) => v
                .validate(ast, qctx)
                .unwrap_or_else(|e| ValidationResult::failure(vec![e])),
            Validator::Fulltext(v) => v
                .validate(ast, qctx)
                .unwrap_or_else(|e| ValidationResult::failure(vec![e])),
            Validator::Vector(v) => v
                .validate(ast, qctx)
                .unwrap_or_else(|e| ValidationResult::failure(vec![e])),
            Validator::Transaction(v) => v
                .validate(ast, qctx)
                .unwrap_or_else(|e| ValidationResult::failure(vec![e])),
        }
    }

    /// Get the input column
    pub fn get_inputs(&self) -> Vec<ColumnDef> {
        match self {
            Validator::Show(v) => v.inputs().to_vec(),
            Validator::Desc(v) => v.inputs().to_vec(),
            Validator::ShowCreate(v) => v.inputs().to_vec(),
            Validator::ShowConfigs(v) => v.inputs().to_vec(),
            Validator::ShowSessions(v) => v.inputs().to_vec(),
            Validator::ShowQueries(v) => v.inputs().to_vec(),
            Validator::KillQuery(v) => v.inputs().to_vec(),
            Validator::CreateUser(v) => v.inputs().to_vec(),
            Validator::DropUser(v) => v.inputs().to_vec(),
            Validator::AlterUser(v) => v.inputs().to_vec(),
            Validator::ChangePassword(v) => v.inputs().to_vec(),
            Validator::Grant(v) => v.inputs().to_vec(),
            Validator::Revoke(v) => v.inputs().to_vec(),
            Validator::DescribeUser(v) => v.inputs().to_vec(),
            Validator::ShowUsers(v) => v.inputs().to_vec(),
            Validator::ShowRoles(v) => v.inputs().to_vec(),
            Validator::Alter(v) => v.inputs().to_vec(),
            Validator::Drop(v) => v.inputs().to_vec(),
            Validator::CreateTagIndex(v) => v.inputs().to_vec(),
            Validator::CreateEdgeIndex(v) => v.inputs().to_vec(),
            Validator::CreateTag(v) => v.inputs().to_vec(),
            Validator::CreateEdge(v) => v.inputs().to_vec(),
            Validator::Create(v) => v.inputs().to_vec(),
            Validator::Use(v) => v.inputs().to_vec(),
            Validator::Set(v) => v.inputs().to_vec(),
            Validator::Assignment(v) => v.inputs().to_vec(),
            Validator::Pipe(v) => v.inputs().to_vec(),
            Validator::Query(v) => v.inputs().to_vec(),
            Validator::SetOperation(v) => v.inputs().to_vec(),
            Validator::Match(v) => v.inputs().to_vec(),
            Validator::Lookup(v) => v.inputs().to_vec(),
            Validator::Go(v) => v.inputs().to_vec(),
            Validator::FindPath(v) => v.inputs().to_vec(),
            Validator::GetSubgraph(v) => v.inputs().to_vec(),
            Validator::FetchVertices(v) => v.inputs().to_vec(),
            Validator::FetchEdges(v) => v.inputs().to_vec(),
            Validator::InsertVertices(v) => v.inputs().to_vec(),
            Validator::InsertEdges(v) => v.inputs().to_vec(),
            Validator::Update(v) => v.inputs().to_vec(),
            Validator::Delete(v) => v.inputs().to_vec(),
            Validator::Merge(v) => v.inputs().to_vec(),
            Validator::Remove(v) => v.inputs().to_vec(),
            Validator::Unwind(v) => v.inputs().to_vec(),
            Validator::OrderBy(v) => v.inputs().to_vec(),
            Validator::GroupBy(v) => v.inputs().to_vec(),
            Validator::Yield(v) => v.inputs().to_vec(),
            Validator::Return(v) => v.inputs().to_vec(),
            Validator::With(v) => v.inputs().to_vec(),
            Validator::Limit(v) => v.inputs().to_vec(),
            Validator::Sequential(v) => v.inputs().to_vec(),
            Validator::Explain(v) => v.inputs().to_vec(),
            Validator::Profile(v) => v.inputs().to_vec(),
            Validator::UpdateConfig(v) => v.inputs().to_vec(),
            Validator::ClearSpace(v) => v.inputs().to_vec(),
            Validator::Fulltext(v) => v.inputs().to_vec(),
            Validator::Vector(v) => v.inputs().to_vec(),
            Validator::Transaction(v) => v.inputs().to_vec(),
        }
    }

    /// Obtain the output column
    pub fn get_outputs(&self) -> Vec<ColumnDef> {
        match self {
            Validator::Show(v) => v.outputs().to_vec(),
            Validator::Desc(v) => v.outputs().to_vec(),
            Validator::ShowCreate(v) => v.outputs().to_vec(),
            Validator::ShowConfigs(v) => v.outputs().to_vec(),
            Validator::ShowSessions(v) => v.outputs().to_vec(),
            Validator::ShowQueries(v) => v.outputs().to_vec(),
            Validator::KillQuery(v) => v.outputs().to_vec(),
            Validator::CreateUser(v) => v.outputs().to_vec(),
            Validator::DropUser(v) => v.outputs().to_vec(),
            Validator::AlterUser(v) => v.outputs().to_vec(),
            Validator::ChangePassword(v) => v.outputs().to_vec(),
            Validator::Grant(v) => v.outputs().to_vec(),
            Validator::Revoke(v) => v.outputs().to_vec(),
            Validator::DescribeUser(v) => v.outputs().to_vec(),
            Validator::ShowUsers(v) => v.outputs().to_vec(),
            Validator::ShowRoles(v) => v.outputs().to_vec(),
            Validator::Alter(v) => v.outputs().to_vec(),
            Validator::Drop(v) => v.outputs().to_vec(),
            Validator::CreateTagIndex(v) => v.outputs().to_vec(),
            Validator::CreateEdgeIndex(v) => v.outputs().to_vec(),
            Validator::CreateTag(v) => v.outputs().to_vec(),
            Validator::CreateEdge(v) => v.outputs().to_vec(),
            Validator::Create(v) => v.outputs().to_vec(),
            Validator::Use(v) => v.outputs().to_vec(),
            Validator::Set(v) => v.outputs().to_vec(),
            Validator::Assignment(v) => v.outputs().to_vec(),
            Validator::Pipe(v) => v.outputs().to_vec(),
            Validator::Query(v) => v.outputs().to_vec(),
            Validator::SetOperation(v) => v.outputs().to_vec(),
            Validator::Match(v) => v.outputs().to_vec(),
            Validator::Lookup(v) => v.outputs().to_vec(),
            Validator::Go(v) => v.outputs().to_vec(),
            Validator::FindPath(v) => v.outputs().to_vec(),
            Validator::GetSubgraph(v) => v.outputs().to_vec(),
            Validator::FetchVertices(v) => v.outputs().to_vec(),
            Validator::FetchEdges(v) => v.outputs().to_vec(),
            Validator::InsertVertices(v) => v.outputs().to_vec(),
            Validator::InsertEdges(v) => v.outputs().to_vec(),
            Validator::Update(v) => v.outputs().to_vec(),
            Validator::Delete(v) => v.outputs().to_vec(),
            Validator::Merge(v) => v.outputs().to_vec(),
            Validator::Remove(v) => v.outputs().to_vec(),
            Validator::Unwind(v) => v.outputs().to_vec(),
            Validator::OrderBy(v) => v.outputs().to_vec(),
            Validator::GroupBy(v) => v.outputs().to_vec(),
            Validator::Yield(v) => v.outputs().to_vec(),
            Validator::Return(v) => v.outputs().to_vec(),
            Validator::With(v) => v.outputs().to_vec(),
            Validator::Limit(v) => v.outputs().to_vec(),
            Validator::Sequential(v) => v.outputs().to_vec(),
            Validator::Explain(v) => v.outputs().to_vec(),
            Validator::Profile(v) => v.outputs().to_vec(),
            Validator::UpdateConfig(v) => v.outputs().to_vec(),
            Validator::ClearSpace(v) => v.outputs().to_vec(),
            Validator::Fulltext(v) => v.outputs().to_vec(),
            Validator::Vector(v) => v.outputs().to_vec(),
            Validator::Transaction(v) => v.outputs().to_vec(),
        }
    }
}

impl Validator {
    /// Create a validator based on the statement.
    pub fn create_from_stmt(stmt: &Stmt) -> Option<Validator> {
        let stmt_type = Self::infer_statement_type(stmt);
        Some(Self::create(stmt_type))
    }

    /// Set schema manager for validators that need it
    pub fn set_schema_manager(&mut self, schema_manager: Arc<SchemaManager>) {
        match self {
            Validator::Create(v) => v.set_schema_manager(schema_manager),
            Validator::Lookup(v) => v.set_schema_manager(schema_manager),
            Validator::Explain(v) => v.set_schema_manager(schema_manager),
            Validator::Profile(v) => v.set_schema_manager(schema_manager),
            Validator::Go(v) => v.set_schema_manager(schema_manager),
            Validator::Match(v) => v.set_schema_manager(schema_manager),
            Validator::FetchVertices(v) => v.set_schema_manager(schema_manager),
            Validator::FetchEdges(v) => v.set_schema_manager(schema_manager),
            Validator::Delete(v) => v.set_schema_manager(schema_manager),
            Validator::GetSubgraph(v) => v.set_schema_manager(schema_manager),
            Validator::FindPath(v) => v.set_schema_manager(schema_manager),
            Validator::Limit(v) => v.set_schema_manager(schema_manager),
            Validator::InsertVertices(v) => v.set_schema_manager(schema_manager),
            Validator::InsertEdges(v) => v.set_schema_manager(schema_manager),
            _ => {}
        }
    }

    /// Create a validator from Arc<Ast>.
    /// This is the new recommendation method; the context of the expressions is defined within Ast.
    pub fn create_from_ast(ast: &Arc<Ast>) -> Option<Validator> {
        let stmt_type = Self::infer_statement_type(&ast.stmt);
        Some(Self::create(stmt_type))
    }

    /// Determine the type of a sentence based on other sentences
    fn infer_statement_type(stmt: &Stmt) -> StatementType {
        match stmt {
            Stmt::Query(_) => StatementType::Query,
            Stmt::Match(_) => StatementType::Match,
            Stmt::Delete(_) => StatementType::Delete,
            Stmt::Update(_) => StatementType::Update,
            Stmt::Go(_) => StatementType::Go,
            Stmt::Fetch(f) => match &f.target {
                FetchTarget::Vertices { .. } => StatementType::FetchVertices,
                FetchTarget::Edges { .. } => StatementType::FetchEdges,
            },
            Stmt::Use(_) => StatementType::Use,
            Stmt::Show(_) => StatementType::Show,
            Stmt::Explain(_) => StatementType::Explain,
            Stmt::Profile(_) => StatementType::Profile,
            Stmt::GroupBy(_) => StatementType::GroupBy,
            Stmt::Lookup(_) => StatementType::Lookup,
            Stmt::Subgraph(_) => StatementType::GetSubgraph,
            Stmt::FindPath(_) => StatementType::FindPath,
            Stmt::Insert(insert_stmt) => match &insert_stmt.target {
                crate::query::parser::ast::stmt::InsertTarget::Vertices { .. } => {
                    StatementType::InsertVertices
                }
                crate::query::parser::ast::stmt::InsertTarget::Edge { .. } => {
                    StatementType::InsertEdges
                }
            },
            Stmt::Merge(_) => StatementType::Merge,
            Stmt::Unwind(_) => StatementType::Unwind,
            Stmt::Return(_) => StatementType::Return,
            Stmt::With(_) => StatementType::With,
            Stmt::Yield(_) => StatementType::Yield,
            Stmt::Set(_) => StatementType::Set,
            Stmt::Remove(_) => StatementType::Remove,
            Stmt::Pipe(_) => StatementType::Pipe,
            Stmt::Drop(_) => StatementType::Drop,
            Stmt::Desc(_) => StatementType::Desc,
            Stmt::Alter(_) => StatementType::Alter,
            Stmt::CreateUser(_) => StatementType::CreateUser,
            Stmt::AlterUser(_) => StatementType::AlterUser,
            Stmt::DropUser(_) => StatementType::DropUser,
            Stmt::ChangePassword(_) => StatementType::ChangePassword,
            Stmt::Grant(_) => StatementType::Grant,
            Stmt::Revoke(_) => StatementType::Revoke,
            Stmt::DescribeUser(_) => StatementType::DescribeUser,
            Stmt::ShowUsers(_) => StatementType::ShowUsers,
            Stmt::ShowRoles(_) => StatementType::ShowRoles,
            Stmt::ShowCreate(_) => StatementType::ShowCreate,
            Stmt::ShowConfigs(_) => StatementType::ShowConfigs,
            Stmt::ShowSessions(_) => StatementType::ShowSessions,
            Stmt::ShowQueries(_) => StatementType::ShowQueries,
            Stmt::KillQuery(_) => StatementType::KillQuery,
            Stmt::Create(c) => match &c.target {
                CreateTarget::Space { .. } => StatementType::CreateSpace,
                CreateTarget::Tag { .. } => StatementType::CreateTag,
                CreateTarget::EdgeType { .. } => StatementType::CreateEdge,
                CreateTarget::Index { index_type, .. } => match index_type {
                    crate::query::parser::ast::stmt::IndexType::Tag => {
                        StatementType::CreateTagIndex
                    }
                    crate::query::parser::ast::stmt::IndexType::Edge => {
                        StatementType::CreateEdgeIndex
                    }
                },
                _ => StatementType::Create,
            },
            Stmt::Assignment(_) => StatementType::Assignment,
            Stmt::SetOperation(_) => StatementType::SetOperation,
            Stmt::UpdateConfigs(_) => StatementType::UpdateConfigs,
            Stmt::ClearSpace(_) => StatementType::ClearSpace,
            // Full-text Search statements
            Stmt::CreateFulltextIndex(_) => StatementType::CreateFulltextIndex,
            Stmt::DropFulltextIndex(_) => StatementType::DropFulltextIndex,
            Stmt::AlterFulltextIndex(_) => StatementType::AlterFulltextIndex,
            Stmt::ShowFulltextIndex(_) => StatementType::ShowFulltextIndex,
            Stmt::DescribeFulltextIndex(_) => StatementType::DescribeFulltextIndex,
            Stmt::Search(_) => StatementType::Search,
            Stmt::LookupFulltext(_) => StatementType::LookupFulltext,
            Stmt::MatchFulltext(_) => StatementType::MatchFulltext,
            // Vector Search statements
            Stmt::CreateVectorIndex(_) => StatementType::CreateVectorIndex,
            Stmt::DropVectorIndex(_) => StatementType::DropVectorIndex,
            Stmt::SearchVector(_) => StatementType::SearchVector,
            Stmt::LookupVector(_) => StatementType::LookupVector,
            Stmt::MatchVector(_) => StatementType::MatchVector,
            // Transaction statements
            Stmt::BeginTransaction(_) => StatementType::BeginTransaction,
            Stmt::CommitTransaction(_) => StatementType::CommitTransaction,
            Stmt::RollbackTransaction(_) => StatementType::RollbackTransaction,
        }
    }

    /// Create validators based on the type of statement.
    pub fn create(stmt_type: StatementType) -> Validator {
        match stmt_type {
            StatementType::Show => Validator::Show(ShowValidator::new()),
            StatementType::Desc => Validator::Desc(DescValidator::new()),
            StatementType::ShowCreate => Validator::ShowCreate(ShowCreateValidator::new()),
            StatementType::ShowConfigs => Validator::ShowConfigs(ShowConfigsValidator::new()),
            StatementType::ShowSessions => Validator::ShowSessions(ShowSessionsValidator::new()),
            StatementType::ShowQueries => Validator::ShowQueries(ShowQueriesValidator::new()),
            StatementType::KillQuery => Validator::KillQuery(KillQueryValidator::new()),
            StatementType::CreateUser => Validator::CreateUser(CreateUserValidator::new()),
            StatementType::DropUser => Validator::DropUser(DropUserValidator::new()),
            StatementType::AlterUser => Validator::AlterUser(AlterUserValidator::new()),
            StatementType::ChangePassword => {
                Validator::ChangePassword(ChangePasswordValidator::new())
            }
            StatementType::Grant => Validator::Grant(GrantValidator::new()),
            StatementType::Revoke => Validator::Revoke(RevokeValidator::new()),
            StatementType::DescribeUser => Validator::DescribeUser(DescribeUserValidator::new()),
            StatementType::ShowUsers => Validator::ShowUsers(ShowUsersValidator::new()),
            StatementType::ShowRoles => Validator::ShowRoles(ShowRolesValidator::new()),
            StatementType::Alter => Validator::Alter(AlterValidator::new()),
            StatementType::Drop => Validator::Drop(DropValidator::new()),
            StatementType::DropTagIndex | StatementType::DropEdgeIndex => {
                Validator::Drop(DropValidator::new())
            }
            StatementType::CreateTagIndex => Validator::CreateTagIndex(CreateIndexValidator::new()),
            StatementType::CreateEdgeIndex => {
                Validator::CreateEdgeIndex(CreateIndexValidator::new())
            }
            StatementType::CreateTag => Validator::CreateTag(CreateTagValidator::new()),
            StatementType::CreateEdge => Validator::CreateEdge(CreateEdgeValidator::new()),
            StatementType::Create | StatementType::CreateSpace => {
                Validator::Create(CreateValidator::new())
            }
            StatementType::Use => Validator::Use(UseValidator::new()),
            StatementType::Set => Validator::Set(SetValidator::new()),
            StatementType::Assignment => Validator::Assignment(AssignmentValidator::new()),
            StatementType::Pipe => Validator::Pipe(PipeValidator::new()),
            StatementType::Query => Validator::Query(QueryValidator::new()),
            StatementType::SetOperation => Validator::SetOperation(SetOperationValidator::new()),
            StatementType::Match => Validator::Match(MatchValidator::new()),
            StatementType::Lookup => Validator::Lookup(LookupValidator::new()),
            StatementType::Go => Validator::Go(GoValidator::new()),
            StatementType::FindPath => Validator::FindPath(FindPathValidator::new()),
            StatementType::GetSubgraph => Validator::GetSubgraph(GetSubgraphValidator::new()),
            StatementType::FetchVertices => Validator::FetchVertices(FetchVerticesValidator::new()),
            StatementType::FetchEdges => Validator::FetchEdges(FetchEdgesValidator::new()),
            StatementType::InsertVertices => {
                Validator::InsertVertices(InsertVerticesValidator::new())
            }
            StatementType::InsertEdges => Validator::InsertEdges(InsertEdgesValidator::new()),
            StatementType::Update => Validator::Update(UpdateValidator::new()),
            StatementType::Delete => Validator::Delete(DeleteValidator::new()),
            StatementType::Merge => Validator::Merge(MergeValidator::new()),
            StatementType::Remove => Validator::Remove(RemoveValidator::new()),
            StatementType::Unwind => Validator::Unwind(UnwindValidator::new()),
            StatementType::OrderBy => Validator::OrderBy(OrderByValidator::new()),
            StatementType::GroupBy => Validator::GroupBy(GroupByValidator::new()),
            StatementType::Yield => Validator::Yield(YieldValidator::new()),
            StatementType::Return => Validator::Return(ReturnValidator::new()),
            StatementType::With => Validator::With(WithValidator::new()),
            StatementType::Limit => Validator::Limit(LimitValidator::new()),
            StatementType::Sequential => Validator::Sequential(SequentialValidator::new()),
            StatementType::Explain => Validator::Explain(ExplainValidator::new()),
            StatementType::Profile => Validator::Profile(ProfileValidator::new()),
            StatementType::UpdateConfigs => Validator::UpdateConfig(UpdateConfigsValidator::new()),
            StatementType::ClearSpace => Validator::ClearSpace(ClearSpaceValidator::new()),
            StatementType::CreateFulltextIndex
            | StatementType::DropFulltextIndex
            | StatementType::AlterFulltextIndex
            | StatementType::ShowFulltextIndex
            | StatementType::DescribeFulltextIndex
            | StatementType::Search
            | StatementType::LookupFulltext
            | StatementType::MatchFulltext => Validator::Fulltext(FulltextValidator::new()),
            StatementType::CreateVectorIndex
            | StatementType::DropVectorIndex
            | StatementType::SearchVector
            | StatementType::LookupVector
            | StatementType::MatchVector => Validator::Vector(VectorValidator::new()),
            // Transaction statements
            StatementType::BeginTransaction => {
                Validator::Transaction(TransactionValidator::new(StatementType::BeginTransaction))
            }
            StatementType::CommitTransaction => {
                Validator::Transaction(TransactionValidator::new(StatementType::CommitTransaction))
            }
            StatementType::RollbackTransaction => Validator::Transaction(
                TransactionValidator::new(StatementType::RollbackTransaction),
            ),
            StatementType::DropSpace
            | StatementType::DropTag
            | StatementType::DropEdge
            | StatementType::AlterTag
            | StatementType::AlterEdge => Validator::Drop(DropValidator::new()),
            StatementType::ShowSpaces
            | StatementType::ShowTags
            | StatementType::ShowEdges
            | StatementType::DescribeSpace
            | StatementType::DescribeTag
            | StatementType::DescribeEdge => Validator::Show(ShowValidator::new()),
        }
    }

    /// Obtain a list of user-defined variables
    pub fn get_user_defined_vars(&self) -> &[String] {
        match self {
            Validator::Show(v) => v.user_defined_vars(),
            Validator::Desc(v) => v.user_defined_vars(),
            Validator::ShowCreate(v) => v.user_defined_vars(),
            Validator::ShowConfigs(v) => v.user_defined_vars(),
            Validator::ShowSessions(v) => v.user_defined_vars(),
            Validator::ShowQueries(v) => v.user_defined_vars(),
            Validator::KillQuery(v) => v.user_defined_vars(),
            Validator::CreateUser(v) => v.user_defined_vars(),
            Validator::DropUser(v) => v.user_defined_vars(),
            Validator::AlterUser(v) => v.user_defined_vars(),
            Validator::ChangePassword(v) => v.user_defined_vars(),
            Validator::Grant(v) => v.user_defined_vars(),
            Validator::Revoke(v) => v.user_defined_vars(),
            Validator::DescribeUser(v) => v.user_defined_vars(),
            Validator::ShowUsers(v) => v.user_defined_vars(),
            Validator::ShowRoles(v) => v.user_defined_vars(),
            Validator::Alter(v) => v.user_defined_vars(),
            Validator::Drop(v) => v.user_defined_vars(),
            Validator::CreateTagIndex(v) => v.user_defined_vars(),
            Validator::CreateEdgeIndex(v) => v.user_defined_vars(),
            Validator::CreateTag(v) => v.user_defined_vars(),
            Validator::CreateEdge(v) => v.user_defined_vars(),
            Validator::Create(v) => v.user_defined_vars(),
            Validator::Use(v) => v.user_defined_vars(),
            Validator::Set(v) => v.user_defined_vars(),
            Validator::Assignment(v) => v.user_defined_vars(),
            Validator::Pipe(v) => v.user_defined_vars(),
            Validator::Query(v) => v.user_defined_vars(),
            Validator::SetOperation(v) => v.user_defined_vars(),
            Validator::Match(v) => v.user_defined_vars(),
            Validator::Lookup(v) => v.user_defined_vars(),
            Validator::Go(v) => v.user_defined_vars(),
            Validator::FindPath(v) => v.user_defined_vars(),
            Validator::GetSubgraph(v) => v.user_defined_vars(),
            Validator::FetchVertices(v) => v.user_defined_vars(),
            Validator::FetchEdges(v) => v.user_defined_vars(),
            Validator::InsertVertices(v) => v.user_defined_vars(),
            Validator::InsertEdges(v) => v.user_defined_vars(),
            Validator::Update(v) => v.user_defined_vars(),
            Validator::Delete(v) => v.user_defined_vars(),
            Validator::Merge(v) => v.user_defined_vars(),
            Validator::Remove(v) => v.user_defined_vars(),
            Validator::Unwind(v) => v.user_defined_vars(),
            Validator::OrderBy(v) => v.user_defined_vars(),
            Validator::GroupBy(v) => v.user_defined_vars(),
            Validator::Yield(v) => v.user_defined_vars(),
            Validator::Return(v) => v.user_defined_vars(),
            Validator::With(v) => v.user_defined_vars(),
            Validator::Limit(v) => v.user_defined_vars(),
            Validator::Sequential(v) => v.user_defined_vars(),
            Validator::Explain(v) => v.user_defined_vars(),
            Validator::Profile(v) => v.user_defined_vars(),
            Validator::UpdateConfig(v) => v.user_defined_vars(),
            Validator::ClearSpace(v) => v.user_defined_vars(),
            Validator::Fulltext(v) => v.user_defined_vars(),
            Validator::Vector(v) => v.user_defined_vars(),
            Validator::Transaction(v) => v.user_defined_vars(),
        }
    }

    /// Determine the type of the statement
    pub fn statement_type(&self) -> StatementType {
        self.get_type()
    }

    /// Obtain the properties of the expression
    pub fn expression_props(&self) -> &ExpressionProps {
        match self {
            Validator::Show(v) => v.expression_props(),
            Validator::Desc(v) => v.expression_props(),
            Validator::ShowCreate(v) => v.expression_props(),
            Validator::ShowConfigs(v) => v.expression_props(),
            Validator::ShowSessions(v) => v.expression_props(),
            Validator::ShowQueries(v) => v.expression_props(),
            Validator::KillQuery(v) => v.expression_props(),
            Validator::CreateUser(v) => v.expression_props(),
            Validator::DropUser(v) => v.expression_props(),
            Validator::AlterUser(v) => v.expression_props(),
            Validator::ChangePassword(v) => v.expression_props(),
            Validator::Grant(v) => v.expression_props(),
            Validator::Revoke(v) => v.expression_props(),
            Validator::DescribeUser(v) => v.expression_props(),
            Validator::ShowUsers(v) => v.expression_props(),
            Validator::ShowRoles(v) => v.expression_props(),
            Validator::Alter(v) => v.expression_props(),
            Validator::Drop(v) => v.expression_props(),
            Validator::CreateTagIndex(v) => v.expression_props(),
            Validator::CreateEdgeIndex(v) => v.expression_props(),
            Validator::CreateTag(v) => v.expression_props(),
            Validator::CreateEdge(v) => v.expression_props(),
            Validator::Create(v) => v.expression_props(),
            Validator::Use(v) => v.expression_props(),
            Validator::Set(v) => v.expression_props(),
            Validator::Assignment(v) => v.expression_props(),
            Validator::Pipe(v) => v.expression_props(),
            Validator::Query(v) => v.expression_props(),
            Validator::SetOperation(v) => v.expression_props(),
            Validator::Match(v) => v.expression_props(),
            Validator::Lookup(v) => v.expression_props(),
            Validator::Go(v) => v.expression_props(),
            Validator::FindPath(v) => v.expression_props(),
            Validator::GetSubgraph(v) => v.expression_props(),
            Validator::FetchVertices(v) => v.expression_props(),
            Validator::FetchEdges(v) => v.expression_props(),
            Validator::InsertVertices(v) => v.expression_props(),
            Validator::InsertEdges(v) => v.expression_props(),
            Validator::Update(v) => v.expression_props(),
            Validator::Delete(v) => v.expression_props(),
            Validator::Merge(v) => v.expression_props(),
            Validator::Remove(v) => v.expression_props(),
            Validator::Unwind(v) => v.expression_props(),
            Validator::OrderBy(v) => v.expression_props(),
            Validator::GroupBy(v) => v.expression_props(),
            Validator::Yield(v) => v.expression_props(),
            Validator::Return(v) => v.expression_props(),
            Validator::With(v) => v.expression_props(),
            Validator::Limit(v) => v.expression_props(),
            Validator::Sequential(v) => v.expression_props(),
            Validator::Explain(v) => v.expression_props(),
            Validator::Profile(v) => v.expression_props(),
            Validator::UpdateConfig(v) => v.expression_props(),
            Validator::ClearSpace(v) => v.expression_props(),
            Validator::Fulltext(v) => v.expression_props(),
            Validator::Vector(v) => v.expression_props(),
            Validator::Transaction(v) => v.expression_props(),
        }
    }
}

/// Collection of validators
pub struct ValidatorCollection {
    validators: Vec<Validator>,
}

impl ValidatorCollection {
    pub fn new() -> Self {
        Self {
            validators: Vec::new(),
        }
    }

    pub fn add(&mut self, validator: Validator) {
        self.validators.push(validator);
    }

    pub fn get_validators(&self) -> &[Validator] {
        &self.validators
    }

    pub fn get_validators_mut(&mut self) -> &mut Vec<Validator> {
        &mut self.validators
    }
}

impl Default for ValidatorCollection {
    fn default() -> Self {
        Self::new()
    }
}
