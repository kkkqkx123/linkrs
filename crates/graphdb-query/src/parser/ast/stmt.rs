use std::sync::Arc;

pub use super::pattern::*;
pub use super::types::*;
use graphdb_core::types::expr::expression_context::ExpressionAnalysisContext;

mod dcl;
mod ddl;
mod dml;
mod dql;
mod management;
mod search;
mod transaction;

pub use dcl::*;
pub use ddl::*;
pub use dml::*;
pub use dql::*;
pub use management::*;
pub use search::*;
pub use transaction::*;

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
        }
    }

    pub fn as_explain(&self) -> Option<&ExplainStmt> {
        match self {
            Stmt::Explain(s) => Some(s),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::ast::vector::{VectorDistance, VectorIndexConfig, VectorMatchCondition};
    use graphdb_core::types::expr::contextual::ContextualExpression;

    fn ctx_expr() -> ContextualExpression {
        let expr_context = Arc::new(ExpressionAnalysisContext::new());
        let expr = graphdb_core::types::expr::Expression::Variable("x".to_string());
        let expr_meta = graphdb_core::types::expr::ExpressionMeta::new(expr);
        let expr_id = expr_context.register_expression(expr_meta);
        ContextualExpression::new(expr_id, expr_context)
    }

    #[test]
    fn test_stmt_category_exhaustive() {
        use StmtCategory::*;
        let span = Span::default();
        let from = FromClause {
            span,
            vertices: vec![],
        };
        let yield_clause = YieldClause {
            span,
            items: vec![],
            where_clause: None,
            order_by: None,
            limit: None,
            skip: None,
            sample: None,
        };
        let nested = move || {
            Stmt::Use(UseStmt {
                span,
                space: "s".to_string(),
            })
        };
        let vector_query = VectorQueryExpr {
            span,
            query_type: crate::parser::ast::vector::VectorQueryType::Vector,
            query_data: "[0.1, 0.2, 0.3]".to_string(),
        };
        let cases: Vec<(Stmt, StmtCategory)> = vec![
            (Stmt::Query(QueryStmt::new(vec![], span)), Query),
            (
                Stmt::Pipe(PipeStmt {
                    span,
                    left: Box::new(nested()),
                    right: Box::new(nested()),
                }),
                Query,
            ),
            (
                Stmt::SetOperation(SetOperationStmt {
                    span,
                    op_type: SetOperationType::Union,
                    left: Box::new(nested()),
                    right: Box::new(nested()),
                }),
                Query,
            ),
            (
                Stmt::Assignment(AssignmentStmt {
                    span,
                    variable: "v".to_string(),
                    statement: Box::new(nested()),
                }),
                Query,
            ),
            (
                Stmt::Match(MatchStmt {
                    span,
                    patterns: vec![],
                    where_clause: None,
                    return_clause: None,
                    order_by: None,
                    limit: None,
                    skip: None,
                    optional: false,
                    delete_clause: None,
                }),
                Dql,
            ),
            (
                Stmt::Go(GoStmt {
                    span,
                    steps: Steps::Fixed(1),
                    from: from.clone(),
                    over: None,
                    where_clause: None,
                    yield_clause: None,
                }),
                Dql,
            ),
            (
                Stmt::Fetch(FetchStmt {
                    span,
                    target: FetchTarget::Vertices {
                        tag_name: None,
                        ids: vec![],
                        properties: None,
                    },
                    yield_clause: None,
                }),
                Dql,
            ),
            (
                Stmt::Lookup(LookupStmt {
                    span,
                    target: LookupTarget::Tag("Person".to_string()),
                    where_clause: None,
                    yield_clause: None,
                }),
                Dql,
            ),
            (
                Stmt::Subgraph(SubgraphStmt {
                    span,
                    steps: Steps::Fixed(1),
                    from: from.clone(),
                    over: None,
                    where_clause: None,
                    yield_clause: None,
                }),
                Dql,
            ),
            (
                Stmt::FindPath(FindPathStmt {
                    span,
                    from,
                    to: ctx_expr(),
                    over: None,
                    where_clause: None,
                    shortest: true,
                    max_steps: None,
                    limit: None,
                    skip: None,
                    yield_clause: None,
                    weight_expression: None,
                    heuristic_expression: None,
                    with_loop: false,
                    with_cycle: false,
                }),
                Dql,
            ),
            (
                Stmt::Unwind(UnwindStmt {
                    span,
                    expression: ctx_expr(),
                    variable: "v".to_string(),
                    return_clause: None,
                    order_by: None,
                    limit: None,
                    skip: None,
                }),
                Dql,
            ),
            (
                Stmt::Return(ReturnStmt {
                    span,
                    items: vec![],
                    distinct: false,
                    order_by: None,
                    skip: None,
                    limit: None,
                }),
                Dql,
            ),
            (
                Stmt::With(WithStmt {
                    span,
                    items: vec![],
                    where_clause: None,
                    distinct: false,
                    order_by: None,
                    skip: None,
                    limit: None,
                    recursive: false,
                }),
                Dql,
            ),
            (
                Stmt::Yield(YieldStmt {
                    span,
                    items: vec![],
                    where_clause: None,
                    distinct: false,
                    order_by: None,
                    skip: None,
                    limit: None,
                }),
                Dql,
            ),
            (
                Stmt::Filter(FilterStmt {
                    span,
                    expression: ctx_expr(),
                }),
                Dql,
            ),
            (
                Stmt::Collect(CollectStmt {
                    span,
                    items: vec![],
                }),
                Dql,
            ),
            (
                Stmt::GroupBy(GroupByStmt {
                    span,
                    group_items: vec![],
                    grouping_type: GroupingType::Standard,
                    yield_clause,
                    having_clause: None,
                }),
                Dql,
            ),
            (
                Stmt::AssignVariable(AssignVariableStmt {
                    span,
                    name: "x".to_string(),
                    expression: ctx_expr(),
                }),
                Dql,
            ),
            (
                Stmt::Insert(InsertStmt {
                    span,
                    target: InsertTarget::Vertices {
                        tags: vec![],
                        values: vec![],
                    },
                    if_not_exists: false,
                }),
                Dml,
            ),
            (
                Stmt::Merge(MergeStmt {
                    span,
                    pattern: Pattern::Node(NodePattern::new(None, vec![], None, vec![], span)),
                    on_create: None,
                    on_match: None,
                }),
                Dml,
            ),
            (
                Stmt::Update(UpdateStmt {
                    span,
                    target: UpdateTarget::Vertex(ctx_expr()),
                    set_clause: SetClause {
                        span,
                        assignments: vec![],
                    },
                    where_clause: None,
                    is_upsert: false,
                    yield_clause: None,
                }),
                Dml,
            ),
            (
                Stmt::Delete(DeleteStmt {
                    span,
                    target: DeleteTarget::Vertices(vec![]),
                    where_clause: None,
                    with_edge: false,
                }),
                Dml,
            ),
            (
                Stmt::Set(SetStmt {
                    span,
                    assignments: vec![],
                }),
                Dml,
            ),
            (
                Stmt::Remove(RemoveStmt {
                    span,
                    items: vec![],
                }),
                Dml,
            ),
            (
                Stmt::Create(CreateStmt {
                    span,
                    target: CreateTarget::Tag {
                        name: "tag".to_string(),
                        properties: vec![],
                        ttl_duration: None,
                        ttl_col: None,
                    },
                    if_not_exists: false,
                }),
                Ddl,
            ),
            (
                Stmt::Drop(DropStmt {
                    span,
                    target: DropTarget::Space("s".to_string()),
                    if_exists: false,
                }),
                Ddl,
            ),
            (
                Stmt::Alter(AlterStmt {
                    span,
                    target: AlterTarget::Space {
                        space_name: "s".to_string(),
                        comment: None,
                    },
                }),
                Ddl,
            ),
            (
                Stmt::Desc(DescStmt {
                    span,
                    target: DescTarget::Space("s".to_string()),
                }),
                Ddl,
            ),
            (
                Stmt::ShowCreate(ShowCreateStmt {
                    span,
                    target: ShowCreateTarget::Space("s".to_string()),
                }),
                Ddl,
            ),
            (
                Stmt::CreateFulltextIndex(CreateFulltextIndex::new(
                    span,
                    "idx".to_string(),
                    "s".to_string(),
                    vec![],
                    graphdb_core::types::FulltextEngineType::Bm25,
                )),
                Ddl,
            ),
            (
                Stmt::DropFulltextIndex(DropFulltextIndex {
                    span,
                    index_name: "idx".to_string(),
                    if_exists: false,
                }),
                Ddl,
            ),
            (
                Stmt::AlterFulltextIndex(AlterFulltextIndex {
                    span,
                    index_name: "idx".to_string(),
                    actions: vec![],
                }),
                Ddl,
            ),
            (
                Stmt::ShowFulltextIndex(ShowFulltextIndex {
                    span,
                    pattern: None,
                    from_schema: None,
                }),
                Ddl,
            ),
            (
                Stmt::DescribeFulltextIndex(DescribeFulltextIndex {
                    span,
                    index_name: "idx".to_string(),
                }),
                Ddl,
            ),
            (
                Stmt::CreateVectorIndex(CreateVectorIndex {
                    span,
                    index_name: "idx".to_string(),
                    schema_name: "s".to_string(),
                    field_name: "f".to_string(),
                    config: VectorIndexConfig {
                        vector_size: 3,
                        distance: VectorDistance::Cosine,
                        hnsw_m: None,
                        hnsw_ef_construct: None,
                        quantization: None,
                        quantile: None,
                        compression: None,
                        always_ram: None,
                    },
                    if_not_exists: false,
                }),
                Ddl,
            ),
            (
                Stmt::DropVectorIndex(DropVectorIndex {
                    span,
                    index_name: "idx".to_string(),
                    if_exists: false,
                }),
                Ddl,
            ),
            (
                Stmt::CreateUser(CreateUserStmt {
                    span,
                    username: "u".to_string(),
                    password: "p".to_string(),
                    role: None,
                    if_not_exists: false,
                }),
                Dcl,
            ),
            (
                Stmt::AlterUser(AlterUserStmt {
                    span,
                    username: "u".to_string(),
                    password: None,
                    new_role: None,
                    is_locked: None,
                }),
                Dcl,
            ),
            (
                Stmt::DropUser(DropUserStmt {
                    span,
                    username: "u".to_string(),
                    if_exists: false,
                }),
                Dcl,
            ),
            (
                Stmt::ChangePassword(ChangePasswordStmt {
                    span,
                    username: None,
                    old_password: "o".to_string(),
                    new_password: "n".to_string(),
                }),
                Dcl,
            ),
            (
                Stmt::Grant(GrantStmt {
                    span,
                    role: RoleType::User,
                    space_name: "s".to_string(),
                    username: "u".to_string(),
                }),
                Dcl,
            ),
            (
                Stmt::Revoke(RevokeStmt {
                    span,
                    role: RoleType::User,
                    space_name: "s".to_string(),
                    username: "u".to_string(),
                }),
                Dcl,
            ),
            (
                Stmt::DescribeUser(DescribeUserStmt {
                    span,
                    username: "u".to_string(),
                }),
                Dcl,
            ),
            (Stmt::ShowUsers(ShowUsersStmt { span }), Dcl),
            (
                Stmt::ShowRoles(ShowRolesStmt {
                    span,
                    space_name: None,
                }),
                Dcl,
            ),
            (
                Stmt::Use(UseStmt {
                    span,
                    space: "s".to_string(),
                }),
                Admin,
            ),
            (
                Stmt::Show(ShowStmt {
                    span,
                    target: ShowTarget::Spaces,
                }),
                Admin,
            ),
            (
                Stmt::Explain(ExplainStmt {
                    span,
                    statement: Box::new(nested()),
                    format: ExplainFormat::Table,
                    analyze: false,
                }),
                Admin,
            ),
            (
                Stmt::Profile(ProfileStmt {
                    span,
                    statement: Box::new(nested()),
                    format: ExplainFormat::Table,
                }),
                Admin,
            ),
            (Stmt::Analyze(AnalyzeStmt { span, space: None }), Admin),
            (Stmt::ShowSessions(ShowSessionsStmt { span }), Admin),
            (Stmt::ShowQueries(ShowQueriesStmt { span }), Admin),
            (
                Stmt::KillQuery(KillQueryStmt {
                    span,
                    session_id: 1,
                    plan_id: 1,
                }),
                Admin,
            ),
            (
                Stmt::ShowConfigs(ShowConfigsStmt { span, module: None }),
                Admin,
            ),
            (
                Stmt::UpdateConfigs(UpdateConfigsStmt {
                    span,
                    module: None,
                    config_name: "c".to_string(),
                    config_value: ctx_expr(),
                }),
                Admin,
            ),
            (
                Stmt::ClearSpace(ClearSpaceStmt {
                    span,
                    space_name: "s".to_string(),
                }),
                Admin,
            ),
            (
                Stmt::Search(SearchStatement::new(
                    "idx".to_string(),
                    FulltextQueryExpr::Simple("q".to_string()),
                )),
                Search,
            ),
            (
                Stmt::SearchVector(SearchVectorStatement {
                    span,
                    index_name: "idx".to_string(),
                    query: vector_query.clone(),
                    threshold: None,
                    where_clause: None,
                    order_clause: None,
                    limit: None,
                    skip: None,
                    yield_clause: None,
                }),
                Search,
            ),
            (
                Stmt::LookupFulltext(LookupFulltext {
                    span,
                    schema_name: "s".to_string(),
                    index_name: "idx".to_string(),
                    query: "q".to_string(),
                    yield_clause: None,
                    limit: None,
                }),
                Search,
            ),
            (
                Stmt::MatchFulltext(MatchFulltext {
                    span,
                    pattern: "p".to_string(),
                    fulltext_condition: FulltextMatchCondition {
                        field: "f".to_string(),
                        query: "q".to_string(),
                        index_name: None,
                    },
                    yield_clause: None,
                }),
                Search,
            ),
            (
                Stmt::LookupVector(LookupVector {
                    span,
                    schema_name: "s".to_string(),
                    index_name: "idx".to_string(),
                    query: vector_query.clone(),
                    yield_clause: None,
                    limit: None,
                }),
                Search,
            ),
            (
                Stmt::MatchVector(MatchVector {
                    span,
                    pattern: "p".to_string(),
                    vector_condition: VectorMatchCondition {
                        field: "f".to_string(),
                        query: vector_query,
                        threshold: None,
                    },
                    yield_clause: None,
                }),
                Search,
            ),
            (
                Stmt::BeginTransaction(BeginTransactionStmt {
                    span,
                    read_only: None,
                }),
                Transaction,
            ),
            (
                Stmt::CommitTransaction(CommitTransactionStmt { span }),
                Transaction,
            ),
            (
                Stmt::RollbackTransaction(RollbackTransactionStmt {
                    span,
                    savepoint_name: None,
                }),
                Transaction,
            ),
            (
                Stmt::Savepoint(SavepointStmt {
                    span,
                    name: "sp".to_string(),
                }),
                Transaction,
            ),
            (
                Stmt::ReleaseSavepoint(ReleaseSavepointStmt {
                    span,
                    name: "sp".to_string(),
                }),
                Transaction,
            ),
            (
                Stmt::Copy(CopyStmt {
                    span,
                    target: CopyTarget::Vertex("t".to_string()),
                    direction: CopyDirection::From,
                    file_path: "f.csv".to_string(),
                    header: true,
                    delimiter: ',',
                    batch_size: None,
                }),
                Dml,
            ),
        ];
        assert_eq!(cases.len(), 68, "Stmt has 68 variants; update this test");
        for (stmt, expected) in cases {
            assert_eq!(
                stmt.category(),
                expected,
                "category mismatch for {}",
                stmt.kind()
            );
        }
    }

    #[test]
    fn test_category_str_roundtrip() {
        let categories = [
            StmtCategory::Query,
            StmtCategory::Dql,
            StmtCategory::Dml,
            StmtCategory::Ddl,
            StmtCategory::Dcl,
            StmtCategory::Admin,
            StmtCategory::Search,
            StmtCategory::Transaction,
        ];
        let mut labels: Vec<&str> = categories.iter().map(|c| c.as_str()).collect();
        labels.sort_unstable();
        labels.dedup();
        assert_eq!(labels.len(), categories.len(), "labels must be distinct");
        for label in labels {
            assert!(!label.is_empty());
        }
    }

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
        let expr = graphdb_core::types::expr::Expression::Variable("target".to_string());
        let expr_meta = graphdb_core::types::expr::ExpressionMeta::new(expr);
        let expr_id = expr_context.register_expression(expr_meta);
        let to_expr =
            graphdb_core::types::expr::contextual::ContextualExpression::new(expr_id, expr_context);

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
            skip: None,
            yield_clause: None,
            weight_expression: None,
            heuristic_expression: None,
            with_loop: false,
            with_cycle: false,
        });

        assert!(matches!(stmt, Stmt::FindPath(_)));
    }
}
