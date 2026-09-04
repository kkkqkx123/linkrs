use crate::error::QueryPipelineError;
use crate::QueryContext;
use std::sync::Arc;

/// A single phase in the query processing pipeline.
///
/// Each stage takes an input and produces an output, with access to the
/// shared [`QueryContext`]. Stages are composable and can be wired
/// together by the pipeline coordinator.
pub trait QueryStage {
    type Input;
    type Output;

    fn process(
        &self,
        input: Self::Input,
        ctx: &QueryContext,
    ) -> Result<Self::Output, QueryPipelineError>;
}

/// Parse stage: raw query text → parsed AST.
pub struct ParseStage;

impl QueryStage for ParseStage {
    type Input = String;
    type Output = crate::parser::ParserResult;

    fn process(
        &self,
        input: String,
        _ctx: &QueryContext,
    ) -> Result<Self::Output, QueryPipelineError> {
        let mut parser = crate::parser::Parser::new(&input);
        let result = parser.parse().map_err(|e| QueryPipelineError::Parse {
            source: Box::new(e),
            query_text: input.clone(),
        })?;
        if parser.has_errors() {
            let errors = parser.take_errors();
            return Err(QueryPipelineError::Pipeline {
                phase: crate::error::PipelinePhase::Parse,
                message: errors.to_string(),
            });
        }
        Ok(result)
    }
}

/// Bind stage: parsed AST → bound statement.
pub struct BindStage {
    pub schema_manager: Option<Arc<graphdb_core::metadata::SchemaManager>>,
}

impl QueryStage for BindStage {
    type Input = (crate::parser::ParserResult, Arc<QueryContext>);
    type Output = Option<crate::binder::BoundStatement>;

    fn process(
        &self,
        input: Self::Input,
        _ctx: &QueryContext,
    ) -> Result<Self::Output, QueryPipelineError> {
        let (parser_result, qctx) = input;
        let space_id = qctx.space_id().unwrap_or(0);
        let space_name = qctx
            .space_name()
            .or_else(|| qctx.request_context().space_name.clone());

        let mut binder = crate::binder::Binder::new().with_space(space_name, space_id);
        if let Some(ref schema_manager) = self.schema_manager {
            binder = binder.with_schema_manager(schema_manager.clone());
        }

        binder
            .bind(parser_result.ast)
            .map(Some)
            .map_err(|e| QueryPipelineError::Pipeline {
                phase: crate::error::PipelinePhase::Bind,
                message: e.to_string(),
            })
    }
}

/// Plan stage: bound statement → execution plan.
pub struct PlanStage;

impl QueryStage for PlanStage {
    type Input = (
        crate::binder::BoundStatement,
        Arc<QueryContext>,
        Arc<crate::parser::ast::stmt::Ast>,
    );
    type Output = crate::planning::plan::ExecutionPlan;

    fn process(
        &self,
        input: Self::Input,
        _ctx: &QueryContext,
    ) -> Result<Self::Output, QueryPipelineError> {
        let (bound, query_context, ast) = input;

        let mut planner_enum = crate::planning::planner::PlannerEnum::from_bound_statement(&bound)
            .ok_or_else(|| QueryPipelineError::Pipeline {
                phase: crate::error::PipelinePhase::Plan,
                message: format!("No planner for bound statement: {}", bound.kind()),
            })?;

        let validated = crate::pipeline::prepared::build_validated_fallback(&ast);

        let ctx = crate::planning::context::PlanContext::without_metadata(
            &bound,
            query_context.clone(),
            &validated,
        );
        let sub_plan = planner_enum
            .plan_bound(&ctx)
            .map_err(|e| QueryPipelineError::Planning {
                source: e,
                statement_type: crate::error::StatementType::from_bound(&bound),
                space_name: query_context
                    .space_name()
                    .or_else(|| query_context.request_context().space_name.clone()),
            })?;

        let root = sub_plan.root().clone();
        let mut execution_plan = crate::planning::plan::ExecutionPlan::new(root);

        if let Some(logical_root) = sub_plan.logical_root().cloned() {
            execution_plan.set_logical_plan(crate::planning::plan::logical_plan::LogicalPlan::new(
                logical_root,
            ));
        } else if let Some(ref root_node) = execution_plan.root {
            if let Ok(logical_plan) =
                crate::planning::plan::logical_plan::LogicalPlan::from_plan_node(root_node)
            {
                execution_plan.set_logical_plan(logical_plan);
            }
        }

        Ok(execution_plan)
    }
}

/// Optimize stage: execution plan → optimized execution plan.
///
/// Currently a placeholder; optimization is handled by the pipeline
/// manager's optimizer engine.
pub struct OptimizeStage;

impl QueryStage for OptimizeStage {
    type Input = (crate::planning::plan::ExecutionPlan, Option<String>);
    type Output = crate::planning::plan::ExecutionPlan;

    fn process(
        &self,
        _input: Self::Input,
        _ctx: &QueryContext,
    ) -> Result<Self::Output, QueryPipelineError> {
        Err(QueryPipelineError::Pipeline {
            phase: crate::error::PipelinePhase::Optimize,
            message: "OptimizeStage not yet standalone; use pipeline manager".to_string(),
        })
    }
}

/// Execute stage: physical plan → execution result.
///
/// Currently a placeholder; execution is handled by the pipeline
/// manager's execution methods.
pub struct ExecuteStage;

impl QueryStage for ExecuteStage {
    type Input = (
        Arc<crate::executor::streaming::plan::PhysicalPlan>,
        Arc<QueryContext>,
    );
    type Output = crate::executor::base::ExecutionResult;

    fn process(
        &self,
        _input: Self::Input,
        _ctx: &QueryContext,
    ) -> Result<Self::Output, QueryPipelineError> {
        Err(QueryPipelineError::Pipeline {
            phase: crate::error::PipelinePhase::Execute,
            message: "ExecuteStage not yet standalone; use pipeline manager".to_string(),
        })
    }
}
