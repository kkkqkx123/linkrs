use super::QueryPipelineManager;
use crate::core::error::{DBError, DBResult, QueryError};
use crate::query::executor::streaming::plan::{
    PhysicalPlan, PhysicalPlanBuildContext, PhysicalPlanBuilder, PhysicalPlanValidator,
};
use crate::query::validator::ValidatedStatement;
use crate::query::QueryContext;
use crate::storage::QueryStorage;
use std::sync::Arc;

impl<S: QueryStorage + 'static> QueryPipelineManager<S> {
    pub(crate) fn generate_execution_plan(
        &mut self,
        query_context: Arc<QueryContext>,
        validated: &ValidatedStatement,
    ) -> DBResult<crate::query::planning::plan::ExecutionPlan> {
        let plan = if let Some(mut planner_enum) =
            crate::query::planning::planner::PlannerEnum::from_ast(&validated.ast)
        {
            let metadata_context = self.build_metadata_context(validated, query_context.clone())?;

            let sub_plan = if let Some(ref ctx) = metadata_context {
                planner_enum
                    .transform_with_metadata(validated, query_context, ctx)
                    .map_err(|e| DBError::from(QueryError::pipeline_planning_error(e)))?
            } else {
                planner_enum
                    .transform(validated, query_context)
                    .map_err(|e| DBError::from(QueryError::pipeline_planning_error(e)))?
            };

            let root = sub_plan.root().clone();
            crate::query::planning::plan::ExecutionPlan::new(root)
        } else {
            return Err(DBError::from(QueryError::pipeline_planning_error(
                crate::query::planning::planner::PlannerError::NoSuitablePlanner(
                    "No suitable planner found".to_string(),
                ),
            )));
        };

        Ok(plan)
    }

    pub(crate) fn optimize_execution_plan(
        &mut self,
        plan: crate::query::planning::plan::ExecutionPlan,
    ) -> DBResult<crate::query::planning::plan::ExecutionPlan> {
        let mut optimized = self
            .optimizer_engine
            .optimize(plan)
            .map_err(|e| DBError::from(QueryError::pipeline_optimization_error(e)))?;
        let cfg = self.optimizer_engine.partitioning_config();
        optimized.set_max_workers(cfg.max_workers.max(1));
        optimized.set_max_buffered_chunks(cfg.max_buffered_chunks.max(1));
        Ok(optimized)
    }

    pub(crate) fn compile(
        &mut self,
        query_context: Arc<QueryContext>,
        validated: &ValidatedStatement,
    ) -> DBResult<(
        Arc<PhysicalPlan>,
        crate::query::planning::plan::ExecutionPlan,
    )> {
        let execution_plan = self.generate_execution_plan(query_context.clone(), validated)?;
        let optimized_plan = self.optimize_execution_plan(execution_plan)?;
        let physical_plan = self.build_physical_plan(&optimized_plan, &query_context)?;
        Ok((physical_plan, optimized_plan))
    }

    pub(crate) fn build_physical_plan(
        &self,
        plan: &crate::query::planning::plan::ExecutionPlan,
        query_context: &QueryContext,
    ) -> DBResult<Arc<PhysicalPlan>> {
        let root_node = plan.root.as_ref().ok_or_else(|| {
            DBError::from(QueryError::execution("Empty execution plan".to_string()))
        })?;

        let exec_ctx = self.build_execution_context(query_context);
        let mut build_ctx = PhysicalPlanBuildContext::from_execution_context(&exec_ctx);
        let physical_plan = PhysicalPlanBuilder::build(root_node, &mut build_ctx, &exec_ctx)
            .map_err(|e| DBError::from(QueryError::execution(e.to_string())))?;

        PhysicalPlanValidator::validate(&physical_plan)
            .map_err(|e| DBError::from(QueryError::execution(e.to_string())))?;

        Ok(Arc::new(physical_plan))
    }
}
