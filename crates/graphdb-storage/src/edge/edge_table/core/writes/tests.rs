use super::super::EdgeStore;
use crate::edge::edge_table::config::EdgeTableConfig;
use crate::edge::edge_table::staging::EdgeStagingBatch;
use crate::edge::{EdgeSchema, EdgeStrategy, RecordForm};
use crate::types::StoragePropertyDef;
use graphdb_core::types::DataType;
use graphdb_core::Value;

fn batch_table() -> EdgeStore {
    let schema = EdgeSchema {
        label_id: 0,
        label_name: "knows".to_string(),
        src_label: 0,
        dst_label: 0,
        properties: vec![StoragePropertyDef {
            name: "weight".to_string(),
            data_type: DataType::Double,
            nullable: false,
            default_value: Some(Value::Double(0.0)),
        }],
        oe_strategy: EdgeStrategy::Multiple,
        ie_strategy: EdgeStrategy::Multiple,
        schema_version: 1,
        record_form: RecordForm::default(),
    };
    EdgeStore::with_config(schema, EdgeTableConfig::default()).unwrap()
}

#[test]
fn committed_batch_replays_whole_after_crash() {
    let dir = tempfile::tempdir().expect("temporary edge table directory");
    let mut table = batch_table();
    table
        .flush(
            dir.path(),
            crate::compression::CompressionType::Zstd { level: 3 },
        )
        .expect("checkpoint truncates WAL");

    let mut batch = EdgeStagingBatch::new();
    for dst in 1..=8u32 {
        batch.stage_insert(
            0,
            dst,
            0,
            &[("weight".to_string(), Value::Double(dst as f64))],
            100,
        );
    }
    assert_eq!(table.commit_staging_batch(batch).unwrap(), 8);

    // Crash without a second checkpoint: reload replays the WAL.
    let mut recovered = batch_table();
    recovered.load(dir.path()).expect("load succeeds");
    for dst in 1..=8u32 {
        assert!(
            recovered.has_edge(0, dst, 0, 200),
            "replayed batch keeps edge 0 -> {}",
            dst
        );
    }
    // No single-direction residue and no ownerless timestamps.
    assert!(recovered.audit_copy_drift().is_empty());
    assert_eq!(recovered.out_edges(0, 200).len(), 8);
}

#[test]
fn failed_batch_leaves_no_visible_residue() {
    let dir = tempfile::tempdir().expect("temporary edge table directory");
    let mut table = batch_table();
    table
        .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.0))], 100)
        .unwrap();
    table
        .flush(
            dir.path(),
            crate::compression::CompressionType::Zstd { level: 3 },
        )
        .expect("checkpoint truncates WAL");

    // The second insert duplicates a committed edge: prevalidation fails
    // the whole batch before the WAL append, so nothing applies.
    let mut batch = EdgeStagingBatch::new();
    batch.stage_insert(0, 2, 0, &[], 100);
    batch.stage_insert(0, 3, 0, &[], 100);
    batch.stage_insert(0, 1, 0, &[], 100);
    assert!(table.commit_staging_batch(batch).is_err());
    assert!(!table.has_edge(0, 2, 0, 200));
    assert!(!table.has_edge(0, 3, 0, 200));
    assert!(table.audit_copy_drift().is_empty());

    // Reload proves the failed batch never reached the WAL either.
    let mut recovered = batch_table();
    recovered.load(dir.path()).expect("load succeeds");
    assert!(!recovered.has_edge(0, 2, 0, 200));
    assert!(!recovered.has_edge(0, 3, 0, 200));
    assert!(recovered.has_edge(0, 1, 0, 200));
}

#[test]
fn large_delete_fanout_fails_atomically() {
    let mut table = batch_table();
    for dst in 1..=200u32 {
        table
            .insert_edge(
                0,
                dst,
                0,
                &[("weight".to_string(), Value::Double(1.0))],
                100,
            )
            .unwrap();
    }
    // A committed edge outside the fanout: duplicating it fails the whole
    // fanout batch. (An insert of a key deleted earlier in the same batch
    // would rebuild instead of failing, so the conflict must target a key
    // the batch does not delete.)
    table
        .insert_edge(5, 6, 0, &[("weight".to_string(), Value::Double(9.0))], 100)
        .unwrap();

    // No half-deleted graph: every fanout edge still visible in both
    // directions after the failed commit.
    let mut bad = EdgeStagingBatch::new();
    for dst in 1..=200u32 {
        bad.stage_delete(0, dst, 0, 150);
    }
    bad.stage_insert(5, 6, 0, &[], 150);
    assert!(table.commit_staging_batch(bad).is_err());
    assert_eq!(table.out_edges(0, 200).len(), 200);
    for dst in 1..=200u32 {
        assert!(table.has_edge(0, dst, 0, 200));
        assert!(
            table
                .in_edges(dst, 200)
                .iter()
                .any(|e| e.src_vid.as_int64() == Some(0)),
            "in-direction of {} keeps the fanout edge",
            dst
        );
    }
    assert!(table.has_edge(5, 6, 0, 200));
    assert!(table.audit_copy_drift().is_empty());

    // The pure fanout batch deletes symmetrically: no out row without its
    // in row and no timestamp without topology.
    let mut good = EdgeStagingBatch::new();
    for dst in 1..=200u32 {
        good.stage_delete(0, dst, 0, 160);
    }
    assert_eq!(table.commit_staging_batch(good).unwrap(), 200);
    assert!(table.out_edges(0, 200).is_empty());
    for dst in 1..=200u32 {
        assert!(
            table
                .in_edges(dst, 200)
                .iter()
                .all(|e| e.src_vid.as_int64() != Some(0)),
            "no in-direction residue of the fanout at {}",
            dst
        );
    }
    assert!(table.audit_copy_drift().is_empty());
}

#[test]
fn incident_vertex_delete_commits_fanout_in_one_batch() {
    let mut table = batch_table();
    for dst in 1..=50u32 {
        table
            .insert_edge(
                0,
                dst,
                0,
                &[("weight".to_string(), Value::Double(dst as f64))],
                100,
            )
            .unwrap();
    }
    for src in 51..=60u32 {
        table
            .insert_edge(
                src,
                0,
                0,
                &[("weight".to_string(), Value::Double(src as f64))],
                100,
            )
            .unwrap();
    }
    table
        .insert_edge(
            70,
            71,
            0,
            &[("weight".to_string(), Value::Double(9.0))],
            100,
        )
        .unwrap();

    // One call removes both directions: each logical edge counts once even
    // though it is stored on both legs.
    let deleted = table
        .delete_incident_edges_of_vertex(Some(0), Some(0), 150)
        .unwrap();
    assert_eq!(deleted.len(), 60);
    assert!(
        deleted.windows(2).all(|w| w[0].edge_id.0 <= w[1].edge_id.0),
        "deleted edges report in edge-id order"
    );
    let hub_out = deleted
        .iter()
        .find(|e| e.src == 0 && e.dst == 5)
        .expect("outgoing fanout edge reported");
    assert_eq!(
        hub_out.properties,
        vec![("weight".to_string(), Value::Double(5.0))]
    );
    let hub_in = deleted
        .iter()
        .find(|e| e.src == 55 && e.dst == 0)
        .expect("incoming fanout edge reported");
    assert_eq!(
        hub_in.properties,
        vec![("weight".to_string(), Value::Double(55.0))]
    );

    assert!(table.out_edges(0, 200).is_empty());
    assert!(table.in_edges(0, 200).is_empty());
    for dst in 1..=50u32 {
        assert!(!table.has_edge(0, dst, 0, 200));
    }
    for src in 51..=60u32 {
        assert!(!table.has_edge(src, 0, 0, 200));
    }
    assert!(table.has_edge(70, 71, 0, 200));
    assert!(table.audit_copy_drift().is_empty());

    // A second call over the emptied vertex is a no-op success.
    assert!(table
        .delete_incident_edges_of_vertex(Some(0), Some(0), 160)
        .unwrap()
        .is_empty());
}

#[test]
fn incident_vertex_delete_handles_single_direction_tables() {
    let mut schema = batch_table().schema().clone();
    schema.ie_strategy = EdgeStrategy::None;
    let mut table =
        EdgeStore::with_config(schema, EdgeTableConfig::default()).expect("out-only table builds");
    for dst in 1..=5u32 {
        table
            .insert_edge(
                0,
                dst,
                0,
                &[("weight".to_string(), Value::Double(1.0))],
                100,
            )
            .unwrap();
    }
    let deleted = table
        .delete_incident_edges_of_vertex(Some(0), Some(0), 150)
        .unwrap();
    assert_eq!(deleted.len(), 5);
    assert!(table.out_edges(0, 200).is_empty());
    assert!(table.audit_copy_drift().is_empty());
}

#[test]
fn incident_vertex_delete_fails_closed_on_frozen_group() {
    use graphdb_core::types::Timestamp;
    let mut table = batch_table();
    table
        .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.0))], 100)
        .unwrap();
    table
        .insert_edge(0, 2, 0, &[("weight".to_string(), Value::Double(2.0))], 100)
        .unwrap();
    table
        .freeze_group(true, 0, Timestamp::MAX, 0.0)
        .expect("out leg freezes");
    assert!(table
        .delete_incident_edges_of_vertex(Some(0), None, 150)
        .is_err());
    // The failed batch leaves the table untouched: frozen reads still serve
    // both edges with no copy drift.
    assert!(table.has_edge(0, 1, 0, 200));
    assert!(table.has_edge(0, 2, 0, 200));
    assert!(table.audit_copy_drift().is_empty());
}

#[test]
fn owner_rebuild_converges_reclaimed_tombstones_with_count() {
    let mut table = batch_table();
    table
        .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.0))], 100)
        .unwrap();
    table
        .insert_edge(0, 2, 0, &[("weight".to_string(), Value::Double(2.0))], 100)
        .unwrap();
    assert!(table.delete_edge(0, 1, 0, 150).unwrap());

    // Reclaim the physical rows while the authority tombstone survives.
    let watermarks = graphdb_transaction::MvccWatermarks::from_parts(
        200,
        200,
        None,
        graphdb_core::types::CommitLsn::ZERO,
    );
    table.compact_csr_only_with_watermarks(&watermarks, 0, 0.2);
    let stats = table.rebuild_owner_map_with_stats();
    assert_eq!(
        stats.mapped, 1,
        "one surviving topology edge maps to group 0"
    );
    assert_eq!(
        stats.relocated_orphans, 1,
        "the reclaimed tombstone converges with a count"
    );
    assert_eq!(stats.fallback_group, 0);
    assert!(table.audit_copy_drift().is_empty());
}

#[test]
fn reclaim_boundary_stamp_uses_shared_gc_predicate() {
    // `delete_ts == watermark` is eligible everywhere (shared predicate),
    // so authority reclaim and CSR reclaim agree at the boundary instead
    // of drifting by one round.
    assert!(crate::mvcc_visibility::Visibility::is_gc_eligible(200, 200));
    let mut table = batch_table();
    table
        .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.0))], 100)
        .unwrap();
    assert!(table.delete_edge(0, 1, 0, 200).unwrap());
    let watermarks = graphdb_transaction::MvccWatermarks::from_parts(
        200,
        200,
        None,
        graphdb_core::types::CommitLsn::ZERO,
    );
    table.compact_csr_only_with_watermarks(&watermarks, 0, 0.2);
    assert!(
        table
            .mvcc
            .edge_timestamps
            .get(&graphdb_core::types::EdgeId(0))
            .is_some(),
        "authority tombstones survive physical reclaim"
    );
    assert!(table.audit_copy_drift().is_empty());
}
