mod common;

use graphdb_query::core::types::{PropertyDef, SpaceInfo, TagInfo, TransactionId, VertexId};
use graphdb_query::core::value::Value;
use graphdb_query::core::vertex_edge_path::{Tag, Vertex};
use graphdb_query::optimizer::stats::{StatisticsCollector, StatisticsManager};
use graphdb_query::storage::{
    GraphStorage, PropertyGraphConfig, StorageCommitOps, StorageOperationContext,
    StorageOperationContextOps, StorageSchemaOps, StorageWriter,
};
use parking_lot::RwLock;
use std::sync::Arc;

fn setup() -> Arc<RwLock<dyn graphdb_query::storage::QueryStorage>> {
    let mut raw =
        GraphStorage::new_with_config(PropertyGraphConfig::test()).expect("create storage");

    {
        let mut space = SpaceInfo::new("col_stats_e2e".to_string())
            .with_vid_type(graphdb_query::core::DataType::BigInt);
        raw.create_space(&mut space).expect("create space");
        raw.create_tag(
            "col_stats_e2e",
            &TagInfo::new("Person".to_string()).with_properties(vec![
                PropertyDef::new("name".to_string(), graphdb_query::core::DataType::String),
                PropertyDef::new("age".to_string(), graphdb_query::core::DataType::BigInt),
            ]),
        )
        .expect("create tag");
    }

    {
        let mut writer = raw.bind_operation_context(StorageOperationContext::transaction(
            TransactionId::from(1),
            10,
            false,
        ));
        for i in 1..=200i64 {
            writer
                .insert_vertex(
                    "col_stats_e2e",
                    Vertex::new(
                        VertexId::from_int64(i),
                        vec![Tag::new(
                            "Person".to_string(),
                            [
                                ("name".to_string(), Value::string(format!("P{i}"))),
                                ("age".to_string(), Value::BigInt(i)),
                            ]
                            .into_iter()
                            .collect(),
                        )],
                    ),
                )
                .expect("insert vertex");
        }
        drop(writer);
        raw.commit_staged_writes(TransactionId::from(1), &[])
            .expect("commit");
    }

    Arc::new(RwLock::new(raw))
}

#[test]
fn snapshot_overrides_sampled_envelope_for_vertex_property() {
    let storage = setup();
    let manager = StatisticsManager::new();

    let summary = StatisticsCollector::collect_space(&manager, &storage, "col_stats_e2e", 1, 1, 50)
        .expect("collect_space");

    assert_eq!(summary.tags, 1);
    assert!(!summary.cached);

    let age = manager
        .get_property_stats("col_stats_e2e", Some("Person"), "age")
        .expect("age stats should exist");

    assert_eq!(age.min_value, Some(Value::BigInt(1)));
    assert_eq!(age.max_value, Some(Value::BigInt(200)));
}

#[test]
fn cache_hit_on_second_collect_with_same_stamp() {
    let storage = setup();
    let manager = StatisticsManager::new();

    let first = StatisticsCollector::collect_space(&manager, &storage, "col_stats_e2e", 1, 1, 50)
        .expect("collect_space 1");
    assert!(!first.cached);

    let second = StatisticsCollector::collect_space(&manager, &storage, "col_stats_e2e", 1, 1, 50)
        .expect("collect_space 2");
    assert!(second.cached);
}

#[test]
fn cache_invalidation_on_epoch_bump() {
    let storage = setup();
    let manager = StatisticsManager::new();

    let first = StatisticsCollector::collect_space(&manager, &storage, "col_stats_e2e", 1, 1, 50)
        .expect("collect_space epoch=1");
    assert!(!first.cached);

    let second = StatisticsCollector::collect_space(&manager, &storage, "col_stats_e2e", 1, 2, 50)
        .expect("collect_space epoch=2");
    assert!(!second.cached);
}
