//! Lifecycle assertion tests (review doc §8.3).
//!
//! Covers the query lifecycle contract that P0/P1 unified:
//! - query_id propagation (request-supplied id vs registry-allocated)
//! - cancellation through all three paths (runtime token, registry, mark_killed)
//!   including the registry-None (pure pipeline / embedded) path
//! - EXPLAIN ANALYZE / PROFILE inheriting the caller's TransactionScope
//! - partition-plan cache hit consistency (hit and miss produce equal results)
//! - DDL invalidating cached (partitioned) plans

use graphdb_query::core::types::expr::expression_context::ExpressionAnalysisContext;
use graphdb_query::query::executor::base::ExecutionContext;
use graphdb_query::query::executor::streaming::instance::{
    QueryBindings, QueryExecutionInstance, ResultSink,
};
use graphdb_query::query::executor::streaming::parameters::ParameterSchema;
use graphdb_query::query::executor::streaming::plan::types::{
    CapabilitySet, FragmentGraph, FragmentId, OutputContract, PhysicalPlan, PipelineMode,
    PlanCompatibility, PlanFingerprint,
};
use graphdb_query::query::executor::streaming::query_registry::CancelToken;
use graphdb_query::query::executor::streaming::query_registry::{QueryId, QueryRegistry};
use graphdb_query::query::executor::streaming::slot::SlotLayout;
use graphdb_query::query::executor::streaming::transaction_scope::CancelReason;
use graphdb_query::query::executor::streaming::transaction_scope::TransactionScope;
use graphdb_query::query::planning::plan::core::nodes::control_flow::start_node::StartNode;
use graphdb_query::query::planning::plan::PlanNodeEnum;
use std::collections::HashMap;
use std::sync::Arc;

mod common;

/// Build a typed TransactionId from a raw integer (tests only).
fn crate_test_txn_id(raw: u64) -> graphdb_query::core::types::TransactionId {
    graphdb_query::core::types::TransactionId(raw)
}

// ── Plan construction helpers ────────────────────────────────────────────────

fn expr_ctx() -> Arc<ExpressionAnalysisContext> {
    Arc::new(ExpressionAnalysisContext::new())
}

/// Build a minimal one-operator (Start) physical plan, mirroring the arena
/// builder unit tests.  Good enough to drive instantiation / cancel / id logic.
fn start_plan() -> Arc<PhysicalPlan> {
    let start = StartNode::new();
    let node = PlanNodeEnum::Start(start);
    let mut ctx = graphdb_query::query::executor::streaming::plan::PhysicalPlanBuildContext::new();
    let exec_ctx = ExecutionContext::new(expr_ctx());
    let plan = graphdb_query::query::executor::streaming::plan::PhysicalPlanBuilder::build(
        &node, &mut ctx, &exec_ctx,
    )
    .expect("build start plan");
    Arc::new(plan)
}

/// Build bindings carrying the given query id, token and transaction scope.
fn bindings(query_id: u64, token: Option<CancelToken>, scope: TransactionScope) -> QueryBindings {
    let mut ctx = ExecutionContext::new(expr_ctx());
    ctx.query_id = query_id;
    ctx.cancel_token = token;
    QueryBindings::from_context(&ctx, scope)
}

// ── §8.3.1 query_id propagation ─────────────────────────────────────────────

#[test]
fn lifecycle_query_id_request_supplied_reaches_runtime() {
    let plan = start_plan();
    let token = CancelToken::new();
    let instance = QueryExecutionInstance::instantiate_plan(
        plan,
        bindings(1234, Some(token), TransactionScope::None),
        ResultSink::Discard,
        None,
    )
    .expect("instantiate");
    assert_eq!(
        instance.runtime().query_id().query_id,
        1234,
        "request-supplied query id must reach the runtime"
    );
}

#[test]
fn lifecycle_query_id_registry_allocates_when_absent() {
    let registry = Arc::new(QueryRegistry::new());
    let instance = QueryExecutionInstance::instantiate_plan(
        start_plan(),
        bindings(0, None, TransactionScope::None),
        ResultSink::Discard,
        Some(registry.clone()),
    )
    .expect("instantiate");
    let qid = instance.runtime().query_id().query_id;
    assert_ne!(qid, 0, "registry must allocate a non-zero query id");
    assert_eq!(
        registry.active_queries().len(),
        1,
        "registry must track the active query"
    );
}

#[test]
fn lifecycle_query_id_registry_honors_requested_when_free() {
    let registry = Arc::new(QueryRegistry::new());
    let instance = QueryExecutionInstance::instantiate_plan(
        start_plan(),
        bindings(777, None, TransactionScope::None),
        ResultSink::Discard,
        Some(registry.clone()),
    )
    .expect("instantiate");
    assert_eq!(
        instance.runtime().query_id().query_id,
        777,
        "registry must honor a free requested id"
    );
}

#[test]
fn lifecycle_query_id_requested_id_already_active_allocates_fresh() {
    let registry = Arc::new(QueryRegistry::new());
    let _first = QueryExecutionInstance::instantiate_plan(
        start_plan(),
        bindings(555, None, TransactionScope::None),
        ResultSink::Discard,
        Some(registry.clone()),
    )
    .expect("instantiate first");
    let second = QueryExecutionInstance::instantiate_plan(
        start_plan(),
        bindings(555, None, TransactionScope::None),
        ResultSink::Discard,
        Some(registry.clone()),
    )
    .expect("instantiate second");
    let qid = second.runtime().query_id().query_id;
    assert_ne!(
        qid, 555,
        "colliding requested id must fall back to allocation"
    );
    assert!(registry.active_queries().contains(&QueryId(qid)));
}

// ── §8.3.2 cancellation: single token across all three paths ────────────────

#[test]
fn lifecycle_cancel_token_marks_runtime_without_registry() {
    // The registry-None path (pure pipeline / embedded): mark_killed on the
    // request-scoped token must be visible on the runtime even though there
    // is no shared registry to re-adopt it.
    let token = CancelToken::new();
    let instance = QueryExecutionInstance::instantiate_plan(
        start_plan(),
        bindings(0, Some(token.clone()), TransactionScope::None),
        ResultSink::Discard,
        None,
    )
    .expect("instantiate without registry");
    assert!(!instance.runtime().is_cancelled());
    token.cancel(CancelReason::UserKill);
    assert!(
        instance.runtime().is_cancelled(),
        "runtime must adopt the request-scoped token even without a registry"
    );
}

#[test]
fn lifecycle_registry_cancel_reaches_runtime() {
    // Path 1: registry.cancel (KILL QUERY) must be visible on the runtime.
    let registry = Arc::new(QueryRegistry::new());
    let token = CancelToken::new();
    let instance = QueryExecutionInstance::instantiate_plan(
        start_plan(),
        bindings(0, Some(token.clone()), TransactionScope::None),
        ResultSink::Discard,
        Some(registry.clone()),
    )
    .expect("instantiate with registry");
    let qid = instance.runtime().query_id();
    registry.cancel(QueryId(qid.query_id), CancelReason::UserKill);
    assert!(
        instance.runtime().is_cancelled(),
        "registry cancel must flip the shared runtime token"
    );
    assert!(
        token.is_cancelled(),
        "registry cancel must be visible on the request-scoped token"
    );
}

#[test]
fn lifecycle_mark_killed_shared_token_path() {
    // Path 2: QueryContext::mark_killed cancels the request token, which the
    // runtime adopted at instantiation (works with and without registry).
    let token = CancelToken::new();
    let instance = QueryExecutionInstance::instantiate_plan(
        start_plan(),
        bindings(0, Some(token.clone()), TransactionScope::None),
        ResultSink::Discard,
        None,
    )
    .expect("instantiate");
    token.cancel(CancelReason::UserKill);
    assert!(instance.runtime().is_cancelled());
    assert!(token.is_cancelled());
    assert_eq!(token.reason(), Some(CancelReason::UserKill));
}

#[test]
fn lifecycle_runtime_cancel_is_visible_on_shared_token() {
    // Path 3: runtime.cancel() must be visible through the request token and
    // (when registered) the registry entry.
    let registry = Arc::new(QueryRegistry::new());
    let token = CancelToken::new();
    let instance = QueryExecutionInstance::instantiate_plan(
        start_plan(),
        bindings(0, Some(token.clone()), TransactionScope::None),
        ResultSink::Discard,
        Some(registry.clone()),
    )
    .expect("instantiate");
    let qid = instance.runtime().query_id();
    instance.runtime().cancel();
    assert!(token.is_cancelled(), "runtime cancel must share the token");
    assert!(
        registry
            .cancellation_reason(QueryId(qid.query_id))
            .is_some(),
        "runtime cancel must be observable through the registry"
    );
}

#[test]
fn lifecycle_registry_none_cancel_aborts_execution() {
    // Full registry-None loop: a query whose mark_killed happens before
    // execution begins must abort at the next cooperative check.
    let token = CancelToken::new();
    let mut instance = QueryExecutionInstance::instantiate_plan(
        start_plan(),
        bindings(0, Some(token.clone()), TransactionScope::None),
        ResultSink::Materialize,
        None,
    )
    .expect("instantiate");
    token.cancel(CancelReason::UserKill);
    let result = instance.execute();
    assert!(
        result.is_err(),
        "cancelled query must not execute: {:?}",
        result.map(|r| r.count())
    );
}

// ── §8.3.3 EXPLAIN ANALYZE / PROFILE share the caller's transaction scope ──

#[test]
fn lifecycle_diagnostic_scopes_instantiate_all_variants() {
    // The diagnostics entry points take the scope explicitly and forward it
    // into QueryBindings; verify every scope variant is accepted end-to-end.
    let plan = start_plan();
    for scope in [
        TransactionScope::None,
        TransactionScope::CommandScope,
        TransactionScope::auto_commit(crate_test_txn_id(7)),
        TransactionScope::explicit(crate_test_txn_id(8), false),
        TransactionScope::explicit(crate_test_txn_id(9), true),
    ] {
        let mut instance = QueryExecutionInstance::instantiate_plan(
            plan.clone(),
            bindings(0, None, scope.clone()),
            ResultSink::Discard,
            None,
        )
        .unwrap_or_else(|e| panic!("scope {:?} must instantiate: {}", scope, e));
        let _ = instance.execute_discard();
    }
}

// ── §8.3.4 partition-plan cache hit/miss consistency ────────────────────────

fn make_partition_spec(
    ranges: &[(i64, i64)],
) -> graphdb_query::query::planning::plan::execution_plan::PartitionSpec {
    use graphdb_query::query::planning::plan::execution_plan::{PartitionSource, PartitionSpec};
    PartitionSpec::try_new(
        ranges.iter().map(|&(s, e)| s..e).collect(),
        PartitionSource::VertexId {
            tag: "lifecycle_space".to_string(),
        },
        Some(1),
    )
    .expect("valid partition spec")
}

fn empty_plan_with_hash(hash: u64) -> Arc<PhysicalPlan> {
    Arc::new(PhysicalPlan {
        operators: Vec::new(),
        logical_to_physical: HashMap::new(),
        fragments: FragmentGraph::new(Vec::new(), FragmentId(0)),
        root_fragment: FragmentId(0),
        output: OutputContract {
            output_layout: SlotLayout::new(Vec::new()),
            always_produces_row: false,
            nullability: Vec::new(),
            ordering: Vec::new(),
            delivery_streamable: true,
            pipeline_mode: PipelineMode::Pipelined,
        },
        compatibility: PlanCompatibility {
            fingerprint: PlanFingerprint { version: 1, hash },
            layout_version: None,
            required_capabilities: CapabilitySet::EMPTY,
            planning_config_hash: 0,
            optimizer_version: 0,
        },
        required_capabilities: CapabilitySet::EMPTY,
        parameter_schema: ParameterSchema {
            params: Vec::new(),
            name_to_slot: HashMap::new(),
        },
        parallel_fallback_reason: String::new(),
        cbo_notes: Vec::new(),
        partition_spec: None,
    })
}

fn cache_context() -> graphdb_query::query::cache::plan_cache::PlanCacheContext {
    graphdb_query::query::cache::plan_cache::PlanCacheContext {
        space_name: Some("lifecycle_space".to_string()),
        schema_version: Some(1),
        index_version: Some(1),
        param_type_signature: None,
        optimizer_version: 1,
        planning_config_hash: 0,
    }
}

fn put_context() -> graphdb_query::query::cache::plan_cache::PlanCachePutContext {
    let ctx = cache_context();
    graphdb_query::query::cache::plan_cache::PlanCachePutContext {
        dependent_tables: vec!["lifecycle_space".to_string()],
        space_name: ctx.space_name.clone(),
        schema_version: ctx.schema_version,
        index_version: ctx.index_version,
        is_dml: false,
        is_transaction: false,
        optimizer_version: ctx.optimizer_version,
        planning_config_hash: ctx.planning_config_hash,
    }
}

#[test]
fn lifecycle_partition_cache_hit_returns_same_plan() {
    use graphdb_query::query::cache::plan_cache::QueryPlanCache;
    let cache = QueryPlanCache::default();
    let query = "MATCH (n:Node) RETURN n";
    let spec = make_partition_spec(&[(0, 100)]);
    let plan = empty_plan_with_hash(42);

    cache.put_with_partition(query, &spec, plan.clone(), Vec::new(), put_context());

    // Hit: same layout + same context returns the cached plan.
    let hit = cache
        .get_with_partition_context(query, &spec, cache_context())
        .expect("partition-keyed lookup must hit");
    assert_eq!(hit.plan.fragment_count(), plan.fragment_count());

    // Miss: a different layout must not be served by the cached entry.
    let other_spec = make_partition_spec(&[(0, 50), (50, 100)]);
    assert!(
        cache.get_with_partition(query, &other_spec).is_none(),
        "different layout must not hit"
    );

    // Miss: same layout but different schema version must not hit.
    let stale_context = graphdb_query::query::cache::plan_cache::PlanCacheContext {
        schema_version: Some(0),
        ..cache_context()
    };
    assert!(
        cache
            .get_with_partition_context(query, &spec, stale_context)
            .is_none(),
        "stale schema version must not hit"
    );
}

#[test]
fn lifecycle_partition_cache_ddl_invalidates_partitioned_plans() {
    use graphdb_query::query::cache::plan_cache::QueryPlanCache;
    let cache = QueryPlanCache::default();
    let query = "MATCH (n:Node) RETURN n";
    let spec = make_partition_spec(&[(0, 100)]);
    cache.put_with_partition(
        query,
        &spec,
        empty_plan_with_hash(7),
        Vec::new(),
        put_context(),
    );
    assert!(cache
        .get_with_partition_context(query, &spec, cache_context())
        .is_some());

    // DDL bumps the schema generation: space-scoped invalidation removes it.
    let removed = cache.invalidate_space("lifecycle_space");
    assert!(removed > 0, "space invalidation must remove cached plans");
    assert!(
        cache
            .get_with_partition_context(query, &spec, cache_context())
            .is_none(),
        "partitioned plan must be gone after DDL invalidation"
    );
}

#[test]
fn lifecycle_partition_cache_hit_returns_exact_compiled_instance() {
    // Regression for the review note: "分区计划 cache 命中与否结果一致".  A
    // cache hit must serve the exact plan instance that a miss would compile
    // (identity, not just equivalence), so hit/miss execution cannot diverge.
    use graphdb_query::query::cache::plan_cache::QueryPlanCache;
    let cache = QueryPlanCache::default();
    let query = "MATCH (n:Node) RETURN n";
    let spec = make_partition_spec(&[(0, 100)]);
    let compiled = empty_plan_with_hash(99);
    let original_ptr = Arc::as_ptr(&compiled);
    cache.put_with_partition(query, &spec, compiled.clone(), Vec::new(), put_context());

    // First hit: serves the compiled instance itself.
    let hit = cache
        .get_with_partition_context(query, &spec, cache_context())
        .expect("hit after put");
    assert!(
        Arc::ptr_eq(&hit.plan, &compiled),
        "hit must serve the exact compiled plan instance"
    );

    // Re-put with the same key replaces the entry (normal cache semantics);
    // a fresh get must serve the new instance, again by identity.
    let recompiled = empty_plan_with_hash(100);
    cache.put_with_partition(query, &spec, recompiled.clone(), Vec::new(), put_context());
    let hit = cache
        .get_with_partition_context(query, &spec, cache_context())
        .expect("hit after re-put");
    assert!(
        Arc::ptr_eq(&hit.plan, &recompiled),
        "hit after re-put must serve the recompiled instance"
    );
    let _ = original_ptr;
}

// ── §8.3.5 EXPLAIN / PROFILE output surfaces lifecycle metadata ─────────────

#[test]
fn lifecycle_explain_plan_description_round_trips_cbo_notes() {
    use graphdb_query::query::executor::explain::physical_plan_explain::physical_plan_to_plan_description;
    let plan = start_plan();
    let desc = physical_plan_to_plan_description(&plan);
    assert!(
        desc.cbo_notes.is_empty(),
        "a start-only plan has no CBO decisions"
    );

    // Plan-level metadata is part of the immutable plan; annotate and verify
    // it survives the conversion (EXPLAIN uses this to print decisions).
    let mut annotated = (*plan).clone();
    annotated.cbo_notes = vec!["join_order: greedy".to_string()];
    let desc = physical_plan_to_plan_description(&Arc::new(annotated));
    assert_eq!(desc.cbo_notes, vec!["join_order: greedy".to_string()]);
}

// ── T2: read-only statements bind a statement-level snapshot context ───────

#[test]
fn lifecycle_read_statement_binds_and_finalizes_read_operation_context() {
    use graphdb_query::core::stats::StatsManager;
    use graphdb_query::core::types::VertexId;
    use graphdb_query::core::types::{PropertyDef, SpaceInfo, TagInfo};
    use graphdb_query::core::vertex_edge_path::Tag;
    use graphdb_query::core::{DataType, Value, Vertex};
    use graphdb_query::query::optimizer::OptimizerEngine;
    use graphdb_query::query::pipeline::QueryPipelineManager;
    use graphdb_query::storage::StorageOperationContextOps;
    use graphdb_query::storage::{
        StorageReader, StorageSchemaContextOps, StorageSchemaOps, StorageWriter,
    };
    use std::collections::HashMap;

    let test_storage = common::TestStorage::new().expect("test storage");
    let storage = test_storage.storage();

    {
        let mut store = storage.write();
        let mut space = SpaceInfo::new("t2".to_string()).with_vid_type(DataType::BigInt);
        store.create_space(&mut space).unwrap();
        let tag = TagInfo::new("Person".to_string())
            .with_properties(vec![PropertyDef::new("name".to_string(), DataType::String)]);
        store.create_tag("t2", &tag).unwrap();
        for i in 0..8i64 {
            let vertex = Vertex::new(
                VertexId::from_int64(i),
                vec![Tag::new(
                    "Person".to_string(),
                    vec![("name".to_string(), Value::string(format!("p{}", i)))]
                        .into_iter()
                        .collect::<HashMap<_, _>>(),
                )],
            );
            store.insert_vertex("t2", vertex).unwrap();
        }
    }

    let stats_manager = Arc::new(StatsManager::new());
    let schema_manager = {
        let guard = storage.read();
        StorageSchemaContextOps::get_schema_manager(&*guard).expect("schema manager")
    };
    let mut pipeline = QueryPipelineManager::with_optimizer(
        storage.clone(),
        stats_manager,
        Arc::new(OptimizerEngine::default()),
    )
    .with_schema_manager(schema_manager);

    // A read-only statement runs through the bound read context and the
    // statement snapshot is finalized (unregistered) after execution.
    for _ in 0..20 {
        let result = pipeline
            .execute_query_with_space(
                "MATCH (n:Person) RETURN n.name",
                Some(storage.read().get_space("t2").unwrap().unwrap()),
            )
            .expect("read query should succeed");
        let rows = result.count();
        assert_eq!(rows, 8);
    }

    // The pipeline base handle itself is untouched: no operation context
    // leaks onto the shared storage instance.
    let guard = storage.read();
    assert!(
        StorageOperationContextOps::operation_context(&*guard).is_none(),
        "read statements must not leak an operation context onto the base handle"
    );
}
