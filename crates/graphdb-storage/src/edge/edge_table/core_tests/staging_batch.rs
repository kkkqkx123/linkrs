use super::common::{create_test_schema, EdgeTable};
use crate::edge::edge_table::config::EdgeTableConfig;
use crate::edge::edge_table::core::EdgeStore;
use crate::edge::EdgeStrategy;

#[test]
fn test_staging_batch_commit_is_atomic_on_duplicate() {
    let schema = create_test_schema();
    let mut table = EdgeTable::with_config(schema, EdgeTableConfig::default()).unwrap();
    table.insert_edge(0, 1, 0, &[], 100).unwrap();
    let committed = table.edge_count();

    let mut batch = EdgeTable::staging_batch();
    batch.stage_insert(0, 2, 0, &[], 110);
    batch.stage_insert(0, 3, 0, &[], 110);
    batch.stage_insert(0, 1, 0, &[], 110);
    assert!(table.commit_staging_batch(batch).is_err());

    // The whole batch is rolled back: earlier entries leave no residue.
    assert_eq!(table.edge_count(), committed);
    assert!(!table.has_edge(0, 2, 0, 120));
    assert!(!table.has_edge(0, 3, 0, 120));
    assert!(table.has_edge(0, 1, 0, 120));
}

#[test]
fn test_staging_batch_multi_entry_commits_together() {
    let schema = create_test_schema();
    let mut table = EdgeTable::with_config(schema, EdgeTableConfig::default()).unwrap();

    let mut batch = EdgeTable::staging_batch();
    assert!(batch.is_empty());
    batch.stage_insert(0, 1, 0, &[], 100);
    batch.stage_insert(0, 2, 0, &[], 100);
    assert!(batch.contains_insert(0, 1, 0));
    batch.stage_delete(0, 1, 0, 150);
    let applied = table.commit_staging_batch(batch).unwrap();
    assert_eq!(applied, 1);
    assert!(!table.has_edge(0, 1, 0, 200));
    assert!(table.has_edge(0, 2, 0, 200));
    assert_eq!(table.mvcc.total_tombstone_count(), 0);
    assert_eq!(table.live_authority_orphans(), 0);
    assert_eq!(table.loaded_copy_mismatches(), (0, 0));
}

#[test]
fn test_batch_insert_insert_same_key_rejected() {
    let schema = create_test_schema();
    let mut table = EdgeTable::with_config(schema, EdgeTableConfig::default()).unwrap();
    let mut batch = EdgeTable::staging_batch();
    batch.stage_insert(0, 1, 0, &[], 100);
    batch.stage_insert(0, 1, 0, &[], 110);
    assert!(table.commit_staging_batch(batch).is_err());
    assert!(!table.has_edge(0, 1, 0, 200));
    assert_eq!(table.live_authority_orphans(), 0);
}

#[test]
fn test_batch_delete_insert_same_key_rebuilds() {
    let schema = create_test_schema();
    let mut table = EdgeTable::with_config(schema, EdgeTableConfig::default()).unwrap();
    table.insert_edge(0, 1, 0, &[], 100).unwrap();
    let mut batch = EdgeTable::staging_batch();
    batch.stage_delete(0, 1, 0, 150);
    batch.stage_insert(0, 1, 0, &[], 160);
    let applied = table.commit_staging_batch(batch).unwrap();
    assert_eq!(applied, 2);
    assert!(table.has_edge(0, 1, 0, 200));
    assert_eq!(table.live_authority_orphans(), 0);
    assert_eq!(table.loaded_copy_mismatches(), (0, 0));
}

#[test]
fn test_batch_insert_delete_same_key_cancels_without_tombstone() {
    let schema = create_test_schema();
    let mut table = EdgeTable::with_config(schema, EdgeTableConfig::default()).unwrap();
    let mut batch = EdgeTable::staging_batch();
    batch.stage_insert(0, 1, 0, &[], 100);
    batch.stage_delete(0, 1, 0, 150);
    let applied = table.commit_staging_batch(batch).unwrap();
    assert_eq!(applied, 0);
    assert!(!table.has_edge(0, 1, 0, 200));
    assert_eq!(table.mvcc.total_tombstone_count(), 0);
    assert_eq!(table.live_authority_orphans(), 0);
    assert_eq!(table.loaded_copy_mismatches(), (0, 0));
}

#[test]
fn test_batch_delete_delete_same_key_idempotent() {
    let schema = create_test_schema();
    let mut table = EdgeTable::with_config(schema, EdgeTableConfig::default()).unwrap();
    table.insert_edge(0, 1, 0, &[], 100).unwrap();
    let mut batch = EdgeTable::staging_batch();
    batch.stage_delete(0, 1, 0, 150);
    batch.stage_delete(0, 1, 0, 150);
    let applied = table.commit_staging_batch(batch).unwrap();
    assert_eq!(applied, 1);
    assert!(!table.has_edge(0, 1, 0, 200));
    assert_eq!(table.live_authority_orphans(), 0);
}

#[test]
fn test_batch_single_slot_second_insert_rejected() {
    let mut schema = create_test_schema();
    schema.oe_strategy = EdgeStrategy::Single;
    schema.ie_strategy = EdgeStrategy::Single;
    let mut table = EdgeTable::with_config(schema, EdgeTableConfig::default()).unwrap();
    let mut batch = EdgeTable::staging_batch();
    batch.stage_insert(0, 1, 0, &[], 100);
    batch.stage_insert(0, 2, 0, &[], 110);
    assert!(table.commit_staging_batch(batch).is_err());
    assert!(!table.has_edge(0, 1, 0, 200));
    assert!(!table.has_edge(0, 2, 0, 200));
    assert_eq!(table.live_authority_orphans(), 0);
}

#[test]
fn test_staging_prevalidate_rejects_batch_without_side_effects() {
    let schema = create_test_schema();
    let mut table = EdgeTable::with_config(schema, EdgeTableConfig::default()).unwrap();
    table.insert_edge(0, 1, 0, &[], 100).unwrap();
    let mut batch = EdgeStore::staging_batch();
    batch.stage_insert(0, 2, 0, &[], 110);
    batch.stage_insert(0, 2, 0, &[], 111);
    assert!(table.commit_staging_batch(batch).is_err());
    assert!(!table.has_edge(0, 2, 0, 120));
    assert!(table.has_edge(0, 1, 0, 120));
}
