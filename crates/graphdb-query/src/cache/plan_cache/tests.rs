use super::*;
use crate::planning::plan::execution_plan::PartitionSource;

#[test]
fn test_plan_cache_key() {
    let key1 = PlanCacheKey::from_query("SELECT * FROM users");
    let key2 = PlanCacheKey::from_query("SELECT * FROM users");
    let key3 = PlanCacheKey::from_query("SELECT * FROM posts");

    assert_eq!(key1, key2);
    assert_ne!(key1, key3);
}

#[test]
fn test_plan_cache_key_verify() {
    let key = PlanCacheKey::from_query("SELECT * FROM users");
    assert!(key.verify_query("SELECT * FROM users"));
    assert!(!key.verify_query("SELECT * FROM posts"));
}

#[test]
fn cache_key_covers_all_compatibility_dimensions() {
    let base = PlanCacheContext {
        space_name: Some("space".to_string()),
        schema_version: Some(1),
        index_version: Some(2),
        param_type_signature: Some(3),
        optimizer_version: 4,
        planning_config_hash: 5,
    };
    let key = PlanCacheKey::from_query_with_context("MATCH (n) RETURN n", base.clone());
    for changed in [
        PlanCacheContext {
            optimizer_version: 7,
            ..base.clone()
        },
        PlanCacheContext {
            planning_config_hash: 7,
            ..base.clone()
        },
        PlanCacheContext {
            schema_version: Some(7),
            ..base.clone()
        },
        PlanCacheContext {
            index_version: Some(7),
            ..base.clone()
        },
    ] {
        assert_ne!(
            key,
            PlanCacheKey::from_query_with_context("MATCH (n) RETURN n", changed)
        );
    }
}

#[test]
fn test_parameterized_query_handler() {
    let handler = ParameterizedQueryHandler::new();

    let params = handler.extract_params("SELECT * FROM users WHERE id = $1 AND name = @name");

    assert_eq!(params.len(), 2);
    assert_eq!(params[0].index, 1);
    assert!(params[0].name.is_none());
    assert_eq!(params[1].index, 1);
    assert_eq!(params[1].name, Some("name".to_string()));
}

#[test]
fn test_parameterized_query_handler_parameterize() {
    let handler = ParameterizedQueryHandler::new();

    let (parameterized, params) = handler.parameterize("SELECT * FROM users WHERE id = $1");

    assert_eq!(parameterized, "SELECT * FROM users WHERE id = ?");
    assert_eq!(params.len(), 1);
}

#[test]
fn test_query_plan_cache_basic() {
    let cache = QueryPlanCache::default();

    assert!(cache.is_empty());
    assert_eq!(cache.len(), 0);
}

#[test]
fn test_cache_priority_ordering() {
    assert!(CachePriority::Critical > CachePriority::High);
    assert!(CachePriority::High > CachePriority::Normal);
    assert!(CachePriority::Normal > CachePriority::Low);
}

fn make_spec(ranges: &[(i64, i64)], layout_version: Option<u64>) -> PartitionSpec {
    PartitionSpec::try_new(
        ranges.iter().map(|(start, end)| *start..*end).collect(),
        PartitionSource::VertexId {
            tag: "Node".to_string(),
        },
        layout_version,
    )
    .expect("valid spec")
}

#[test]
fn partition_fingerprint_changes_with_ranges() {
    let key_a = PlanCacheKey::from_query_with_partition(
        "MATCH (n:Node) RETURN n",
        &make_spec(&[(0, 100), (100, 200)], Some(1)),
    );
    let key_b = PlanCacheKey::from_query_with_partition(
        "MATCH (n:Node) RETURN n",
        &make_spec(&[(0, 50), (50, 100), (100, 200)], Some(1)),
    );
    assert_ne!(key_a, key_b, "different range splits must not share a key");
}

#[test]
fn partition_fingerprint_changes_with_source_and_layout_version() {
    let base = make_spec(&[(0, 100)], Some(1));
    let other_source = PartitionSpec::try_new(
        std::iter::once(0..100).collect(),
        PartitionSource::EdgeId {
            edge_type: "Link".to_string(),
        },
        Some(1),
    )
    .expect("valid spec");
    let bumped_version = make_spec(&[(0, 100)], Some(2));

    let key_base = PlanCacheKey::from_query_with_partition("MATCH (n:Node) RETURN n", &base);
    assert_ne!(
        key_base,
        PlanCacheKey::from_query_with_partition("MATCH (n:Node) RETURN n", &other_source),
        "different data domains must not share a key"
    );
    assert_ne!(
        key_base,
        PlanCacheKey::from_query_with_partition("MATCH (n:Node) RETURN n", &bumped_version),
        "layout version bumps must not share a key"
    );
}

#[test]
fn partitioned_plan_is_isolated_from_plain_text_lookup() {
    use crate::executor::streaming::parameters::ParameterSchema;
    use crate::executor::streaming::plan::types::{
        CapabilitySet, FragmentGraph, FragmentId, OutputContract, PipelineMode, PlanCompatibility,
        PlanFingerprint,
    };
    use crate::executor::streaming::slot::SlotLayout;
    use std::collections::HashMap;

    let cache = QueryPlanCache::default();
    let query = "MATCH (n:Node) RETURN n";
    let spec = make_spec(&[(0, 100)], Some(1));
    let plan = Arc::new(PhysicalPlan {
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
            fingerprint: PlanFingerprint {
                version: 1,
                hash: 0,
            },
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
        partition_spec: Some(spec.clone()),
    });

    cache.put_with_partition(query, &spec, plan.clone(), Vec::new(), Default::default());
    assert!(
        cache.get(query).is_none(),
        "plain-text lookup must not serve a partitioned plan"
    );
    let cached = cache
        .get_with_partition(query, &spec)
        .expect("partition-keyed lookup should hit");
    assert_eq!(cached.plan.fragment_count(), 0);

    let other = make_spec(&[(0, 50), (50, 100)], Some(1));
    assert!(
        cache.get_with_partition(query, &other).is_none(),
        "a different layout must not hit the cached partitioned plan"
    );
}

#[test]
fn partitioned_plan_hit_respects_compatibility_context() {
    use crate::executor::streaming::parameters::ParameterSchema;
    use crate::executor::streaming::plan::types::{
        CapabilitySet, FragmentGraph, FragmentId, OutputContract, PipelineMode, PlanCompatibility,
        PlanFingerprint,
    };
    use crate::executor::streaming::slot::SlotLayout;
    use std::collections::HashMap;

    let cache = QueryPlanCache::default();
    let query = "MATCH (n:Node) RETURN n";
    let spec = make_spec(&[(0, 100)], Some(1));
    let plan = Arc::new(PhysicalPlan {
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
            fingerprint: PlanFingerprint {
                version: 1,
                hash: 0,
            },
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
        partition_spec: Some(spec.clone()),
    });

    let context = PlanCacheContext {
        space_name: Some("space_a".to_string()),
        schema_version: Some(1),
        index_version: Some(1),
        param_type_signature: Some(99),
        optimizer_version: 2,
        planning_config_hash: 3,
    };
    // The put path derives the param signature from the param positions;
    // use the same positions so the keys line up.
    let param_positions = vec![ParamPosition {
        index: 1,
        name: None,
        position: 0,
        expected_type: Some(graphdb_core::types::DataType::String),
    }];
    let param_sig = QueryPlanCache::compute_param_type_signature(&param_positions);
    let context = PlanCacheContext {
        param_type_signature: param_sig,
        ..context
    };
    cache.put_with_partition(
        query,
        &spec,
        plan.clone(),
        param_positions,
        PlanCachePutContext {
            space_name: context.space_name.clone(),
            schema_version: context.schema_version,
            index_version: context.index_version,
            optimizer_version: context.optimizer_version,
            planning_config_hash: context.planning_config_hash,
            ..PlanCachePutContext::default()
        },
    );

    let hit = cache
        .get_with_partition_context(query, &spec, context.clone())
        .expect("same layout + context must hit");
    assert_eq!(hit.plan.fragment_count(), 0);

    let changed_space = PlanCacheContext {
        space_name: Some("space_b".to_string()),
        ..context.clone()
    };
    assert!(
        cache
            .get_with_partition_context(query, &spec, changed_space)
            .is_none(),
        "a different space must miss the partitioned entry"
    );

    let changed_schema = PlanCacheContext {
        schema_version: Some(2),
        ..context.clone()
    };
    assert!(
        cache
            .get_with_partition_context(query, &spec, changed_schema)
            .is_none(),
        "a schema-version bump must miss the partitioned entry"
    );
}
