mod common;

use common::TestStorage;

use graphdb_query::core::types::{EdgeTypeInfo, PropertyDef, SpaceInfo, TagInfo, VertexId};
use graphdb_query::core::vertex_edge_path::Tag;
use graphdb_query::core::{DataType, StatsManager, Value, Vertex};
use graphdb_query::executor::base::ExecutionResult;
use graphdb_query::optimizer::{OptimizerEngine, PartitioningConfig};
use graphdb_query::pipeline::QueryPipelineManager;
use graphdb_query::storage::{GraphStorage, StorageSchemaOps, StorageWriter};
use parking_lot::RwLock;
use std::sync::Arc;

const SPACE: &str = "pp";
const TAG: &str = "Node";
const TAG2: &str = "Other";
const EDGE: &str = "Link";
const VERTEX_COUNT: i64 = 2000;

fn insert_vertices(storage: &Arc<RwLock<GraphStorage>>) {
    let mut start = 0i64;
    while start < VERTEX_COUNT {
        let end = (start + 500).min(VERTEX_COUNT);
        let vertices: Vec<Vertex> = (start..end)
            .map(|i| {
                Vertex::new(
                    VertexId::from_int64(i),
                    vec![Tag::new(
                        TAG.to_string(),
                        vec![
                            ("value".to_string(), Value::BigInt(i)),
                            ("group_id".to_string(), Value::BigInt(i % 20)),
                        ]
                        .into_iter()
                        .collect(),
                    )],
                )
            })
            .collect();
        storage
            .write()
            .batch_insert_vertices(SPACE, vertices)
            .expect("insert vertices");
        start = end;
    }
}

fn setup_storage() -> Arc<RwLock<GraphStorage>> {
    let storage = TestStorage::new().expect("storage init").storage();
    {
        let mut guard = storage.write();
        let mut space = SpaceInfo::new(SPACE.to_string()).with_vid_type(DataType::BigInt);
        guard.create_space(&mut space).expect("create space");
        guard
            .create_tag(
                SPACE,
                &TagInfo::new(TAG.to_string()).with_properties(vec![
                    PropertyDef::new("value".to_string(), DataType::BigInt),
                    PropertyDef::new("group_id".to_string(), DataType::BigInt),
                ]),
            )
            .expect("create tag");
        guard
            .create_tag(
                SPACE,
                &TagInfo::new(TAG2.to_string()).with_properties(vec![PropertyDef::new(
                    "value".to_string(),
                    DataType::BigInt,
                )]),
            )
            .expect("create tag 2");
        guard
            .create_edge_type(
                SPACE,
                &EdgeTypeInfo::new(EDGE.to_string())
                    .with_src_tag(TAG.to_string())
                    .with_dst_tag(TAG.to_string()),
            )
            .expect("create edge type");
    }
    insert_vertices(&storage);
    {
        let mut start = 0i64;
        while start < VERTEX_COUNT {
            let end = (start + 500).min(VERTEX_COUNT);
            let vertices: Vec<Vertex> = (start..end)
                .map(|i| {
                    Vertex::new(
                        VertexId::from_int64(i),
                        vec![Tag::new(
                            TAG2.to_string(),
                            vec![("value".to_string(), Value::BigInt(i + 1000))]
                                .into_iter()
                                .collect(),
                        )],
                    )
                })
                .collect();
            storage
                .write()
                .batch_insert_vertices(SPACE, vertices)
                .expect("insert tag2 vertices");
            start = end;
        }
    }
    storage
}

fn build_pipeline(storage: &Arc<RwLock<GraphStorage>>) -> QueryPipelineManager<GraphStorage> {
    let mut engine = OptimizerEngine::default();
    engine.set_partitioning_config(PartitioningConfig {
        max_workers: 1,
        ..PartitioningConfig::default()
    });
    let stats = Arc::new(StatsManager::new());
    let pipeline = QueryPipelineManager::with_optimizer(storage.clone(), stats, Arc::new(engine));
    pipeline.collect_statistics(SPACE, true).expect("stats");
    pipeline
}

fn space_info() -> SpaceInfo {
    let mut info = SpaceInfo::new(SPACE.to_string());
    info.space_id = 1;
    info
}

fn query_rows(pipeline: &mut QueryPipelineManager<GraphStorage>, query: &str) -> Vec<Vec<Value>> {
    let space = space_info();
    match pipeline
        .execute_query_with_space(query, Some(space))
        .expect("query should succeed")
    {
        ExecutionResult::DataSet { data, .. } => data.rows,
        ExecutionResult::Empty => vec![],
        other => panic!("unexpected result: {:?}", other),
    }
}

#[test]
fn probe_serial_join_plan() {
    let storage = setup_storage();
    let mut serial = build_pipeline(&storage);

    for q in [
        "MATCH (a:Node),(b:Other) WHERE a.value = b.value RETURN count(*)",
        "MATCH (a:Node),(b:Other) RETURN count(*)",
        "MATCH (a:Node),(b:Other) WHERE a.value = b.value RETURN a",
        "MATCH (a:Node) WHERE a.value < 5 RETURN count(*)",
        "MATCH (a:Node) RETURN count(*)",
    ] {
        let plan = query_rows(&mut serial, &format!("EXPLAIN {q}"));
        let plan_str = format!("{:?}", plan);
        let summary = plan_str
            .lines()
            .filter(|l| {
                l.contains("StorageScanVe")
                    || l.contains("| Filter")
                    || l.contains("Aggregate")
                    || l.contains("Join")
                    || l.contains("Project")
                    || l.contains("Return")
            })
            .collect::<Vec<_>>()
            .join("\n");
        println!("=== QUERY: {q}\n{summary}");
    }

    let rows = query_rows(
        &mut serial,
        "MATCH (a:Node),(b:Other) WHERE a.value = b.value RETURN count(*)",
    );
    println!(
        "=== SERIAL COUNT ROWS (len {}) === {:?}",
        rows.len(),
        rows.iter().take(5).collect::<Vec<_>>()
    );
}
