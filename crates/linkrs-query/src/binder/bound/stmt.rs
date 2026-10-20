use crate::parser::ast::Stmt;

use super::stmt_ddl::*;
use super::stmt_dml::*;
use super::stmt_dql::*;

/// Fully bound query statement with all names resolved.
#[derive(Debug, Clone)]
pub enum BoundStatement {
    // ── DQL ─────────────────────────────────────────────────────────────────
    Match(BoundMatchStatement),
    Go(BoundGoStatement),
    Lookup(BoundLookupStatement),
    FetchVertices(BoundFetchVerticesStatement),
    FetchEdges(BoundFetchEdgesStatement),
    FindPath(BoundFindPathStatement),
    Subgraph(BoundSubgraphStatement),

    // ── Clause operators ────────────────────────────────────────────────────
    Pipe(BoundPipeStatement),
    Return(BoundReturnStatement),
    With(BoundWithStatement),
    Unwind(BoundUnwindStatement),
    GroupBy(BoundGroupByStatement),
    SetOperation(BoundSetOperationStatement),
    Filter(BoundFilter),
    Yield(BoundYield),
    Collect(BoundCollect),
    AssignVariable(BoundAssignVariable),

    // ── DML ─────────────────────────────────────────────────────────────────
    Insert(BoundInsert),
    Update(BoundUpdate),
    Delete(BoundDelete),
    Merge(BoundMerge),
    Set(BoundSet),
    Remove(BoundRemove),
    Copy(BoundCopy),

    // ── DDL ─────────────────────────────────────────────────────────────────
    Drop(BoundDrop),
    Alter(BoundAlter),
    Show(BoundShow),
    ShowCreate(BoundShowCreate),
    Desc(BoundDesc),
    ClearSpace(BoundClearSpace),
    CreateUser(BoundCreateUser),
    DropUser(BoundDropUser),
    AlterUser(BoundAlterUser),
    ChangePassword(BoundChangePassword),
    Grant(BoundGrant),
    Revoke(BoundRevoke),
    DescribeUser(BoundDescribeUser),
    ShowUsers(BoundShowUsers),
    ShowRoles(BoundShowRoles),
    CreateFulltextIndex(BoundCreateFulltextIndex),
    CreateVectorIndex(BoundCreateVectorIndex),

    // ── EXPLAIN / PROFILE ──────────────────────────────────────────────────
    Explain(BoundExplain),
    Profile(BoundProfile),

    // ── USE ────────────────────────────────────────────────────────────────
    Use(BoundUse),

    // ── Transaction ─────────────────────────────────────────────────────────
    BeginTransaction(BoundBeginTransaction),
    Commit(BoundCommit),
    Rollback(BoundRollback),
    Savepoint(BoundSavepoint),
    ReleaseSavepoint(BoundReleaseSavepoint),

    // ── Statement-level variable assignment (`$var = <query>`) ─────────────
    Assignment(BoundAssignmentStatement),

    // ── Placeholder for management/DDL/DML ─────────────────────────────────
    Other(Box<Stmt>),
}

impl BoundStatement {
    pub fn kind(&self) -> &str {
        match self {
            Self::Match(_) => "Match",
            Self::Go(_) => "Go",
            Self::Lookup(_) => "Lookup",
            Self::FetchVertices(_) => "FetchVertices",
            Self::FetchEdges(_) => "FetchEdges",
            Self::FindPath(_) => "FindPath",
            Self::Subgraph(_) => "Subgraph",
            Self::Pipe(_) => "Pipe",
            Self::Return(_) => "Return",
            Self::With(_) => "With",
            Self::Unwind(_) => "Unwind",
            Self::GroupBy(_) => "GroupBy",
            Self::SetOperation(_) => "SetOperation",
            Self::Filter(_) => "Filter",
            Self::Yield(_) => "Yield",
            Self::Collect(_) => "Collect",
            Self::AssignVariable(_) => "AssignVariable",
            Self::Insert(_) => "Insert",
            Self::Update(_) => "Update",
            Self::Delete(_) => "Delete",
            Self::Merge(_) => "Merge",
            Self::Set(_) => "Set",
            Self::Remove(_) => "Remove",
            Self::Copy(_) => "Copy",
            Self::Drop(_) => "Drop",
            Self::Alter(_) => "Alter",
            Self::Show(_) => "Show",
            Self::ShowCreate(_) => "ShowCreate",
            Self::Desc(_) => "Desc",
            Self::ClearSpace(_) => "ClearSpace",
            Self::CreateUser(_) => "CreateUser",
            Self::DropUser(_) => "DropUser",
            Self::AlterUser(_) => "AlterUser",
            Self::ChangePassword(_) => "ChangePassword",
            Self::Grant(_) => "Grant",
            Self::Revoke(_) => "Revoke",
            Self::DescribeUser(_) => "DescribeUser",
            Self::ShowUsers(_) => "ShowUsers",
            Self::ShowRoles(_) => "ShowRoles",
            Self::CreateFulltextIndex(_) => "CreateFulltextIndex",
            Self::CreateVectorIndex(_) => "CreateVectorIndex",
            Self::Explain(_) => "Explain",
            Self::Profile(_) => "Profile",
            Self::Use(_) => "Use",
            Self::BeginTransaction(s) => match s.read_only {
                Some(true) => "BeginTransactionReadOnly",
                Some(false) => "BeginTransactionReadWrite",
                None => "BeginTransaction",
            },
            Self::Commit(_) => "Commit",
            Self::Rollback(s) => {
                if s.savepoint_name.is_some() {
                    "RollbackToSavepoint"
                } else {
                    "Rollback"
                }
            }
            Self::Savepoint(_) => "Savepoint",
            Self::ReleaseSavepoint(_) => "ReleaseSavepoint",
            Self::Assignment(_) => "Assignment",
            Self::Other(stmt) => stmt.kind(),
        }
    }

    pub fn as_match(&self) -> Option<&BoundMatchStatement> {
        if let Self::Match(v) = self {
            Some(v)
        } else {
            None
        }
    }

    pub fn as_go(&self) -> Option<&BoundGoStatement> {
        if let Self::Go(v) = self {
            Some(v)
        } else {
            None
        }
    }
}
