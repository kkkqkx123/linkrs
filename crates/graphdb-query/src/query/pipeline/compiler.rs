use super::QueryPipelineManager;
use crate::core::error::{DBError, DBResult, QueryError};
use crate::query::binder::BoundStatement;
use crate::query::executor::streaming::plan::{
    PhysicalPlan, PhysicalPlanBuildContext, PhysicalPlanBuilder, PhysicalPlanValidator,
};
use crate::query::parser::ast::Stmt;
use crate::query::QueryContext;
use crate::storage::QueryStorage;
use std::sync::Arc;

use crate::query::executor::streaming::parameters::{
    ParameterDesc, ParameterSchema, ParameterSlot,
};

impl<S: QueryStorage + 'static> QueryPipelineManager<S> {
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

    /// Compile a BoundStatement directly into a physical plan.
    ///
    /// Uses `plan_bound()` on the selected planner to produce a SubPlan,
    /// then proceeds through optimization and physical plan building.
    pub(crate) fn compile_from_bound(
        &mut self,
        query_context: Arc<QueryContext>,
        bound: &BoundStatement,
        ast: &Arc<crate::query::parser::ast::stmt::Ast>,
    ) -> DBResult<(
        Arc<PhysicalPlan>,
        crate::query::planning::plan::ExecutionPlan,
    )> {
        let execution_plan = self.generate_execution_plan_from_bound(query_context.clone(), bound, ast)?;
        let optimized_plan = self.optimize_execution_plan(execution_plan)?;
        let physical_plan = self.build_physical_plan(&optimized_plan, &query_context)?;
        Ok((physical_plan, optimized_plan))
    }

    pub(crate) fn generate_execution_plan_from_bound(
        &mut self,
        query_context: Arc<QueryContext>,
        bound: &BoundStatement,
        ast: &Arc<crate::query::parser::ast::stmt::Ast>,
    ) -> DBResult<crate::query::planning::plan::ExecutionPlan> {
        use crate::query::planning::planner::PlannerError;

        let mut planner_enum = crate::query::planning::planner::PlannerEnum::from_bound_statement(bound)
            .ok_or_else(|| DBError::from(QueryError::pipeline_planning_error(
                PlannerError::NoSuitablePlanner(
                    format!("No planner for bound statement: {}", bound.kind())
                )
            )))?;

        let sub_plan = match planner_enum.plan_bound(bound, query_context.clone()) {
            Ok(plan) => plan,
            Err(PlannerError::UnsupportedOperation(_)) => {
                let validated = super::prepared::build_validated_fallback(ast);
                planner_enum.transform(&validated, query_context.clone())
                    .map_err(|e| DBError::from(QueryError::pipeline_planning_error(e)))?
            }
            Err(e) => return Err(DBError::from(QueryError::pipeline_planning_error(e))),
        };

        let root = sub_plan.root().clone();
        let mut execution_plan = crate::query::planning::plan::ExecutionPlan::new(root);

        if let Some(ref root_node) = execution_plan.root {
            if let Ok(logical_plan) =
                crate::query::planning::plan::logical_plan::LogicalPlan::from_plan_node(root_node)
            {
                execution_plan.set_logical_plan(logical_plan);
            }
        }

        Ok(execution_plan)
    }

    pub(crate) fn compile_or_get_cached(
        &mut self,
        query_text: &str,
        query_context: Arc<QueryContext>,
        bound: &BoundStatement,
        stmt: &Stmt,
        ast: &Arc<crate::query::parser::ast::stmt::Ast>,
    ) -> DBResult<Arc<PhysicalPlan>> {
        let request = query_context.request_context();
        let space_name = query_context
            .space_name()
            .or_else(|| request.space_name.clone());
        let schema_version = Some(
            self.schema_generation
                .load(std::sync::atomic::Ordering::Relaxed),
        );
        let index_version = Some(
            self.index_generation
                .load(std::sync::atomic::Ordering::Relaxed),
        );
        let mut param_positions = self.param_handler.extract_params(query_text);
        for position in &mut param_positions {
            let name = position
                .name
                .clone()
                .unwrap_or_else(|| position.index.to_string());
            position.expected_type = request.parameters.get(&name).map(|value| value.data_type());
        }
        let param_signature =
            crate::query::cache::plan_cache::QueryPlanCache::compute_param_type_signature(
                &param_positions,
            );

        // Use the same planning config source as PhysicalPlanBuildContext
        // to ensure cache key dimensions match what the builder embeds in plan metadata.
        let planning_config = crate::query::executor::streaming::plan::context::PlanningConfig {
            max_partitions: self
                .optimizer_engine
                .partitioning_config()
                .max_workers
                .max(1),
            ..Default::default()
        };
        let cache_context = crate::query::cache::PlanCacheContext {
            space_name: space_name.clone(),
            schema_version,
            index_version,
            param_type_signature: param_signature,
            optimizer_version: planning_config.optimizer_version,
            planning_config_hash: planning_config.config_hash,
            capability_set: 0,
        };
        if let Some(cached) = self.plan_cache.get_with_context(query_text, cache_context) {
            crate::query::executor::streaming::plan::PhysicalPlanValidator::check_compatibility(
                &cached.plan,
                schema_version,
            )
            .map_err(DBError::from)?;
            return Ok(cached.plan.clone());
        }

        let (plan, _) = self.compile_from_bound(query_context, bound, ast)?;
        if super::prepared::is_read_only_cacheable(stmt) {
            let dependent_tables = collect_dependent_tables(bound);
            self.plan_cache.put_with_context(
                query_text,
                plan.clone(),
                param_positions,
                crate::query::cache::plan_cache::PlanCachePutContext {
                    dependent_tables,
                    space_name,
                    schema_version,
                    index_version,
                    is_dml: false,
                    is_transaction: false,
                    optimizer_version: planning_config.optimizer_version,
                    planning_config_hash: planning_config.config_hash,
                    capability_set: 0,
                },
            );
        }
        Ok(plan)
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
        if let Some(schema) = build_ctx.schema.as_mut() {
            schema.layout_version = self
                .schema_generation
                .load(std::sync::atomic::Ordering::Relaxed);
        }
        build_ctx.parameter_schema = self.parameter_schema(query_context);
        let physical_plan = PhysicalPlanBuilder::build(root_node, &mut build_ctx, &exec_ctx)
            .map_err(|e| DBError::from(QueryError::execution(e.to_string())))?;

        PhysicalPlanValidator::validate(&physical_plan)
            .map_err(|e| DBError::from(QueryError::execution(e.to_string())))?;

        Ok(Arc::new(physical_plan))
    }

    fn parameter_schema(&self, query_context: &QueryContext) -> ParameterSchema {
        let request = query_context.request_context();
        let mut seen = std::collections::HashSet::new();
        let params = self
            .param_handler
            .extract_params(&request.query)
            .into_iter()
            .filter_map(|position| {
                let name = position.name.unwrap_or_else(|| position.index.to_string());
                if !seen.insert(name.clone()) {
                    return None;
                }
                let value_type = request.parameters.get(&name).map(|value| value.data_type());
                Some(ParameterDesc {
                    name,
                    slot: ParameterSlot(seen.len() - 1),
                    value_type,
                    nullable: false,
                    default: None,
                })
            })
            .collect();
        ParameterSchema::new(params)
    }
}

/// Collect dependent table names from a BoundStatement for cache invalidation.
fn collect_dependent_tables(bound: &crate::query::binder::BoundStatement) -> Vec<String> {
    let mut tables = Vec::new();
    if let crate::query::binder::BoundStatement::Match(match_stmt) = bound {
        for node in &match_stmt.query_graph.nodes {
            for tag in &node.tags {
                tables.push(tag.tag_name.clone());
            }
        }
        for edge in &match_stmt.query_graph.edges {
            for edge_type in &edge.edge_types {
                tables.push(edge_type.edge_type_name.clone());
            }
        }
    }
    tables
}
