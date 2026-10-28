use super::QueryPipelineManager;
use crate::binder::BoundStatement;
use crate::executor::streaming::plan::{
    PhysicalPlan, PhysicalPlanBuildContext, PhysicalPlanBuilder, PhysicalPlanValidator,
};
use crate::optimizer::PartitioningConfig;
use crate::parser::ast::Stmt;
use crate::storage::QueryStorage;
use crate::QueryContext;
use linkrs_core::error::{DBError, DBResult, QueryError};
use std::sync::Arc;

use crate::executor::streaming::parameters::{ParameterDesc, ParameterSchema, ParameterSlot};

impl<S: QueryStorage + 'static> QueryPipelineManager<S> {
    pub(crate) fn optimize_execution_plan(
        &mut self,
        plan: crate::planning::plan::ExecutionPlan,
        space_name: Option<&str>,
    ) -> DBResult<crate::planning::plan::ExecutionPlan> {
        // Statistics freshness gate: missing or stale stamps trigger one
        // best-effort collection, never blocking the query on failure.
        if let Some(space) = space_name {
            self.ensure_statistics(space);
        }
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
        // Record the effective cost profile so EXPLAIN shows which pricing
        // produced the plan. Unassembled pipelines carry no label and stay
        // silent.
        if let Some(label) = self.cost_profile_label_for(space_name) {
            optimized.cbo_notes.push(format!(
                "cost_profile: {} (space '{}')",
                label,
                space_name.unwrap_or_default()
            ));
        }
        let cfg = self.optimizer_engine.partitioning_config();
        optimized.set_max_workers(cfg.max_workers.max(1));
        optimized.set_max_buffered_chunks(cfg.max_buffered_chunks.max(1));
        Ok(optimized)
    }

    /// Bind + plan + optimize only, returning the optimized logical plan
    /// without materializing the physical plan.
    ///
    /// Callers can inspect the partition layout and serve a cached physical
    /// plan before building it.
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
        let optimized = self.optimize_execution_plan(execution_plan, space_name.as_deref())?;
        Ok(optimized)
    }

    pub(crate) fn generate_execution_plan_from_bound(
        &mut self,
        query_context: Arc<QueryContext>,
        bound: &BoundStatement,
        ast: &Arc<crate::parser::ast::stmt::Ast>,
    ) -> DBResult<crate::planning::plan::ExecutionPlan> {
        use crate::planning::planner::PlannerError;

        let sub_plan = {
            let mut planner_enum = crate::planning::planner::PlannerEnum::from_bound_statement(
                bound,
            )
            .ok_or_else(|| {
                DBError::from(QueryError::pipeline_planning_error(
                    PlannerError::NoSuitablePlanner(format!(
                        "No planner for bound statement: {}",
                        bound.kind()
                    )),
                ))
            })?;

            let metadata = self.build_metadata_context(&query_context, bound);

            let space_for_stats = query_context
                .space_name()
                .or_else(|| query_context.request_context().space_name.clone());
            let join_stats = space_for_stats.as_deref().map(|space| {
                crate::planning::join_order::JoinOrderStats::from_manager(
                    self.optimizer_engine.stats_manager(),
                    space,
                )
            });
            let mut ctx = crate::planning::context::PlanContext::new(
                bound,
                query_context.clone(),
                metadata.as_ref(),
            )
            .with_ast(std::sync::Arc::clone(ast));
            if let Some(stats) = join_stats {
                ctx = ctx.with_join_order_stats(stats);
            }
            planner_enum
                .plan_bound(&ctx)
                .map_err(|e| DBError::from(QueryError::pipeline_planning_error(e)))?
        };

        let root = sub_plan.root().clone();
        let mut execution_plan = crate::planning::plan::ExecutionPlan::new(root);

        // Attach the natively produced logical tree when the planner emitted
        // one. Physical-only plans are left as-is here; the optimizer owns
        // the single reverse-conversion bridge and records its decision.
        // The warning below keeps the silent fallback visible: physical-only
        // output is supported, but new planners must emit both trees.
        if let Some(logical_root) = sub_plan.logical_root().cloned() {
            let mut logical = crate::planning::plan::logical_plan::LogicalPlan::new(logical_root);
            // A user join hint pins the join shape: flag the logical plan so
            // the cost-based reviewer skips it instead of overturning the
            // requested order. A statistics-backed planning enumeration
            // skips re-enumeration the same way; default-statistics products
            // stay unflagged and always go through review.
            if let crate::binder::BoundStatement::Match(match_stmt) = bound {
                logical.join_order_hinted = match_stmt.join_hint.is_some();
            }
            logical.stats_backed = sub_plan.join_order_stats_backed;
            execution_plan.set_logical_plan(logical);
        } else {
            log::warn!(
                "Planner emitted physical-only plan without logical tree: {}",
                bound.kind()
            );
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
        // Shared extract + type-fill path; the positions also feed the plan
        // cache put below, and the signature feeds the memo key.
        let param_positions = self.typed_param_positions(query_text, &request.parameters);
        let param_signature =
            crate::cache::plan_cache::QueryPlanCache::compute_param_type_signature(
                &param_positions,
            );

        // Memo key: fingerprint-less plan key over the normalized text,
        // built from the shared plan-cache identity context. The partition
        // fingerprint is not computable before binding; memo hits use the
        // single compatibility exit before being served.
        let cache_context = self.plan_cache_context(space_name.clone(), param_signature);
        let dml_memo_key = if dml_shape_cacheable {
            Some(super::DmlPlanMemoKey::from_context(
                query_text.to_string(),
                cache_context.clone(),
            ))
        } else {
            None
        };

        if let Some(plan) = self.serve_dml_memo(dml_memo_key.as_ref(), schema_version) {
            return Ok(plan);
        }

        if let Some(plan) = self.serve_plain_cache(
            query_text,
            &cache_context,
            dml_memo_key.clone(),
            schema_version,
        ) {
            return Ok(plan);
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
                if self
                    .plan_cache
                    .serve_compatible(&cached.plan, schema_version)
                    .is_ok()
                {
                    if let Some(key) = dml_memo_key.clone() {
                        self.store_dml_plan(key, cached.plan.clone());
                    }
                    return Ok(cached.plan.clone());
                }
                log::debug!(
                    "Partitioned plan failed compatibility check, falling through to rebuild"
                );
            }
        }

        let plan = self.build_physical_plan(&optimized_plan, &query_context)?;
        let cacheable = super::prepared::is_read_only_cacheable(stmt) || dml_shape_cacheable;
        if cacheable {
            if let Some(key) = dml_memo_key.clone() {
                self.store_dml_plan(key, plan.clone());
            }
            if let Some(spec) = plan.partition_spec() {
                let dependent_tables = collect_dependent_tables(bound);
                self.plan_cache.put_with_partition(
                    query_text,
                    spec,
                    plan.clone(),
                    param_positions,
                    crate::cache::plan_cache::PlanCachePutContext::from_identity(
                        cache_context,
                        dependent_tables,
                        dml_shape_cacheable,
                        false,
                    ),
                );
            } else {
                let dependent_tables = collect_dependent_tables(bound);
                self.plan_cache.put_with_context(
                    query_text,
                    plan.clone(),
                    param_positions,
                    crate::cache::plan_cache::PlanCachePutContext::from_identity(
                        cache_context,
                        dependent_tables,
                        dml_shape_cacheable,
                        false,
                    ),
                );
            }
        }
        Ok(plan)
    }

    /// Serve a DML memo hit through the single compatibility exit.
    ///
    /// Single resolver for the memo probe shared by the prepare-time
    /// bind-skip check and the compile-time serve path. Returns the plan
    /// only after `serve_compatible` validates the layout version; compat
    /// failures fall through to replan and bump the failure counter.
    pub(crate) fn serve_dml_memo(
        &self,
        key: Option<&super::DmlPlanMemoKey>,
        schema_version: Option<u64>,
    ) -> Option<Arc<PhysicalPlan>> {
        let key = key?;
        let plan = self.find_dml_plan(key)?;
        if self
            .plan_cache
            .serve_compatible(&plan, schema_version)
            .is_ok()
        {
            self.last_dml_plan_hits
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            return Some(plan);
        }
        self.dml_memo_compat_failures
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        log::debug!("DML memo plan failed compatibility check, falling through to replan");
        None
    }

    /// Serve a plain (non-partitioned) plan-cache hit.
    ///
    /// Memo backfill keeps the bind-skip fast path warm when the plan cache
    /// serves a shape-normalized entry first. Partitioned entries are
    /// resolved separately after optimization when the partition spec exists.
    pub(crate) fn serve_plain_cache(
        &self,
        query_text: &str,
        cache_context: &crate::cache::PlanCacheContext,
        dml_memo_key: Option<super::DmlPlanMemoKey>,
        schema_version: Option<u64>,
    ) -> Option<Arc<PhysicalPlan>> {
        let cached = self
            .plan_cache
            .get_with_context(query_text, cache_context.clone())?;
        if self
            .plan_cache
            .serve_compatible(&cached.plan, schema_version)
            .is_ok()
        {
            if let Some(key) = dml_memo_key {
                self.store_dml_plan(key, cached.plan.clone());
            }
            return Some(cached.plan.clone());
        }
        None
    }

    /// Deterministic hash of the live partitioning configuration used to
    /// scope plan-cache entries.  Toggling/enabling partitioning, changing
    /// worker count, thresholds, or the trusted vertex-id range produces a
    /// different hash so cached single-tree plans cannot be reused under a
    /// different layout policy.
    pub(crate) fn partitioning_config_hash(config: &PartitioningConfig) -> u64 {
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
                    indexes.push(linkrs_core::types::Index {
                        id: index_meta.index_id,
                        name: index_meta.index_name.clone(),
                        space_id,
                        schema_name: index_meta.tag_name.clone(),
                        fields: Vec::new(),
                        properties: vec![index_meta.field_name.clone()],
                        index_type: linkrs_core::types::IndexType::TagIndex,
                        status: linkrs_core::types::IndexStatus::Active,
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
        build_ctx.statistics_summary = self.statistics_settings_summary();
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
    let (tags, edges, _) = referenced_schema_objects(bound);
    let mut tables = tags;
    tables.extend(edges);
    tables
}

/// Split the schema objects referenced by a bound statement into referenced
/// tag names and edge-type names, used to scope lazy metadata loading.
///
/// DQL references must be exact because planners consume the metadata
/// context. DML references are extracted from the bound targets so
/// single-tag writes avoid full-space loading; DML planners ignore the
/// metadata context today, so precise scoping only reduces load. For
/// composite statements (`Pipe` / `SetOperation`) the references are merged
/// from the child statements. Statements bound as [`BoundStatement::Other`]
/// (DDL / management / fulltext / vector / EXPLAIN) cannot be
/// introspected and must fall back to full-space metadata loading.
fn referenced_schema_objects(
    bound: &crate::binder::BoundStatement,
) -> (Vec<String>, Vec<String>, bool) {
    use crate::binder::bound::{
        BoundDeleteTarget, BoundInsertTarget, BoundLookupTarget, BoundMergePattern, BoundStatement,
        BoundUpdateTarget,
    };

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
        BoundStatement::Assignment(assignment) => referenced_schema_objects(&assignment.statement),
        BoundStatement::Return(_)
        | BoundStatement::With(_)
        | BoundStatement::Unwind(_)
        | BoundStatement::GroupBy(_)
        | BoundStatement::Filter(_)
        | BoundStatement::Yield(_)
        | BoundStatement::Collect(_)
        | BoundStatement::AssignVariable(_) => (tags, edges, false),
        BoundStatement::Insert(insert) => {
            match &insert.target {
                BoundInsertTarget::Vertices { tag, .. } => {
                    push_unique(&mut tags, &tag.tag_name);
                }
                BoundInsertTarget::Edge { edge_name, .. } => {
                    push_unique(&mut edges, edge_name);
                }
            }
            (tags, edges, false)
        }
        BoundStatement::Update(update) => {
            match &update.target {
                BoundUpdateTarget::Tag(name) => {
                    push_unique(&mut tags, name);
                }
                BoundUpdateTarget::TagOnVertex { tag_name, .. } => {
                    push_unique(&mut tags, tag_name);
                }
                BoundUpdateTarget::Edge(edge) => {
                    if let Some(name) = &edge.edge_type {
                        push_unique(&mut edges, name);
                    }
                }
                BoundUpdateTarget::Vertex(_) => {}
            }
            (tags, edges, false)
        }
        BoundStatement::Delete(delete) => {
            match &delete.target {
                BoundDeleteTarget::Vertices { tag, .. } => {
                    push_unique(&mut tags, tag);
                }
                BoundDeleteTarget::Edges { edge_type, .. } => {
                    if let Some(name) = edge_type {
                        push_unique(&mut edges, name);
                    }
                }
                BoundDeleteTarget::Index(_) => {}
            }
            (tags, edges, false)
        }
        BoundStatement::Merge(merge) => {
            match &merge.pattern {
                BoundMergePattern::Node(vertex) => {
                    for label in &vertex.labels {
                        push_unique(&mut tags, label);
                    }
                }
                BoundMergePattern::Edge { src, edge, dst } => {
                    for label in &src.labels {
                        push_unique(&mut tags, label);
                    }
                    for label in &dst.labels {
                        push_unique(&mut tags, label);
                    }
                    for edge_type in &edge.edge_types {
                        push_unique(&mut edges, edge_type);
                    }
                }
            }
            (tags, edges, false)
        }
        BoundStatement::Copy(copy) => {
            match &copy.target {
                crate::parser::ast::CopyTarget::Vertex(name) => {
                    push_unique(&mut tags, name);
                }
                crate::parser::ast::CopyTarget::Edge(name) => {
                    push_unique(&mut edges, name);
                }
            }
            (tags, edges, false)
        }
        BoundStatement::Set(_) | BoundStatement::Remove(_) => (tags, edges, false),
        // Access-control statements address users, roles and spaces; they
        // never reference a tag or an edge type.
        BoundStatement::CreateUser(_)
        | BoundStatement::DropUser(_)
        | BoundStatement::AlterUser(_)
        | BoundStatement::ChangePassword(_)
        | BoundStatement::Grant(_)
        | BoundStatement::Revoke(_)
        | BoundStatement::DescribeUser(_)
        | BoundStatement::ShowUsers(_)
        | BoundStatement::ShowRoles(_) => (tags, edges, false),
        BoundStatement::Drop(_)
        | BoundStatement::Alter(_)
        | BoundStatement::Show(_)
        | BoundStatement::ShowCreate(_)
        | BoundStatement::Desc(_)
        | BoundStatement::ClearSpace(_)
        | BoundStatement::CreateFulltextIndex(_)
        | BoundStatement::CreateVectorIndex(_)
        | BoundStatement::Explain(_)
        | BoundStatement::Profile(_)
        | BoundStatement::Use(_)
        | BoundStatement::BeginTransaction(_)
        | BoundStatement::Commit(_)
        | BoundStatement::Rollback(_)
        | BoundStatement::Savepoint(_)
        | BoundStatement::ReleaseSavepoint(_) => (tags, edges, true),
        BoundStatement::Other(_) => (tags, edges, true),
    }
}
