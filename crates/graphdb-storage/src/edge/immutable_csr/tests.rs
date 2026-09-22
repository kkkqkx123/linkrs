use super::super::{
    EdgePosition, MutableCsr, MutableCsrTrait, Nbr, SingleMutableCsr, Timestamp, VertexId,
    INVALID_EDGE_ID,
};
use super::*;
use graphdb_core::types::EdgeId;

fn packed_endpoint(endpoint: u32, rank: i64) -> VertexId {
    VertexId::edge_endpoint_key(endpoint, rank)
}

/// Logical-content comparison: same multiset of entries in frozen row
/// order, independent of the source physical order.
fn sorted_physical(entries: Vec<Nbr>) -> Vec<Nbr> {
    let mut sorted = entries;
    sorted.sort_by_key(super::pack::frozen_row_key);
    sorted
}

fn sample_mutable() -> MutableCsr {
    let mut csr = MutableCsr::with_capacity(8, 64);
    csr.insert_edge(0, packed_endpoint(10, 0), EdgeId(1), 1)
        .unwrap();
    csr.insert_edge(0, packed_endpoint(11, 0), EdgeId(2), 1)
        .unwrap();
    csr.insert_edge(3, packed_endpoint(12, 1), EdgeId(3), 2)
        .unwrap();
    csr.delete_edge(0, EdgeId(2), 5).unwrap();
    csr
}

#[test]
fn pack_preserves_logical_reads_in_sorted_order() {
    let mutable = sample_mutable();
    let frozen = ImmutableCsr::pack_from_mutable(&mutable);
    assert_eq!(frozen.vertex_capacity(), mutable.vertex_capacity());
    assert_eq!(frozen.edge_count(), mutable.edge_count());
    for vid in 0..8u32 {
        // Logical equivalence, not byte order: frozen rows are sorted.
        assert_eq!(
            frozen.physical_edges_of(vid),
            sorted_physical(mutable.physical_edges_of(vid))
        );
        for ts in [1u64, 2, 4, 5, 6] {
            assert_eq!(
                sorted_physical(frozen.edges_of(vid, ts)),
                sorted_physical(mutable.edges_of(vid, ts))
            );
        }
        // Same live/dead split; capacity differs by construction:
        // frozen rows carry no reserved gaps.
        let (f_live, f_dead, f_cap) = frozen.vertex_census(vid);
        let (m_live, m_dead, _) = mutable.vertex_census(vid);
        assert_eq!((f_live, f_dead), (m_live, m_dead));
        assert_eq!(f_cap, frozen.physical_edges_of(vid).len());
        assert_eq!(
            frozen.get_edge(vid, packed_endpoint(10, 0), 6),
            mutable.get_edge(vid, packed_endpoint(10, 0), 6)
        );
    }
    assert_eq!(
        frozen.get_edge_physical(0, packed_endpoint(10, 0)),
        mutable.get_edge_physical(0, packed_endpoint(10, 0))
    );
    assert!(frozen.primary_contains(0, EdgeId(1)));
    assert!(!frozen.primary_contains(0, EdgeId(999)));
    assert!(frozen.has_physical_entries(0));
    assert!(!frozen.has_physical_entries(1));
    // Offsets address the sorted row: position 0 is the smallest key.
    let row0 = frozen.physical_edges_of(0);
    for (pos, nbr) in row0.iter().enumerate() {
        assert_eq!(frozen.nbr_at_offset(0, pos as i32), Some(*nbr));
    }
    assert_eq!(frozen.nbr_at_offset(0, 99), None);
    assert_eq!(frozen.nbr_at_offset(9, 0), None);
    assert_eq!(
        frozen.locate_edge(0, EdgeId(1)).map(|(_, nbr)| nbr),
        mutable.locate_edge(0, EdgeId(1)).map(|(_, nbr)| nbr)
    );
}

#[test]
fn pack_single_matches_mutable_reads() {
    let mut single = SingleMutableCsr::with_capacity(4);
    single
        .insert_edge(1, packed_endpoint(20, 0), EdgeId(7), 3)
        .unwrap();
    let frozen = ImmutableCsr::pack_single_from(&single);
    assert_eq!(frozen.vertex_capacity(), 4);
    assert_eq!(frozen.edge_count(), 1);
    assert_eq!(frozen.edges_of(1, 3), single.edges_of(1, 3));
    assert_eq!(
        frozen.get_edge(1, packed_endpoint(20, 0), 9),
        single.get_edge(1, packed_endpoint(20, 0), 9)
    );
    assert!(frozen.physical_edges_of(0).is_empty());
}

#[test]
fn frozen_writes_are_rejected() {
    let mut frozen = ImmutableCsr::pack_from_mutable(&sample_mutable());
    assert!(frozen
        .insert_edge(0, packed_endpoint(30, 0), EdgeId(100), 9)
        .is_err());
    assert!(frozen.delete_edge(0, EdgeId(1), 9).is_err());
    assert_eq!(frozen.delete_edge_by_dst(0, packed_endpoint(10, 0), 9), 0);
    assert!(frozen.delete_edge_by_offset(0, 0, 9).is_err());
    assert!(frozen
        .delete_edge_at_position(0, EdgePosition::Primary { slot: 0 }, EdgeId(1), 9)
        .is_err());
    assert!(!frozen.rollback_insert(0, EdgeId(1)));
    assert!(!frozen.revert_delete_by_edge_id(0, EdgeId(2), 9));
    assert!(!frozen.revert_delete_by_offset(0, 0, 9));
    assert!(!frozen.revert_delete_at_position(0, EdgePosition::Primary { slot: 0 }, EdgeId(2), 9));
    // Single-row frozen reclaim is prohibited through the trait entry; the
    // group paths reclaim instead. The offline `compact_row` tooling entry
    // below still covers the single-row mechanics.
    assert_eq!(
        frozen.compact_vertex_with_reporting(0, 9, &mut |_, _| {}),
        0
    );
    assert_eq!(frozen.compact_rows_batched(&[0], 9, &mut |_, _| {}), 1);
    assert_eq!(frozen.reclaimable_count(0, 9), 0);
    assert!(!frozen.vertex_needs_compact(0, 9));
    assert_eq!(frozen.row_gap(0), 0);
    assert_eq!(frozen.row_density(0), 1.0);
    assert!(frozen.rebalance_row(0));
}

#[test]
fn dump_load_roundtrip_restores_reads() {
    let frozen = ImmutableCsr::pack_from_mutable(&sample_mutable());
    let bytes = frozen.dump();
    let mut loaded = ImmutableCsr::new();
    loaded.load(&bytes).unwrap();
    assert_eq!(loaded.vertex_capacity(), frozen.vertex_capacity());
    assert_eq!(loaded.edge_count(), frozen.edge_count());
    for vid in 0..8u32 {
        assert_eq!(loaded.physical_edges_of(vid), frozen.physical_edges_of(vid));
        assert_eq!(loaded.edges_of(vid, 6), frozen.edges_of(vid, 6));
    }
    let mut scratch = super::super::mutable_csr::persistence::CsrDumpScratch::new();
    let mut via_scratch = Vec::new();
    loaded.dump_into_with_scratch(&mut via_scratch, &mut scratch);
    assert_eq!(via_scratch, bytes);
}

#[test]
fn load_rejects_damage() {
    let bytes = ImmutableCsr::pack_from_mutable(&sample_mutable()).dump();
    let mut loaded = ImmutableCsr::new();
    assert!(loaded.load(&[]).is_err());
    assert!(loaded.load(&bytes[..10]).is_err());
    let mut bad_version = bytes.clone();
    bad_version[0] = 99;
    assert!(loaded.load(&bad_version).is_err());
    let mut trailing = bytes.clone();
    trailing.push(0);
    assert!(loaded.load(&trailing).is_err());
}

#[test]
fn iterators_match_mutable_counts() {
    let mutable = sample_mutable();
    let frozen = ImmutableCsr::pack_from_mutable(&mutable);
    assert_eq!(frozen.iter(6).count(), mutable.iter(6).count());
    assert_eq!(frozen.iter_all().count(), mutable.iter_all().count());
    // Full scans stay linear; frozen walks sorted rows, so compare the
    // same logical multiset instead of the walk order.
    let mut frozen_pairs: Vec<(VertexId, EdgeId)> = frozen
        .iter_all()
        .map(|(vid, nbr)| (vid, nbr.edge_id))
        .collect();
    let mut mutable_pairs: Vec<(VertexId, EdgeId)> = mutable
        .iter_all()
        .map(|(vid, nbr)| (vid, nbr.edge_id))
        .collect();
    frozen_pairs.sort_by_key(|(vid, edge)| (vid.as_int64().unwrap_or(0), edge.0));
    mutable_pairs.sort_by_key(|(vid, edge)| (vid.as_int64().unwrap_or(0), edge.0));
    assert_eq!(frozen_pairs, mutable_pairs);
}

#[test]
fn empty_table_packs_and_loads() {
    let mutable = MutableCsr::with_capacity(4, 16);
    let frozen = ImmutableCsr::pack_from_mutable(&mutable);
    assert_eq!(frozen.edge_count(), 0);
    assert_eq!(frozen.vertex_capacity(), 4);
    assert!(frozen.edges_of(0, 1).is_empty());
    let bytes = frozen.dump();
    let mut loaded = ImmutableCsr::new();
    loaded.load(&bytes).unwrap();
    assert_eq!(loaded.edge_count(), 0);
    assert!(loaded.used_memory_size() > 0);
}

#[test]
fn packed_rows_are_sorted_and_sentinel_free() {
    let mut mutable = MutableCsr::with_capacity(4, 64);
    // Inserted out of key order on purpose.
    for (endpoint, edge) in [(50, 1), (10, 2), (30, 3), (20, 4), (40, 5)] {
        mutable
            .insert_edge(0, packed_endpoint(endpoint, 0), EdgeId(edge), 1)
            .unwrap();
    }
    mutable.delete_edge(0, EdgeId(3), 4).unwrap();
    let frozen = ImmutableCsr::pack_from_mutable(&mutable);
    let row = frozen.physical_edges_of(0);
    assert_eq!(row.len(), 5);
    let keys: Vec<(u32, i64)> = row.iter().map(|nbr| (nbr.endpoint, nbr.rank)).collect();
    assert_eq!(keys, vec![(10, 0), (20, 0), (30, 0), (40, 0), (50, 0)]);
    assert!(row.iter().all(|nbr| nbr.edge_id != INVALID_EDGE_ID));
    // Tombstone kept and still filtered by timestamp.
    assert_eq!(frozen.edges_of(0, 5).len(), 4);
    assert_eq!(frozen.edges_of(0, 3).len(), 5);
}

#[test]
fn wide_row_point_queries_match_mutable() {
    let mut mutable = MutableCsr::with_capacity(2, 256);
    // Monotonic timestamps: earliest-created equals first-inserted, so
    // the frozen version rule and the mutable order agree everywhere.
    let mut endpoints: Vec<u32> = (0..120).collect();
    endpoints.reverse();
    for (i, endpoint) in endpoints.iter().enumerate() {
        mutable
            .insert_edge(
                0,
                packed_endpoint(*endpoint, (i % 3) as i64),
                EdgeId(1000 + i as u64),
                1 + i as u64,
            )
            .unwrap();
    }
    for i in (0..120usize).step_by(7) {
        mutable
            .delete_edge(0, EdgeId(1000 + i as u64), 1000)
            .unwrap();
    }
    let frozen = ImmutableCsr::pack_from_mutable(&mutable);
    for endpoint in [0u32, 1, 59, 60, 119] {
        for rank in [0i64, 1, 2] {
            for ts in [1u64, 60, 500, 999, 1000, 2000] {
                let key = packed_endpoint(endpoint, rank);
                assert_eq!(
                    frozen.get_edge(0, key, ts),
                    mutable.get_edge(0, key, ts),
                    "endpoint={endpoint} rank={rank} ts={ts}"
                );
                assert_eq!(
                    frozen.get_edge_physical(0, key),
                    mutable.get_edge_physical(0, key),
                    "physical endpoint={endpoint} rank={rank}"
                );
            }
        }
    }
    assert_eq!(
        sorted_physical(frozen.edges_of(0, 2000)),
        sorted_physical(mutable.edges_of(0, 2000))
    );
}

#[test]
fn same_key_versions_select_earliest_created() {
    let mut mutable = MutableCsr::with_capacity(2, 16);
    let key = packed_endpoint(42, 0);
    // Same key twice via delete plus reinsert: EdgeId(1) at ts=5 then
    // EdgeId(2) at ts=3. create_ts no longer lives in the row, so both
    // mutable and frozen pick the first surviving slot at the given ts.
    mutable.insert_edge(0, key, EdgeId(1), 5).unwrap();
    mutable.delete_edge(0, EdgeId(1), 9).unwrap();
    mutable.insert_edge(0, key, EdgeId(2), 3).unwrap();
    let frozen = ImmutableCsr::pack_from_mutable(&mutable);
    // At ts=6 EdgeId(1) is alive (delete_ts=9 > 6), EdgeId(2) is also
    // alive; frozen row order is (endpoint, rank, edge_id) so EdgeId(1)
    // comes first.
    assert_eq!(
        mutable.get_edge(0, key, 6).map(|nbr| nbr.edge_id),
        Some(EdgeId(1))
    );
    assert_eq!(
        frozen.get_edge(0, key, 6).map(|nbr| nbr.edge_id),
        Some(EdgeId(1))
    );
    // After the first version's tombstone closes, both agree.
    assert_eq!(
        frozen.get_edge(0, key, 9).map(|nbr| nbr.edge_id),
        mutable.get_edge(0, key, 9).map(|nbr| nbr.edge_id)
    );
    assert_eq!(
        frozen.get_edge(0, key, 9).map(|nbr| nbr.edge_id),
        Some(EdgeId(2))
    );
}

#[test]
fn frozen_compact_drops_eligible_tombstones_in_place() {
    let mut mutable = MutableCsr::with_capacity(4, 16);
    for (endpoint, edge) in [(1, 10), (2, 11), (3, 12)] {
        mutable
            .insert_edge(0, packed_endpoint(endpoint, 0), EdgeId(edge), 1)
            .unwrap();
    }
    mutable.delete_edge(0, EdgeId(11), 5).unwrap();
    let mut frozen = ImmutableCsr::pack_from_mutable(&mutable);
    assert_eq!(frozen.reclaimable_count(0, 10), 1);
    let mut removed = Vec::new();
    let dropped = frozen.compact_with_cutoff(10, &mut |id, ts| removed.push((id, ts)));
    assert_eq!(dropped, 1);
    assert_eq!(removed, vec![(EdgeId(11), 5)]);
    assert_eq!(frozen.reclaimable_count(0, 10), 0);
    assert_eq!(frozen.edge_count(), 2);
    let keys: Vec<u32> = frozen
        .physical_edges_of(0)
        .iter()
        .map(|nbr| nbr.endpoint)
        .collect();
    assert_eq!(keys, vec![1, 3]);
}

#[test]
fn compact_row_leaves_other_rows_untouched() {
    let mut mutable = MutableCsr::with_capacity(4, 32);
    for (row, base) in [(0u32, 10u64), (1, 20), (2, 30)] {
        for i in 0..4 {
            mutable
                .insert_edge(
                    row,
                    packed_endpoint(base as u32 + i as u32, 0),
                    EdgeId(base + i),
                    1,
                )
                .unwrap();
        }
    }
    mutable.delete_edge(1, EdgeId(21), 5).unwrap();
    mutable.delete_edge(1, EdgeId(23), 5).unwrap();
    let mut frozen = ImmutableCsr::pack_from_mutable(&mutable);
    let before_0 = frozen.physical_edges_of(0);
    let before_2 = frozen.physical_edges_of(2);
    let live_before = frozen.edge_count();
    let mut removed = Vec::new();
    let dropped = frozen.compact_row(1, 10, &mut |id, ts| removed.push((id, ts)));
    assert_eq!(dropped, 2);
    assert_eq!(removed, vec![(EdgeId(21), 5), (EdgeId(23), 5)]);
    assert_eq!(frozen.physical_edges_of(0), before_0);
    assert_eq!(frozen.physical_edges_of(2), before_2);
    assert_eq!(frozen.row_degree(0), 4);
    assert_eq!(frozen.row_degree(1), 2);
    assert_eq!(frozen.row_degree(2), 4);
    assert_eq!(frozen.edge_count(), live_before);
    assert_eq!(frozen.reclaimable_count(1, 10), 0);
    let keys: Vec<u32> = frozen
        .physical_edges_of(1)
        .iter()
        .map(|nbr| nbr.endpoint)
        .collect();
    assert_eq!(keys, vec![20, 22]);
    assert_eq!(frozen.edges_of(1, 10).len(), 2);
    assert_eq!(
        frozen
            .get_edge(2, packed_endpoint(31, 0), 10)
            .map(|n| n.edge_id),
        Some(EdgeId(31))
    );
}

#[test]
fn compact_row_without_reclaimable_changes_nothing() {
    let mut frozen = ImmutableCsr::pack_from_mutable(&sample_mutable());
    let before: Vec<Vec<Nbr>> = (0..8u32).map(|vid| frozen.physical_edges_of(vid)).collect();
    let live_before = frozen.edge_count();
    let mut removed = Vec::new();
    assert_eq!(
        frozen.compact_row(0, 4, &mut |id, ts| removed.push((id, ts))),
        0
    );
    assert!(removed.is_empty());
    assert_eq!(frozen.compact_row(9, 10, &mut |_, _| {}), 0);
    assert_eq!(frozen.compact_row(0, Timestamp::MAX, &mut |_, _| {}), 0);
    let after: Vec<Vec<Nbr>> = (0..8u32).map(|vid| frozen.physical_edges_of(vid)).collect();
    assert_eq!(before, after);
    assert_eq!(frozen.edge_count(), live_before);
}

#[test]
fn mutable_row_sorted_reflects_insertion_order() {
    let mut mutable = MutableCsr::with_capacity(4, 16);
    assert!(mutable.is_row_sorted(0));
    mutable
        .insert_edge(0, packed_endpoint(10, 0), EdgeId(1), 1)
        .unwrap();
    assert!(mutable.is_row_sorted(0));
    mutable
        .insert_edge(0, packed_endpoint(5, 0), EdgeId(2), 1)
        .unwrap();
    assert!(!mutable.is_row_sorted(0));
}

#[test]
fn batched_frozen_reclaim_matches_per_row_loop() {
    let mut mutable = MutableCsr::with_capacity(8, 64);
    mutable
        .insert_edge(0, packed_endpoint(10, 0), EdgeId(1), 1)
        .unwrap();
    mutable
        .insert_edge(0, packed_endpoint(11, 0), EdgeId(2), 1)
        .unwrap();
    mutable
        .insert_edge(0, packed_endpoint(12, 0), EdgeId(3), 1)
        .unwrap();
    mutable.delete_edge(0, EdgeId(3), 5).unwrap();
    mutable
        .insert_edge(1, packed_endpoint(20, 0), EdgeId(4), 1)
        .unwrap();
    mutable
        .insert_edge(1, packed_endpoint(21, 0), EdgeId(5), 1)
        .unwrap();
    mutable.delete_edge(1, EdgeId(4), 6).unwrap();
    mutable
        .insert_edge(2, packed_endpoint(30, 0), EdgeId(6), 1)
        .unwrap();
    let cutoff = 9 as Timestamp;

    // Offline single-row loop, one trailing memmove per row.
    let mut by_row = ImmutableCsr::pack_from_mutable(&mutable);
    let mut row_reported = Vec::new();
    let mut row_removed = 0usize;
    for vid in 0..8u32 {
        row_removed += by_row.compact_row(vid, cutoff, &mut |id, ts| {
            row_reported.push((id, ts));
        });
    }

    // Production batched path, one linear pass for the listed rows.
    let mut batched = ImmutableCsr::pack_from_mutable(&mutable);
    let mut batched_reported = Vec::new();
    let vids: Vec<u32> = (0..8u32).collect();
    let batched_removed = batched.compact_rows_batched(&vids, cutoff, &mut |id, ts| {
        batched_reported.push((id, ts));
    });

    assert_eq!(row_removed, 2);
    assert_eq!(batched_removed, row_removed);
    assert_eq!(batched_reported, row_reported);
    assert_eq!(batched.edge_count(), by_row.edge_count());
    for vid in 0..8u32 {
        assert_eq!(
            batched.physical_edges_of(vid),
            by_row.physical_edges_of(vid),
            "row {} diverges",
            vid
        );
    }
}
