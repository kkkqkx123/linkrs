use super::common::make_table;
use crate::edge::edge_table::checkpoint::props_group_file;
use crate::edge::edge_table::config::EdgeTableConfig;
use crate::edge::edge_table::core::EdgeStore;
use crate::edge::{EdgeSchema, EdgeStrategy, RecordForm};
use crate::types::StoragePropertyDef;
use graphdb_core::Value;

#[test]
fn properties_file_skipped_when_clean() {
    let mut table = make_table();
    table.insert_edge(0, 1, 0, &[], 100).unwrap();
    let dir = tempfile::tempdir().expect("temporary edge table directory");
    table
        .flush(
            dir.path(),
            crate::compression::CompressionType::Zstd { level: 3 },
        )
        .expect("flush should succeed");
    let props = dir.path().join(props_group_file(0));
    assert!(props.exists());
    let stamp = props.metadata().unwrap().modified().unwrap();
    table
        .flush(
            dir.path(),
            crate::compression::CompressionType::Zstd { level: 3 },
        )
        .expect("second flush should succeed");
    assert_eq!(props.metadata().unwrap().modified().unwrap(), stamp);
}

#[test]
fn unpublished_column_is_dropped_on_reload() {
    let mut table = make_table();
    table
        .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.0))], 100)
        .unwrap();
    // Fill the physical column but never publish: a crash here must be
    // equivalent to aborting the staged change.
    table
        .prepare_add_property("score".to_string(), graphdb_core::DataType::Int, true, None)
        .unwrap();
    table.fill_pending_add_property().unwrap();
    assert!(table.properties.has_property("score"));
    let dir = tempfile::tempdir().expect("temporary edge table directory");
    table
        .flush(
            dir.path(),
            crate::compression::CompressionType::Zstd { level: 3 },
        )
        .expect("flush should succeed");

    let mut loaded = make_table();
    loaded.load(dir.path()).expect("load should succeed");
    assert!(!loaded.properties.has_property("score"));
    assert!(!loaded.schema.properties.iter().any(|p| p.name == "score"));
    assert!(loaded.has_edge(0, 1, 0, 200));
    assert!(loaded.pending_add_column().is_none());
}

#[test]
fn published_column_survives_reload_with_stats() {
    let mut table = make_table();
    table
        .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(3.0))], 100)
        .unwrap();
    table
        .prepare_add_property(
            "score".to_string(),
            graphdb_core::DataType::Int,
            true,
            Some(Value::Int(7)),
        )
        .unwrap();
    table.fill_pending_add_property().unwrap();
    table.publish_pending_add_property().unwrap();
    let dir = tempfile::tempdir().expect("temporary edge table directory");
    table
        .flush(
            dir.path(),
            crate::compression::CompressionType::Zstd { level: 3 },
        )
        .expect("flush should succeed");

    // A fresh table only knows the published schema when loading.
    let schema = EdgeSchema {
        label_id: 0,
        label_name: "knows".to_string(),
        src_label: 0,
        dst_label: 0,
        properties: vec![
            StoragePropertyDef::new("weight".to_string(), graphdb_core::types::DataType::Double),
            StoragePropertyDef::new("score".to_string(), graphdb_core::types::DataType::Int),
        ],
        oe_strategy: EdgeStrategy::Multiple,
        ie_strategy: EdgeStrategy::Multiple,
        schema_version: 1,
        record_form: RecordForm::default(),
    };
    let mut loaded =
        EdgeStore::with_config(schema, EdgeTableConfig::default()).expect("table builds");
    loaded.load(dir.path()).expect("load should succeed");
    assert!(loaded.properties.has_property("score"));
    let snapshot = loaded
        .column_stats_snapshot("weight")
        .expect("flushed stats should be queryable");
    assert_eq!(snapshot.row_count, 1);
    assert_eq!(snapshot.min_value, Some(Value::Double(3.0)));
    assert_eq!(snapshot.max_value, Some(Value::Double(3.0)));
}

#[test]
fn encoded_values_survive_reload_with_encoding() {
    let mut table = make_table();
    for i in 0..20 {
        table
            .insert_edge(
                i,
                i + 100,
                0,
                &[("weight".to_string(), Value::Double(i as f64))],
                100,
            )
            .unwrap();
    }
    let encoded = table.encode_property_columns();
    assert!(encoded > 0);
    let before = table.properties.column_encoding_type("weight");
    assert!(before.is_some_and(|enc| enc != crate::encoding::EncodingType::None));
    let dir = tempfile::tempdir().expect("temporary edge table directory");
    table
        .flush(
            dir.path(),
            crate::compression::CompressionType::Zstd { level: 3 },
        )
        .expect("flush should succeed");

    let mut loaded = make_table();
    loaded.load(dir.path()).expect("load should succeed");
    assert_eq!(loaded.properties.column_encoding_type("weight"), before);
    let record = loaded.get_edge(3, 103, 0, 200).expect("edge survives");
    assert!(record
        .properties
        .iter()
        .any(|(k, v)| k == "weight" && *v == Value::Double(3.0)));
    let snapshot = loaded
        .column_stats_snapshot("weight")
        .expect("flushed stats should be queryable");
    assert_eq!(snapshot.row_count, 20);
    assert!(snapshot.null_count.is_some());
}

fn make_two_prop_table() -> EdgeStore {
    let schema = EdgeSchema {
        label_id: 0,
        label_name: "knows".to_string(),
        src_label: 0,
        dst_label: 0,
        properties: vec![
            StoragePropertyDef::new("a".to_string(), graphdb_core::types::DataType::Int),
            StoragePropertyDef::new("b".to_string(), graphdb_core::types::DataType::Double),
        ],
        oe_strategy: EdgeStrategy::Multiple,
        ie_strategy: EdgeStrategy::Multiple,
        schema_version: 1,
        record_form: RecordForm::default(),
    };
    EdgeStore::with_config(schema, EdgeTableConfig::default()).expect("table builds")
}

#[test]
fn point_write_tracks_group_column_scope() {
    // Group-precise dirt: a point write to one owner records only its own
    // column, so the next flush patches that group alone and clean groups
    // reuse their files untouched.
    let mut table = make_two_prop_table();
    let props = |a: i32, b: f64| {
        vec![
            ("a".to_string(), Value::Int(a)),
            ("b".to_string(), Value::Double(b)),
        ]
    };
    table.insert_edge(0, 1, 0, &props(1, 1.5), 100).unwrap();
    table
        .insert_edge(5000, 6000, 0, &props(2, 2.5), 100)
        .unwrap();
    let dir = tempfile::tempdir().expect("temporary edge table directory");
    table
        .flush(
            dir.path(),
            crate::compression::CompressionType::Zstd { level: 3 },
        )
        .expect("baseline flush should succeed");

    let other_props = dir.path().join(props_group_file(1));
    assert!(other_props.exists());
    let stamp = other_props.metadata().unwrap().modified().unwrap();

    table
        .update_edge_property(0, 1, 0, "a", &Value::Int(7), 200)
        .expect("point write should succeed");
    let owner_zero = table.owner_gid_for(0, 1);
    let owner_other = table.owner_gid_for(5000, 6000);
    assert_ne!(owner_zero, owner_other);
    let scoped = table.property_column_dirt.get(&owner_zero);
    assert!(scoped.is_some_and(|set| set.len() == 1 && set.contains("a")));
    assert!(!table.property_column_dirt.contains_key(&owner_other));

    table
        .flush(
            dir.path(),
            crate::compression::CompressionType::Zstd { level: 3 },
        )
        .expect("incremental flush should succeed");
    assert!(
        table.property_column_dirt.is_empty(),
        "flushed group scopes must clear"
    );
    assert_eq!(
        other_props.metadata().unwrap().modified().unwrap(),
        stamp,
        "clean owner shard must be reused untouched"
    );

    let mut loaded = make_two_prop_table();
    loaded.load(dir.path()).expect("load should succeed");
    let near = loaded.get_edge(0, 1, 0, 300).expect("edge survives");
    assert!(near
        .properties
        .iter()
        .any(|(k, v)| k == "a" && *v == Value::Int(7)));
    assert!(near
        .properties
        .iter()
        .any(|(k, v)| k == "b" && *v == Value::Double(1.5)));
    let far = loaded
        .get_edge(5000, 6000, 0, 300)
        .expect("far edge survives");
    assert!(far
        .properties
        .iter()
        .any(|(k, v)| k == "a" && *v == Value::Int(2)));
    assert!(far
        .properties
        .iter()
        .any(|(k, v)| k == "b" && *v == Value::Double(2.5)));
}

#[test]
fn truncated_properties_payload_is_rejected() {
    let mut table = make_table();
    table
        .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.0))], 100)
        .unwrap();
    let payload = table.properties.dump();
    assert!(!payload.is_empty());
    let mut reloaded = make_table();
    assert!(reloaded
        .properties
        .load(&payload[..payload.len() / 2])
        .is_err());
}

#[test]
fn stable_column_ids_survive_drop_and_reload() {
    let mut table = make_table();
    table
        .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.0))], 100)
        .unwrap();
    table
        .add_property("score".to_string(), graphdb_core::DataType::Int, true)
        .expect("add score should succeed");
    let score_id = table
        .properties
        .get_property_id("score")
        .expect("score id should exist");
    table
        .remove_property("weight")
        .expect("drop should succeed");
    let dir = tempfile::tempdir().expect("temporary edge table directory");
    table
        .flush(
            dir.path(),
            crate::compression::CompressionType::Zstd { level: 3 },
        )
        .expect("flush should succeed");

    let schema = EdgeSchema {
        label_id: 0,
        label_name: "knows".to_string(),
        src_label: 0,
        dst_label: 0,
        properties: vec![StoragePropertyDef::new(
            "score".to_string(),
            graphdb_core::types::DataType::Int,
        )],
        oe_strategy: EdgeStrategy::Multiple,
        ie_strategy: EdgeStrategy::Multiple,
        schema_version: 1,
        record_form: RecordForm::default(),
    };
    let mut loaded =
        EdgeStore::with_config(schema, EdgeTableConfig::default()).expect("table builds");
    loaded.load(dir.path()).expect("load should succeed");
    assert_eq!(loaded.properties.get_property_id("score"), Some(score_id));
    loaded
        .add_property("extra".to_string(), graphdb_core::DataType::Int, true)
        .expect("add extra should succeed");
    let extra_id = loaded
        .properties
        .get_property_id("extra")
        .expect("extra id should exist");
    assert!(extra_id != score_id);
    assert!(extra_id > score_id);
}
