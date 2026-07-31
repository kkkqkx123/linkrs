use super::QueryPipelineManager;
use crate::core::error::{DBError, DBResult, QueryError};
use crate::core::MetricType;
use crate::query::binder::Binder;
use crate::query::binder::BoundStatement;
use crate::query::parser::Parser;
use crate::query::QueryContext;
use crate::storage::QueryStorage;
use std::sync::Arc;

impl<S: QueryStorage + 'static> QueryPipelineManager<S> {
    pub(crate) fn parse_into_context(
        &mut self,
        query_text: &str,
    ) -> DBResult<crate::query::parser::ParserResult> {
        let mut parser = Parser::new(query_text);
        parser
            .parse()
            .map_err(|e| DBError::from(QueryError::pipeline_parse_error(e)))
    }

    pub(crate) fn record_query_type_counter(&self, stmt: &crate::query::parser::ast::Stmt) {
        use crate::query::parser::ast::Stmt;
        let metric_type = match stmt {
            Stmt::Match(_) => Some(MetricType::NumMatchQueries),
            Stmt::Create(_) => Some(MetricType::NumCreateQueries),
            Stmt::Update(_) => Some(MetricType::NumUpdateQueries),
            Stmt::Delete(_) => Some(MetricType::NumDeleteQueries),
            Stmt::Insert(_) => Some(MetricType::NumInsertQueries),
            Stmt::Go(_) => Some(MetricType::NumGoQueries),
            Stmt::Fetch(_) => Some(MetricType::NumFetchQueries),
            Stmt::Lookup(_) => Some(MetricType::NumLookupQueries),
            Stmt::Show(_) => Some(MetricType::NumShowQueries),
            _ => None,
        };
        if let Some(metric) = metric_type {
            self.stats_manager.add_value(metric);
        }
    }

    /// Bind a parsed AST into a [`BoundStatement`].
    ///
    /// The Binder performs both semantic validation and name resolution
    /// in a single pass, so a separate validation phase is unnecessary.
    pub(crate) fn bind_parsed_statement(
        &mut self,
        ast: Arc<crate::query::parser::ast::stmt::Ast>,
        qctx: Arc<QueryContext>,
    ) -> DBResult<Option<BoundStatement>> {
        let space_id = qctx.space_id().unwrap_or(0);
        let space_name = qctx
            .space_name()
            .or_else(|| qctx.request_context().space_name.clone());

        let mut binder = Binder::new().with_space(space_name.clone(), space_id);

        if let Some(ref schema_manager) = self.schema_manager {
            binder = binder.with_schema_manager(schema_manager.clone());
        }

        binder.bind(ast, qctx).map(Some)
    }
}
