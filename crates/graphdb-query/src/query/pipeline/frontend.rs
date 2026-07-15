use super::QueryPipelineManager;
use crate::core::error::{DBError, DBResult, QueryError};
use crate::core::MetricType;
use crate::query::parser::Parser;
use crate::query::validator::ValidationInfo;
use crate::query::QueryContext;
use crate::storage::StorageClient;
use std::sync::Arc;

impl<S: StorageClient + 'static> QueryPipelineManager<S> {
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

    pub(crate) fn validate_query_with_context(
        &mut self,
        ast: Arc<crate::query::parser::ast::stmt::Ast>,
        qctx: Arc<QueryContext>,
    ) -> DBResult<ValidationInfo> {
        let mut validator =
            crate::query::validator::Validator::create_from_ast(&ast).ok_or_else(|| {
                DBError::from(QueryError::invalid_query(format!(
                    "Unsupported statement type: {:?}",
                    ast.stmt
                )))
            })?;

        if let Some(ref schema_manager) = self.schema_manager {
            validator.set_schema_manager(schema_manager.clone());
        }

        let validation_result = validator.validate(ast.clone(), qctx);

        if validation_result.success {
            Ok(validation_result.info.unwrap_or_default())
        } else {
            let error_msg = validation_result
                .errors
                .iter()
                .map(|e| e.to_string())
                .collect::<Vec<_>>()
                .join(", ");
            Err(DBError::from(QueryError::invalid_query(error_msg)))
        }
    }
}
