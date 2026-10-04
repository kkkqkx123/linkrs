use std::sync::Arc;

pub use super::pattern::*;
pub use super::types::*;
use graphdb_core::types::expr::expression_context::ExpressionAnalysisContext;

mod dcl;
mod ddl;
mod dml;
mod dql;
mod management;
mod migrate;
mod search;
mod transaction;

pub use dcl::*;
pub use ddl::*;
pub use dml::*;
pub use dql::*;
pub use management::*;
pub use migrate::*;
pub use search::*;
pub use transaction::*;

#[cfg(test)]
mod tests;

#[derive(Debug, Clone)]
pub struct Ast {
    pub stmt: Stmt,
    pub expr_context: Arc<ExpressionAnalysisContext>,
}

impl Ast {
    pub fn new(stmt: Stmt, expr_context: Arc<ExpressionAnalysisContext>) -> Self {
        Self { stmt, expr_context }
    }

    pub fn stmt(&self) -> &Stmt {
        &self.stmt
    }

    pub fn expr_context(&self) -> &Arc<ExpressionAnalysisContext> {
        &self.expr_context
    }

    pub fn into_stmt(self) -> Stmt {
        self.stmt
    }
}

/// Coarse classification of a statement, used for routing/permission/statistics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StmtCategory {
    /// Subquery / compound statement wrappers (QUERY / PIPE / SET OPERATION / ASSIGNMENT).
    Query,
    /// Read & analytic statements (MATCH / GO / LOOKUP / FIND PATH / ... / RETURN / YIELD / LET).
    Dql,
    /// Write statements (INSERT / MERGE / UPDATE / DELETE / SET / REMOVE).
    Dml,
    /// Schema statements (CREATE / DROP / ALTER / DESC / SHOW CREATE / index DDL).
    Ddl,
    /// Access-control statements (CREATE USER / GRANT / REVOKE / ...).
    Dcl,
    /// Server / session administration (USE / SHOW / EXPLAIN / PROFILE / ANALYZE / KILL QUERY / ...).
    Admin,
    /// Full-text & vector search statements (SEARCH / SEARCH VECTOR / LOOKUP|MATCH FULLTEXT|VECTOR).
    Search,
    /// Transaction control (BEGIN / COMMIT / ROLLBACK / SAVEPOINT / RELEASE SAVEPOINT).
    Transaction,
}

impl StmtCategory {
    pub fn as_str(&self) -> &'static str {
        match self {
            StmtCategory::Query => "QUERY",
            StmtCategory::Dql => "DQL",
            StmtCategory::Dml => "DML",
            StmtCategory::Ddl => "DDL",
            StmtCategory::Dcl => "DCL",
            StmtCategory::Admin => "ADMIN",
            StmtCategory::Search => "SEARCH",
            StmtCategory::Transaction => "TRANSACTION",
        }
    }
}

#[derive(Debug, Clone)]
pub enum Stmt {
    Query(QueryStmt),
    Create(CreateStmt),
    Match(MatchStmt),
    Delete(DeleteStmt),
    Update(UpdateStmt),
    Go(GoStmt),
    Fetch(FetchStmt),
    Use(UseStmt),
    Show(ShowStmt),
    Explain(ExplainStmt),
    Profile(ProfileStmt),
    Analyze(AnalyzeStmt),
    GroupBy(GroupByStmt),
    Lookup(LookupStmt),
    Subgraph(SubgraphStmt),
    FindPath(FindPathStmt),
    Insert(InsertStmt),
    Merge(MergeStmt),
    Unwind(UnwindStmt),
    Return(ReturnStmt),
    With(WithStmt),
    Yield(YieldStmt),
    Filter(FilterStmt),
    Collect(CollectStmt),
    Set(SetStmt),
    Remove(RemoveStmt),
    Pipe(PipeStmt),
    Drop(DropStmt),
    Desc(DescStmt),
    Alter(AlterStmt),
    CreateUser(CreateUserStmt),
    AlterUser(AlterUserStmt),
    DropUser(DropUserStmt),
    ChangePassword(ChangePasswordStmt),
    Grant(GrantStmt),
    Revoke(RevokeStmt),
    DescribeUser(DescribeUserStmt),
    ShowUsers(ShowUsersStmt),
    ShowRoles(ShowRolesStmt),
    ShowCreate(ShowCreateStmt),
    ShowSessions(ShowSessionsStmt),
    ShowQueries(ShowQueriesStmt),
    KillQuery(KillQueryStmt),
    ShowConfigs(ShowConfigsStmt),
    UpdateConfigs(UpdateConfigsStmt),
    Assignment(AssignmentStmt),
    SetOperation(SetOperationStmt),
    ClearSpace(ClearSpaceStmt),
    CreateFulltextIndex(CreateFulltextIndex),
    DropFulltextIndex(DropFulltextIndex),
    AlterFulltextIndex(AlterFulltextIndex),
    ShowFulltextIndex(ShowFulltextIndex),
    DescribeFulltextIndex(DescribeFulltextIndex),
    Search(SearchStatement),
    LookupFulltext(LookupFulltext),
    MatchFulltext(MatchFulltext),
    CreateVectorIndex(CreateVectorIndex),
    DropVectorIndex(DropVectorIndex),
    SearchVector(SearchVectorStatement),
    LookupVector(LookupVector),
    MatchVector(MatchVector),
    BeginTransaction(BeginTransactionStmt),
    CommitTransaction(CommitTransactionStmt),
    RollbackTransaction(RollbackTransactionStmt),
    Savepoint(SavepointStmt),
    ReleaseSavepoint(ReleaseSavepointStmt),
    AssignVariable(AssignVariableStmt),
    Copy(CopyStmt),
    Migrate(MigrateStmt),
    CommentOn(CommentOnStmt),
    Checkpoint(CheckpointStmt),
    LoadFrom(LoadFromStmt),
    InQueryCall(InQueryCallStmt),
    ExportDatabase(ExportDatabaseStmt),
    ImportDatabase(ImportDatabaseStmt),
    CreateMacro(CreateMacroStmt),
    DropMacro(DropMacroStmt),
    CreateType(CreateTypeStmt),
    DropType(DropTypeStmt),
    Extension(ExtensionStmt),
    AttachDatabase(AttachDatabaseStmt),
    DetachDatabase(DetachDatabaseStmt),
}

crate::define_stmt_helpers! {
    Query => Query,
    Create => Ddl,
    Match => Dql,
    Delete => Dml,
    Update => Dml,
    Go => Dql,
    Fetch => Dql,
    Use => Admin,
    Show => Admin,
    Explain => Admin,
    Profile => Admin,
    Analyze => Admin,
    GroupBy => Dql,
    Lookup => Dql,
    Subgraph => Dql,
    FindPath => Dql,
    Insert => Dml,
    Merge => Dml,
    Unwind => Dql,
    Return => Dql,
    With => Dql,
    Yield => Dql,
    Filter => Dql,
    Collect => Dql,
    Set => Dml,
    Remove => Dml,
    Pipe => Query,
    Drop => Ddl,
    Desc => Ddl,
    Alter => Ddl,
    CreateUser => Dcl,
    AlterUser => Dcl,
    DropUser => Dcl,
    ChangePassword => Dcl,
    Grant => Dcl,
    Revoke => Dcl,
    DescribeUser => Dcl,
    ShowUsers => Dcl,
    ShowRoles => Dcl,
    ShowCreate => Ddl,
    ShowSessions => Admin,
    ShowQueries => Admin,
    KillQuery => Admin,
    ShowConfigs => Admin,
    UpdateConfigs => Admin,
    Assignment => Query,
    SetOperation => Query,
    ClearSpace => Admin,
    CreateFulltextIndex => Ddl,
    DropFulltextIndex => Ddl,
    AlterFulltextIndex => Ddl,
    ShowFulltextIndex => Ddl,
    DescribeFulltextIndex => Ddl,
    Search => Search,
    LookupFulltext => Search,
    MatchFulltext => Search,
    CreateVectorIndex => Ddl,
    DropVectorIndex => Ddl,
    SearchVector => Search,
    LookupVector => Search,
    MatchVector => Search,
    BeginTransaction => Transaction,
    CommitTransaction => Transaction,
    RollbackTransaction => Transaction,
    Savepoint => Transaction,
    ReleaseSavepoint => Transaction,
    AssignVariable => Dql,
    Copy => Dml,
    CommentOn => Ddl,
    Checkpoint => Admin,
    LoadFrom => Dql,
    InQueryCall => Dql,
    ExportDatabase => Admin,
    ImportDatabase => Admin,
    CreateMacro => Ddl,
    DropMacro => Ddl,
    CreateType => Ddl,
    DropType => Ddl,
    Extension => Admin,
    AttachDatabase => Admin,
    DetachDatabase => Admin,
}

impl Stmt {
    pub fn kind(&self) -> &'static str {
        match self {
            Stmt::Query(_) => "QUERY",
            Stmt::Create(_) => "CREATE",
            Stmt::Match(_) => "MATCH",
            Stmt::Delete(_) => "DELETE",
            Stmt::Update(s) => {
                if s.is_upsert {
                    "UPSERT"
                } else {
                    "UPDATE"
                }
            }
            Stmt::Go(_) => "GO",
            Stmt::Fetch(_) => "FETCH",
            Stmt::Use(_) => "USE",
            Stmt::Show(_) => "SHOW",
            Stmt::Explain(_) => "EXPLAIN",
            Stmt::Profile(_) => "PROFILE",
            Stmt::Analyze(_) => "ANALYZE",
            Stmt::GroupBy(_) => "GROUP BY",
            Stmt::Lookup(_) => "LOOKUP",
            Stmt::Subgraph(_) => "SUBGRAPH",
            Stmt::FindPath(_) => "FIND PATH",
            Stmt::Insert(_) => "INSERT",
            Stmt::Merge(_) => "MERGE",
            Stmt::Unwind(_) => "UNWIND",
            Stmt::Return(_) => "RETURN",
            Stmt::With(_) => "WITH",
            Stmt::Yield(_) => "YIELD",
            Stmt::Filter(_) => "WHERE",
            Stmt::Collect(_) => "COLLECT",
            Stmt::Set(_) => "SET",
            Stmt::Remove(_) => "REMOVE",
            Stmt::Pipe(_) => "PIPE",
            Stmt::Drop(_) => "DROP",
            Stmt::Desc(_) => "DESC",
            Stmt::Alter(_) => "ALTER",
            Stmt::CreateUser(_) => "CREATE USER",
            Stmt::AlterUser(_) => "ALTER USER",
            Stmt::DropUser(_) => "DROP USER",
            Stmt::ChangePassword(_) => "CHANGE PASSWORD",
            Stmt::Grant(_) => "GRANT",
            Stmt::Revoke(_) => "REVOKE",
            Stmt::DescribeUser(_) => "DESCRIBE USER",
            Stmt::ShowUsers(_) => "SHOW USERS",
            Stmt::ShowRoles(_) => "SHOW ROLES",
            Stmt::ShowCreate(_) => "SHOW CREATE",
            Stmt::ShowSessions(_) => "SHOW SESSIONS",
            Stmt::ShowQueries(_) => "SHOW QUERIES",
            Stmt::KillQuery(_) => "KILL QUERY",
            Stmt::ShowConfigs(_) => "SHOW CONFIGS",
            Stmt::UpdateConfigs(_) => "UPDATE CONFIGS",
            Stmt::Assignment(_) => "ASSIGNMENT",
            Stmt::SetOperation(_) => "SET OPERATION",
            Stmt::ClearSpace(_) => "CLEAR SPACE",
            Stmt::CreateFulltextIndex(_) => "CREATE FULLTEXT INDEX",
            Stmt::DropFulltextIndex(_) => "DROP FULLTEXT INDEX",
            Stmt::AlterFulltextIndex(_) => "ALTER FULLTEXT INDEX",
            Stmt::ShowFulltextIndex(_) => "SHOW FULLTEXT INDEX",
            Stmt::DescribeFulltextIndex(_) => "DESCRIBE FULLTEXT INDEX",
            Stmt::Search(_) => "SEARCH",
            Stmt::LookupFulltext(_) => "LOOKUP FULLTEXT",
            Stmt::MatchFulltext(_) => "MATCH FULLTEXT",
            Stmt::CreateVectorIndex(_) => "CREATE VECTOR INDEX",
            Stmt::DropVectorIndex(_) => "DROP VECTOR INDEX",
            Stmt::SearchVector(_) => "SEARCH VECTOR",
            Stmt::LookupVector(_) => "LOOKUP VECTOR",
            Stmt::MatchVector(_) => "MATCH VECTOR",
            Stmt::BeginTransaction(stmt) => match stmt.read_only {
                Some(true) => "BEGIN TRANSACTION READ ONLY",
                Some(false) => "BEGIN TRANSACTION READ WRITE",
                None => "BEGIN TRANSACTION",
            },
            Stmt::CommitTransaction(_) => "COMMIT TRANSACTION",
            Stmt::RollbackTransaction(stmt) => {
                if stmt.savepoint_name.is_some() {
                    "ROLLBACK TRANSACTION TO SAVEPOINT"
                } else {
                    "ROLLBACK TRANSACTION"
                }
            }
            Stmt::Savepoint(_) => "SAVEPOINT",
            Stmt::ReleaseSavepoint(_) => "RELEASE SAVEPOINT",
            Stmt::AssignVariable(_) => "LET",
            Stmt::Copy(_) => "COPY",
            Stmt::Migrate(m) => match m {
                MigrateStmt::Plan(_) => "MIGRATE PLAN",
                MigrateStmt::Execute(_) => "MIGRATE EXECUTE",
                MigrateStmt::Rollback(_) => "MIGRATE ROLLBACK",
            },
            Stmt::CommentOn(_) => "COMMENT ON",
            Stmt::Checkpoint(_) => "CHECKPOINT",
            Stmt::LoadFrom(_) => "LOAD FROM",
            Stmt::InQueryCall(_) => "CALL",
            Stmt::ExportDatabase(_) => "EXPORT DATABASE",
            Stmt::ImportDatabase(_) => "IMPORT DATABASE",
            Stmt::CreateMacro(_) => "CREATE MACRO",
            Stmt::DropMacro(_) => "DROP MACRO",
            Stmt::CreateType(_) => "CREATE TYPE",
            Stmt::DropType(_) => "DROP TYPE",
            Stmt::Extension(s) => match s.action {
                ExtensionAction::Load => "LOAD EXTENSION",
                ExtensionAction::Install => "INSTALL EXTENSION",
                ExtensionAction::Uninstall => "UNINSTALL EXTENSION",
                ExtensionAction::Update => "UPDATE EXTENSION",
            },
            Stmt::AttachDatabase(_) => "ATTACH DATABASE",
            Stmt::DetachDatabase(_) => "DETACH DATABASE",
        }
    }

    pub fn as_explain(&self) -> Option<&ExplainStmt> {
        match self {
            Stmt::Explain(s) => Some(s),
            _ => None,
        }
    }
}
