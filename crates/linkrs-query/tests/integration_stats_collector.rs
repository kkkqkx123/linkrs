mod common;

use linkrs_core::types::{PropertyDef, SpaceInfo, TagInfo, TransactionId, VertexId};
use linkrs_core::value::Value;
use linkrs_core::vertex_edge_path::{Tag, Vertex};
use linkrs_query::optimizer::stats::{StatisticsCollector, StatisticsManager};
use linkrs_query::storage::{
    GraphStorage, PropertyGraphConfig, StorageCommitOps, StorageOperationContext,
    StorageOperationContextOps, StorageSchemaOps, StorageWriter,
};
use parking_lot::RwLock;
use std::sync::Arc;

fn setup() -> Arc<RwLock<dyn linkrs_query::storage::QueryStorage>> {
    let mut raw =
        GraphStorage::new_with_config(PropertyGraphConfig::test()).expect("create storage");

    {
        let mut space = SpaceInfo::new("col_stats_e2e".to_string())
            .with_vid_type(linkrs_core::DataType::BigInt);
        raw.create_space(&mut space).expect("create space");
        raw.create_tag(
            "col_stats_e2e",
            &TagInfo::new("Person".to_string()).with_properties(vec![
                PropertyDef::new("id".to_string(), linkrs_core::DataType::BigInt),
                PropertyDef::new("name".to_string(), linkrs_core::DataType::String),
                PropertyDef::new("age".to_string(), linkrs_core::DataType::BigInt),
            ]),
        )
        .expect("create tag");
    }

    {
        let mut writer =
            raw.bind_operation_context(StorageOperationContext::transaction_with_timestamps(
                TransactionId::from(1),
                10,
                Some(10),
                false,
                false,
            ));
        for i in 1..=200i64 {
            writer
                .insert_vertex(
                    "col_stats_e2e",
                    Vertex::new(
                        VertexId::try_from_int64(i).expect("test vertex id"),
                        Tag::new(
                            "Person".to_string(),
                            [
                                ("id".to_string(), Value::BigInt(i)),
                                ("name".to_string(), Value::string(format!("P{i}"))),
                                ("age".to_string(), Value::BigInt(i)),
                            ]
                            .into_iter()
                            .map(|(name, value)| (Arc::from(name.as_str()), value))
                            .collect(),
                        ),
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
