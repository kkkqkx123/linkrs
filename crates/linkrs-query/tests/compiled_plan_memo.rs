//! Read-only compiled physical-plan memo regression tests.
//!
//! A repeated partitioned read must skip bind, planning, optimization and
//! physical build while returning the same rows. Catalog and statistics
//! changes must retire the memo entry and force a fresh compile before the
//! next hit is allowed.

#![allow(clippy::arc_with_non_send_sync)]

mod common;

use common::TestStorage;
use linkrs_core::types::{PropertyDef, SpaceInfo, TagInfo, VertexId};
use linkrs_core::vertex_edge_path::Tag;
use linkrs_core::{DataType, Value, Vertex};
use linkrs_metrics::{QueryProfile, StatsManager};
use linkrs_query::executor::base::ExecutionResult;
use linkrs_query::optimizer::{OptimizerEngine, PartitioningConfig};
use linkrs_query::pipeline::QueryPipelineManager;
use linkrs_query::storage::{GraphStorage, StorageSchemaOps, StorageWriter};
use parking_lot::RwLock;
use std::sync::Arc;

const SPACE: &str = "compiled_memo";
const TAG: &str = "Node";
const VERTEX_COUNT: i64 = 2000;
const QUERY: &str = "MATCH (n:Node) WHERE n.value < 800 RETURN count(n)";

fn space_info() -> SpaceInfo {
    let mut info = SpaceInfo::new(SPACE.to_string()).with_vid_type(DataType::BigInt);
    info.space_id = 1;
    info
}

fn partition_config(workers: usize) -> PartitioningConfig {
    PartitioningConfig {
        enabled: true,
        min_rows_per_partition: 400,
        max_partitions: workers * 2,
        vertex_id_range: Some(0i64..2 * VERTEX_COUNT),
        max_workers: workers,
        max_buffered_chunks: 10,
        fixed_overhead_rows: 0,
        per_partition_rows: 0,
        exchange_row_per_mille: 0,
        allow_pure_count_chain: false,
        allow_global_aggregate: false,
        allow_ordered_parallel_expand: false,
        allow_parallel_count_frontier: false,
        allow_partition_metadata_count: false,
    }
}

fn fixture() -> QueryPipelineManager<GraphStorage> {
    let storage = TestStorage::new().expect("storage init").storage();
    {
        let mut guard = storage.write();
        let mut space = space_info();
        guard.create_space(&mut space).expect("create space");
        guard
            .create_tag(
                SPACE,
                &TagInfo::new(TAG.to_string()).with_properties(vec![
                    PropertyDef::new("id".to_string(), DataType::BigInt),
                    PropertyDef::new("value".to_string(), DataType::BigInt),
                ]),
            )
            .expect("create tag");
    }

    let mut start = 0i64;
    while start < VERTEX_COUNT {
        let end = (start + 500).min(VERTEX_COUNT);
        let vertices: Vec<Vertex> = (start..end)
            .map(|index| {
                Vertex::new(
                    VertexId::try_from_int64(index).expect("test vertex id"),
                    Tag::new(
                        TAG.to_string(),
                        vec![
                            ("id".to_string(), Value::BigInt(index)),
                            ("value".to_string(), Value::BigInt(index)),
                        ]
                        .into_iter()
                        .map(|(name, value)| (Arc::from(name.as_str()), value))
                        .collect(),
                    ),
                )
            })
            .collect();
        storage
            .write()
            .batch_insert_vertices(SPACE, vertices)
            .expect("insert vertices");
        start = end;
    }

    let mut engine = OptimizerEngine::default();
    engine.set_partitioning_config(partition_config(2));
    let pipeline = QueryPipelineManager::with_optimizer(
        storage,
        Arc::new(StatsManager::new()),
        Arc::new(engine),
    );
    pipeline
        .collect_statistics(SPACE, true)
        .expect("collect statistics");
    pipeline
}

fn rows(result: ExecutionResult) -> Vec<Vec<Value>> {
    match result {
        ExecutionResult::DataSet { data, .. } => data.rows,
        ExecutionResult::Empty => Vec::new(),
        other => panic!("unexpected result: {other:?}"),
    }
}

fn profiled(
    pipeline: &mut QueryPipelineManager<GraphStorage>,
    query: &str,
) -> (Vec<Vec<Value>>, QueryProfile) {
    let (result, _metrics, profile) = pipeline
        .execute_query_with_profile(query, 1, Some(space_info()))
        .expect("query should succeed");
    (rows(result), profile)
}

#[test]
fn repeated_partitioned_read_hits_memo_without_bind_or_optimize() {
    let mut pipeline = fixture();

    let (cold_rows, cold_profile) = profiled(&mut pipeline, QUERY);
    assert_eq!(cold_rows, vec![vec![Value::BigInt(800)]]);
    assert!(!cold_profile.stages.compiled_plan_memo_hit);
    assert_eq!(
        pipeline.compiled_plan_memo_len(),
        1,
        "a partitioned plan must be stored under the pre-bind key"
    );

    let (hot_rows, hot_profile) = profiled(&mut pipeline, QUERY);
    assert_eq!(hot_rows, cold_rows, "memo hit must return identical values");
    assert!(hot_profile.stages.plan_cache_hit);
    assert!(hot_profile.stages.compiled_plan_memo_hit);
    assert_eq!(hot_profile.stages.plan_us, 0);
    assert_eq!(hot_profile.stages.optimize_us, 0);
    assert_eq!(hot_profile.stages.statistics_us, 0);
    assert_eq!(pipeline.compiled_plan_memo_hits(), 1);
    assert_eq!(pipeline.dml_bind_skipped_count(), 1);
}

#[test]
fn ddl_and_statistics_refresh_retire_the_memo() {
    let mut pipeline = fixture();
    profiled(&mut pipeline, QUERY);
    profiled(&mut pipeline, QUERY);
    assert_eq!(pipeline.compiled_plan_memo_hits(), 1);

    pipeline
        .execute_query_with_space("ALTER TAG Node ADD (note STRING)", Some(space_info()))
        .expect("alter tag");
    assert_eq!(pipeline.compiled_plan_memo_len(), 0);

    let (after_ddl, ddl_profile) = profiled(&mut pipeline, QUERY);
    assert_eq!(after_ddl, vec![vec![Value::BigInt(800)]]);
    assert!(!ddl_profile.stages.compiled_plan_memo_hit);
    assert_eq!(pipeline.compiled_plan_memo_hits(), 1);
    assert_eq!(pipeline.compiled_plan_memo_len(), 1);

    profiled(&mut pipeline, QUERY);
    assert_eq!(pipeline.compiled_plan_memo_hits(), 2);

    pipeline
        .collect_statistics(SPACE, true)
        .expect("forced statistics refresh");
    let (after_analyze, analyze_profile) = profiled(&mut pipeline, QUERY);
    assert_eq!(after_analyze, vec![vec![Value::BigInt(800)]]);
    assert!(!analyze_profile.stages.compiled_plan_memo_hit);
    assert_eq!(pipeline.compiled_plan_memo_hits(), 2);

    profiled(&mut pipeline, QUERY);
    assert_eq!(pipeline.compiled_plan_memo_hits(), 3);
}
