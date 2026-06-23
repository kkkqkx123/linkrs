use super::*;
use super::EdgeTableCore;
use crate::core::types::{VertexId, DataType};
use crate::core::Value;
use crate::storage::types::StoragePropertyDef;
use crate::storage::schema::ChangeDetails;

// Type alias for backward compatibility with existing tests
#[allow(dead_code)]
type EdgeTable = EdgeTableCore;

fn create_test_schema() -> EdgeSchema {
    EdgeSchema {
        label_id: 0,
        label_name: "knows".to_string(),
        src_label: 0,
        dst_label: 0,
        properties: vec![StoragePropertyDef::new(
            "weight".to_string(),
            DataType::Double,
        )],
        oe_strategy: EdgeStrategy::Multiple,
        ie_strategy: EdgeStrategy::Multiple,
        schema_version: 1,
    }
}

#[test]
fn test_insert_and_get() {
    let schema = create_test_schema();
    let mut table = EdgeTable::new(schema).unwrap();

    table
        .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.5))], 100)
        .unwrap();

    assert!(table.has_edge(0, 1, 0, 100));

    let edge = table.get_edge(0, 1, 0, 100).unwrap();
    assert_eq!(edge.src_vid, VertexId::from_int64(0));
    assert_eq!(edge.dst_vid, VertexId::from_int64(1));
    assert_eq!(edge.properties.len(), 1);
}

#[test]
fn test_rank_distinguishes_parallel_edges() {
    let schema = create_test_schema();
    let mut table = EdgeTable::new(schema).unwrap();

    table
        .insert_edge(0, 1, 10, &[("weight".to_string(), Value::Double(1.0))], 100)
        .unwrap();
    table
        .insert_edge(0, 1, 20, &[("weight".to_string(), Value::Double(2.0))], 100)
        .unwrap();

    let rank_10 = table.get_edge(0, 1, 10, 100).unwrap();
    let rank_20 = table.get_edge(0, 1, 20, 100).unwrap();

    assert_eq!(rank_10.rank, 10);
    assert_eq!(rank_20.rank, 20);
    assert_eq!(table.out_edges(0, 100).len(), 2);
}

#[test]
fn test_delete() {
    let schema = create_test_schema();
    let mut table = EdgeTable::new(schema).unwrap();

    table
        .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.5))], 100)
        .unwrap();

    assert!(table.delete_edge(0, 1, 0, 200).unwrap());
    assert!(!table.has_edge(0, 1, 0, 300));
}

#[test]
fn test_out_in_edges() {
    let schema = create_test_schema();
    let mut table = EdgeTable::new(schema).unwrap();

    table.insert_edge(0, 1, 0, &[], 100).unwrap();
    table.insert_edge(0, 2, 0, &[], 100).unwrap();
    table.insert_edge(1, 0, 0, &[], 100).unwrap();

    assert_eq!(table.out_edges(0, 100).len(), 2);
    assert_eq!(table.in_edges(0, 100).len(), 1);
    assert_eq!(table.out_edges(1, 100).len(), 1);
    assert_eq!(table.in_edges(1, 100).len(), 1);
}

#[test]
fn test_update_edge_property() {
    let schema = create_test_schema();
    let mut table = EdgeTable::new(schema).unwrap();

    table
        .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.0))], 100)
        .unwrap();

    let updated = table
        .update_edge_property(0, 1, 0, "weight", &Value::Double(2.0), 100)
        .unwrap();
    assert!(updated);

    let edge = table.get_edge(0, 1, 0, 100).unwrap();
    assert_eq!(edge.properties.len(), 1);
}

#[test]
fn test_self_loop_edge() {
    let schema = create_test_schema();
    let mut table = EdgeTable::new(schema).unwrap();

    table
        .insert_edge(0, 0, 0, &[("weight".to_string(), Value::Double(1.5))], 100)
        .unwrap();

    assert!(table.has_edge(0, 0, 0, 100));

    let out_edges = table.out_edges(0, 100);
    assert!(out_edges.iter().any(|edge| edge.dst_vid == VertexId::from_int64(0)));

    let in_edges = table.in_edges(0, 100);
    assert!(in_edges.iter().any(|edge| edge.src_vid == VertexId::from_int64(0)));
}

#[test]
fn test_multiple_parallel_edges() {
    let schema = create_test_schema();
    let mut table = EdgeTable::new(schema).unwrap();

    let src = 1u32;
    let dst = 2u32;

    for rank in 0..5 {
        table
            .insert_edge(
                src,
                dst,
                rank as i64,
                &[("weight".to_string(), Value::Double((rank as f64) * 0.5))],
                100,
            )
            .unwrap();
    }

    for rank in 0..5 {
        assert!(table.has_edge(src, dst, rank as i64, 100));
    }

    let edges = table.out_edges(src, 100);
    assert_eq!(edges.len(), 5);

    let incoming = table.in_edges(dst, 100);
    assert_eq!(incoming.len(), 5);
}

#[test]
fn test_edge_deletion_with_timestamps() {
    let schema = create_test_schema();
    let mut table = EdgeTable::new(schema).unwrap();

    table
        .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.5))], 100)
        .unwrap();

    assert!(table.has_edge(0, 1, 0, 100));

    let deleted = table.delete_edge(0, 1, 0, 200).unwrap();
    assert!(deleted);

    assert!(!table.has_edge(0, 1, 0, 200));
    assert!(!table.has_edge(0, 1, 0, 300));
}

#[test]
fn test_property_updates_multiple_edges() {
    let schema = create_test_schema();
    let mut table = EdgeTable::new(schema).unwrap();

    for i in 0..3 {
        table
            .insert_edge(0, 1, i as i64, &[("weight".to_string(), Value::Double(1.0))], 100)
            .unwrap();
    }

    for i in 0..3 {
        let updated = table
            .update_edge_property(
                0,
                1,
                i as i64,
                "weight",
                &Value::Double(2.0 + (i as f64)),
                100,
            )
            .unwrap();
        assert!(updated);
    }

    for i in 0..3 {
        let edge = table.get_edge(0, 1, i as i64, 100).unwrap();
        assert_eq!(edge.rank, i as i64);
    }
}

// ==================== Reverse Index Consistency ====================

#[test]
fn test_reverse_index_consistency_insert() {
    let schema = create_test_schema();
    let mut table = EdgeTable::new(schema).unwrap();

    let src = 0u32;
    let dst = 1u32;
    let rank = 10i64;
    let ts = 100u32;

    table
        .insert_edge(src, dst, rank, &[("weight".to_string(), Value::Double(2.5))], ts)
        .unwrap();

    let out = table.out_edges(src, ts);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].src_vid, VertexId::from_int64(src as i64));
    assert_eq!(out[0].dst_vid, VertexId::from_int64(dst as i64));
    assert_eq!(out[0].rank, rank);

    let in_edges = table.in_edges(dst, ts);
    assert_eq!(in_edges.len(), 1);
    assert_eq!(in_edges[0].src_vid, VertexId::from_int64(src as i64));
    assert_eq!(in_edges[0].dst_vid, VertexId::from_int64(dst as i64));
    assert_eq!(in_edges[0].rank, rank);
}

#[test]
fn test_reverse_index_consistency_delete() {
    let schema = create_test_schema();
    let mut table = EdgeTable::new(schema).unwrap();

    let src = 0u32;
    let dst = 1u32;
    let rank = 10i64;

    table
        .insert_edge(src, dst, rank, &[("weight".to_string(), Value::Double(2.5))], 100)
        .unwrap();

    let deleted = table.delete_edge(src, dst, rank, 200).unwrap();
    assert!(deleted);

    let out = table.out_edges(src, 200);
    assert_eq!(out.len(), 0);

    let in_edges = table.in_edges(dst, 200);
    assert_eq!(in_edges.len(), 0);

    let out_old = table.out_edges(src, 100);
    assert_eq!(out_old.len(), 1);

    let in_old = table.in_edges(dst, 100);
    assert_eq!(in_old.len(), 1);
}

#[test]
fn test_reverse_index_consistency_parallel_edges() {
    let schema = create_test_schema();
    let mut table = EdgeTable::new(schema).unwrap();

    let src = 0u32;
    let dst = 1u32;

    for rank in 0..3 {
        table
            .insert_edge(src, dst, rank, &[("weight".to_string(), Value::Double(rank as f64))], 100)
            .unwrap();
    }

    let out = table.out_edges(src, 100);
    assert_eq!(out.len(), 3);

    let in_edges = table.in_edges(dst, 100);
    assert_eq!(in_edges.len(), 3);

    let deleted = table.delete_edge(src, dst, 1, 200).unwrap();
    assert!(deleted);

    let out_after = table.out_edges(src, 200);
    assert_eq!(out_after.len(), 2);

    let in_after = table.in_edges(dst, 200);
    assert_eq!(in_after.len(), 2);
}

// ==================== P0 Priority Tests ====================

#[test]
fn test_p0_segment_reverse_index_sync_on_delete() {
    let schema = create_test_schema();
    let mut table = EdgeTable::new(schema).unwrap();

    let src = 5u32;
    let dst = 10u32;
    let rank = 100i64;

    table
        .insert_edge(src, dst, rank, &[("weight".to_string(), Value::Double(1.5))], 100)
        .unwrap();

    assert!(table.has_edge(src, dst, rank, 100));
    let out_before = table.out_edges(src, 100);
    let in_before = table.in_edges(dst, 100);
    assert_eq!(out_before.len(), 1);
    assert_eq!(in_before.len(), 1);

    table.freeze_csr_only(150);

    let out_after_freeze = table.out_edges(src, 150);
    let in_after_freeze = table.in_edges(dst, 150);
    assert_eq!(out_after_freeze.len(), 1);
    assert_eq!(in_after_freeze.len(), 1);

    let deleted = table.delete_edge(src, dst, rank, 200).unwrap();
    assert!(deleted);

    let out_after_delete = table.out_edges(src, 200);
    let in_after_delete = table.in_edges(dst, 200);

    assert_eq!(out_after_delete.len(), 0);
    assert_eq!(in_after_delete.len(), 0);

    let out_old = table.out_edges(src, 150);
    let in_old = table.in_edges(dst, 150);
    assert_eq!(out_old.len(), 1);
    assert_eq!(in_old.len(), 1);
}

#[test]
fn test_p0_multi_edge_segment_delete_consistency() {
    let schema = create_test_schema();
    let mut table = EdgeTable::new(schema).unwrap();

    let src = 0u32;
    let dst = 1u32;

    for rank in 0..3 {
        table
            .insert_edge(src, dst, rank, &[("weight".to_string(), Value::Double(rank as f64))], 100)
            .unwrap();
    }

    table.freeze_csr_only(150);

    assert_eq!(table.out_edges(src, 150).len(), 3);
    assert_eq!(table.in_edges(dst, 150).len(), 3);

    table.delete_edge(src, dst, 1, 200).unwrap();

    assert_eq!(table.out_edges(src, 200).len(), 2);
    assert_eq!(table.in_edges(dst, 200).len(), 2);

    table.delete_edge(src, dst, 0, 200).unwrap();

    assert_eq!(table.out_edges(src, 200).len(), 1);
    assert_eq!(table.in_edges(dst, 200).len(), 1);
}

#[test]
fn test_write_backpressure_triggers_freeze() {
    use crate::core::stats::StatsManager;
    use std::sync::Arc;

    let schema = create_test_schema();
    let mut config = EdgeTableConfig::default();
    // Set very small backpressure limit (1MB) to trigger easily
    config.max_mutable_csr_bytes = 1024 * 1024;

    let mut table = EdgeTable::with_config(schema, config).unwrap();

    // Set up stats manager to record metrics
    let stats = Arc::new(StatsManager::new());
    table.set_stats_manager(stats.clone());

    let initial_segments = table.out_segments.len();

    // Insert many edges to trigger backpressure
    for src in 0..100 {
        for dst in 0..50 {
            for rank in 0..2 {
                let _ = table.insert_edge(
                    src,
                    dst,
                    rank as i64,
                    &[("weight".to_string(), Value::Double(1.5))],
                    100,
                );
            }
        }
    }

    // Check that freeze was triggered (segments should be created)
    let final_segments = table.out_segments.len();
    assert!(
        final_segments > initial_segments,
        "Backpressure should trigger freeze and create segments"
    );

    // Verify metrics were recorded
    let freeze_count = stats.get_value(crate::core::stats::MetricType::MutableCsrFreezeCount);
    assert!(
        freeze_count.is_some() && freeze_count.unwrap() > 0,
        "Freeze count should be recorded"
    );

    let mutable_size = stats.get_value(crate::core::stats::MetricType::MutableCsrBytes);
    assert!(
        mutable_size.is_some(),
        "Mutable CSR size should be recorded"
    );
}

#[test]
fn test_write_backpressure_disabled() {
    use crate::core::stats::StatsManager;
    use std::sync::Arc;

    let schema = create_test_schema();
    let mut config = EdgeTableConfig::default();
    // Disable backpressure
    config.max_mutable_csr_bytes = 0;

    let mut table = EdgeTable::with_config(schema, config).unwrap();
    let stats = Arc::new(StatsManager::new());
    table.set_stats_manager(stats.clone());

    let initial_segments = table.out_segments.len();

    // Insert many edges
    for src in 0..50 {
        for dst in 0..50 {
            let _ = table.insert_edge(
                src,
                dst,
                0,
                &[("weight".to_string(), Value::Double(1.5))],
                100,
            );
        }
    }

    // Without backpressure, no freeze should be triggered during inserts
    // (unless it hits max_segments_per_direction limit)
    let final_segments = table.out_segments.len();

    // Verify no freeze was triggered from backpressure
    // (segments might exist from other freezes, but not from backpressure)
    let freeze_count = stats.get_value(crate::core::stats::MetricType::MutableCsrFreezeCount)
        .unwrap_or(0);
    // Should be 0 since backpressure is disabled
    assert_eq!(freeze_count, 0, "No freeze should occur when backpressure is disabled");
}

#[test]
fn test_mutable_csr_memory_size() {
    let schema = create_test_schema();
    let mut table = EdgeTable::new(schema).unwrap();

    let initial_size = table.mutable_csr_memory_size();

    // Insert an edge
    table
        .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.5))], 100)
        .unwrap();

    let after_insert_size = table.mutable_csr_memory_size();
    assert!(
        after_insert_size >= initial_size,
        "Mutable CSR size should increase after insertion"
    );

    // After freeze, mutable CSR should be cleared
    table.freeze_csr_only(150);
    let after_freeze_size = table.mutable_csr_memory_size();
    assert!(
        after_freeze_size < after_insert_size,
        "Mutable CSR size should decrease after freeze"
    );
}

#[test]
fn test_add_property_increments_version() {
    let schema = create_test_schema();
    let mut table = EdgeTable::new(schema).unwrap();

    let v1 = table.schema().schema_version;
    assert_eq!(v1, 1, "Initial version should be 1");

    table.add_property("strength".to_string(), DataType::Double, false)
        .expect("add_property should succeed");

    let v2 = table.schema().schema_version;
    assert_eq!(v2, 2, "Version should increment after add_property");
}

#[test]
fn test_remove_property_increments_version() {
    let schema = create_test_schema();
    let mut table = EdgeTable::new(schema).unwrap();

    let v1 = table.schema().schema_version;

    table.remove_property("weight")
        .expect("remove_property should succeed");

    let v2 = table.schema().schema_version;
    assert_eq!(v2, v1 + 1, "Version should increment after remove_property");
}

#[test]
fn test_rename_property_increments_version() {
    let schema = create_test_schema();
    let mut table = EdgeTable::new(schema).unwrap();

    let v1 = table.schema().schema_version;

    table.rename_property("weight", "edge_weight")
        .expect("rename_property should succeed");

    let v2 = table.schema().schema_version;
    assert_eq!(v2, v1 + 1, "Version should increment after rename_property");
}

#[test]
fn test_sequential_property_modifications() {
    let schema = create_test_schema();
    let mut table = EdgeTable::new(schema).unwrap();

    // Initial version should be 1
    assert_eq!(table.schema().schema_version, 1);

    // Add first property
    table.add_property("strength".to_string(), DataType::Double, false)
        .expect("add_property 1 should succeed");
    assert_eq!(table.schema().schema_version, 2);

    // Add second property
    table.add_property("reliability".to_string(), DataType::Double, false)
        .expect("add_property 2 should succeed");
    assert_eq!(table.schema().schema_version, 3);

    // Rename property
    table.rename_property("weight", "edge_weight")
        .expect("rename_property should succeed");
    assert_eq!(table.schema().schema_version, 4);

    // Remove property
    table.remove_property("reliability")
        .expect("remove_property should succeed");
    assert_eq!(table.schema().schema_version, 5);
}

#[test]
fn test_version_history_add_property() -> StorageResult<()> {
    let schema = EdgeSchema {
        label_id: 1,
        label_name: "Likes".to_string(),
        src_label: 1,
        dst_label: 1,
        properties: vec![],
        oe_strategy: EdgeStrategy::Multiple,
        ie_strategy: EdgeStrategy::Multiple,
        schema_version: 1,
    };
    let mut table = EdgeTableCore::with_config(schema, Default::default())?;

    let initial_version = table.schema.schema_version;
    assert_eq!(initial_version, 1);

    // Add a property
    table.add_property(
        "weight".to_string(),
        DataType::Float,
        true,
    )?;

    // Version should increment
    assert_eq!(table.schema.schema_version, 2);

    // Check version history was updated
    let history = table.version_history.lock().unwrap();
    let changes = history.change_log.get_version_changes(2);
    assert!(changes.is_some(), "Should have changes for version 2");

    let changes = changes.unwrap();
    assert_eq!(changes.len(), 1, "Should have exactly one change");

    let change = &changes[0];
    match &change.details {
        ChangeDetails::PropertyAdded { name, data_type, nullable, .. } => {
            assert_eq!(name, "weight");
            assert_eq!(*data_type, DataType::Float);
            assert_eq!(*nullable, true);
        }
        _ => panic!("Expected PropertyAdded change"),
    }

    Ok(())
}

#[test]
fn test_version_history_remove_property() -> StorageResult<()> {
    let schema = EdgeSchema {
        label_id: 1,
        label_name: "Likes".to_string(),
        src_label: 1,
        dst_label: 1,
        properties: vec![StoragePropertyDef::new("weight".to_string(), DataType::Float)],
        oe_strategy: EdgeStrategy::Multiple,
        ie_strategy: EdgeStrategy::Multiple,
        schema_version: 1,
    };
    let mut table = EdgeTableCore::with_config(schema, Default::default())?;

    // Remove the property
    table.remove_property("weight")?;

    // Version should increment to 2
    assert_eq!(table.schema.schema_version, 2);

    // Check version history was updated
    let history = table.version_history.lock().unwrap();
    let changes = history.change_log.get_version_changes(2);
    assert!(changes.is_some(), "Should have changes for version 2");

    let changes = changes.unwrap();
    assert_eq!(changes.len(), 1, "Should have exactly one change");

    let change = &changes[0];
    match &change.details {
        ChangeDetails::PropertyRemoved { name, .. } => {
            assert_eq!(name, "weight");
        }
        _ => panic!("Expected PropertyRemoved change"),
    }

    Ok(())
}

#[test]
fn test_version_history_rename_property() -> StorageResult<()> {
    let schema = EdgeSchema {
        label_id: 1,
        label_name: "Follows".to_string(),
        src_label: 1,
        dst_label: 1,
        properties: vec![StoragePropertyDef::new("weight".to_string(), DataType::Float)],
        oe_strategy: EdgeStrategy::Multiple,
        ie_strategy: EdgeStrategy::Multiple,
        schema_version: 1,
    };
    let mut table = EdgeTableCore::with_config(schema, Default::default())?;

    // Rename the property
    table.rename_property("weight", "strength")?;

    // Version should increment to 2
    assert_eq!(table.schema.schema_version, 2);

    // Check version history was updated
    let history = table.version_history.lock().unwrap();
    let changes = history.change_log.get_version_changes(2);
    assert!(changes.is_some(), "Should have changes for version 2");

    let changes = changes.unwrap();
    assert_eq!(changes.len(), 1, "Should have exactly one change");

    let change = &changes[0];
    match &change.details {
        ChangeDetails::PropertyRenamed { old_name, new_name } => {
            assert_eq!(old_name, "weight");
            assert_eq!(new_name, "strength");
        }
        _ => panic!("Expected PropertyRenamed change"),
    }

    Ok(())
}

#[test]
fn test_version_history_multiple_changes() -> StorageResult<()> {
    let schema = EdgeSchema {
        label_id: 1,
        label_name: "Interacts".to_string(),
        src_label: 1,
        dst_label: 1,
        properties: vec![],
        oe_strategy: EdgeStrategy::Multiple,
        ie_strategy: EdgeStrategy::Multiple,
        schema_version: 1,
    };
    let mut table = EdgeTableCore::with_config(schema, Default::default())?;

    // Make several changes
    table.add_property(
        "strength".to_string(),
        DataType::Float,
        true,
    )?;
    assert_eq!(table.schema.schema_version, 2);

    table.add_property(
        "frequency".to_string(),
        DataType::Int,
        false,
    )?;
    assert_eq!(table.schema.schema_version, 3);

    table.rename_property("strength", "intensity")?;
    assert_eq!(table.schema.schema_version, 4);

    // Check history
    let history = table.version_history.lock().unwrap();

    // Should have changes at versions 2, 3, and 4
    assert!(history.change_log.get_version_changes(2).is_some());
    assert!(history.change_log.get_version_changes(3).is_some());
    assert!(history.change_log.get_version_changes(4).is_some());

    Ok(())
}
