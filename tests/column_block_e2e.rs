//! A1 column-block path end-to-end equivalence: queries executed with the
//! storage column-block switch ON must return identical results to the
//! row-based path (switch OFF).

use graphdb::core::stats::StatsManager;
use graphdb::core::types::{PropertyDef, SpaceInfo, TagInfo, VertexId};
use graphdb::core::vertex_edge_path::Tag;
use graphdb::core::{DataType, Value, Vertex};
use graphdb::query::executor::streaming::operators::source_operator::{
    column_block_enabled, set_column_block_enabled,
};
use graphdb::query::optimizer::OptimizerEngine;
use graphdb::query::QueryPipelineManager;
use graphdb::storage::{GraphStorage, StorageReader, StorageSchemaOps, StorageWriter};
use graphdb::test_utils::TestStorage;
use parking_lot::RwLock;
use std::sync::Arc;

const SPACE: &str = "cb_test";

fn space_info(storage: &Arc<RwLock<GraphStorage>>) -> SpaceInfo {
    let id = storage.read().get_space_id(SPACE).expect("space id");
    let mut info = SpaceInfo::new(SPACE.to_string());
    info.space_id = id;
    info
}

fn query_rows(
    pipeline: &mut QueryPipelineManager<GraphStorage>,
    space: &SpaceInfo,
    sql: &str,
) -> Vec<String> {
    let result = pipeline
        .execute_query_with_space(sql, Some(space.clone()))
        .expect("query should succeed");
    let dataset = result.to_data_set().expect("dataset result");
    dataset
        .rows
        .iter()
        .map(|row| format!("{:?}", row))
        .collect()
}

#[test]
fn column_block_matches_row_path_e2e() {
    let test_storage = TestStorage::new().expect("storage");
    let storage = test_storage.storage();
    {
        let mut storage = storage.write();
        let mut space = SpaceInfo::new(SPACE.to_string()).with_vid_type(DataType::BigInt);
        storage.create_space(&mut space).expect("create space");
        storage
            .create_tag(
                SPACE,
                &TagInfo::new("node".to_string()).with_properties(vec![
                    PropertyDef::new("value".to_string(), DataType::BigInt),
                    PropertyDef::new("group_id".to_string(), DataType::BigInt),
                    PropertyDef::new("name".to_string(), DataType::String),
                ]),
            )
            .expect("create tag");
        let vertices: Vec<Vertex> = (0..100)
            .map(|i| {
                Vertex::new(
                    VertexId::from_int64(i),
                    vec![Tag::new(
                        "node".to_string(),
                        vec![
                            ("value".to_string(), Value::BigInt(i)),
                            ("group_id".to_string(), Value::BigInt(i % 5)),
                            ("name".to_string(), Value::string(format!("node_{i}"))),
                        ]
                        .into_iter()
                        .collect(),
                    )],
                )
            })
            .collect();
        storage
            .batch_insert_vertices(SPACE, vertices)
            .expect("insert");
    }

    let mut pipeline = QueryPipelineManager::with_optimizer(
        storage.clone(),
        Arc::new(StatsManager::new()),
        Arc::new(OptimizerEngine::default()),
    );
    let space = space_info(&storage);

    assert!(!column_block_enabled(), "column-block must default to off");

    let queries = [
        "MATCH (n:node) RETURN count(n)",
        "MATCH (n:node) WHERE n.value < 50 RETURN count(n)",
        "MATCH (n:node) RETURN n.value, n.name",
    ];

    let row_results: Vec<Vec<String>> = queries
        .iter()
        .map(|sql| query_rows(&mut pipeline, &space, sql))
        .collect();

    set_column_block_enabled(true);
    assert!(column_block_enabled(), "column-block should be enabled");
    let column_results: Vec<Vec<String>> = queries
        .iter()
        .map(|sql| query_rows(&mut pipeline, &space, sql))
        .collect();
    set_column_block_enabled(false);
    assert!(!column_block_enabled(), "column-block should be restored");

    for (sql, (row, column)) in queries
        .iter()
        .zip(row_results.iter().zip(column_results.iter()))
    {
        assert_eq!(
            row, column,
            "column-block and row paths diverge for query: {}",
            sql
        );
    }
}

/// Differential test: enrich scan slots rule on/off.
///
/// The `EnrichScanSlotsWithFilterPropsRule` widens the scan output layout
/// with predicate columns so the columnar evaluator can serve WHERE clauses
/// directly. This test verifies that results are identical with the optimizer
/// enabled (rule applies) vs. disabled (rule does not apply).
#[test]
fn enrich_scan_slots_rule_differential() {
    let test_storage = TestStorage::new().expect("storage");
    let storage = test_storage.storage();
    {
        let mut storage = storage.write();
        let mut space = SpaceInfo::new(SPACE.to_string()).with_vid_type(DataType::BigInt);
        storage.create_space(&mut space).expect("create space");
        storage
            .create_tag(
                SPACE,
                &TagInfo::new("node".to_string()).with_properties(vec![
                    PropertyDef::new("value".to_string(), DataType::BigInt),
                    PropertyDef::new("name".to_string(), DataType::String),
                ]),
            )
            .expect("create tag");
        let vertices: Vec<Vertex> = (0..100)
            .map(|i| {
                Vertex::new(
                    VertexId::from_int64(i),
                    vec![Tag::new(
                        "node".to_string(),
                        vec![
                            ("value".to_string(), Value::BigInt(i)),
                            ("name".to_string(), Value::string(format!("node_{i}"))),
                        ]
                        .into_iter()
                        .collect(),
                    )],
                )
            })
            .collect();
        storage
            .batch_insert_vertices(SPACE, vertices)
            .expect("insert");
    }

    let stats = Arc::new(StatsManager::new());
    let space = space_info(&storage);

    // Pipeline WITH optimizer (enrich rule applies)
    let mut pipeline_on = QueryPipelineManager::with_optimizer(
        storage.clone(),
        stats.clone(),
        Arc::new(OptimizerEngine::default()),
    );

    // Pipeline WITHOUT optimizer (enrich rule does not apply)
    let mut opt_off_engine = OptimizerEngine::default();
    opt_off_engine.set_enable_heuristic(false);
    let mut pipeline_off =
        QueryPipelineManager::with_optimizer(storage.clone(), stats, Arc::new(opt_off_engine));

    // Queries that trigger the enrich rule: Filter(ScanVertices) where the
    // predicate column is NOT in the RETURN clause.
    let queries = [
        "MATCH (n:node) WHERE n.value < 50 RETURN n.name",
        "MATCH (n:node) WHERE n.name > 'node_50' RETURN n.value",
        "MATCH (n:node) WHERE n.value > 10 AND n.value < 30 RETURN n.name",
    ];

    for sql in &queries {
        let on_result = query_rows(&mut pipeline_on, &space, sql);
        let off_result = query_rows(&mut pipeline_off, &space, sql);
        assert_eq!(
            on_result.len(),
            off_result.len(),
            "row count diverge for query: {}",
            sql
        );
        assert_eq!(
            on_result, off_result,
            "enrich rule changes results for query: {}",
            sql
        );
    }
}
