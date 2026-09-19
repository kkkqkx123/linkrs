use graphdb_core::types::{EdgeId, VertexId};

use super::super::{CsrBase, MutableCsrTrait};
use super::*;

fn multi_set() -> CsrShardSet {
    CsrShardSet::new(EdgeStrategy::Multiple, DEFAULT_NODE_GROUP_BITS, 4096).unwrap()
}

fn endpoint(dst: u32, rank: i64) -> VertexId {
    VertexId::edge_endpoint_key(dst, rank)
}

#[test]
fn group_mapping_splits_at_group_boundary() {
    assert_eq!(group_id_for(0, 12), 0);
    assert_eq!(group_id_for(4095, 12), 0);
    assert_eq!(group_id_for(4096, 12), 1);
    assert_eq!(local_vid(4096, 12), 0);
    assert_eq!(local_vid(5000, 12), 904);
    assert_eq!(group_base(1, 12), 4096);
}

#[test]
fn invalid_group_bits_rejected() {
    assert!(CsrShardSet::new(EdgeStrategy::Multiple, 0, 4096).is_err());
    assert!(CsrShardSet::new(EdgeStrategy::Multiple, 21, 4096).is_err());
}

#[test]
fn cross_group_insert_and_read() {
    let mut set = multi_set();
    set.insert_edge(0, endpoint(1, 0), EdgeId(0), 100).unwrap();
    set.insert_edge(5000, endpoint(6000, 1), EdgeId(1), 100)
        .unwrap();
    assert_eq!(set.group_count(), 2);
    assert_eq!(set.edge_count(), 2);
    assert!(set.get_edge(0, endpoint(1, 0), 200).is_some());
    assert!(set.get_edge(5000, endpoint(6000, 1), 200).is_some());
    assert_eq!(set.edges_of(0, 200).len(), 1);
    assert_eq!(set.edges_of(5000, 200).len(), 1);
    assert!(set.edges_of(1, 200).is_empty());
}

#[test]
fn reads_never_create_groups() {
    let set = multi_set();
    assert_eq!(set.group_count(), 1);
    assert!(set.get_edge(9000, endpoint(1, 0), 200).is_none());
    assert!(set.edges_of(9000, 200).is_empty());
    assert_eq!(set.group_count(), 1);
}

#[test]
fn dirty_tracking_per_group() {
    let mut set = multi_set();
    assert!(set.dirty_group_ids().is_empty());
    set.insert_edge(5000, endpoint(1, 0), EdgeId(0), 100)
        .unwrap();
    assert_eq!(set.dirty_group_ids(), vec![1]);
    assert!(!set.needs_checkpoint(0));
    assert!(set.needs_checkpoint(1));
    set.clear_group_dirty(1);
    assert!(set.dirty_group_ids().is_empty());
}

#[test]
fn delete_marks_group_dirty() {
    let mut set = multi_set();
    set.insert_edge(0, endpoint(1, 0), EdgeId(0), 100).unwrap();
    set.clear_all_dirty();
    assert!(set.delete_edge(0, EdgeId(0), 150).unwrap());
    assert_eq!(set.dirty_group_ids(), vec![0]);
}

#[test]
fn sharded_iter_translates_to_global_rows() {
    let mut set = multi_set();
    set.insert_edge(0, endpoint(1, 0), EdgeId(0), 100).unwrap();
    set.insert_edge(5000, endpoint(2, 0), EdgeId(1), 100)
        .unwrap();
    let rows: Vec<(i64, EdgeId)> = set
        .iter(200)
        .map(|(src, nbr)| (src.as_int64().unwrap_or(-1), nbr.edge_id))
        .collect();
    assert_eq!(rows.len(), 2);
    assert!(rows.contains(&(0, EdgeId(0))));
    assert!(rows.contains(&(5000, EdgeId(1))));
}

#[test]
fn container_dump_load_roundtrip() {
    let mut set = multi_set();
    set.insert_edge(0, endpoint(1, 0), EdgeId(0), 100).unwrap();
    set.insert_edge(5000, endpoint(2, 0), EdgeId(1), 100)
        .unwrap();
    // Mirror the production load order: materialize the manifest group
    // list first, then fill each group payload.
    let mut loaded = multi_set();
    let gids: Vec<u32> = set
        .existing_group_ids()
        .into_iter()
        .map(|gid| gid as u32)
        .collect();
    loaded.set_groups(&gids).unwrap();
    for gid in set.existing_group_ids() {
        let payload = set.group_variant(gid).expect("group must exist").dump();
        loaded.load_group(gid, &payload).unwrap();
    }
    assert_eq!(loaded.group_count(), 2);
    assert_eq!(loaded.edge_count(), 2);
    assert!(loaded.get_edge(5000, endpoint(2, 0), 200).is_some());
}

#[test]
fn manifest_rejects_bad_version_and_trailing() {
    let manifest = TableShardManifest {
        group_bits: 12,
        out_groups: vec![0, 1],
        in_groups: vec![0],
    };
    let payload = manifest.encode();
    assert_eq!(TableShardManifest::decode(&payload).unwrap(), manifest);
    let mut bad = payload.clone();
    bad[0] = 99;
    assert!(TableShardManifest::decode(&bad).is_err());
    let mut trailing = payload.clone();
    trailing.push(0);
    assert!(TableShardManifest::decode(&trailing).is_err());
    assert!(TableShardManifest::decode(&payload[..8]).is_err());
}

#[test]
fn manifest_v3_counts_are_rejected() {
    let mut legacy = Vec::new();
    legacy.extend_from_slice(&3u32.to_le_bytes());
    legacy.extend_from_slice(&12u32.to_le_bytes());
    legacy.extend_from_slice(&2u32.to_le_bytes());
    legacy.extend_from_slice(&1u32.to_le_bytes());
    assert!(TableShardManifest::decode(&legacy).is_err());
}

#[test]
fn sparse_holes_stay_absent_until_written() {
    let mut set = multi_set();
    set.insert_edge(9000, endpoint(1, 0), EdgeId(0), 100)
        .unwrap();
    assert_eq!(set.existing_group_ids(), vec![0, 2]);
    assert_eq!(set.group_count(), 2);
    assert!(set.get_edge(5000, endpoint(9, 0), 200).is_none());
    assert_eq!(set.existing_group_ids(), vec![0, 2]);
    assert!(set.edges_of(5000, 200).is_empty());
    assert_eq!(set.existing_group_ids(), vec![0, 2]);
}

#[test]
fn fill_physical_into_matches_allocating_accessor() {
    let mut set = multi_set();
    set.insert_edge(0, endpoint(1, 0), EdgeId(0), 100).unwrap();
    set.insert_edge(0, endpoint(2, 0), EdgeId(1), 100).unwrap();
    set.insert_edge(5000, endpoint(3, 0), EdgeId(2), 100)
        .unwrap();
    let mut buf = Vec::new();
    for vid in [0u32, 1, 5000, 9000] {
        set.fill_physical_into(vid, &mut buf);
        assert_eq!(buf, set.physical_edges_of(vid));
    }
}

#[test]
fn fill_physical_batch_slices_per_vertex() {
    let mut set = multi_set();
    set.insert_edge(0, endpoint(1, 0), EdgeId(0), 100).unwrap();
    set.insert_edge(0, endpoint(2, 0), EdgeId(1), 100).unwrap();
    set.insert_edge(5000, endpoint(3, 0), EdgeId(2), 100)
        .unwrap();
    let vids = vec![0u32, 1, 5000, 9000];
    let mut out = Vec::new();
    let mut offsets = Vec::new();
    set.fill_physical_batch_into(&vids, &mut out, &mut offsets);
    assert_eq!(offsets.len(), vids.len() + 1);
    assert_eq!(*offsets.last().unwrap(), out.len());
    for (i, vid) in vids.iter().enumerate() {
        assert_eq!(out[offsets[i]..offsets[i + 1]], set.physical_edges_of(*vid));
    }
    assert_eq!(set.existing_group_ids(), vec![0, 1]);
}

#[test]
fn calibrator_density_grades_downward() {
    let height = calibrator_tree_height(DEFAULT_NODE_GROUP_BITS);
    assert_eq!(height, 4);
    assert!((calibrator_max_density(0, height) - 1.0).abs() < f32::EPSILON);
    assert!((calibrator_max_density(height, height) - 0.8).abs() < 1e-6);
    let mut prev = f32::INFINITY;
    for level in 0..=height {
        let density = calibrator_max_density(level, height);
        assert!(density <= prev);
        prev = density;
    }
    assert!((calibrator_max_density(height + 2, height) - 0.8).abs() < 1e-6);
}

#[test]
fn region_tail_gap_follows_packed_density() {
    assert_eq!(region_tail_gap(0), 0);
    assert_eq!(region_tail_gap(8), 2);
    assert_eq!(region_tail_gap(4), 1);
}

#[test]
fn region_live_and_gap_agree_with_census() {
    let mut set = multi_set();
    set.insert_edge(0, endpoint(1, 0), EdgeId(0), 100).unwrap();
    set.insert_edge(1, endpoint(2, 0), EdgeId(1), 100).unwrap();
    let (live, _, capacity) = set.region_census(0, 0);
    assert_eq!(set.region_live(0, 0), live);
    assert_eq!(set.region_gap(0, 0), capacity.saturating_sub(live));
}

#[test]
fn sparse_span_covers_holes_while_capacity_counts_materialized() {
    let mut set = multi_set();
    set.insert_edge(9000, endpoint(1, 0), EdgeId(0), 100)
        .unwrap();
    assert_eq!(set.existing_group_ids(), vec![0, 2]);
    let group_size = set.group_size();
    assert_eq!(set.vertex_capacity(), 2 * group_size);
    assert_eq!(set.address_span_rows(), 3 * group_size);
    assert!(set.address_span_rows() > set.vertex_capacity());
}

#[test]
fn none_strategy_holds_no_groups() {
    let mut set = CsrShardSet::new(EdgeStrategy::None, 12, 4096).unwrap();
    assert_eq!(set.group_count(), 0);
    assert_eq!(set.vertex_capacity(), 0);
    assert!(set.insert_edge(0, endpoint(1, 0), EdgeId(0), 100).is_err());
    assert!(set.delete_edge(0, EdgeId(0), 100).is_err());
    assert_eq!(set.delete_edge_by_dst(0, endpoint(1, 0), 100), 0);
}

#[test]
fn truncate_drops_trailing_empty_groups() {
    let mut set = multi_set();
    set.insert_edge(9000, endpoint(1, 0), EdgeId(0), 100)
        .unwrap();
    assert_eq!(set.existing_group_ids(), vec![0, 2]);
    assert!(set.remove_edge(9000, EdgeId(0)));
    set.truncate_trailing_empty_groups();
    assert_eq!(set.group_count(), 1);
}

#[test]
fn truncate_keeps_tombstone_only_tail_groups() {
    let mut set = multi_set();
    set.insert_edge(9000, endpoint(1, 0), EdgeId(0), 100)
        .unwrap();
    assert_eq!(set.existing_group_ids(), vec![0, 2]);
    assert!(set.delete_edge(9000, EdgeId(0), 150).unwrap());
    assert_eq!(set.edge_count(), 0);
    set.truncate_trailing_empty_groups();
    assert_eq!(set.group_count(), 1);
    assert_eq!(set.existing_group_ids(), vec![2]);
}

#[test]
fn offset_delete_rejects_out_of_degree() {
    let mut set = multi_set();
    set.insert_edge(0, endpoint(1, 0), EdgeId(0), 100).unwrap();
    assert!(!set.delete_edge_by_offset(0, 5, 150).unwrap());
    assert!(set.delete_edge_by_offset(0, 0, 150).unwrap());
}

#[test]
fn physical_reads_ignore_timestamps() {
    let mut set = multi_set();
    set.insert_edge(0, endpoint(1, 0), EdgeId(0), 100).unwrap();
    assert!(set.delete_edge(0, EdgeId(0), 150).unwrap());
    assert!(set.get_edge_physical(0, endpoint(1, 0)).is_none());
    assert_eq!(set.physical_edges_of(0).len(), 1);
    assert!(set.has_physical_entries(0));
}

#[test]
fn column_dirt_does_not_force_topology_checkpoint() {
    let mut set = multi_set();
    set.insert_edge(0, endpoint(1, 0), EdgeId(0), 100).unwrap();
    set.clear_all_dirty();
    set.mark_column_updated_for(0);
    assert!(set.column_dirty_group_ids() == vec![0]);
    assert!(set.dirty_group_ids().is_empty());
    assert!(!set.needs_checkpoint(0));
    assert_eq!(set.checkpoint_kind(), EdgeCheckpointKind::AppendOnly);
}

#[test]
fn column_trace_materializes_missing_owner_group() {
    let mut set = multi_set();
    assert_eq!(set.group_count(), 1);
    set.mark_column_updated_for(9000);
    // Precise write-time marking never drops a trace: the owning group
    // is materialized so the property-only write reaches the flush set.
    assert_eq!(set.column_dirty_group_ids(), vec![2]);
    assert_eq!(set.dirty_group_ids(), Vec::<usize>::new());
}

#[test]
fn checkpoint_kind_turns_rebalance_on_delete_dirt() {
    let mut set = multi_set();
    set.insert_edge(0, endpoint(1, 0), EdgeId(0), 100).unwrap();
    assert_eq!(set.checkpoint_kind(), EdgeCheckpointKind::AppendOnly);
    assert!(set.delete_edge(0, EdgeId(0), 150).unwrap());
    assert_eq!(set.checkpoint_kind(), EdgeCheckpointKind::Rebalance);
    set.clear_all_dirty();
    assert_eq!(set.checkpoint_kind(), EdgeCheckpointKind::AppendOnly);
    set.mark_all_dirty();
    assert_eq!(set.checkpoint_kind(), EdgeCheckpointKind::Rebalance);
}

fn narrow_set() -> CsrShardSet {
    CsrShardSet::new(EdgeStrategy::Multiple, 9, 4096).unwrap()
}

#[test]
fn region_dirt_tracks_per_region_inserts_and_deletes() {
    let mut set = narrow_set();
    assert_eq!(regions_per_group(set.group_size()), 2);
    set.insert_edge(10, endpoint(1, 0), EdgeId(0), 100).unwrap();
    set.insert_edge(300, endpoint(2, 0), EdgeId(1), 100)
        .unwrap();
    assert_eq!(set.dirty_region_ids(0), vec![0, 1]);
    assert!(set.region_needs_checkpoint(0, 0));
    assert!(set.region_needs_checkpoint(0, 1));
    assert!(!set.region_needs_rebalance(0, 0));

    set.clear_region_dirty(0, 0);
    assert!(!set.region_needs_checkpoint(0, 0));
    assert!(set.region_needs_checkpoint(0, 1));
    assert_eq!(set.dirty_group_ids(), vec![0]);

    set.clear_region_dirty(0, 1);
    assert!(set.dirty_group_ids().is_empty());
    assert!(!set.needs_checkpoint(0));
}

#[test]
fn region_delete_dirt_drives_rebalance_signal() {
    let mut set = narrow_set();
    set.insert_edge(10, endpoint(1, 0), EdgeId(0), 100).unwrap();
    set.clear_all_dirty();
    assert!(set.delete_edge(10, EdgeId(0), 150).unwrap());
    assert!(set.region_needs_rebalance(0, 0));
    assert!(!set.region_needs_rebalance(0, 1));
    assert!(set.group_needs_rebalance(0));
}

#[test]
fn region_census_and_density_observe_rows() {
    let mut set = narrow_set();
    for i in 0..8u32 {
        set.insert_edge(i, endpoint(100 + i, 0), EdgeId(i as u64), 100)
            .unwrap();
    }
    let (live, dead, capacity) = set.region_census(0, 0);
    assert_eq!(live, 8);
    assert_eq!(dead, 0);
    assert!(capacity >= live);
    let density = set.region_density(0, 0);
    assert!((density - live as f32 / capacity as f32).abs() < 1e-6);
    let (live_empty, _, _) = set.region_census(0, 1);
    assert_eq!(live_empty, 0);
    assert_eq!(set.region_density(0, 1), 1.0);
}

#[test]
fn merge_scope_trigger_stays_per_row_reclaimable() {
    let mut set = narrow_set();
    set.insert_edge(10, endpoint(1, 0), EdgeId(0), 100).unwrap();
    // No tombstone and no delete dirt: nothing to merge at any cutoff.
    assert_eq!(
        set.select_merge_scope(0, 0, 200, REGION_MERGE_MIN_DENSITY, GROUP_MERGE_MIN_DENSITY),
        None
    );
    assert!(set.delete_edge(10, EdgeId(0), 150).unwrap());
    // Delete dirt alone selects a scope; density only widens it.
    let scope =
        set.select_merge_scope(0, 0, 140, REGION_MERGE_MIN_DENSITY, GROUP_MERGE_MIN_DENSITY);
    assert!(scope.is_some());
    // Below the deletion stamp nothing is reclaimable and the region
    // carries delete dirt, so the narrow row scope holds.
    assert_eq!(
        set.select_merge_scope(0, 0, 140, REGION_MERGE_MIN_DENSITY, GROUP_MERGE_MIN_DENSITY),
        Some(RegionMergeScope::Row)
    );
}

#[test]
fn merge_scope_thresholds_drive_widening() {
    let mut set = narrow_set();
    set.insert_edge(10, endpoint(1, 0), EdgeId(0), 100).unwrap();
    set.insert_edge(11, endpoint(2, 0), EdgeId(1), 100).unwrap();
    assert!(set.delete_edge(10, EdgeId(0), 150).unwrap());
    // A zero region bar widens to the region scope on density alone.
    assert_eq!(
        set.select_merge_scope(0, 0, 200, 0.0, GROUP_MERGE_MIN_DENSITY),
        Some(RegionMergeScope::Region)
    );
    // An unreachable region bar keeps the narrow row scope.
    assert_eq!(
        set.select_merge_scope(0, 0, 200, 1.0, GROUP_MERGE_MIN_DENSITY),
        Some(RegionMergeScope::Row)
    );
}

#[test]
fn compact_region_is_scoped_to_its_window() {
    let mut set = narrow_set();
    set.insert_edge(10, endpoint(1, 0), EdgeId(0), 100).unwrap();
    set.insert_edge(300, endpoint(2, 0), EdgeId(1), 100)
        .unwrap();
    assert!(set.delete_edge(10, EdgeId(0), 150).unwrap());
    assert!(set.delete_edge(300, EdgeId(1), 150).unwrap());
    let mut reported = Vec::new();
    let removed =
        set.compact_region_with_reporting(0, 0, 200, &mut |id, ts| reported.push((id, ts)));
    assert_eq!(removed, 1);
    assert_eq!(reported, vec![(EdgeId(0), 150)]);
    // The sibling region still holds its tombstone.
    assert_eq!(set.region_reclaimable_count(0, 1, 200), 1);
    assert_eq!(set.region_reclaimable_count(0, 0, 200), 0);
}

#[test]
fn append_log_encode_replay_roundtrip() {
    let mut set = narrow_set();
    set.insert_edge(10, endpoint(1, 0), EdgeId(0), 100).unwrap();
    set.insert_edge(11, endpoint(2, 0), EdgeId(1), 100).unwrap();
    assert_eq!(set.group_append_op_count(0), 2);
    let manifest = TableShardManifest {
        group_bits: 9,
        out_groups: vec![0],
        in_groups: vec![0],
    };
    let payload = set.encode_group_append_log(0, &manifest);

    let mut loaded = narrow_set();
    loaded
        .replay_group_append_log(0, &payload, &manifest)
        .unwrap();
    assert!(loaded.get_edge(10, endpoint(1, 0), 200).is_some());
    assert!(loaded.get_edge(11, endpoint(2, 0), 200).is_some());
    assert_eq!(loaded.edge_count(), 2);
}

#[test]
fn append_log_rejects_version_manifest_and_trailing() {
    let mut set = narrow_set();
    set.insert_edge(10, endpoint(1, 0), EdgeId(0), 100).unwrap();
    let manifest = TableShardManifest {
        group_bits: 9,
        out_groups: vec![0],
        in_groups: vec![0],
    };
    let payload = set.encode_group_append_log(0, &manifest);

    let mut bad = payload.clone();
    bad[0] = 99;
    let mut loaded = narrow_set();
    assert!(loaded.replay_group_append_log(0, &bad, &manifest).is_err());

    let other = TableShardManifest {
        group_bits: 10,
        out_groups: vec![0],
        in_groups: vec![0],
    };
    assert!(loaded.replay_group_append_log(0, &payload, &other).is_err());

    let mut trailing = payload.clone();
    trailing.push(0);
    assert!(loaded
        .replay_group_append_log(0, &trailing, &manifest)
        .is_err());
}

#[test]
fn append_log_cleared_after_merge() {
    let mut set = narrow_set();
    set.insert_edge(10, endpoint(1, 0), EdgeId(0), 100).unwrap();
    assert!(set.group_has_append_log(0));
    set.clear_group_append_log(0);
    assert!(!set.group_has_append_log(0));
    assert_eq!(set.group_append_op_count(0), 0);
}
