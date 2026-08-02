use std::sync::Arc;

pub use super::pattern::*;
pub use super::types::*;
use crate::core::types::expr::expression_context::ExpressionAnalysisContext;

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
}

impl Stmt {
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
            Stmt::Analyze(s) => s.span,
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
            Stmt::Filter(s) => s.span,
            Stmt::Collect(s) => s.span,
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
            Stmt::CreateFulltextIndex(s) => s.span,
            Stmt::DropFulltextIndex(s) => s.span,
            Stmt::AlterFulltextIndex(s) => s.span,
            Stmt::ShowFulltextIndex(s) => s.span,
            Stmt::DescribeFulltextIndex(s) => s.span,
            Stmt::Search(s) => s.span,
            Stmt::LookupFulltext(s) => s.span,
            Stmt::MatchFulltext(s) => s.span,
            Stmt::CreateVectorIndex(s) => s.span,
            Stmt::DropVectorIndex(s) => s.span,
            Stmt::SearchVector(s) => s.span,
            Stmt::LookupVector(s) => s.span,
            Stmt::MatchVector(s) => s.span,
            Stmt::BeginTransaction(s) => s.span,
            Stmt::CommitTransaction(s) => s.span,
            Stmt::RollbackTransaction(s) => s.span,
        }
    }

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
            Stmt::BeginTransaction(_) => "BEGIN TRANSACTION",
            Stmt::CommitTransaction(_) => "COMMIT TRANSACTION",
            Stmt::RollbackTransaction(_) => "ROLLBACK TRANSACTION",
        }
    }

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
    pub fn as_filter(&self) -> Option<&FilterStmt> {
        match self {
            Stmt::Filter(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_collect(&self) -> Option<&CollectStmt> {
        match self {
            Stmt::Collect(s) => Some(s),
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
