use super::QueryPipelineManager;
use crate::binder::Binder;
use crate::binder::BoundStatement;
use crate::parser::Parser;
use crate::storage::QueryStorage;
use crate::QueryContext;
use graphdb_core::error::{DBError, DBResult, QueryError};
use graphdb_metrics::MetricType;
use std::sync::Arc;

impl<S: QueryStorage + 'static> QueryPipelineManager<S> {
    pub(crate) fn parse_into_context(
        &mut self,
        query_text: &str,
    ) -> DBResult<crate::parser::ParserResult> {
        // Experimental parser extensions get first shot at the raw text.
        let rewritten_text;
        let effective_text = match self.extensions.try_parse_transform(query_text) {
            Some(Ok(rewritten)) => {
                rewritten_text = rewritten;
                rewritten_text.as_str()
            }
            Some(Err(error)) => return Err(error),
            None => query_text,
        };
        let mut parser = Parser::new(effective_text)
            // User-defined type aliases resolve in CAST targets and DDL
            // column types against the shared engine-wide catalog.
            .with_type_alias_manager(self.type_alias_manager.clone());
        let result = parser
            .parse()
            .map_err(|e| DBError::from(QueryError::pipeline_parse_error(e)))?;
        if parser.has_errors() {
            return Err(DBError::from(QueryError::pipeline_parse_error(
                parser.take_errors(),
            )));
        }
        Ok(result)
    }

    pub(crate) fn record_query_type_counter(&self, stmt: &crate::parser::ast::Stmt) {
        use crate::parser::ast::Stmt;
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
        ast: Arc<crate::parser::ast::stmt::Ast>,
        qctx: Arc<QueryContext>,
    ) -> DBResult<Option<BoundStatement>> {
        let space_id = qctx.space_id().unwrap_or(0);
        let space_name = qctx
            .space_name()
            .or_else(|| qctx.request_context().space_name.clone());

        let mut binder = Binder::new().with_space(space_name.clone(), space_id);

        // Experimental binder extensions get first shot at the parsed AST.
        let extension_ctx = crate::extensions::BinderExtensionContext {
            space_name: space_name.clone(),
            space_id,
        };
        if let Some(rewritten) = self.extensions.try_bind(&ast, &extension_ctx) {
            return rewritten.map(Some);
        }

        if let Some(ref schema_manager) = self.schema_manager {
            binder = binder.with_schema_manager(schema_manager.clone());
        }
        // User-defined macros expand at bind time against the shared
        // engine-wide catalog.
        binder = binder.with_macro_manager(self.macro_manager.clone());

        binder.bind(ast).map(Some)
    }
}
