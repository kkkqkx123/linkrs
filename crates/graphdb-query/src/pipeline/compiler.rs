use super::QueryPipelineManager;
use crate::binder::BoundStatement;
use crate::executor::streaming::plan::{
    PhysicalPlan, PhysicalPlanBuildContext, PhysicalPlanBuilder, PhysicalPlanValidator,
};
use crate::optimizer::PartitioningConfig;
use crate::parser::ast::Stmt;
use crate::storage::QueryStorage;
use crate::QueryContext;
use graphdb_core::error::{DBError, DBResult, QueryError};
use std::sync::Arc;

use crate::executor::streaming::parameters::{ParameterDesc, ParameterSchema, ParameterSlot};

impl<S: QueryStorage + 'static> QueryPipelineManager<S> {
    pub(crate) fn optimize_execution_plan(
        &mut self,
        plan: crate::planning::plan::ExecutionPlan,
        space_name: Option<&str>,
    ) -> DBResult<crate::planning::plan::ExecutionPlan> {
        // Read the storage-provided layout information (monotonic layout
        // version + self-proven vertex-id domain) so partitioning can be
        // enabled safely when evidence exists. Without storage access the
        // default (config-only) layout is used.
        let layout_info = match &self.storage {
            Some(storage) => {
                let guard = storage.read();
                crate::optimizer::partitioning::PartitioningLayoutInfo {
                    layout_version: guard.layout_version(),
                    vertex_id_range: space_name.and_then(|space| guard.vertex_id_domain(space)),
                }
            }
            None => crate::optimizer::partitioning::PartitioningLayoutInfo::default(),
        };
        let mut optimized = self
            .optimizer_engine
            .optimize_with_layout(plan, space_name, &layout_info)
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
        ast: &Arc<crate::parser::ast::stmt::Ast>,
    ) -> DBResult<Arc<PhysicalPlan>> {
        let optimized_plan = self.optimize_from_bound(query_context.clone(), bound, ast)?;
        let physical_plan = self.build_physical_plan(&optimized_plan, &query_context)?;
        Ok(physical_plan)
    }

    /// Bind + plan + optimize only, returning the optimized logical plan
    /// without materializing the physical plan.
    ///
    /// Split out of [`compile_from_bound`] so callers can inspect the
    /// partition layout and serve a cached physical plan before building it.
    pub(crate) fn optimize_from_bound(
        &mut self,
        query_context: Arc<QueryContext>,
        bound: &BoundStatement,
        ast: &Arc<crate::parser::ast::stmt::Ast>,
    ) -> DBResult<crate::planning::plan::ExecutionPlan> {
        let execution_plan =
            self.generate_execution_plan_from_bound(query_context.clone(), bound, ast)?;
        let space_name = query_context
            .space_name()
            .or_else(|| query_context.request_context().space_name.clone());
        self.optimize_execution_plan(execution_plan, space_name.as_deref())
    }

    pub(crate) fn generate_execution_plan_from_bound(
        &mut self,
        query_context: Arc<QueryContext>,
        bound: &BoundStatement,
        ast: &Arc<crate::parser::ast::stmt::Ast>,
    ) -> DBResult<crate::planning::plan::ExecutionPlan> {
        use crate::planning::planner::PlannerError;

        let mut planner_enum = crate::planning::planner::PlannerEnum::from_bound_statement(bound)
            .ok_or_else(|| {
            DBError::from(QueryError::pipeline_planning_error(
                PlannerError::NoSuitablePlanner(format!(
                    "No planner for bound statement: {}",
                    bound.kind()
                )),
            ))
        })?;

        // Build a lightweight ValidatedStatement for expression context (used
        // by clause planners like MATCH that still need the AST expression
        // analysis context for YIELD column construction).
        let validated = super::prepared::build_validated_fallback(ast);
        let metadata = self.build_metadata_context(&query_context, bound);

        let ctx = crate::planning::context::PlanContext::new(
            bound,
            query_context.clone(),
            metadata.as_ref(),
            &validated,
        );
        let sub_plan = planner_enum
            .plan_bound(&ctx)
            .map_err(|e| DBError::from(QueryError::pipeline_planning_error(e)))?;

        let root = sub_plan.root().clone();
        let mut execution_plan = crate::planning::plan::ExecutionPlan::new(root);

        // Migrated planners produce the native logical tree directly; the
        // reverse stripping (`LogicalPlan::from_plan_node`) remains only as
        // a fallback for planners that still emit physical trees.
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

    pub(crate) fn compile_or_get_cached(
        &mut self,
        query_text: &str,
        query_context: Arc<QueryContext>,
        bound: Option<&BoundStatement>,
        stmt: &Stmt,
        ast: &Arc<crate::parser::ast::stmt::Ast>,
        dml_shape_cacheable: bool,
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
        let dml_param_sig = if dml_shape_cacheable {
            Some(Self::dml_param_signature(&request.parameters))
        } else {
            None
        };

        if let Some(sig) = dml_param_sig {
            if let Some(entry) = &*self.last_dml_plan.lock() {
                if entry.normalized_text == query_text
                    && entry.space_name == space_name
                    && entry.schema_version == schema_version
                    && entry.param_sig == sig
                {
                    self.last_dml_plan_hits
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    self.plan_cache.record_memo_hit();
                    return Ok(entry.plan.clone());
                }
            }
        }

        let mut param_positions = self.param_handler.extract_params(query_text);
        for position in &mut param_positions {
            let name = position
                .name
                .clone()
                .unwrap_or_else(|| position.index.to_string());
            position.expected_type = request.parameters.get(&name).map(|value| value.data_type());
        }
        let param_signature =
            crate::cache::plan_cache::QueryPlanCache::compute_param_type_signature(
                &param_positions,
            );

        let planning_config = crate::executor::streaming::plan::context::PlanningConfig {
            max_partitions: self
                .optimizer_engine
                .partitioning_config()
                .max_workers
                .max(1),
            config_hash: Self::partitioning_config_hash(
                self.optimizer_engine.partitioning_config(),
            ),
            ..Default::default()
        };
        let cache_context = crate::cache::PlanCacheContext {
            space_name: space_name.clone(),
            schema_version,
            index_version,
            param_type_signature: param_signature,
            optimizer_version: planning_config.optimizer_version,
            planning_config_hash: planning_config.config_hash,
        };
        if let Some(cached) = self
            .plan_cache
            .get_with_context(query_text, cache_context.clone())
        {
            crate::executor::streaming::plan::PhysicalPlanValidator::check_compatibility(
                &cached.plan,
                schema_version,
            )
            .map_err(DBError::from)?;
            if let Some(sig) = dml_param_sig {
                *self.last_dml_plan.lock() = Some(super::LastDmlPlan {
                    normalized_text: query_text.to_string(),
                    space_name: space_name.clone(),
                    schema_version,
                    param_sig: sig,
                    plan: cached.plan.clone(),
                });
            }
            return Ok(cached.plan.clone());
        }

        let owned_bound;
        let bound: &BoundStatement = match bound {
            Some(b) => b,
            None => {
                owned_bound = self
                    .bind_parsed_statement(ast.clone(), query_context.clone())?
                    .ok_or_else(|| {
                        DBError::from(QueryError::execution("No bound statement".to_string()))
                    })?;
                &owned_bound
            }
        };
        let optimized_plan = self.optimize_from_bound(query_context.clone(), bound, ast)?;
        if let Some(spec) = optimized_plan.partition_spec() {
            if let Some(cached) =
                self.plan_cache
                    .get_with_partition_context(query_text, spec, cache_context.clone())
            {
                crate::executor::streaming::plan::PhysicalPlanValidator::check_compatibility(
                    &cached.plan,
                    schema_version,
                )
                .map_err(DBError::from)?;
                if let Some(sig) = dml_param_sig {
                    *self.last_dml_plan.lock() = Some(super::LastDmlPlan {
                        normalized_text: query_text.to_string(),
                        space_name: space_name.clone(),
                        schema_version,
                        param_sig: sig,
                        plan: cached.plan.clone(),
                    });
                }
                return Ok(cached.plan.clone());
            }
        }

        let plan = self.build_physical_plan(&optimized_plan, &query_context)?;
        let cacheable = super::prepared::is_read_only_cacheable(stmt) || dml_shape_cacheable;
        if cacheable {
            if let Some(sig) = dml_param_sig {
                *self.last_dml_plan.lock() = Some(super::LastDmlPlan {
                    normalized_text: query_text.to_string(),
                    space_name: space_name.clone(),
                    schema_version,
                    param_sig: sig,
                    plan: plan.clone(),
                });
            }
            if let Some(spec) = plan.partition_spec() {
                let dependent_tables = collect_dependent_tables(bound);
                self.plan_cache.put_with_partition(
                    query_text,
                    spec,
                    plan.clone(),
                    param_positions,
                    crate::cache::plan_cache::PlanCachePutContext {
                        dependent_tables,
                        space_name,
                        schema_version,
                        index_version,
                        is_dml: dml_shape_cacheable,
                        is_transaction: false,
                        optimizer_version: planning_config.optimizer_version,
                        planning_config_hash: planning_config.config_hash,
                    },
                );
            } else {
                let dependent_tables = collect_dependent_tables(bound);
                self.plan_cache.put_with_context(
                    query_text,
                    plan.clone(),
                    param_positions,
                    crate::cache::plan_cache::PlanCachePutContext {
                        dependent_tables,
                        space_name,
                        schema_version,
                        index_version,
                        is_dml: dml_shape_cacheable,
                        is_transaction: false,
                        optimizer_version: planning_config.optimizer_version,
                        planning_config_hash: planning_config.config_hash,
                    },
                );
            }
        }
        Ok(plan)
    }

    /// Deterministic hash of the live partitioning configuration used to
    /// scope plan-cache entries.  Toggling/enabling partitioning, changing
    /// worker count, thresholds, or the trusted vertex-id range produces a
    /// different hash so cached single-tree plans cannot be reused under a
    /// different layout policy.
    fn partitioning_config_hash(config: &PartitioningConfig) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        config.enabled.hash(&mut hasher);
        config.min_rows_per_partition.hash(&mut hasher);
        config.max_partitions.hash(&mut hasher);
        config.max_workers.hash(&mut hasher);
        match &config.vertex_id_range {
            Some(range) => {
                range.start.hash(&mut hasher);
                range.end.hash(&mut hasher);
            }
            None => {
                hasher.write_u8(0);
            }
        }
        hasher.finish()
    }

    pub(crate) fn dml_param_signature(
        params: &std::collections::HashMap<String, graphdb_core::Value>,
    ) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut names: Vec<&String> = params
            .keys()
            .filter(|name| name.starts_with(crate::planning::dml_shape::DML_PARAM_PREFIX))
            .collect();
        names.sort_unstable();
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        for name in names {
            name.hash(&mut hasher);
            params
                .get(name)
                .expect("name comes from params keys")
                .data_type()
                .hash(&mut hasher);
        }
        hasher.finish()
    }

    /// Build a `MetadataContext` for the current space scoped to the schema
    /// objects referenced by the bound statement.
    ///
    /// Only the referenced tags, edge types, and their indexes are loaded, so
    /// planning does not pay the cost of the full space schema on every query.
    fn build_metadata_context(
        &self,
        query_context: &QueryContext,
        bound: &BoundStatement,
    ) -> Option<crate::metadata::MetadataContext> {
        use crate::metadata::{
            EdgeTypeMetadata, IndexMetadata, IndexType, MetadataContext, PropertyDefinition,
            PropertyType, TagMetadata,
        };

        let space_name = query_context
            .space_name()
            .or_else(|| query_context.request_context().space_name.clone())?;
        let space_id = query_context.space_id().unwrap_or(0);

        let (referenced_tags, referenced_edges, full_load) = referenced_schema_objects(bound);

        let mut metadata = MetadataContext::new();

        if let Some(schema_manager) = &self.schema_manager {
            if full_load {
                if let Ok(tags) = schema_manager.list_tags(&space_name) {
                    for tag in tags {
                        let mut tag_metadata = TagMetadata::new(tag.tag_name.clone(), space_id)
                            .with_tag_id(tag.tag_id);
                        for prop in &tag.properties {
                            tag_metadata.properties.push(PropertyDefinition::new(
                                prop.name.clone(),
                                PropertyType::from(prop.data_type.clone()),
                            ));
                        }
                        metadata.set_tag_metadata(tag.tag_name.clone(), tag_metadata);
                    }
                }
                if let Ok(edge_types) = schema_manager.list_edge_types(&space_name) {
                    for edge_type in edge_types {
                        let mut edge_metadata =
                            EdgeTypeMetadata::new(edge_type.edge_type_name.clone(), space_id);
                        for prop in &edge_type.properties {
                            edge_metadata.properties.push(PropertyDefinition::new(
                                prop.name.clone(),
                                PropertyType::from(prop.data_type.clone()),
                            ));
                        }
                        metadata.set_edge_type_metadata(
                            edge_type.edge_type_name.clone(),
                            edge_metadata,
                        );
                    }
                }
            } else {
                for tag_name in &referenced_tags {
                    if let Ok(Some(tag)) = schema_manager.get_tag(&space_name, tag_name) {
                        let mut tag_metadata = TagMetadata::new(tag.tag_name.clone(), space_id)
                            .with_tag_id(tag.tag_id);
                        for prop in &tag.properties {
                            tag_metadata.properties.push(PropertyDefinition::new(
                                prop.name.clone(),
                                PropertyType::from(prop.data_type.clone()),
                            ));
                        }
                        metadata.set_tag_metadata(tag.tag_name.clone(), tag_metadata);
                    }
                }
                for edge_type_name in &referenced_edges {
                    if let Ok(Some(edge_type)) =
                        schema_manager.get_edge_type(&space_name, edge_type_name)
                    {
                        let mut edge_metadata =
                            EdgeTypeMetadata::new(edge_type.edge_type_name.clone(), space_id);
                        for prop in &edge_type.properties {
                            edge_metadata.properties.push(PropertyDefinition::new(
                                prop.name.clone(),
                                PropertyType::from(prop.data_type.clone()),
                            ));
                        }
                        metadata.set_edge_type_metadata(
                            edge_type.edge_type_name.clone(),
                            edge_metadata,
                        );
                    }
                }
            }
        }

        let index_manager = self.index_manager.clone().or_else(|| {
            self.storage
                .as_ref()
                .and_then(|storage| storage.read().get_index_metadata_manager())
        });
        if let Some(index_manager) = index_manager {
            if let Ok(indexes) = index_manager.list_tag_indexes(space_id) {
                for index in indexes {
                    if !full_load
                        && !referenced_tags
                            .iter()
                            .any(|t| t.as_str() == index.schema_name.as_str())
                    {
                        continue;
                    }
                    let field_name = index
                        .fields
                        .first()
                        .map(|f| f.name.clone())
                        .unwrap_or_default();
                    let mut index_metadata = IndexMetadata::new(
                        index.name.clone(),
                        space_id,
                        index.schema_name.clone(),
                        field_name,
                        IndexType::Property,
                    );
                    index_metadata.index_id = index.id;
                    metadata.set_index_metadata(index.name.clone(), index_metadata);
                }
            }
            if let Ok(indexes) = index_manager.list_edge_indexes(space_id) {
                for index in indexes {
                    if !full_load
                        && !referenced_edges
                            .iter()
                            .any(|e| e.as_str() == index.schema_name.as_str())
                    {
                        continue;
                    }
                    let field_name = index
                        .fields
                        .first()
                        .map(|f| f.name.clone())
                        .unwrap_or_default();
                    let mut index_metadata = IndexMetadata::new(
                        index.name.clone(),
                        space_id,
                        index.schema_name.clone(),
                        field_name,
                        IndexType::Property,
                    );
                    index_metadata.index_id = index.id;
                    index_metadata.is_edge = true;
                    metadata.set_index_metadata(index.name.clone(), index_metadata);
                }
            }
        }

        // Vector indexes live in the sync coordinator's logical-index registry,
        // not in the storage property-index manager. Statement-level names are
        // recorded there at CREATE time so they can be resolved back to their
        // (space_id, tag, field) location during planning.
        #[cfg(feature = "vector")]
        if let Some(coordinator) = &self.vector_coordinator {
            for wrapper in coordinator.list_indexes() {
                if let Some(name) = wrapper.index_name {
                    if wrapper.space_id != space_id || wrapper.tag_name.is_empty() {
                        continue;
                    }
                    let index_metadata = IndexMetadata::new(
                        name.clone(),
                        space_id,
                        wrapper.tag_name,
                        wrapper.field_name,
                        IndexType::Vector,
                    );
                    metadata.set_index_metadata(name, index_metadata);
                }
            }
        }

        // Fulltext indexes live in the FulltextIndexManager's in-memory registry.
        // Statement-level names are recorded there at CREATE time so they can be
        // resolved back to their (space_id, tag, field) location during planning.
        #[cfg(feature = "fulltext")]
        if let Some(manager) = &self.fulltext_manager {
            for ft_meta in manager.list_indexes() {
                if ft_meta.space_id != space_id || ft_meta.tag_name.is_empty() {
                    continue;
                }
                let index_metadata = IndexMetadata::new(
                    ft_meta.index_name.clone(),
                    space_id,
                    ft_meta.tag_name,
                    ft_meta.field_name,
                    IndexType::Fulltext,
                );
                metadata.set_index_metadata(ft_meta.index_name, index_metadata);
            }
        }

        let tag_names: Vec<String> = metadata
            .get_all_tags()
            .map(|t| t.tag_name.clone())
            .collect();
        for tag_name in tag_names {
            let indexes: Vec<String> = metadata
                .get_all_indexes()
                .filter(|i| !i.is_edge && i.tag_name == tag_name)
                .map(|i| i.index_name.clone())
                .collect();
            if let Some(tag_metadata) = metadata.get_tag_metadata_mut(&tag_name) {
                tag_metadata.indexes = indexes;
            }
        }

        let edge_type_names: Vec<String> = metadata
            .get_all_edge_types()
            .map(|t| t.edge_type.clone())
            .collect();
        for edge_type_name in edge_type_names {
            let indexes: Vec<String> = metadata
                .get_all_indexes()
                .filter(|i| i.is_edge && i.tag_name == edge_type_name)
                .map(|i| i.index_name.clone())
                .collect();
            if let Some(edge_metadata) = metadata.get_edge_type_metadata_mut(&edge_type_name) {
                edge_metadata.indexes = indexes;
            }
        }

        // Register the index catalog for cost-based index selection. The
        // optimizer engine has no schema access, so the per-query metadata
        // context (which only loads referenced objects) feeds the catalog.
        let stats_manager = self.optimizer_engine.stats_manager();
        for tag_metadata in metadata.get_all_tags() {
            let mut indexes = Vec::new();
            for index_name in &tag_metadata.indexes {
                if let Some(index_meta) = metadata.get_index_metadata(index_name) {
                    indexes.push(graphdb_core::types::Index {
                        id: index_meta.index_id,
                        name: index_meta.index_name.clone(),
                        space_id,
                        schema_name: index_meta.tag_name.clone(),
                        fields: Vec::new(),
                        properties: vec![index_meta.field_name.clone()],
                        index_type: graphdb_core::types::IndexType::TagIndex,
                        status: graphdb_core::types::IndexStatus::Active,
                        is_unique: false,
                        comment: None,
                        covering: false,
                        partial_condition: None,
                    });
                }
            }
            stats_manager.register_tag_indexes(
                &space_name,
                &tag_metadata.tag_name,
                tag_metadata.tag_id as i32,
                indexes,
            );
        }

        Some(metadata)
    }

    pub(crate) fn build_physical_plan(
        &self,
        plan: &crate::planning::plan::ExecutionPlan,
        query_context: &QueryContext,
    ) -> DBResult<Arc<PhysicalPlan>> {
        let root_node = plan.root.as_ref().ok_or_else(|| {
            DBError::from(QueryError::execution("Empty execution plan".to_string()))
        })?;

        let mut exec_ctx = self.build_execution_context(query_context);
        exec_ctx.join_algorithms = plan.join_algorithms.clone();
        let mut build_ctx = PhysicalPlanBuildContext::from_execution_context(&exec_ctx);
        if let Some(schema) = build_ctx.schema.as_mut() {
            schema.layout_version = self
                .schema_generation
                .load(std::sync::atomic::Ordering::Relaxed);
        }
        build_ctx.parameter_schema = self.parameter_schema(query_context);
        build_ctx.partition_spec = plan.partition_spec().cloned();
        build_ctx.parallel_fallback_reason = plan.parallel_fallback_reason.clone();
        build_ctx.cbo_notes = plan.cbo_notes.clone();
        build_ctx.statistics.per_node_row_estimates = plan.row_estimates.clone();
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
fn collect_dependent_tables(bound: &crate::binder::BoundStatement) -> Vec<String> {
    let mut tables = Vec::new();
    if let crate::binder::BoundStatement::Match(match_stmt) = bound {
        for node in &match_stmt.query_graph.nodes {
            for tag in &node.tags {
                tables.push(tag.tag_name.to_string());
            }
        }
        for edge in &match_stmt.query_graph.edges {
            for edge_type in &edge.edge_types {
                tables.push(edge_type.edge_type_name.to_string());
            }
        }
    }
    tables
}

/// Split the schema objects referenced by a bound statement into referenced
/// tag names and edge-type names, used to scope lazy metadata loading.
///
/// Only [`BoundStatement::Match`] and [`BoundStatement::Lookup`] consume the
/// metadata context during planning, so their references must be exact.  For
/// composite statements (`Pipe` / `SetOperation`) the references are merged
/// from the child statements.  Statements bound as [`BoundStatement::Other`]
/// (DDL / DML / management / fulltext / vector / EXPLAIN) cannot be
/// introspected and must fall back to full-space metadata loading.
fn referenced_schema_objects(
    bound: &crate::binder::BoundStatement,
) -> (Vec<String>, Vec<String>, bool) {
    use crate::binder::bound::{BoundLookupTarget, BoundStatement};

    fn push_unique(target: &mut Vec<String>, name: &str) {
        if !target.iter().any(|t| t.as_str() == name) {
            target.push(name.to_string());
        }
    }

    fn merge(
        tags: &mut Vec<String>,
        edges: &mut Vec<String>,
        other: &(Vec<String>, Vec<String>, bool),
    ) -> bool {
        for name in &other.0 {
            push_unique(tags, name);
        }
        for name in &other.1 {
            push_unique(edges, name);
        }
        other.2
    }

    let mut tags: Vec<String> = Vec::new();
    let mut edges: Vec<String> = Vec::new();

    match bound {
        BoundStatement::Match(match_stmt) => {
            for node in &match_stmt.query_graph.nodes {
                for tag in &node.tags {
                    push_unique(&mut tags, tag.tag_name.as_ref());
                }
            }
            for edge in &match_stmt.query_graph.edges {
                for edge_type in &edge.edge_types {
                    push_unique(&mut edges, edge_type.edge_type_name.as_ref());
                }
            }
            (tags, edges, false)
        }
        BoundStatement::Lookup(lookup) => match &lookup.target {
            BoundLookupTarget::Tag(name) => {
                push_unique(&mut tags, name);
                (tags, edges, false)
            }
            BoundLookupTarget::Edge(name) => {
                push_unique(&mut edges, name);
                (tags, edges, false)
            }
        },
        BoundStatement::Go(go) => {
            if let Some(over) = &go.over {
                for name in over {
                    push_unique(&mut edges, name);
                }
            }
            (tags, edges, false)
        }
        BoundStatement::FetchVertices(fetch) => {
            if let Some(name) = &fetch.tag_name {
                push_unique(&mut tags, name);
            }
            (tags, edges, false)
        }
        BoundStatement::FetchEdges(fetch) => {
            push_unique(&mut edges, &fetch.edge_type);
            (tags, edges, false)
        }
        BoundStatement::FindPath(path) => {
            if let Some((over, _)) = &path.over {
                for name in over {
                    push_unique(&mut edges, name);
                }
            }
            (tags, edges, false)
        }
        BoundStatement::Subgraph(subgraph) => {
            if let Some((over, _)) = &subgraph.over {
                for name in over {
                    push_unique(&mut edges, name);
                }
            }
            (tags, edges, false)
        }
        BoundStatement::Pipe(pipe) => {
            let mut full = false;
            for stmt in &pipe.statements {
                full |= merge(&mut tags, &mut edges, &referenced_schema_objects(stmt));
            }
            (tags, edges, full)
        }
        BoundStatement::SetOperation(set_op) => {
            let full = merge(
                &mut tags,
                &mut edges,
                &referenced_schema_objects(&set_op.left),
            ) | merge(
                &mut tags,
                &mut edges,
                &referenced_schema_objects(&set_op.right),
            );
            (tags, edges, full)
        }
        BoundStatement::Return(_)
        | BoundStatement::With(_)
        | BoundStatement::Unwind(_)
        | BoundStatement::GroupBy(_)
        | BoundStatement::Filter(_)
        | BoundStatement::Yield(_)
        | BoundStatement::Collect(_)
        | BoundStatement::AssignVariable(_) => (tags, edges, false),
        BoundStatement::Insert(_)
        | BoundStatement::Update(_)
        | BoundStatement::Delete(_)
        | BoundStatement::Merge(_)
        | BoundStatement::Set(_)
        | BoundStatement::Remove(_)
        | BoundStatement::Copy(_)
        | BoundStatement::Create(_)
        | BoundStatement::Drop(_)
        | BoundStatement::Alter(_)
        | BoundStatement::Show(_)
        | BoundStatement::ShowCreate(_)
        | BoundStatement::Desc(_)
        | BoundStatement::ClearSpace(_)
        | BoundStatement::CreateUser(_)
        | BoundStatement::DropUser(_)
        | BoundStatement::AlterUser(_)
        | BoundStatement::CreateFulltextIndex(_)
        | BoundStatement::CreateVectorIndex(_)
        | BoundStatement::Explain(_)
        | BoundStatement::Profile(_)
        | BoundStatement::Use(_)
        | BoundStatement::BeginTransaction(_)
        | BoundStatement::Commit(_)
        | BoundStatement::Rollback(_) => (tags, edges, true),
        BoundStatement::Other(_) => (tags, edges, true),
    }
}
