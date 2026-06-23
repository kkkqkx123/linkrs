//! AST statement definition (v2)
//!
//! Based on a simplified statement definition using enumerations, all graph database operation statements are supported.

use std::sync::Arc;

pub use super::fulltext::*;
pub use super::pattern::*;
pub use super::types::*;
pub use super::vector::{
    CreateVectorIndex, DropVectorIndex, LookupVector, MatchVector, SearchVectorStatement,
    VectorQueryExpr, VectorQueryType,
};
use crate::core::types::expr::analysis_utils::collect_variables_from_contextual;
use crate::core::types::expr::contextual::ContextualExpression;
use crate::core::types::PropertyDef;
use crate::query::validator::context::ExpressionAnalysisContext;

/// AST Packaging Type – Contains the context of statements and expressions
///
/// # Explanation of the reconstruction process
/// Merge the expression context into the AST to avoid passing it separately in the ParserResult and QueryContext.
#[derive(Debug, Clone)]
pub struct Ast {
    pub stmt: Stmt,
    pub expr_context: Arc<ExpressionAnalysisContext>,
}

impl Ast {
    /// Create a new AST.
    pub fn new(stmt: Stmt, expr_context: Arc<ExpressionAnalysisContext>) -> Self {
        Self { stmt, expr_context }
    }

    /// Obtain statement references
    pub fn stmt(&self) -> &Stmt {
        &self.stmt
    }

    /// Obtain the context of the expression.
    pub fn expr_context(&self) -> &Arc<ExpressionAnalysisContext> {
        &self.expr_context
    }

    /// Acquiring ownership of a statement
    pub fn into_stmt(self) -> Stmt {
        self.stmt
    }
}

/// Statement Enumeration – All database operation statements for graph databases
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
    // Full-text search statements
    CreateFulltextIndex(CreateFulltextIndex),
    DropFulltextIndex(DropFulltextIndex),
    AlterFulltextIndex(AlterFulltextIndex),
    ShowFulltextIndex(ShowFulltextIndex),
    DescribeFulltextIndex(DescribeFulltextIndex),
    Search(SearchStatement),
    LookupFulltext(LookupFulltext),
    MatchFulltext(MatchFulltext),
    // Vector search statements
    CreateVectorIndex(CreateVectorIndex),
    DropVectorIndex(DropVectorIndex),
    SearchVector(SearchVectorStatement),
    LookupVector(LookupVector),
    MatchVector(MatchVector),
    // Transaction statements
    BeginTransaction(BeginTransactionStmt),
    CommitTransaction(CommitTransactionStmt),
    RollbackTransaction(RollbackTransactionStmt),
}

impl Stmt {
    /// Obtain the location information of the statement.
    pub fn span(&self) -> Span {
        match self {
            Stmt::Query(s) => s.span,
            Stmt::Create(s) => s.span,
            Stmt::Match(s) => s.span,
            Stmt::Delete(s) => s.span,
            Stmt::Update(s) => s.span,
            Stmt::Go(s) => s.span,
            Stmt::Fetch(s) => s.span,
            Stmt::Use(s) => s.span,
            Stmt::Show(s) => s.span,
            Stmt::Explain(s) => s.span,
            Stmt::Profile(s) => s.span,
            Stmt::GroupBy(s) => s.span,
            Stmt::Lookup(s) => s.span,
            Stmt::Subgraph(s) => s.span,
            Stmt::FindPath(s) => s.span,
            Stmt::Insert(s) => s.span,
            Stmt::Merge(s) => s.span,
            Stmt::Unwind(s) => s.span,
            Stmt::Return(s) => s.span,
            Stmt::With(s) => s.span,
            Stmt::Yield(s) => s.span,
            Stmt::Set(s) => s.span,
            Stmt::Remove(s) => s.span,
            Stmt::Pipe(s) => s.span,
            Stmt::Drop(s) => s.span,
            Stmt::Desc(s) => s.span,
            Stmt::Alter(s) => s.span,
            Stmt::CreateUser(s) => s.span,
            Stmt::AlterUser(s) => s.span,
            Stmt::DropUser(s) => s.span,
            Stmt::ChangePassword(s) => s.span,
            Stmt::Grant(s) => s.span,
            Stmt::Revoke(s) => s.span,
            Stmt::DescribeUser(s) => s.span,
            Stmt::ShowUsers(s) => s.span,
            Stmt::ShowRoles(s) => s.span,
            Stmt::ShowCreate(s) => s.span,
            Stmt::ShowSessions(s) => s.span,
            Stmt::ShowQueries(s) => s.span,
            Stmt::KillQuery(s) => s.span,
            Stmt::ShowConfigs(s) => s.span,
            Stmt::UpdateConfigs(s) => s.span,
            Stmt::Assignment(s) => s.span,
            Stmt::SetOperation(s) => s.span,
            Stmt::ClearSpace(s) => s.span,
            // Full-text search statements
            Stmt::CreateFulltextIndex(s) => s.span,
            Stmt::DropFulltextIndex(s) => s.span,
            Stmt::AlterFulltextIndex(s) => s.span,
            Stmt::ShowFulltextIndex(s) => s.span,
            Stmt::DescribeFulltextIndex(s) => s.span,
            Stmt::Search(s) => s.span,
            Stmt::LookupFulltext(s) => s.span,
            Stmt::MatchFulltext(s) => s.span,
            // Vector search statements
            Stmt::CreateVectorIndex(s) => s.span,
            Stmt::DropVectorIndex(s) => s.span,
            Stmt::SearchVector(s) => s.span,
            Stmt::LookupVector(s) => s.span,
            Stmt::MatchVector(s) => s.span,
            // Transaction statements
            Stmt::BeginTransaction(s) => s.span,
            Stmt::CommitTransaction(s) => s.span,
            Stmt::RollbackTransaction(s) => s.span,
        }
    }

    /// Obtain the names of the statement type categories
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
            // Full-text search statements
            Stmt::CreateFulltextIndex(_) => "CREATE FULLTEXT INDEX",
            Stmt::DropFulltextIndex(_) => "DROP FULLTEXT INDEX",
            Stmt::AlterFulltextIndex(_) => "ALTER FULLTEXT INDEX",
            Stmt::ShowFulltextIndex(_) => "SHOW FULLTEXT INDEX",
            Stmt::DescribeFulltextIndex(_) => "DESCRIBE FULLTEXT INDEX",
            Stmt::Search(_) => "SEARCH",
            Stmt::LookupFulltext(_) => "LOOKUP FULLTEXT",
            Stmt::MatchFulltext(_) => "MATCH FULLTEXT",
            // Vector search statements
            Stmt::CreateVectorIndex(_) => "CREATE VECTOR INDEX",
            Stmt::DropVectorIndex(_) => "DROP VECTOR INDEX",
            Stmt::SearchVector(_) => "SEARCH VECTOR",
            Stmt::LookupVector(_) => "LOOKUP VECTOR",
            Stmt::MatchVector(_) => "MATCH VECTOR",
            // Transaction statements
            Stmt::BeginTransaction(_) => "BEGIN TRANSACTION",
            Stmt::CommitTransaction(_) => "COMMIT TRANSACTION",
            Stmt::RollbackTransaction(_) => "ROLLBACK TRANSACTION",
        }
    }

    // Type conversion methods
    pub fn as_query(&self) -> Option<&QueryStmt> {
        match self {
            Stmt::Query(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_create(&self) -> Option<&CreateStmt> {
        match self {
            Stmt::Create(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_match(&self) -> Option<&MatchStmt> {
        match self {
            Stmt::Match(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_delete(&self) -> Option<&DeleteStmt> {
        match self {
            Stmt::Delete(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_update(&self) -> Option<&UpdateStmt> {
        match self {
            Stmt::Update(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_go(&self) -> Option<&GoStmt> {
        match self {
            Stmt::Go(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_fetch(&self) -> Option<&FetchStmt> {
        match self {
            Stmt::Fetch(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_use(&self) -> Option<&UseStmt> {
        match self {
            Stmt::Use(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_show(&self) -> Option<&ShowStmt> {
        match self {
            Stmt::Show(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_explain(&self) -> Option<&ExplainStmt> {
        match self {
            Stmt::Explain(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_profile(&self) -> Option<&ProfileStmt> {
        match self {
            Stmt::Profile(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_group_by(&self) -> Option<&GroupByStmt> {
        match self {
            Stmt::GroupBy(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_lookup(&self) -> Option<&LookupStmt> {
        match self {
            Stmt::Lookup(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_subgraph(&self) -> Option<&SubgraphStmt> {
        match self {
            Stmt::Subgraph(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_find_path(&self) -> Option<&FindPathStmt> {
        match self {
            Stmt::FindPath(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_insert(&self) -> Option<&InsertStmt> {
        match self {
            Stmt::Insert(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_merge(&self) -> Option<&MergeStmt> {
        match self {
            Stmt::Merge(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_unwind(&self) -> Option<&UnwindStmt> {
        match self {
            Stmt::Unwind(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_return(&self) -> Option<&ReturnStmt> {
        match self {
            Stmt::Return(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_with(&self) -> Option<&WithStmt> {
        match self {
            Stmt::With(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_yield(&self) -> Option<&YieldStmt> {
        match self {
            Stmt::Yield(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_set(&self) -> Option<&SetStmt> {
        match self {
            Stmt::Set(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_remove(&self) -> Option<&RemoveStmt> {
        match self {
            Stmt::Remove(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_pipe(&self) -> Option<&PipeStmt> {
        match self {
            Stmt::Pipe(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_drop(&self) -> Option<&DropStmt> {
        match self {
            Stmt::Drop(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_desc(&self) -> Option<&DescStmt> {
        match self {
            Stmt::Desc(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_alter(&self) -> Option<&AlterStmt> {
        match self {
            Stmt::Alter(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_create_user(&self) -> Option<&CreateUserStmt> {
        match self {
            Stmt::CreateUser(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_alter_user(&self) -> Option<&AlterUserStmt> {
        match self {
            Stmt::AlterUser(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_drop_user(&self) -> Option<&DropUserStmt> {
        match self {
            Stmt::DropUser(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_change_password(&self) -> Option<&ChangePasswordStmt> {
        match self {
            Stmt::ChangePassword(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_grant(&self) -> Option<&GrantStmt> {
        match self {
            Stmt::Grant(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_revoke(&self) -> Option<&RevokeStmt> {
        match self {
            Stmt::Revoke(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_describe_user(&self) -> Option<&DescribeUserStmt> {
        match self {
            Stmt::DescribeUser(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_show_users(&self) -> Option<&ShowUsersStmt> {
        match self {
            Stmt::ShowUsers(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_show_roles(&self) -> Option<&ShowRolesStmt> {
        match self {
            Stmt::ShowRoles(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_show_create(&self) -> Option<&ShowCreateStmt> {
        match self {
            Stmt::ShowCreate(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_show_sessions(&self) -> Option<&ShowSessionsStmt> {
        match self {
            Stmt::ShowSessions(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_show_queries(&self) -> Option<&ShowQueriesStmt> {
        match self {
            Stmt::ShowQueries(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_kill_query(&self) -> Option<&KillQueryStmt> {
        match self {
            Stmt::KillQuery(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_show_configs(&self) -> Option<&ShowConfigsStmt> {
        match self {
            Stmt::ShowConfigs(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_update_configs(&self) -> Option<&UpdateConfigsStmt> {
        match self {
            Stmt::UpdateConfigs(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_assignment(&self) -> Option<&AssignmentStmt> {
        match self {
            Stmt::Assignment(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_set_operation(&self) -> Option<&SetOperationStmt> {
        match self {
            Stmt::SetOperation(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_create_fulltext_index(&self) -> Option<&CreateFulltextIndex> {
        match self {
            Stmt::CreateFulltextIndex(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_drop_fulltext_index(&self) -> Option<&DropFulltextIndex> {
        match self {
            Stmt::DropFulltextIndex(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_alter_fulltext_index(&self) -> Option<&AlterFulltextIndex> {
        match self {
            Stmt::AlterFulltextIndex(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_show_fulltext_index(&self) -> Option<&ShowFulltextIndex> {
        match self {
            Stmt::ShowFulltextIndex(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_describe_fulltext_index(&self) -> Option<&DescribeFulltextIndex> {
        match self {
            Stmt::DescribeFulltextIndex(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_search(&self) -> Option<&SearchStatement> {
        match self {
            Stmt::Search(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_lookup_fulltext(&self) -> Option<&LookupFulltext> {
        match self {
            Stmt::LookupFulltext(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_match_fulltext(&self) -> Option<&MatchFulltext> {
        match self {
            Stmt::MatchFulltext(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_create_vector_index(&self) -> Option<&CreateVectorIndex> {
        match self {
            Stmt::CreateVectorIndex(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_drop_vector_index(&self) -> Option<&DropVectorIndex> {
        match self {
            Stmt::DropVectorIndex(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_search_vector(&self) -> Option<&SearchVectorStatement> {
        match self {
            Stmt::SearchVector(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_lookup_vector(&self) -> Option<&LookupVector> {
        match self {
            Stmt::LookupVector(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_match_vector(&self) -> Option<&MatchVector> {
        match self {
            Stmt::MatchVector(s) => Some(s),
            _ => None,
        }
    }
}

/// Query statement
#[derive(Debug, Clone)]
pub struct QueryStmt {
    pub span: Span,
    pub statements: Vec<Stmt>,
}

impl QueryStmt {
    pub fn new(statements: Vec<Stmt>, span: Span) -> Self {
        Self { span, statements }
    }
}

impl PartialEq for QueryStmt {
    fn eq(&self, other: &Self) -> bool {
        self.span == other.span && self.statements.len() == other.statements.len()
    }
}

/// The CREATE statement
#[derive(Debug, Clone, PartialEq)]
pub struct CreateStmt {
    pub span: Span,
    pub target: CreateTarget,
    pub if_not_exists: bool,
}

/// Create the target.
#[derive(Debug, Clone, PartialEq)]
pub enum CreateTarget {
    /// Cypher-style node creation: CREATE (n:Label {props})
    Node {
        variable: Option<String>,
        labels: Vec<String>,
        properties: Option<ContextualExpression>,
    },
    /// Cypher-style edge creation: CREATE ()-[:Type {props}]->()
    Edge {
        variable: Option<String>,
        edge_type: String,
        src: ContextualExpression,
        dst: ContextualExpression,
        properties: Option<ContextualExpression>,
        direction: EdgeDirection,
    },
    /// Cypher-style full path creation: CREATE (a)-[:FRIEND]->(b)
    Path { patterns: Vec<Pattern> },
    /// Schema definition – TAG
    Tag {
        name: String,
        properties: Vec<PropertyDef>,
        ttl_duration: Option<i64>,
        ttl_col: Option<String>,
    },
    EdgeType {
        name: String,
        properties: Vec<PropertyDef>,
        ttl_duration: Option<i64>,
        ttl_col: Option<String>,
        src_tag: Option<String>,
        dst_tag: Option<String>,
    },
    Space {
        name: String,
        vid_type: String,
        comment: Option<String>,
    },
    Index {
        index_type: IndexType,
        name: String,
        on: String,
        properties: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum IndexType {
    Tag,
    Edge,
}

/// The MATCH statement
#[derive(Debug, Clone, PartialEq)]
pub struct MatchStmt {
    pub span: Span,
    pub patterns: Vec<Pattern>,
    pub where_clause: Option<ContextualExpression>,
    pub return_clause: Option<ReturnClause>,
    pub order_by: Option<OrderByClause>,
    pub limit: Option<usize>,
    pub skip: Option<usize>,
    pub optional: bool,
    pub delete_clause: Option<MatchDeleteClause>,
}

/// MATCH...DELETE clause
/// Supports deleting vertices or edges matched by a MATCH pattern.
#[derive(Debug, Clone, PartialEq)]
pub struct MatchDeleteClause {
    pub span: Span,
    pub target: MatchDeleteTarget,
    pub with_edge: bool,
}

/// Target for MATCH...DELETE
#[derive(Debug, Clone, PartialEq)]
pub enum MatchDeleteTarget {
    Vertices(Vec<ContextualExpression>),
    Edges(Vec<ContextualExpression>),
    /// Edge references in the form of (src, dst, rank)
    EdgeRefs(
        Vec<(
            ContextualExpression,
            ContextualExpression,
            Option<ContextualExpression>,
        )>,
    ),
}

/// Return the subquery.
#[derive(Debug, Clone, PartialEq)]
pub struct ReturnClause {
    pub span: Span,
    pub items: Vec<ReturnItem>,
    pub distinct: bool,
    pub order_by: Option<OrderByClause>,
    pub limit: Option<super::types::LimitClause>,
    pub skip: Option<super::types::SkipClause>,
    pub sample: Option<super::types::SampleClause>,
    pub having_clause: Option<ContextualExpression>,
}

/// Return items
#[derive(Debug, Clone, PartialEq)]
pub enum ReturnItem {
    Expression {
        expression: ContextualExpression,
        alias: Option<String>,
    },
}

/// Sorting clause
#[derive(Debug, Clone, PartialEq)]
pub struct OrderByClause {
    pub span: Span,
    pub items: Vec<OrderByItem>,
}

/// Sorting items
#[derive(Debug, Clone, PartialEq)]
pub struct OrderByItem {
    pub expression: ContextualExpression,
    pub direction: crate::core::types::OrderDirection,
}

/// The DELETE statement
#[derive(Debug, Clone, PartialEq)]
pub struct DeleteStmt {
    pub span: Span,
    pub target: DeleteTarget,
    pub where_clause: Option<ContextualExpression>,
    pub with_edge: bool, // Should the associated edges also be deleted simultaneously?
}

impl DeleteStmt {
    /// Create a new DELETE statement.
    pub fn new(target: DeleteTarget, span: Span) -> Self {
        Self {
            span,
            target,
            where_clause: None,
            with_edge: false,
        }
    }

    /// Set whether to delete the associated edges.
    pub fn with_edge(mut self, with_edge: bool) -> Self {
        self.with_edge = with_edge;
        self
    }
}

/// Delete the target.
#[derive(Debug, Clone, PartialEq)]
pub enum DeleteTarget {
    Vertices(Vec<ContextualExpression>),
    Edges {
        edge_type: Option<String>,
        edges: Vec<(
            ContextualExpression,
            ContextualExpression,
            Option<ContextualExpression>,
        )>,
    },
    /// Remove tags – This includes a list of tag names and a list of vertex IDs.
    Tags {
        tag_names: Vec<String>,
        vertex_ids: Vec<ContextualExpression>,
        is_all_tags: bool,
    },
    Index(String),
}

/// UPDATE statement
#[derive(Debug, Clone, PartialEq)]
pub struct UpdateStmt {
    pub span: Span,
    pub target: UpdateTarget,
    pub set_clause: SetClause,
    pub where_clause: Option<ContextualExpression>,
    pub is_upsert: bool,
    pub yield_clause: Option<YieldClause>,
}

/// Update target
#[derive(Debug, Clone, PartialEq)]
pub enum UpdateTarget {
    Vertex(ContextualExpression),
    Edge {
        src: ContextualExpression,
        dst: ContextualExpression,
        edge_type: Option<String>,
        rank: Option<ContextualExpression>,
    },
    Tag(String),
    /// Update of vertices with the specified tag: UPDATE VERTEX <vid> ON <tag> SET ...
    TagOnVertex {
        vid: Box<ContextualExpression>,
        tag_name: String,
    },
}

/// SET clause
#[derive(Debug, Clone, PartialEq)]
pub struct SetClause {
    pub span: Span,
    pub assignments: Vec<Assignment>,
}

/// Assignment operation
#[derive(Debug, Clone, PartialEq)]
pub struct Assignment {
    pub property: String,
    pub value: ContextualExpression,
    /// Optional target object (e.g., vertex id for SET 1.age = 31)
    pub target: Option<ContextualExpression>,
    /// Optional object expression for property access (e.g., the "1" in "1.age")
    pub object: Option<ContextualExpression>,
}

/// GO statement
#[derive(Debug, Clone, PartialEq)]
pub struct GoStmt {
    pub span: Span,
    pub steps: Steps,
    pub from: FromClause,
    pub over: Option<OverClause>,
    pub where_clause: Option<ContextualExpression>,
    pub yield_clause: Option<YieldClause>,
}

/// Step definition
#[derive(Debug, Clone, PartialEq)]
pub enum Steps {
    Fixed(usize),
    Range { min: usize, max: usize },
    Variable(String),
}

/// STEP clause
#[derive(Debug, Clone, PartialEq)]
pub struct StepClause {
    pub span: Span,
    pub steps: Steps,
}

/// The WHERE clause
#[derive(Debug, Clone, PartialEq)]
pub struct WhereClause {
    pub span: Span,
    pub condition: ContextualExpression,
}

/// FROM clause
#[derive(Debug, Clone, PartialEq)]
pub struct FromClause {
    pub span: Span,
    pub vertices: Vec<ContextualExpression>,
}

/// OVER clause
#[derive(Debug, Clone, PartialEq)]
pub struct OverClause {
    pub span: Span,
    pub edge_types: Vec<String>,
    pub direction: EdgeDirection,
}

/// YIELD clause
#[derive(Debug, Clone, PartialEq)]
pub struct YieldClause {
    pub span: Span,
    pub items: Vec<YieldItem>,
    pub where_clause: Option<ContextualExpression>,
    pub order_by: Option<OrderByClause>,
    pub limit: Option<super::types::LimitClause>,
    pub skip: Option<super::types::SkipClause>,
    pub sample: Option<super::types::SampleClause>,
}

/// The “YIELD” field
#[derive(Debug, Clone, PartialEq)]
pub struct YieldItem {
    pub expression: ContextualExpression,
    pub alias: Option<String>,
}

/// The FETCH statement
#[derive(Debug, Clone, PartialEq)]
pub struct FetchStmt {
    pub span: Span,
    pub target: FetchTarget,
}

/// Obtain the target.
#[derive(Debug, Clone, PartialEq)]
pub enum FetchTarget {
    Vertices {
        tag_name: Option<String>,
        ids: Vec<ContextualExpression>,
        properties: Option<Vec<String>>,
    },
    Edges {
        src: ContextualExpression,
        dst: ContextualExpression,
        edge_type: String,
        rank: Option<ContextualExpression>,
        properties: Option<Vec<String>>,
    },
}

/// USE statement
#[derive(Debug, Clone, PartialEq)]
pub struct UseStmt {
    pub span: Span,
    pub space: String,
}

/// The SHOW statement
#[derive(Debug, Clone, PartialEq)]
pub struct ShowStmt {
    pub span: Span,
    pub target: ShowTarget,
}

/// Display the target.
#[derive(Debug, Clone, PartialEq)]
pub enum ShowTarget {
    Spaces,
    Tags,
    Edges,
    Tag(String),
    Edge(String),
    Indexes,
    Index(String),
    Users,
    Roles,
    Stats,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub enum ExplainFormat {
    #[default]
    Table,
    Dot,
}

#[derive(Debug, Clone)]
pub struct ExplainStmt {
    pub span: Span,
    pub statement: Box<Stmt>,
    pub format: ExplainFormat,
}

impl PartialEq for ExplainStmt {
    fn eq(&self, other: &Self) -> bool {
        self.span == other.span && self.format == other.format
    }
}

/// PROFILE statement
#[derive(Debug, Clone)]
pub struct ProfileStmt {
    pub span: Span,
    pub statement: Box<Stmt>,
    pub format: ExplainFormat,
}

impl PartialEq for ProfileStmt {
    fn eq(&self, other: &Self) -> bool {
        self.span == other.span && self.format == other.format
    }
}

/// The GROUP BY statement
#[derive(Debug, Clone, PartialEq)]
pub struct GroupByStmt {
    pub span: Span,
    pub group_items: Vec<ContextualExpression>,
    pub yield_clause: YieldClause,
    pub having_clause: Option<ContextualExpression>,
}

/// LOOKUP statement (newly added)
#[derive(Debug, Clone, PartialEq)]
pub struct LookupStmt {
    pub span: Span,
    pub target: LookupTarget,
    pub where_clause: Option<ContextualExpression>,
    pub yield_clause: Option<YieldClause>,
}

/// LOOKUP target
#[derive(Debug, Clone, PartialEq)]
pub enum LookupTarget {
    Tag(String),
    Edge(String),
    /// Unspecified type - will be resolved during validation
    Unspecified(String),
}

/// The SUBGRAPH statement (newly added)
#[derive(Debug, Clone, PartialEq)]
pub struct SubgraphStmt {
    pub span: Span,
    pub steps: Steps,
    pub from: FromClause,
    pub over: Option<OverClause>,
    pub where_clause: Option<ContextualExpression>,
    pub yield_clause: Option<YieldClause>,
}

/// The “FIND PATH” statement (newly added)
#[derive(Debug, Clone, PartialEq)]
pub struct FindPathStmt {
    pub span: Span,
    pub from: FromClause,
    pub to: ContextualExpression,
    pub over: Option<OverClause>,
    pub where_clause: Option<ContextualExpression>,
    pub shortest: bool,
    pub max_steps: Option<usize>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
    pub yield_clause: Option<YieldClause>,
    pub weight_expression: Option<String>,
    pub heuristic_expression: Option<String>,
    pub with_loop: bool,
    pub with_cycle: bool,
}

/// INSERT statement
#[derive(Debug, Clone, PartialEq)]
pub struct InsertStmt {
    pub span: Span,
    pub target: InsertTarget,
    pub if_not_exists: bool,
}

/// INSERT target
#[derive(Debug, Clone, PartialEq)]
pub enum InsertTarget {
    Vertices {
        tags: Vec<TagInsertSpec>,
        values: Vec<VertexRow>,
    },
    Edge {
        edge_name: String,
        prop_names: Vec<String>,
        edges: Vec<(
            ContextualExpression,
            ContextualExpression,
            Option<ContextualExpression>,
            Vec<ContextualExpression>,
        )>,
    },
}

/// Tag insertion specifications
#[derive(Debug, Clone, PartialEq)]
pub struct TagInsertSpec {
    pub tag_name: String,
    pub prop_names: Vec<String>,
    pub is_default_props: bool,
}

/// Vertex row data
#[derive(Debug, Clone, PartialEq)]
pub struct VertexRow {
    pub vid: ContextualExpression,
    pub tag_values: Vec<Vec<ContextualExpression>>,
}

/// The MERGE statement
#[derive(Debug, Clone, PartialEq)]
pub struct MergeStmt {
    pub span: Span,
    pub pattern: Pattern,
    pub on_create: Option<SetClause>,
    pub on_match: Option<SetClause>,
}

/// The `SHOW SESSIONS` statement
#[derive(Debug, Clone, PartialEq)]
pub struct ShowSessionsStmt {
    pub span: Span,
}

/// The SHOW QUERIES statement
#[derive(Debug, Clone, PartialEq)]
pub struct ShowQueriesStmt {
    pub span: Span,
}

/// KILL QUERY statement
#[derive(Debug, Clone, PartialEq)]
pub struct KillQueryStmt {
    pub span: Span,
    pub session_id: i64,
    pub plan_id: i64,
}

/// The `SHOW CONFIGS` statement
#[derive(Debug, Clone, PartialEq)]
pub struct ShowConfigsStmt {
    pub span: Span,
    pub module: Option<String>, // Optional module name filtering
}

/// The “UPDATE CONFIGS” statement
#[derive(Debug, Clone, PartialEq)]
pub struct UpdateConfigsStmt {
    pub span: Span,
    pub module: Option<String>, // Optional module name
    pub config_name: String,
    pub config_value: ContextualExpression,
}

/// Variable assignment statement
#[derive(Debug, Clone)]
pub struct AssignmentStmt {
    pub span: Span,
    pub variable: String, // Variable name (without $ prefix)
    pub statement: Box<Stmt>,
}

impl PartialEq for AssignmentStmt {
    fn eq(&self, other: &Self) -> bool {
        self.span == other.span && self.variable == other.variable
    }
}

/// Types of set operations
#[derive(Debug, Clone, PartialEq)]
pub enum SetOperationType {
    Union,
    UnionAll,
    Intersect,
    Minus,
}

/// Set operation statements
#[derive(Debug, Clone)]
pub struct SetOperationStmt {
    pub span: Span,
    pub op_type: SetOperationType,
    pub left: Box<Stmt>,
    pub right: Box<Stmt>,
}

impl PartialEq for SetOperationStmt {
    fn eq(&self, other: &Self) -> bool {
        self.span == other.span && self.op_type == other.op_type
    }
}

/// UNWIND statement
#[derive(Debug, Clone, PartialEq)]
pub struct UnwindStmt {
    pub span: Span,
    pub expression: ContextualExpression,
    pub variable: String,
    pub return_clause: Option<ReturnClause>,
    pub order_by: Option<OrderByClause>,
    pub limit: Option<usize>,
    pub skip: Option<usize>,
}

/// The RETURN statement
#[derive(Debug, Clone, PartialEq)]
pub struct ReturnStmt {
    pub span: Span,
    pub items: Vec<ReturnItem>,
    pub distinct: bool,
    pub order_by: Option<OrderByClause>,
    pub skip: Option<usize>,
    pub limit: Option<usize>,
}

/// The `WITH` statement
#[derive(Debug, Clone, PartialEq)]
pub struct WithStmt {
    pub span: Span,
    pub items: Vec<ReturnItem>,
    pub where_clause: Option<ContextualExpression>,
    pub distinct: bool,
    pub order_by: Option<OrderByClause>,
    pub skip: Option<usize>,
    pub limit: Option<usize>,
}

/// The YIELD statement
#[derive(Debug, Clone, PartialEq)]
pub struct YieldStmt {
    pub span: Span,
    pub items: Vec<YieldItem>,
    pub where_clause: Option<ContextualExpression>,
    pub distinct: bool,
    pub order_by: Option<OrderByClause>,
    pub skip: Option<usize>,
    pub limit: Option<usize>,
}

/// SET statement
#[derive(Debug, Clone, PartialEq)]
pub struct SetStmt {
    pub span: Span,
    pub assignments: Vec<Assignment>,
}

/// “REMOVE” statement
#[derive(Debug, Clone, PartialEq)]
pub struct RemoveStmt {
    pub span: Span,
    pub items: Vec<ContextualExpression>,
}

/// The PIPE statement
#[derive(Debug, Clone)]
pub struct PipeStmt {
    pub span: Span,
    pub left: Box<Stmt>,
    pub right: Box<Stmt>,
}

impl PartialEq for PipeStmt {
    fn eq(&self, other: &Self) -> bool {
        self.span == other.span
    }
}

/// MATCH clause (used within the MATCH statement)
#[derive(Debug, Clone, PartialEq)]
pub struct MatchClause {
    pub span: Span,
    pub patterns: Vec<Pattern>,
    pub optional: bool,
}

/// The WITH clause (used for subquery pipelines)
#[derive(Debug, Clone, PartialEq)]
pub struct WithClause {
    pub span: Span,
    pub items: Vec<ReturnItem>,
    pub where_clause: Option<ContextualExpression>,
}

// Statement Tool Functions
pub struct StmtUtils;

impl StmtUtils {
    /// Retrieve all the variables used in the statement.
    pub fn find_variables(stmt: &Stmt) -> Vec<String> {
        let mut variables = Vec::new();
        Self::find_variables_recursive(stmt, &mut variables);
        variables
    }

    fn find_variables_recursive(stmt: &Stmt, variables: &mut Vec<String>) {
        match stmt {
            Stmt::Match(s) => {
                for pattern in &s.patterns {
                    variables.extend(PatternUtils::find_variables(pattern));
                }
                if let Some(ref where_clause) = s.where_clause {
                    variables.extend(collect_variables_from_contextual(where_clause));
                }
            }
            Stmt::Create(s) => match &s.target {
                CreateTarget::Node {
                    properties: Some(props),
                    ..
                } => {
                    variables.extend(collect_variables_from_contextual(props));
                }
                CreateTarget::Edge {
                    src,
                    dst,
                    properties: Some(props),
                    ..
                } => {
                    variables.extend(collect_variables_from_contextual(src));
                    variables.extend(collect_variables_from_contextual(dst));
                    variables.extend(collect_variables_from_contextual(props));
                }
                _ => {}
            },
            Stmt::Delete(s) => {
                match &s.target {
                    DeleteTarget::Vertices(vertices) => {
                        for vertex in vertices {
                            variables.extend(collect_variables_from_contextual(vertex));
                        }
                    }
                    DeleteTarget::Edges { edges, .. } => {
                        for (src, dst, rank) in edges {
                            variables.extend(collect_variables_from_contextual(src));
                            variables.extend(collect_variables_from_contextual(dst));
                            if let Some(ref rank) = rank {
                                variables.extend(collect_variables_from_contextual(rank));
                            }
                        }
                    }
                    _ => {}
                }
                if let Some(ref where_clause) = s.where_clause {
                    variables.extend(collect_variables_from_contextual(where_clause));
                }
            }
            Stmt::Update(s) => {
                match &s.target {
                    UpdateTarget::Vertex(vertex) => {
                        variables.extend(collect_variables_from_contextual(vertex));
                    }
                    UpdateTarget::Edge { src, dst, rank, .. } => {
                        variables.extend(collect_variables_from_contextual(src));
                        variables.extend(collect_variables_from_contextual(dst));
                        if let Some(ref rank) = rank {
                            variables.extend(collect_variables_from_contextual(rank));
                        }
                    }
                    _ => {}
                }
                for assignment in &s.set_clause.assignments {
                    variables.extend(collect_variables_from_contextual(&assignment.value));
                }
                if let Some(ref where_clause) = s.where_clause {
                    variables.extend(collect_variables_from_contextual(where_clause));
                }
            }
            Stmt::Go(s) => {
                for vertex in &s.from.vertices {
                    variables.extend(collect_variables_from_contextual(vertex));
                }
                if let Some(ref where_clause) = s.where_clause {
                    variables.extend(collect_variables_from_contextual(where_clause));
                }
            }
            Stmt::Fetch(s) => match &s.target {
                FetchTarget::Vertices { ids, .. } => {
                    for id in ids {
                        variables.extend(collect_variables_from_contextual(id));
                    }
                }
                FetchTarget::Edges { src, dst, rank, .. } => {
                    variables.extend(collect_variables_from_contextual(src));
                    variables.extend(collect_variables_from_contextual(dst));
                    if let Some(ref rank) = rank {
                        variables.extend(collect_variables_from_contextual(rank));
                    }
                }
            },
            Stmt::Lookup(s) => {
                if let Some(ref where_clause) = s.where_clause {
                    variables.extend(collect_variables_from_contextual(where_clause));
                }
            }
            Stmt::Subgraph(s) => {
                for vertex in &s.from.vertices {
                    variables.extend(collect_variables_from_contextual(vertex));
                }
                if let Some(ref where_clause) = s.where_clause {
                    variables.extend(collect_variables_from_contextual(where_clause));
                }
            }
            Stmt::FindPath(s) => {
                for vertex in &s.from.vertices {
                    variables.extend(collect_variables_from_contextual(vertex));
                }
                variables.extend(collect_variables_from_contextual(&s.to));
                if let Some(ref where_clause) = s.where_clause {
                    variables.extend(collect_variables_from_contextual(where_clause));
                }
            }
            _ => {}
        }
    }
}

/// DROP statement – Deletes spaces, tags, edge types, or indexes
#[derive(Debug, Clone, PartialEq)]
pub struct DropStmt {
    pub span: Span,
    pub target: DropTarget,
    pub if_exists: bool,
}

/// DROP target
#[derive(Debug, Clone, PartialEq)]
pub enum DropTarget {
    Space(String),
    Tags(Vec<String>),
    Edges(Vec<String>),
    TagIndex {
        space_name: String,
        index_name: String,
    },
    EdgeIndex {
        space_name: String,
        index_name: String,
    },
}

/// The `DESCRIBE` statement is used to describe the type of a space, tag, or edge.
#[derive(Debug, Clone, PartialEq)]
pub struct DescStmt {
    pub span: Span,
    pub target: DescTarget,
}

/// Describe the target.
#[derive(Debug, Clone, PartialEq)]
pub enum DescTarget {
    Space(String),
    Tag {
        space_name: String,
        tag_name: String,
    },
    Edge {
        space_name: String,
        edge_name: String,
    },
}

/// ALTER statement – Modifies the type of tags or edges
#[derive(Debug, Clone, PartialEq)]
pub struct AlterStmt {
    pub span: Span,
    pub target: AlterTarget,
}

/// Attribute modification definition (used for the CHANGE operation)
#[derive(Debug, Clone, PartialEq)]
pub struct PropertyChange {
    pub old_name: String,
    pub new_name: String,
    pub data_type: super::types::DataType,
}

/// ALTER target
#[derive(Debug, Clone, PartialEq)]
pub enum AlterTarget {
    Tag {
        tag_name: String,
        additions: Vec<PropertyDef>,
        deletions: Vec<String>,
        changes: Vec<PropertyChange>,
    },
    Edge {
        edge_name: String,
        additions: Vec<PropertyDef>,
        deletions: Vec<String>,
        changes: Vec<PropertyChange>,
    },
    Space {
        space_name: String,
        comment: Option<String>,
    },
}

/// The `CREATE USER` statement
#[derive(Debug, Clone, PartialEq)]
pub struct CreateUserStmt {
    pub span: Span,
    pub username: String,
    pub password: String,
    pub role: Option<String>,
    pub if_not_exists: bool,
}

/// The `ALTER USER` statement
#[derive(Debug, Clone, PartialEq)]
pub struct AlterUserStmt {
    pub span: Span,
    pub username: String,
    pub password: Option<String>,
    pub new_role: Option<String>,
    pub is_locked: Option<bool>,
}

/// The `DROP USER` statement
#[derive(Debug, Clone, PartialEq)]
pub struct DropUserStmt {
    pub span: Span,
    pub username: String,
    pub if_exists: bool,
}

/// The “CHANGE PASSWORD” statement
#[derive(Debug, Clone, PartialEq)]
pub struct ChangePasswordStmt {
    pub span: Span,
    pub username: Option<String>,
    pub old_password: String,
    pub new_password: String,
}

/// Role types – used in GRANT/REVOKE statements
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoleType {
    God,
    Admin,
    Dba,
    User,
    Guest,
}

impl RoleType {
    pub fn as_str(&self) -> &'static str {
        match self {
            RoleType::God => "GOD",
            RoleType::Admin => "ADMIN",
            RoleType::Dba => "DBA",
            RoleType::User => "USER",
            RoleType::Guest => "GUEST",
        }
    }
}

impl std::str::FromStr for RoleType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_uppercase().as_str() {
            "GOD" => Ok(RoleType::God),
            "ADMIN" => Ok(RoleType::Admin),
            "DBA" => Ok(RoleType::Dba),
            "USER" => Ok(RoleType::User),
            "GUEST" => Ok(RoleType::Guest),
            _ => Err(format!("Unknown character type: {}", s)),
        }
    }
}

/// The `GRANT` statement
#[derive(Debug, Clone, PartialEq)]
pub struct GrantStmt {
    pub span: Span,
    pub role: RoleType,
    pub space_name: String,
    pub username: String,
}

/// The REVOKE statement
#[derive(Debug, Clone, PartialEq)]
pub struct RevokeStmt {
    pub span: Span,
    pub role: RoleType,
    pub space_name: String,
    pub username: String,
}

/// “DESCRIBE USER” statement
#[derive(Debug, Clone, PartialEq)]
pub struct DescribeUserStmt {
    pub span: Span,
    pub username: String,
}

/// The “SHOW USERS” statement
#[derive(Debug, Clone, PartialEq)]
pub struct ShowUsersStmt {
    pub span: Span,
}

/// The `SHOW ROLES` statement
#[derive(Debug, Clone, PartialEq)]
pub struct ShowRolesStmt {
    pub span: Span,
    pub space_name: Option<String>,
}

/// The `SHOW CREATE` statement
#[derive(Debug, Clone, PartialEq)]
pub struct ShowCreateStmt {
    pub span: Span,
    pub target: ShowCreateTarget,
}

/// The SHOW CREATE statement is used to display information about the creation of a database object, such as a table, index, view, or procedure.
#[derive(Debug, Clone, PartialEq)]
pub enum ShowCreateTarget {
    Space(String),
    Tag(String),
    Edge(String),
    Index(String),
}

/// The “CLEAR SPACE” statement
#[derive(Debug, Clone, PartialEq)]
pub struct ClearSpaceStmt {
    pub span: Span,
    pub space_name: String,
}

/// BEGIN TRANSACTION statement
#[derive(Debug, Clone, PartialEq)]
pub struct BeginTransactionStmt {
    pub span: Span,
}

/// COMMIT TRANSACTION statement
#[derive(Debug, Clone, PartialEq)]
pub struct CommitTransactionStmt {
    pub span: Span,
}

/// ROLLBACK TRANSACTION statement
#[derive(Debug, Clone, PartialEq)]
pub struct RollbackTransactionStmt {
    pub span: Span,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_stmt() {
        let stmt = Stmt::Create(CreateStmt {
            span: Span::default(),
            target: CreateTarget::Node {
                variable: Some("n".to_string()),
                labels: vec!["Person".to_string()],
                properties: None,
            },
            if_not_exists: false,
        });

        assert!(matches!(stmt, Stmt::Create(_)));
    }

    #[test]
    fn test_match_stmt() {
        let stmt = Stmt::Match(MatchStmt {
            span: Span::default(),
            patterns: vec![],
            where_clause: None,
            return_clause: None,
            order_by: None,
            limit: None,
            skip: None,
            optional: false,
            delete_clause: None,
        });

        assert!(matches!(stmt, Stmt::Match(_)));
    }

    #[test]
    fn test_lookup_stmt() {
        let stmt = Stmt::Lookup(LookupStmt {
            span: Span::default(),
            target: LookupTarget::Tag("Person".to_string()),
            where_clause: None,
            yield_clause: None,
        });

        assert!(matches!(stmt, Stmt::Lookup(_)));
    }

    #[test]
    fn test_subgraph_stmt() {
        let stmt = Stmt::Subgraph(SubgraphStmt {
            span: Span::default(),
            steps: Steps::Fixed(1),
            from: FromClause {
                span: Span::default(),
                vertices: vec![],
            },
            over: None,
            where_clause: None,
            yield_clause: None,
        });

        assert!(matches!(stmt, Stmt::Subgraph(_)));
    }

    #[test]
    fn test_find_path_stmt() {
        use std::sync::Arc;

        let expr_context = Arc::new(ExpressionAnalysisContext::new());
        let expr = crate::core::types::expr::Expression::Variable("target".to_string());
        let expr_meta = crate::core::types::expr::ExpressionMeta::new(expr);
        let expr_id = expr_context.register_expression(expr_meta);
        let to_expr =
            crate::core::types::expr::contextual::ContextualExpression::new(expr_id, expr_context);

        let stmt = Stmt::FindPath(FindPathStmt {
            span: Span::default(),
            from: FromClause {
                span: Span::default(),
                vertices: vec![],
            },
            to: to_expr,
            over: None,
            where_clause: None,
            shortest: true,
            max_steps: None,
            limit: None,
            offset: None,
            yield_clause: None,
            weight_expression: None,
            heuristic_expression: None,
            with_loop: false,
            with_cycle: false,
        });

        assert!(matches!(stmt, Stmt::FindPath(_)));
    }
}
