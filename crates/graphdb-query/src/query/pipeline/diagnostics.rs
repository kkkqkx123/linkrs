use super::QueryPipelineManager;
use crate::core::error::{DBError, DBResult, QueryError};
use crate::core::types::expr::expression_context::ExpressionAnalysisContext;
use crate::query::executor::base::{BaseExecutor, ExecutionResult, Executor};
use crate::query::executor::explain::ProfileExecutor;
use crate::query::executor::streaming::instance::{
    QueryBindings, QueryExecutionInstance, ResultSink,
};
use crate::query::executor::streaming::transaction_scope::TransactionScope;
use crate::query::parser::ast::stmt::{ExplainStmt, ProfileStmt};
use crate::query::validator::ValidatedStatement;
use crate::query::QueryContext;
use crate::storage::QueryStorage;
use std::sync::Arc;

impl<S: QueryStorage + 'static> QueryPipelineManager<S> {
    pub fn execute_explain(
        &mut self,
        explain_stmt: &ExplainStmt,
        qctx: Arc<QueryContext>,
    ) -> DBResult<ExecutionResult> {
        let inner_ast = &explain_stmt.statement;
        let expr_ctx = Arc::new(ExpressionAnalysisContext::new());
        let validation_info = self.validate_query_with_context(
            Arc::new(crate::query::parser::ast::stmt::Ast::new(
                (**inner_ast).clone(),
                expr_ctx.clone(),
            )),
            qctx.clone(),
        )?;
        let inner_validated = ValidatedStatement::new(
            Arc::new(crate::query::parser::ast::stmt::Ast::new(
                (**inner_ast).clone(),
                expr_ctx,
            )),
            validation_info,
        );

        let (physical_plan, _optimized_plan) = self.compile(qctx.clone(), &inner_validated)?;

        let plan_desc =
            crate::query::executor::explain::physical_plan_explain::physical_plan_to_plan_description(
                &physical_plan,
            );

        let output = match explain_stmt.format {
            crate::query::parser::ast::stmt::ExplainFormat::Table => {
                crate::query::executor::explain::format::format_plan_as_table(&plan_desc)
            }
            crate::query::parser::ast::stmt::ExplainFormat::Dot => {
                crate::query::executor::explain::format::format_plan_as_dot(&plan_desc)
            }
        };

        let data_set = crate::core::DataSet::from_rows(
            vec![vec![crate::core::Value::String(output)]],
            vec!["plan".to_string()],
        );
        Ok(ExecutionResult::DataSet { data: data_set })
    }

    pub fn execute_explain_analyze(
        &mut self,
        explain_stmt: &ExplainStmt,
        qctx: Arc<QueryContext>,
    ) -> DBResult<ExecutionResult> {
        let inner_ast = &explain_stmt.statement;
        let expr_ctx = Arc::new(ExpressionAnalysisContext::new());
        let validation_info = self.validate_query_with_context(
            Arc::new(crate::query::parser::ast::stmt::Ast::new(
                (**inner_ast).clone(),
                expr_ctx.clone(),
            )),
            qctx.clone(),
        )?;
        let inner_validated = ValidatedStatement::new(
            Arc::new(crate::query::parser::ast::stmt::Ast::new(
                (**inner_ast).clone(),
                expr_ctx,
            )),
            validation_info,
        );

        let (physical_plan, _optimized_plan) = self.compile(qctx.clone(), &inner_validated)?;

        let exec_ctx = self.build_execution_context(&qctx);
        let mut bindings = QueryBindings::from_context(&exec_ctx, TransactionScope::None);
        bindings.query_id = exec_ctx.query_id;

        let mut instance = QueryExecutionInstance::instantiate_plan(
            physical_plan.clone(),
            bindings,
            ResultSink::Materialize,
            self.query_registry.clone(),
        )
        .map_err(|e| DBError::from(QueryError::execution(e.to_string())))?;

        let _result = instance
            .execute()
            .map_err(|e| DBError::from(QueryError::execution(e.to_string())))?;

        let plan_desc =
            crate::query::executor::explain::physical_plan_explain::physical_plan_to_plan_description(
                &physical_plan,
            );

        let output = match explain_stmt.format {
            crate::query::parser::ast::stmt::ExplainFormat::Table => {
                crate::query::executor::explain::format::format_plan_as_table(&plan_desc)
            }
            crate::query::parser::ast::stmt::ExplainFormat::Dot => {
                crate::query::executor::explain::format::format_plan_as_dot(&plan_desc)
            }
        };

        let data_set = crate::core::DataSet::from_rows(
            vec![vec![crate::core::Value::String(output)]],
            vec!["plan".to_string()],
        );
        Ok(ExecutionResult::DataSet { data: data_set })
    }

    pub fn execute_profile(
        &mut self,
        profile_stmt: &ProfileStmt,
        qctx: Arc<QueryContext>,
    ) -> DBResult<ExecutionResult> {
        let inner_ast = &profile_stmt.statement;
        let expr_ctx = Arc::new(ExpressionAnalysisContext::new());
        let validation_info = self.validate_query_with_context(
            Arc::new(crate::query::parser::ast::stmt::Ast::new(
                (**inner_ast).clone(),
                expr_ctx.clone(),
            )),
            qctx.clone(),
        )?;
        let inner_validated = ValidatedStatement::new(
            Arc::new(crate::query::parser::ast::stmt::Ast::new(
                (**inner_ast).clone(),
                expr_ctx,
            )),
            validation_info,
        );
        let inner_plan = self.generate_execution_plan(qctx.clone(), &inner_validated)?;
        let optimized_plan = self.optimize_execution_plan(inner_plan)?;

        let storage = self.storage.clone().ok_or_else(|| {
            DBError::from(QueryError::execution("Storage not available".to_string()))
        })?;

        let base = BaseExecutor::new(
            -1,
            "ProfileExecutor".to_string(),
            storage,
            Arc::new(ExpressionAnalysisContext::new()),
        );

        let mut profile_executor =
            ProfileExecutor::new(base, optimized_plan, profile_stmt.format.clone());

        profile_executor
            .execute()
            .map_err(|e| DBError::from(QueryError::execution(e.to_string())))
    }
}
