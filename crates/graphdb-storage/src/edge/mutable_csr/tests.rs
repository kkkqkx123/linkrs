use super::super::{EdgeId, Nbr, Timestamp, VertexId};
use super::overflow::OVERFLOW_REPACK_CHUNKS_PER_VERTEX;
use super::row::{
    graded_overflow_chunk_edges, OVERFLOW_CHUNK_MAX, OVERFLOW_CHUNK_MIN, PACKED_CSR_DENSITY,
};
use super::MutableCsr;

#[test]
fn test_basic_insert_and_query() {
    let mut csr = MutableCsr::with_capacity(10, 100);

    csr.insert_edge(0u32, VertexId::from_int64(1), EdgeId(100), 1)
        .unwrap();
    csr.insert_edge(0u32, VertexId::from_int64(2), EdgeId(101), 1)
        .unwrap();
    csr.insert_edge(1u32, VertexId::from_int64(3), EdgeId(102), 1)
        .unwrap();

    assert!(csr
        .insert_edge(0u32, VertexId::from_int64(1), EdgeId(103), 1)
        .is_err());

    assert_eq!(csr.edge_count(), 3);
}

#[test]
fn test_delete_edge() {
    let mut csr = MutableCsr::with_capacity(10, 100);

    csr.insert_edge(0u32, VertexId::from_int64(1), EdgeId(100), 1)
        .unwrap();
    csr.insert_edge(0u32, VertexId::from_int64(2), EdgeId(101), 1)
        .unwrap();

    assert!(csr.delete_edge(0u32, EdgeId(100), 2).unwrap());

    assert_eq!(csr.edge_count(), 1);
}

#[test]
fn test_double_delete_conflict() {
    let mut csr = MutableCsr::with_capacity(10, 100);
    csr.insert_edge(0u32, VertexId::from_int64(1), EdgeId(100), 10)
        .unwrap();

    // First delete succeeds.
    assert!(csr.delete_edge(0u32, EdgeId(100), 100).unwrap());
    // Idempotent re-delete at the same timestamp is a no-op, not a conflict.
    assert!(!csr.delete_edge(0u32, EdgeId(100), 100).unwrap());
    // Deleting the same edge at a different timestamp is a write-write
    // conflict, surfaced at the storage write path.
    let err = csr.delete_edge(0u32, EdgeId(100), 200).unwrap_err();
    assert_eq!(
        err.kind(),
        graphdb_core::error::storage::StorageErrorKind::Conflict
    );

    // The edge is still logically deleted at the original timestamp.
    assert_eq!(csr.edges_of(0u32, 50).len(), 1);
    assert_eq!(csr.edges_of(0u32, 150).len(), 0);
}

#[test]
fn test_dump_and_load() {
    let mut csr1 = MutableCsr::with_capacity(10, 100);

    csr1.insert_edge(0u32, VertexId::from_int64(1), EdgeId(100), 1)
        .unwrap();
    csr1.insert_edge(0u32, VertexId::from_int64(2), EdgeId(101), 1)
        .unwrap();
    csr1.insert_edge(1u32, VertexId::from_int64(3), EdgeId(102), 1)
        .unwrap();

    let data = csr1.dump();

    let mut csr2 = MutableCsr::new();
    let _ = csr2.load(&data);

    assert_eq!(csr2.vertex_capacity(), csr1.vertex_capacity());
    assert_eq!(csr2.edge_count(), csr1.edge_count());
}

#[test]
fn test_load_rejects_tampered_edge_count() {
    let mut csr1 = MutableCsr::with_capacity(10, 100);
    csr1.insert_edge(0u32, VertexId::from_int64(1), EdgeId(100), 1)
        .unwrap();
    csr1.insert_edge(0u32, VertexId::from_int64(2), EdgeId(101), 1)
        .unwrap();
    let data = csr1.dump();
    let mut ok = MutableCsr::new();
    ok.load(&data).expect("normal payload must load");

    let mut tampered = data.clone();
    let stored = u64::from_le_bytes(tampered[12..20].try_into().unwrap());
    tampered[12..20].copy_from_slice(&(stored + 1).to_le_bytes());
    let mut csr2 = MutableCsr::new();
    let err = csr2.load(&tampered).expect_err("tampered count must fail");
    assert!(err.to_string().contains("CRC mismatch"));

    // Re-seal the trailer so the CRC passes: the structural edge-count
    // validation underneath must still catch the tamper.
    let body_len = tampered.len() - 4;
    let resealed = crc32fast::hash(&tampered[..body_len]);
    tampered[body_len..].copy_from_slice(&resealed.to_le_bytes());
    let mut csr3 = MutableCsr::new();
    let err = csr3.load(&tampered).expect_err("resealed count must fail");
    assert!(err.to_string().contains("edge count mismatch"));
}

#[test]
fn test_resize() {
    let mut csr = MutableCsr::with_capacity(2, 10);

    csr.insert_edge(0u32, VertexId::from_int64(1), EdgeId(100), 1)
        .unwrap();
    csr.insert_edge(100u32, VertexId::from_int64(1), EdgeId(101), 1)
        .unwrap();

    assert!(csr.vertex_capacity() >= 101);
}

#[test]
fn test_iterator() {
    let mut csr = MutableCsr::with_capacity(10, 100);

    csr.insert_edge(0u32, VertexId::from_int64(1), EdgeId(100), 1)
        .unwrap();
    csr.insert_edge(0u32, VertexId::from_int64(2), EdgeId(101), 1)
        .unwrap();
    csr.insert_edge(1u32, VertexId::from_int64(3), EdgeId(102), 1)
        .unwrap();

    let edges: Vec<_> = csr.iter(1).collect();
    assert_eq!(edges.len(), 3);
}

#[test]
fn test_overflow_insert() {
    let mut csr = MutableCsr::with_capacity(10, 100);

    csr.insert_edge(0u32, VertexId::from_int64(1), EdgeId(100), 1)
        .unwrap();
    csr.insert_edge(0u32, VertexId::from_int64(2), EdgeId(101), 1)
        .unwrap();
    csr.insert_edge(0u32, VertexId::from_int64(3), EdgeId(102), 1)
        .unwrap();
    csr.insert_edge(0u32, VertexId::from_int64(4), EdgeId(103), 1)
        .unwrap();
    csr.insert_edge(0u32, VertexId::from_int64(5), EdgeId(104), 1)
        .unwrap();

    assert_eq!(csr.edge_count(), 5);

    let edges = csr.edges_of(0u32, 1);
    assert_eq!(edges.len(), 5);

    assert!(csr
        .insert_edge(0u32, VertexId::from_int64(5), EdgeId(105), 1)
        .is_err());

    assert!(csr.delete_edge(0u32, EdgeId(104), 2).unwrap());
}

#[test]
fn test_overflow_dump_and_load() {
    let mut csr1 = MutableCsr::with_capacity(10, 100);

    for i in 1..=6 {
        let dst = VertexId::from_int64(i as i64);
        csr1.insert_edge(0u32, dst, EdgeId(i as u64), 1).unwrap();
    }

    let data = csr1.dump();

    let mut csr2 = MutableCsr::new();
    let _ = csr2.load(&data);

    assert_eq!(csr2.vertex_capacity(), csr1.vertex_capacity());
    assert_eq!(csr2.edge_count(), csr1.edge_count());
    assert_eq!(
        csr2.overflow_chunks.get(0).map_or(0, |chunks| {
            chunks.iter().map(|chunk| chunk.len()).sum::<usize>()
        }),
        2
    );
}

#[test]
fn test_compact_with_ts_merges_overflow() {
    let mut csr = MutableCsr::with_capacity(10, 100);

    for i in 1..=6 {
        let dst = VertexId::from_int64(i as i64);
        csr.insert_edge(0u32, dst, EdgeId(i as u64), 1).unwrap();
    }

    csr.delete_edge(0u32, EdgeId(3), 5).unwrap();
    csr.delete_edge(0u32, EdgeId(5), 5).unwrap();
    csr.delete_edge(0u32, EdgeId(6), 5).unwrap();

    // Cutoff 6: deletions at 5 predate the cutoff, so they are removed.
    let removed = csr.compact_with_ts_reporting(6, 0.25, &mut |_, _| {});
    assert_eq!(removed, 3);

    assert!(csr.overflow_chunks.get(0).is_none_or(Vec::is_empty));

    let edges = csr.edges_of(0u32, 3);
    assert_eq!(edges.len(), 3);
}

#[test]
fn test_compact_with_ts_keeps_deleted_entries_without_cutoff() {
    let mut csr = MutableCsr::with_capacity(10, 100);

    for i in 1..=3 {
        let dst = VertexId::from_int64(i as i64);
        csr.insert_edge(0u32, dst, EdgeId(i as u64), 1).unwrap();
    }
    csr.delete_edge(0u32, EdgeId(2), 5).unwrap();

    // cutoff == MAX (no active snapshot): the deletion history must be
    // preserved for time-travel queries before the deletion.
    let removed = csr.compact_with_ts_reporting(Timestamp::MAX, 0.25, &mut |_, _| {});
    assert_eq!(removed, 0);

    assert_eq!(csr.edges_of(0u32, 3).len(), 3);
    assert_eq!(csr.edges_of(0u32, 6).len(), 2);

    // A real cutoff drops the entry again.
    let removed = csr.compact_with_ts_reporting(6, 0.25, &mut |_, _| {});
    assert_eq!(removed, 1);
    assert_eq!(csr.edges_of(0u32, 3).len(), 2);
}

#[test]
fn test_compact_with_ts_reporting_reports_removed_edges() {
    let mut csr = MutableCsr::with_capacity(10, 100);

    for i in 1..=3 {
        let dst = VertexId::from_int64(i as i64);
        csr.insert_edge(0u32, dst, EdgeId(i as u64), 1).unwrap();
    }
    csr.delete_edge(0u32, EdgeId(2), 5).unwrap();

    let mut reported = Vec::new();
    let removed = csr.compact_with_ts_reporting(6, 0.25, &mut |edge_id, delete_ts| {
        reported.push((edge_id, delete_ts));
    });
    assert_eq!(removed, 1);
    assert_eq!(reported, vec![(EdgeId(2), 5)]);
}

#[test]
fn test_compact_with_ts_guards_reserve_ratio_ge_one() {
    // reserve_ratio >= 1.0 used to produce valid / 0.0 = inf, saturating
    // the cast to u32::MAX per vertex and exploding the rebuilt CSR
    // allocation (OOM on ~800k+ edge partitions under background freeze).
    let mut csr = MutableCsr::with_capacity(4, 100);
    for i in 1..=6i64 {
        csr.insert_edge(0u32, VertexId::from_int64(i), EdgeId(i as u64), 1)
            .unwrap();
    }
    csr.insert_edge(1u32, VertexId::from_int64(1), EdgeId(7), 1)
        .unwrap();

    let removed = csr.compact_with_ts_reporting(3, 1.0, &mut |_, _| {});
    assert_eq!(removed, 0);

    let capacity = csr.total_edge_capacity;
    assert!(
        capacity <= 7 + 4,
        "capacity must stay bounded, got {}",
        capacity
    );
    assert_eq!(csr.edges_of(0u32, 3).len(), 6);
    assert_eq!(csr.edges_of(1u32, 3).len(), 1);
}

#[test]
fn test_compact_with_ts_zero_ratio_keeps_exact_degree() {
    let mut csr = MutableCsr::with_capacity(4, 100);
    for i in 1..=3i64 {
        csr.insert_edge(0u32, VertexId::from_int64(i), EdgeId(i as u64), 1)
            .unwrap();
    }
    let removed = csr.compact_with_ts_reporting(3, 0.0, &mut |_, _| {});
    assert_eq!(removed, 0);
    assert_eq!(csr.total_edge_capacity, 3);
    assert_eq!(csr.edges_of(0u32, 3).len(), 3);
}

#[test]
fn test_overflow_iterator() {
    let mut csr = MutableCsr::with_capacity(10, 100);

    for i in 1..=6 {
        let dst = VertexId::from_int64(i as i64);
        csr.insert_edge(0u32, dst, EdgeId(i as u64), 1).unwrap();
    }

    let all_edges: Vec<_> = csr.iter(1).collect();
    assert_eq!(all_edges.len(), 6);
}

#[test]
fn test_supernode_overflow_consolidates_repack_into_single_block() {
    let mut csr = MutableCsr::with_overflow_chunk_edges(1, 4, 32);
    for i in 0..4_096u64 {
        csr.insert_edge(0, VertexId::from_int64(i as i64 + 1), EdgeId(i + 1), 1)
            .unwrap();
    }

    let chunks = csr.overflow_chunks.get(0).expect("vertex 0 has overflow");
    // Repacks consolidate the chain: at most one consolidated block plus
    // the small graded tail chunks grown since the last repack.
    assert!(
        chunks.len() <= OVERFLOW_REPACK_CHUNKS_PER_VERTEX + 1,
        "overflow chain must stay bounded, got {}",
        chunks.len()
    );
    let consolidated = chunks.iter().filter(|chunk| chunk.capacity() > 32).count();
    assert!(
        consolidated <= 1,
        "at most one consolidated block per row, got {}",
        consolidated
    );
    assert!(chunks.iter().all(|chunk| chunk.len() <= chunk.capacity()));
    assert_eq!(csr.physical_edges_of(0).len(), 4_096);
    assert_eq!(csr.edges_of(0, 1).len(), 4_096);
}

#[test]
fn test_zero_degree_rows_hold_no_slots() {
    let mut csr = MutableCsr::with_capacity(1024, 4096);
    assert_eq!(csr.total_edge_capacity, 0);

    // A single edge allocates exactly one primary block
    csr.insert_edge(0u32, VertexId::from_int64(1), EdgeId(100), 1)
        .unwrap();
    assert_eq!(csr.total_edge_capacity, 4);

    // Sparse high vertex ids allocate blocks only for themselves
    csr.insert_edge(10_000u32, VertexId::from_int64(2), EdgeId(101), 1)
        .unwrap();
    assert_eq!(csr.vertex_capacity(), 12_502);
    assert_eq!(csr.total_edge_capacity, 8);

    // Growth is proportional (1.25x), not power-of-two doubling
    assert_eq!(csr.vertex_capacity(), (10_001.0_f64 * 1.25).ceil() as usize);

    // Compact reclaims slots of rows whose edges were all removed
    csr.delete_edge(0u32, EdgeId(100), 2).unwrap();
    csr.compact_with_ts_reporting(3, 0.0, &mut |_, _| {});
    assert_eq!(csr.total_edge_capacity, 1);
    assert_eq!(csr.primary_capacities[0], 0);
}

#[test]
fn test_fragmentation_ratio() {
    let mut csr = MutableCsr::with_capacity(10, 100);

    // No edges - ratio should be 0.0
    assert_eq!(csr.fragmentation_ratio(), 0.0);

    // Insert edges to trigger overflow
    for i in 1..=6 {
        let dst = VertexId::from_int64(i as i64);
        csr.insert_edge(0u32, dst, EdgeId(i as u64), 1).unwrap();
    }

    // After overflow the table holds mostly reserved waste: the wasted
    // share sits strictly between empty and fully wasted.
    let ratio = csr.fragmentation_ratio();
    assert!(
        ratio > 0.0 && ratio < 1.0,
        "Expected wasted share in (0, 1), got {}",
        ratio
    );
}

#[test]
fn test_wasted_bytes_estimate() {
    let mut csr = MutableCsr::with_capacity(10, 100);

    for i in 1..=6 {
        let dst = VertexId::from_int64(i as i64);
        csr.insert_edge(0u32, dst, EdgeId(i as u64), 1).unwrap();
    }

    let wasted = csr.wasted_bytes_estimate();
    let active = csr.edge_count() as usize;
    let total_capacity = csr.total_edge_capacity;

    // Wasted should be roughly (total - active) * sizeof(Nbr)
    let expected_wasted = (total_capacity - active) * std::mem::size_of::<Nbr>();
    assert_eq!(wasted, expected_wasted, "Wasted bytes estimate mismatch");
}

#[test]
fn test_compact_reduces_fragmentation() {
    let mut csr = MutableCsr::with_capacity(10, 100);

    for i in 1..=6 {
        let dst = VertexId::from_int64(i as i64);
        csr.insert_edge(0u32, dst, EdgeId(i as u64), 1).unwrap();
    }
    for i in 1..=3 {
        csr.delete_edge(0u32, EdgeId(i as u64), 2).unwrap();
    }

    let ratio_before = csr.fragmentation_ratio();
    assert!(
        ratio_before > 0.5,
        "Setup failed: insufficient fragmentation"
    );

    csr.compact_with_ts_reporting(100, 0.25, &mut |_, _| {});

    let ratio_after = csr.fragmentation_ratio();
    assert!(
        ratio_after < ratio_before,
        "Compact did not reduce fragmentation: before={}, after={}",
        ratio_before,
        ratio_after
    );
}

#[test]
fn test_vertex_edges_iter_no_allocation() {
    let mut csr = MutableCsr::with_capacity(10, 100);

    // Insert multiple edges for vertex 0
    csr.insert_edge(0u32, VertexId::from_int64(1), EdgeId(100), 1)
        .unwrap();
    csr.insert_edge(0u32, VertexId::from_int64(2), EdgeId(101), 1)
        .unwrap();
    csr.insert_edge(0u32, VertexId::from_int64(3), EdgeId(102), 1)
        .unwrap();
    csr.insert_edge(0u32, VertexId::from_int64(4), EdgeId(103), 1)
        .unwrap();
    csr.insert_edge(0u32, VertexId::from_int64(5), EdgeId(104), 1)
        .unwrap();

    // Test iter_edges_of yields same neighbors as edges_of without allocation
    let iter_neighbors: Vec<_> = csr
        .iter_edges_of(0u32, 1)
        .map(|nbr| nbr.to_vertex_id())
        .collect();
    let vec_neighbors: Vec<_> = csr
        .edges_of(0u32, 1)
        .iter()
        .map(|nbr| nbr.to_vertex_id())
        .collect();

    assert_eq!(iter_neighbors.len(), vec_neighbors.len());
    assert_eq!(iter_neighbors, vec_neighbors);
}

#[test]
fn test_vertex_edges_iter_respects_timestamp() {
    let mut csr = MutableCsr::with_capacity(10, 100);

    csr.insert_edge(0u32, VertexId::from_int64(1), EdgeId(100), 1)
        .unwrap();
    csr.insert_edge(0u32, VertexId::from_int64(2), EdgeId(101), 2)
        .unwrap();
    csr.insert_edge(0u32, VertexId::from_int64(3), EdgeId(102), 3)
        .unwrap();

    // Delete the second edge at ts=2
    csr.delete_edge(0u32, EdgeId(101), 2).unwrap();

    // At ts=1, only first edge should be visible
    let edges_ts1: Vec<_> = csr.iter_edges_of(0u32, 1).collect();
    assert_eq!(edges_ts1.len(), 1);
    assert_eq!(edges_ts1[0].edge_id, EdgeId(100));

    // At ts=2, first two edges are visible (but second is deleted)
    let edges_ts2: Vec<_> = csr.iter_edges_of(0u32, 2).collect();
    assert_eq!(edges_ts2.len(), 1);

    // At ts=3, all three are visible (but second is deleted)
    let edges_ts3: Vec<_> = csr.iter_edges_of(0u32, 3).collect();
    assert_eq!(edges_ts3.len(), 2);
}

#[test]
fn test_overflow_storage_lookup() {
    let mut csr = MutableCsr::with_overflow_chunk_edges(10, 100, 2);
    for vid in 0..5u32 {
        for i in 0..6 {
            let dst = VertexId::from_int64((vid as i64 + 1) * 100 + i as i64);
            csr.insert_edge(vid, dst, EdgeId(vid as u64 * 10 + i as u64), 1)
                .unwrap();
        }
    }
    assert!(csr.get_overflow_chunks(0).is_some());
    assert!(csr.get_overflow_chunks(999).is_none());
}

#[test]
fn test_overflow_get_chunks_transparent() {
    let mut csr = MutableCsr::with_overflow_chunk_edges(10, 100, 2);
    for vid in 0..20u32 {
        for i in 0..6 {
            let dst = VertexId::from_int64((vid as i64 + 1) * 100 + i as i64);
            csr.insert_edge(vid, dst, EdgeId(vid as u64 * 10 + i as u64), 1)
                .unwrap();
        }
    }
    // All chunks should still be accessible via get_overflow_chunks
    for vid in 0..20u32 {
        let chunks = csr.get_overflow_chunks(vid).expect("should have overflow");
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].len(), 2);
    }
    for vid in 0..20u32 {
        let edges = csr.edges_of(vid, 1);
        assert_eq!(edges.len(), 6);
    }
}

#[test]
fn test_overflow_cleared_after_compact() {
    let mut csr = MutableCsr::with_overflow_chunk_edges(10, 100, 2);
    for vid in 0..20u32 {
        for i in 0..8 {
            let dst = VertexId::from_int64((vid as i64 + 1) * 100 + i as i64);
            csr.insert_edge(vid, dst, EdgeId(vid as u64 * 10 + i as u64), 1)
                .unwrap();
        }
    }
    assert!(!csr.overflow_chunks.is_empty());
    let mut removed = Vec::new();
    csr.compact_with_ts_reporting(2, 0.0, &mut |id, ts| removed.push((id, ts)));
    assert!(csr.overflow_chunks.is_empty());
}

#[test]
fn test_compact_vertex_is_row_scoped() {
    let mut csr = MutableCsr::with_capacity(10, 100);
    csr.insert_edge(0u32, VertexId::from_int64(1), EdgeId(100), 1)
        .unwrap();
    csr.insert_edge(0u32, VertexId::from_int64(2), EdgeId(101), 1)
        .unwrap();
    csr.insert_edge(5u32, VertexId::from_int64(6), EdgeId(102), 1)
        .unwrap();
    assert!(csr.delete_edge(0u32, EdgeId(100), 2).unwrap());

    assert_eq!(csr.reclaimable_count(0, 3), 1);
    assert_eq!(csr.reclaimable_count(5, 3), 0);
    assert!(csr.vertex_needs_compact(0, 3));
    assert!(!csr.vertex_needs_compact(5, 3));

    let mut reported = Vec::new();
    let removed = csr.compact_vertex_with_reporting(0, 3, &mut |id, ts| reported.push((id, ts)));
    assert_eq!(removed, 1);
    assert_eq!(reported, vec![(EdgeId(100), 2)]);

    // Target row reclaimed, other row untouched.
    assert_eq!(csr.reclaimable_count(0, 3), 0);
    assert_eq!(csr.edges_of(5, 3).len(), 1);
    assert_eq!(csr.edges_of(0, 3).len(), 1);
    let (live, dead, _) = csr.vertex_census(0);
    assert_eq!((live, dead), (1, 0));
}

#[test]
fn test_compact_vertex_keeps_pinned_tombstones() {
    let mut csr = MutableCsr::with_capacity(10, 100);
    csr.insert_edge(0u32, VertexId::from_int64(1), EdgeId(100), 1)
        .unwrap();
    assert!(csr.delete_edge(0u32, EdgeId(100), 10).unwrap());

    // Cutoff below the deletion stamp: nothing is eligible.
    assert_eq!(csr.reclaimable_count(0, 5), 0);
    assert!(!csr.vertex_needs_compact(0, 5));
    let removed = csr.compact_vertex_with_reporting(0, 5, &mut |_, _| {});
    assert_eq!(removed, 0);
    // The tombstone stays readable for older snapshots.
    assert_eq!(csr.edges_of(0, 9).len(), 1);
    assert_eq!(csr.edges_of(0, 10).len(), 0);
}

#[test]
fn test_compact_vertex_repacks_overflow() {
    let mut csr = MutableCsr::with_overflow_chunk_edges(10, 100, 2);
    for i in 0..6u64 {
        let dst = VertexId::from_int64(100 + i as i64);
        csr.insert_edge(0u32, dst, EdgeId(i), 1).unwrap();
    }
    // 4 primary + 2 overflow.
    assert!(csr.get_overflow_chunks(0).is_some());
    assert!(csr.delete_edge(0u32, EdgeId(0), 2).unwrap());
    assert!(csr.delete_edge(0u32, EdgeId(5), 2).unwrap());

    let removed = csr.compact_vertex_with_reporting(0, 3, &mut |_, _| {});
    assert_eq!(removed, 2);
    assert_eq!(csr.edges_of(0, 3).len(), 4);
    assert_eq!(csr.reclaimable_count(0, 3), 0);
}

#[test]
fn test_fragmentation_stats_report_dead_entries() {
    let mut csr = MutableCsr::with_capacity(10, 100);
    for i in 0..3u64 {
        let dst = VertexId::from_int64(10 + i as i64);
        csr.insert_edge(0u32, dst, EdgeId(i), 1).unwrap();
    }
    assert!(csr.delete_edge(0u32, EdgeId(0), 2).unwrap());

    let stats = csr.get_fragmentation_stats();
    assert_eq!(stats.reachable_edges, 2);
    assert_eq!(stats.dead_entries, 1);
    assert_eq!(
        stats.wasted_capacity,
        stats.total_capacity.saturating_sub(2)
    );
    let (live, dead, _) = csr.vertex_census(0);
    assert_eq!((live, dead), (2, 1));
}

#[test]
fn test_remove_after_delete_does_not_double_count() {
    let mut csr = MutableCsr::with_capacity(10, 100);
    csr.insert_edge(0u32, VertexId::from_int64(1), EdgeId(100), 1)
        .unwrap();
    csr.insert_edge(0u32, VertexId::from_int64(2), EdgeId(101), 1)
        .unwrap();
    assert!(csr.delete_edge(0u32, EdgeId(100), 2).unwrap());
    assert_eq!(csr.edge_count(), 1);
    assert!(csr.rollback_insert(0u32, EdgeId(100)));
    assert_eq!(csr.edge_count(), 1);
    assert!(csr.rollback_insert(0u32, EdgeId(101)));
    assert_eq!(csr.edge_count(), 0);
}

#[test]
fn test_remove_after_delete_overflow_does_not_double_count() {
    let mut csr = MutableCsr::with_overflow_chunk_edges(10, 100, 2);
    for i in 0..6u64 {
        csr.insert_edge(0u32, VertexId::from_int64(100 + i as i64), EdgeId(i), 1)
            .unwrap();
    }
    assert_eq!(csr.edge_count(), 6);
    assert!(csr.delete_edge(0u32, EdgeId(5), 2).unwrap());
    assert_eq!(csr.edge_count(), 5);
    assert!(csr.rollback_insert(0u32, EdgeId(5)));
    assert_eq!(csr.edge_count(), 5);
}

#[test]
fn test_offset_delete_rejects_out_of_degree() {
    let mut csr = MutableCsr::with_capacity(10, 100);
    csr.insert_edge(0u32, VertexId::from_int64(1), EdgeId(100), 1)
        .unwrap();
    csr.insert_edge(1u32, VertexId::from_int64(2), EdgeId(101), 1)
        .unwrap();
    // Row 0 holds one live entry; offset 1 addresses reserved capacity.
    assert!(!csr.delete_edge_by_offset(0u32, 1, 2).unwrap());
    assert_eq!(csr.edges_of(0u32, 2).len(), 1);
    assert_eq!(csr.edges_of(1u32, 2).len(), 1);
    assert!(!csr.revert_delete_by_offset(0u32, 1, 2));
    // Valid offset still works.
    assert!(csr.delete_edge_by_offset(0u32, 0, 2).unwrap());
    assert_eq!(csr.edges_of(0u32, 2).len(), 0);
    assert!(csr.revert_delete_by_offset(0u32, 0, 2));
    assert_eq!(csr.edges_of(0u32, 2).len(), 1);
}

#[test]
fn test_nbr_at_offset_views_primary_slot() {
    let mut csr = MutableCsr::with_capacity(10, 100);
    csr.insert_edge(0u32, VertexId::from_int64(7), EdgeId(100), 1)
        .unwrap();
    let slot = csr.nbr_at_offset(0u32, 0).expect("slot exists");
    assert_eq!(slot.edge_id, EdgeId(100));
    assert!(csr.nbr_at_offset(0u32, 1).is_none());
    assert!(csr.nbr_at_offset(0u32, -1).is_none());
}

#[test]
fn test_physical_reads_ignore_timestamps() {
    let mut csr = MutableCsr::with_capacity(10, 100);
    csr.insert_edge(0u32, VertexId::from_int64(1), EdgeId(100), 10)
        .unwrap();
    assert!(csr.delete_edge(0u32, EdgeId(100), 20).unwrap());
    assert!(csr
        .get_edge_physical(0u32, VertexId::from_int64(1))
        .is_none());
    assert_eq!(csr.physical_edges_of(0u32).len(), 1);
    assert!(csr.has_physical_entries(0u32));
    assert!(!csr.has_physical_entries(1u32));
}

#[test]
fn test_physical_lookup_returns_rebuilt_live_edge() {
    let mut csr = MutableCsr::with_capacity(10, 100);
    csr.insert_edge(0u32, VertexId::from_int64(1), EdgeId(100), 10)
        .unwrap();
    assert!(csr.delete_edge(0u32, EdgeId(100), 20).unwrap());
    csr.insert_edge(0u32, VertexId::from_int64(1), EdgeId(101), 30)
        .unwrap();
    let found = csr
        .get_edge_physical(0u32, VertexId::from_int64(1))
        .expect("rebuilt live edge must be found");
    assert_eq!(found.edge_id, EdgeId(101));
}

#[test]
fn test_steady_state_gap_fill_before_overflow() {
    let mut csr = MutableCsr::with_capacity(10, 100);
    for i in 1..=5i64 {
        csr.insert_edge(0u32, VertexId::from_int64(i), EdgeId(i as u64), 1)
            .unwrap();
    }
    // 4 primary slots plus one overflow entry.
    assert!(csr.get_overflow_chunks(0).is_some());
    let overflow_before: usize = csr
        .get_overflow_chunks(0)
        .map(|chunks| chunks.iter().map(|chunk| chunk.len()).sum())
        .unwrap_or(0);
    assert_eq!(overflow_before, 1);

    // Reclaim two primary tombstones at an eligible cutoff so trailing
    // gaps open without touching overflow.
    assert!(csr.delete_edge(0u32, EdgeId(1), 2).unwrap());
    assert!(csr.delete_edge(0u32, EdgeId(2), 2).unwrap());
    let removed = csr.compact_vertex_with_reporting(0, 3, &mut |_, _| {});
    assert_eq!(removed, 2);

    // Everyday writes fill the freed primary gaps first even though
    // overflow exists: overflow length stays put.
    csr.insert_edge(0u32, VertexId::from_int64(10), EdgeId(10), 3)
        .unwrap();
    csr.insert_edge(0u32, VertexId::from_int64(11), EdgeId(11), 3)
        .unwrap();
    let overflow_after: usize = csr
        .get_overflow_chunks(0)
        .map(|chunks| chunks.iter().map(|chunk| chunk.len()).sum())
        .unwrap_or(0);
    assert_eq!(overflow_after, overflow_before);
    assert_eq!(csr.edges_of(0u32, 3).len(), 5);
}

#[test]
fn test_single_live_set_rejects_duplicates_across_tiers() {
    let mut csr = MutableCsr::with_capacity(10, 100);
    for i in 1..=6i64 {
        csr.insert_edge(0u32, VertexId::from_int64(i), EdgeId(i as u64), 1)
            .unwrap();
    }
    // Primary-tier duplicate rejected without any scan fallback.
    assert!(csr
        .insert_edge(0u32, VertexId::from_int64(1), EdgeId(100), 1)
        .is_err());
    // Overflow-tier duplicate rejected through the same single set.
    assert!(csr
        .insert_edge(0u32, VertexId::from_int64(6), EdgeId(101), 1)
        .is_err());
    assert_eq!(csr.edge_count(), 6);
}

#[test]
fn test_graded_overflow_tiers_bound_small_row_chunks() {
    assert_eq!(graded_overflow_chunk_edges(0), OVERFLOW_CHUNK_MIN);
    assert_eq!(graded_overflow_chunk_edges(1), OVERFLOW_CHUNK_MIN);
    assert_eq!(graded_overflow_chunk_edges(5), OVERFLOW_CHUNK_MIN);
    assert_eq!(graded_overflow_chunk_edges(64), 64);
    assert_eq!(graded_overflow_chunk_edges(65), 128);
    assert_eq!(graded_overflow_chunk_edges(1024), 1024);
    assert_eq!(graded_overflow_chunk_edges(1025), 2048);
    assert_eq!(graded_overflow_chunk_edges(1 << 20), OVERFLOW_CHUNK_MAX);

    // Small rows allocate small chunks: 300 edges stay far below the old
    // fixed 4096-edge reservation per chunk.
    let mut csr = MutableCsr::with_capacity(10, 100);
    for i in 1..=300i64 {
        csr.insert_edge(0u32, VertexId::from_int64(i), EdgeId(i as u64), 1)
            .unwrap();
    }
    let chunks = csr.get_overflow_chunks(0).expect("vertex 0 has overflow");
    assert_eq!(chunks[0].capacity(), OVERFLOW_CHUNK_MIN);
    assert!(
        chunks
            .iter()
            .all(|chunk| chunk.capacity() <= OVERFLOW_CHUNK_MAX),
        "graded chunks must stay at or below the cap for 300 live edges"
    );
    assert_eq!(csr.edges_of(0u32, 1).len(), 300);
    // Single repack bound: small-row chunk counts stay bounded.
    assert!(
        chunks.len() <= OVERFLOW_REPACK_CHUNKS_PER_VERTEX + 1,
        "overflow chunks must stay bounded, got {}",
        chunks.len()
    );
}

#[test]
fn test_rebalance_row_drains_overflow_into_gaps() {
    let mut csr = MutableCsr::with_overflow_chunk_edges(10, 100, 2);
    for i in 0..6u64 {
        csr.insert_edge(0u32, VertexId::from_int64(100 + i as i64), EdgeId(i), 1)
            .unwrap();
    }
    assert!(csr.get_overflow_chunks(0).is_some());
    // Reclaim primary tombstones so gaps open, then rebalance pulls the
    // overflow live entries back into the primary row.
    assert!(csr.delete_edge(0u32, EdgeId(0), 2).unwrap());
    assert!(csr.delete_edge(0u32, EdgeId(1), 2).unwrap());
    let removed = csr.compact_vertex_with_reporting(0, 3, &mut |_, _| {});
    assert_eq!(removed, 2);
    assert!(csr.rebalance_row(0));
    assert!(csr.get_overflow_chunks(0).is_none_or(Vec::is_empty));
    assert_eq!(csr.edges_of(0u32, 3).len(), 4);
}

#[test]
fn test_row_gap_and_density_observe_reserve() {
    let mut csr = MutableCsr::with_capacity(10, 100);
    csr.insert_edge(0u32, VertexId::from_int64(1), EdgeId(1), 1)
        .unwrap();
    // One live entry in a 4-slot block: three write gaps remain.
    assert_eq!(csr.row_gap(0), 3);
    assert!((csr.row_density(0) - 0.25).abs() < 1e-6);
    // Rebuilds size rows at the packed density target with gaps.
    let removed = csr.compact_with_ts_reporting(2, 1.0 - PACKED_CSR_DENSITY, &mut |_, _| {});
    assert_eq!(removed, 0);
    assert_eq!(csr.row_gap(0), 1);
    assert!((csr.row_density(0) - 0.5).abs() < 1e-6);
}

#[test]
fn test_topology_encoding_roundtrip_keeps_snapshot_reads() {
    let mut csr = MutableCsr::with_capacity(16, 64);
    for i in 0..20u64 {
        csr.insert_edge(
            (i % 4) as u32,
            VertexId::from_int64(100 + i as i64),
            EdgeId(i),
            10,
        )
        .unwrap();
    }
    assert!(csr.delete_edge(0u32, EdgeId(0), 20).unwrap());
    let before: Vec<Nbr> = {
        let mut all = csr.physical_edges_of(0);
        all.extend(csr.physical_edges_of(1));
        all
    };
    let live_before = csr.edges_of(1u32, 30);

    let payload = csr.dump();
    let mut loaded = MutableCsr::new();
    loaded.load(&payload).expect("encoded load must succeed");
    let mut after = loaded.physical_edges_of(0);
    after.extend(loaded.physical_edges_of(1));
    assert_eq!(before, after);
    assert_eq!(loaded.edges_of(1u32, 30), live_before);
    assert_eq!(loaded.edge_count(), csr.edge_count());

    let report = loaded.topology_encoding_report();
    assert_eq!(report.len(), 5);
    assert!(report.iter().any(|(name, _, _, _)| name == "neighbor"));
    assert!(report.iter().any(|(name, _, _, _)| name == "edge_id"));
}

#[test]
fn test_topology_encoding_rejects_old_version() {
    let mut payload = Vec::new();
    payload.extend_from_slice(&2u32.to_le_bytes());
    payload.extend_from_slice(&[0u8; 32]);
    let mut csr = MutableCsr::new();
    assert!(csr.load(&payload).is_err());
}

#[test]
fn test_offset_delete_propagates_conflict() {
    let mut csr = MutableCsr::with_capacity(10, 100);
    csr.insert_edge(0u32, VertexId::from_int64(10), EdgeId(100), 100)
        .unwrap();
    assert!(csr.delete_edge(0u32, EdgeId(100), 150).unwrap());
    assert!(!csr.delete_edge(0u32, EdgeId(100), 150).unwrap());
    assert!(csr.delete_edge(0u32, EdgeId(100), 160).is_err());
    assert!(csr.delete_edge_by_offset(0, 0, 160).is_err());
    assert!(!csr.delete_edge_by_offset(0, 1, 160).unwrap());
    assert!(!csr.delete_edge_by_offset(0, 0, 150).unwrap());
}

#[test]
fn test_single_generic_delete_consistency_on_missing_id() {
    let mut multi = MutableCsr::with_capacity(10, 100);
    multi
        .insert_edge(0u32, VertexId::from_int64(10), EdgeId(100), 100)
        .unwrap();
    assert!(multi.delete_edge(0u32, EdgeId(100), 150).unwrap());
    assert!(!multi.delete_edge(0u32, EdgeId(999), 160).unwrap());

    let mut single = crate::edge::SingleMutableCsr::with_capacity(4);
    single
        .insert_edge(0u32, VertexId::from_int64(10), EdgeId(100), 100)
        .unwrap();
    assert!(single.delete_edge(0, EdgeId(100), 150).unwrap());
    assert!(!single.delete_edge(0, EdgeId(999), 160).unwrap());
}

#[test]
fn test_overflow_repack_preserves_unexpired_tombstones() {
    let mut csr = MutableCsr::with_overflow_chunk_edges(10, 100, 2);
    for i in 0..8u64 {
        csr.insert_edge(0u32, VertexId::from_int64(100 + i as i64), EdgeId(i), 1)
            .unwrap();
    }
    assert!(csr.delete_edge(0u32, EdgeId(6), 10).unwrap());
    assert!(csr.delete_edge(0u32, EdgeId(7), 10).unwrap());
    let overflow_before: usize = csr
        .get_overflow_chunks(0)
        .map(|chunks| chunks.iter().map(|chunk| chunk.len()).sum())
        .unwrap_or(0);
    assert!(overflow_before > 0);

    let mut reported = Vec::new();
    csr.compact_overflow_for_vertex(0, Timestamp::MAX, &mut |id, ts| reported.push((id, ts)));
    assert!(reported.is_empty());
    let kept: usize = csr
        .get_overflow_chunks(0)
        .map(|chunks| chunks.iter().map(|chunk| chunk.len()).sum())
        .unwrap_or(0);
    assert_eq!(kept, overflow_before);

    let mut reported = Vec::new();
    csr.compact_overflow_for_vertex(0, 10, &mut |id, ts| reported.push((id, ts)));
    assert_eq!(reported.len(), 2);
    let kept: usize = csr
        .get_overflow_chunks(0)
        .map(|chunks| chunks.iter().map(|chunk| chunk.len()).sum())
        .unwrap_or(0);
    assert_eq!(kept, overflow_before - 2);
}

#[test]
fn test_overflow_repack_consolidates_to_single_chunk() {
    let mut csr = MutableCsr::with_overflow_chunk_edges(10, 100, 2);
    for i in 0..24u64 {
        csr.insert_edge(0u32, VertexId::from_int64(100 + i as i64), EdgeId(i), 1)
            .unwrap();
    }
    let before = csr.physical_edges_of(0);
    assert_eq!(before.len(), 24);

    let mut noop = |_: EdgeId, _: Timestamp| {};
    csr.compact_overflow_for_vertex(0, Timestamp::MAX, &mut noop);
    let chunks = csr
        .get_overflow_chunks(0)
        .expect("overflow remains after preserve-all repack");
    assert_eq!(
        chunks.len(),
        1,
        "repack must consolidate the row to one contiguous chunk"
    );
    // No entry lost or reordered by the consolidation.
    assert_eq!(csr.physical_edges_of(0), before);
    assert_eq!(csr.edges_of(0u32, 1).len(), 24);
}

#[test]
fn insert_reuses_gc_eligible_primary_tombstone() {
    let mut csr = MutableCsr::with_overflow_chunk_edges(4, 16, 8);
    for i in 0..4i64 {
        csr.insert_edge(0u32, VertexId::from_int64(i), EdgeId(100 + i as u64), 1)
            .unwrap();
    }
    assert!(csr.delete_edge(0u32, EdgeId(100), 10).unwrap());
    assert!(csr.delete_edge(0u32, EdgeId(101), 10).unwrap());
    assert_eq!(csr.edge_count(), 2);
    csr.set_tombstone_reuse_cutoff(10);
    csr.insert_edge(0u32, VertexId::from_int64(10), EdgeId(200), 11)
        .unwrap();
    assert_eq!(csr.edge_count(), 3);
    assert!(csr.get_overflow_chunks(0).is_none());
    let found = csr
        .get_edge_physical(0u32, VertexId::from_int64(10))
        .expect("reused slot holds the new edge");
    assert_eq!(found.edge_id, EdgeId(200));
}

#[test]
fn insert_without_reuse_cutoff_spills_to_overflow() {
    let mut csr = MutableCsr::with_overflow_chunk_edges(4, 16, 8);
    for i in 0..4i64 {
        csr.insert_edge(0u32, VertexId::from_int64(i), EdgeId(100 + i as u64), 1)
            .unwrap();
    }
    assert!(csr.delete_edge(0u32, EdgeId(100), 10).unwrap());
    assert!(csr.delete_edge(0u32, EdgeId(101), 10).unwrap());
    csr.insert_edge(0u32, VertexId::from_int64(10), EdgeId(200), 11)
        .unwrap();
    assert_eq!(csr.edge_count(), 3);
    let spilled: usize = csr
        .get_overflow_chunks(0)
        .map(|chunks| chunks.iter().map(|chunk| chunk.len()).sum())
        .unwrap_or(0);
    assert_eq!(spilled, 1);
}

#[test]
fn insert_keeps_pinned_tombstone_when_cutoff_below_delete_ts() {
    let mut csr = MutableCsr::with_overflow_chunk_edges(4, 16, 8);
    for i in 0..4i64 {
        csr.insert_edge(0u32, VertexId::from_int64(i), EdgeId(100 + i as u64), 1)
            .unwrap();
    }
    assert!(csr.delete_edge(0u32, EdgeId(100), 10).unwrap());
    csr.set_tombstone_reuse_cutoff(9);
    csr.insert_edge(0u32, VertexId::from_int64(10), EdgeId(200), 11)
        .unwrap();
    let spilled: usize = csr
        .get_overflow_chunks(0)
        .map(|chunks| chunks.iter().map(|chunk| chunk.len()).sum())
        .unwrap_or(0);
    assert_eq!(spilled, 1);
    let tombstone = csr
        .physical_edges_of(0u32)
        .into_iter()
        .find(|nbr| nbr.edge_id == EdgeId(100))
        .expect("pinned tombstone stays in the primary row");
    assert_eq!(tombstone.delete_ts, 10);
}

#[test]
fn live_set_installed_only_past_bound() {
    use super::live_set::LIVE_SET_WIDTH_BOUND;
    let mut csr = MutableCsr::with_overflow_chunk_edges(4, 64, 64);
    for i in 0..=(LIVE_SET_WIDTH_BOUND as i64) {
        csr.insert_edge(
            0u32,
            VertexId::from_int64(1000 + i),
            EdgeId(500 + i as u64),
            1,
        )
        .unwrap();
    }
    assert_eq!(csr.live_key_count(0), LIVE_SET_WIDTH_BOUND + 1);
    assert!(csr.live_sets.get(&0).is_some());
    assert!(csr
        .insert_edge(0u32, VertexId::from_int64(1000), EdgeId(999), 1)
        .is_err());
    // Narrow rows stay set-free yet answer duplicate checks through scans.
    csr.insert_edge(1u32, VertexId::from_int64(1), EdgeId(1), 1)
        .unwrap();
    assert!(csr.live_sets.get(&1).is_none());
    assert_eq!(csr.live_key_count(1), 1);
    assert!(csr.get_edge(1, VertexId::from_int64(1), 1).is_some());
    assert!(csr
        .insert_edge(1u32, VertexId::from_int64(1), EdgeId(2), 1)
        .is_err());
}

#[test]
fn dense_slots_reused_after_remove_and_reinsert() {
    let mut csr = MutableCsr::with_overflow_chunk_edges(4, 64, 8);
    for i in 0..10i64 {
        csr.insert_edge(0u32, VertexId::from_int64(i), EdgeId(i as u64), 1)
            .unwrap();
    }
    assert!(csr.get_overflow_chunks(0).is_some());
    for i in 0..10u64 {
        assert!(csr.rollback_insert(0u32, EdgeId(i)));
    }
    assert!(csr.get_overflow_chunks(0).is_none_or(Vec::is_empty));
    assert_eq!(csr.live_key_count(0), 0);
    assert_eq!(csr.edge_count(), 0);
    for i in 0..6i64 {
        csr.insert_edge(
            0u32,
            VertexId::from_int64(100 + i),
            EdgeId(100 + i as u64),
            1,
        )
        .unwrap();
    }
    let seen: Vec<EdgeId> = csr
        .physical_edges_of(0u32)
        .into_iter()
        .map(|nbr| nbr.edge_id)
        .collect();
    assert_eq!(seen.len(), 6);
    assert_eq!(csr.live_key_count(0), 6);
    let visited = {
        let mut count = 0usize;
        for (_, _) in csr.iter_all() {
            count += 1;
        }
        count
    };
    assert_eq!(visited, 6);
}

#[test]
fn bulk_insert_matches_sequential_inserts() {
    let mut sequential = MutableCsr::with_capacity(16, 64);
    let mut bulk = MutableCsr::with_capacity(16, 64);
    let mut groups: Vec<(u32, Vec<(u32, i64, EdgeId, u64)>)> = Vec::new();
    for src in 0..4u32 {
        let mut batch = Vec::new();
        for k in 0..20u64 {
            let edge_id = EdgeId(src as u64 * 100 + k);
            sequential
                .insert_edge(src, VertexId::from_int64(k as i64), edge_id, 1)
                .unwrap();
            batch.push((k as u32, 0, edge_id, 1));
        }
        groups.push((src, batch));
    }
    let count = bulk.batch_put_edges(&groups, true).unwrap();
    assert_eq!(count, 80);
    assert_eq!(bulk.edge_count(), sequential.edge_count());
    for src in 0..4u32 {
        let mut a = sequential.physical_edges_of(src);
        let mut b = bulk.physical_edges_of(src);
        a.sort_by_key(|nbr| nbr.edge_id.0);
        b.sort_by_key(|nbr| nbr.edge_id.0);
        assert_eq!(a, b);
    }
}

#[test]
fn positional_delete_and_revert_roundtrip() {
    use super::EdgePosition;
    let mut csr = MutableCsr::with_overflow_chunk_edges(4, 64, 8);
    for i in 0..10i64 {
        csr.insert_edge(0u32, VertexId::from_int64(i), EdgeId(i as u64), 1)
            .unwrap();
    }
    let (position, nbr) = csr.locate_edge(0u32, EdgeId(7)).expect("edge present");
    assert!(matches!(position, EdgePosition::Overflow { .. }));
    assert!(csr
        .delete_edge_at_position(0u32, position, nbr.edge_id, 2)
        .unwrap());
    assert!(!csr.row_live_scan(0, nbr.endpoint, nbr.rank).0);
    assert!(csr.revert_delete_at_position(0u32, position, nbr.edge_id, 2));
    assert!(csr.row_live_scan(0, nbr.endpoint, nbr.rank).0);
    assert!(!csr
        .delete_edge_at_position(0u32, position, EdgeId(999), 3)
        .unwrap());
}

#[test]
fn wide_row_point_lookup_uses_location_index() {
    let mut csr = MutableCsr::with_capacity(4, 64);
    for i in 0..20i64 {
        csr.insert_edge(0u32, VertexId::from_int64(i + 1), EdgeId(i as u64 + 1), 1)
            .unwrap();
    }
    assert!(csr.live_sets.get(&0).is_some());

    let hit = csr
        .get_edge(0u32, VertexId::from_int64(7), 1)
        .expect("indexed edge present");
    assert_eq!(hit.edge_id, EdgeId(7));
    assert_eq!(
        csr.get_edge_physical(0u32, VertexId::from_int64(7))
            .expect("indexed physical hit")
            .edge_id,
        EdgeId(7)
    );
    assert!(csr
        .get_edge(0u32, VertexId::from_int64(999), Timestamp::MAX)
        .is_none());
    assert!(csr
        .get_edge_physical(0u32, VertexId::from_int64(999))
        .is_none());

    assert!(csr.delete_edge(0u32, EdgeId(7), 2).unwrap());
    assert!(csr
        .get_edge_physical(0u32, VertexId::from_int64(7))
        .is_none());
    assert_eq!(
        csr.get_edge(0u32, VertexId::from_int64(7), 1)
            .expect("pre-delete version stays visible historically")
            .edge_id,
        EdgeId(7)
    );

    assert!(csr.rollback_insert(0u32, EdgeId(8)));
    for endpoint in [1i64, 2, 3, 9, 20] {
        let found = csr
            .get_edge_physical(0u32, VertexId::from_int64(endpoint))
            .expect("remaining edges stay addressable after gap close");
        assert_eq!(found.edge_id, EdgeId(endpoint as u64));
    }

    let bytes = csr.dump();
    let mut loaded = MutableCsr::new();
    loaded.load(&bytes).unwrap();
    assert!(loaded.live_sets.get(&0).is_some());
    assert_eq!(
        loaded
            .get_edge_physical(0u32, VertexId::from_int64(9))
            .expect("reloaded index hit")
            .edge_id,
        EdgeId(9)
    );
}

#[test]
fn consolidated_row_reads_single_block() {
    let mut csr = MutableCsr::with_overflow_chunk_edges(2, 16, 8);
    for i in 0..30i64 {
        csr.insert_edge(0u32, VertexId::from_int64(i + 1), EdgeId(i as u64 + 1), 1)
            .unwrap();
    }
    csr.rebalance_row(0u32);
    if let Some(chunks) = csr.overflow_chunks.get(0) {
        assert_eq!(chunks.len(), 1, "merged rows keep a single block");
    }
    let mut via_visit = Vec::new();
    csr.visit_physical(0u32, |nbr| {
        via_visit.push(nbr.edge_id);
        true
    });
    let mut via_fill = Vec::new();
    csr.fill_physical_into(0u32, &mut via_fill);
    assert_eq!(via_visit.len(), 30);
    assert_eq!(via_fill.len(), 30);
    let hit = csr
        .get_edge(0u32, VertexId::from_int64(17), 1)
        .expect("single-block row answers point lookups");
    assert_eq!(hit.edge_id, EdgeId(17));
}

#[test]
fn raw_dump_load_roundtrip_matches_encoded_mode() {
    use super::serialization::MUTABLE_CSR_FORMAT_RAW_VERSION;

    let mut csr = MutableCsr::with_overflow_chunk_edges(2, 16, 8);
    for i in 0..30i64 {
        csr.insert_edge(0u32, VertexId::from_int64(i + 1), EdgeId(i as u64 + 1), 1)
            .unwrap();
    }
    for i in 1..10i64 {
        csr.insert_edge(
            1u32,
            VertexId::from_int64(i + 100),
            EdgeId(1000 + i as u64),
            2,
        )
        .unwrap();
    }
    assert!(csr.delete_edge(0u32, EdgeId(5), 3).unwrap());

    let raw = csr.dump_raw();
    let marker = u32::from_le_bytes(raw[0..4].try_into().expect("raw header present"));
    assert_eq!(marker, MUTABLE_CSR_FORMAT_RAW_VERSION);

    let mut loaded = MutableCsr::new();
    loaded.load(&raw).expect("raw dump loads by marker");
    assert_eq!(loaded.edge_count(), csr.edge_count());

    let mut expected = Vec::new();
    csr.fill_physical_into(0u32, &mut expected);
    let mut actual = Vec::new();
    loaded.fill_physical_into(0u32, &mut actual);
    assert_eq!(actual, expected);

    let mut expected_one = Vec::new();
    csr.fill_physical_into(1u32, &mut expected_one);
    let mut actual_one = Vec::new();
    loaded.fill_physical_into(1u32, &mut actual_one);
    assert_eq!(actual_one, expected_one);

    assert!(loaded
        .get_edge(0u32, VertexId::from_int64(5), Timestamp::MAX)
        .is_none());
    assert_eq!(
        loaded
            .get_edge(0u32, VertexId::from_int64(5), 2)
            .expect("pre-delete version stays visible")
            .edge_id,
        EdgeId(5)
    );

    // Encoded dumps still load: the loader accepts both markers.
    let mut from_encoded = MutableCsr::new();
    from_encoded.load(&csr.dump()).expect("encoded dump loads");
    assert_eq!(from_encoded.edge_count(), csr.edge_count());
}

#[test]
fn raw_dump_rejects_bad_marker_and_truncation() {
    let mut csr = MutableCsr::with_capacity(4, 16);
    csr.insert_edge(0u32, VertexId::from_int64(1), EdgeId(7), 1)
        .unwrap();

    let mut bad_marker = csr.dump_raw();
    bad_marker[0..4].copy_from_slice(&99u32.to_le_bytes());
    assert!(MutableCsr::new().load(&bad_marker).is_err());

    let raw = csr.dump_raw();
    assert!(MutableCsr::new().load(&raw[..raw.len() - 1]).is_err());
    assert!(MutableCsr::new().load(&[]).is_err());
}

#[test]
fn test_offset_delete_shares_conflict_semantics() {
    let mut csr = MutableCsr::with_capacity(10, 100);
    csr.insert_edge(0u32, VertexId::from_int64(1), EdgeId(100), 10)
        .unwrap();
    assert!(csr.delete_edge_by_offset(0u32, 0, 20).unwrap());
    assert!(!csr.delete_edge_by_offset(0u32, 0, 20).unwrap());
    assert!(csr.delete_edge_by_offset(0u32, 0, 30).is_err());
    assert_eq!(csr.edge_count(), 0);
}

/// Cross-check the three delete-model ledgers after every mutation stage:
/// `edge_count`, the per-row live index (`live_key_count`) and the
/// fragmentation census must all agree with a full physical scan.
fn assert_delete_model_invariants(csr: &MutableCsr, vertex_range: std::ops::Range<u32>) {
    let mut total_live = 0usize;
    let mut buf = Vec::new();
    for vid in vertex_range {
        csr.fill_physical_into(vid, &mut buf);
        let live = buf
            .iter()
            .filter(|nbr| nbr.delete_ts == Timestamp::MAX)
            .count();
        let dead = buf.len() - live;
        total_live += live;
        assert_eq!(
            csr.live_key_count(vid),
            live,
            "live index drift on vertex {vid}"
        );
        let (census_live, census_dead, _) = csr.vertex_census(vid);
        assert_eq!(
            (census_live, census_dead),
            (live, dead),
            "census drift on vertex {vid}"
        );
    }
    assert_eq!(csr.edge_count(), total_live as u64, "edge count drift");
}

#[test]
fn test_delete_model_invariants_under_mixed_workload() {
    let mut csr = MutableCsr::with_overflow_chunk_edges(16, 128, 4);
    for i in 0..3u64 {
        csr.insert_edge(
            0u32,
            VertexId::from_int64(10 + i as i64),
            EdgeId(100 + i),
            1,
        )
        .unwrap();
    }
    for i in 0..20u64 {
        csr.insert_edge(
            1u32,
            VertexId::from_int64(100 + i as i64),
            EdgeId(200 + i),
            1,
        )
        .unwrap();
    }
    for i in 0..6u64 {
        csr.insert_edge(
            2u32,
            VertexId::from_int64(200 + i as i64),
            EdgeId(300 + i),
            1,
        )
        .unwrap();
    }
    assert_delete_model_invariants(&csr, 0..3);

    // Tombstone deletes across primary and overflow rows.
    assert!(csr.delete_edge(1u32, EdgeId(205), 5).unwrap());
    assert_eq!(
        csr.delete_edge_by_dst(2u32, VertexId::from_int64(202), 5),
        1
    );
    assert!(csr.delete_edge_by_offset(0u32, 0, 5).unwrap());
    assert_delete_model_invariants(&csr, 0..3);

    // Insert rollback erases without tombstone residue.
    assert!(csr.rollback_insert(1u32, EdgeId(210)));
    assert_delete_model_invariants(&csr, 0..3);

    // Maintenance passes must preserve the ledgers.
    let mut removed = Vec::new();
    csr.compact_vertex_with_reporting(1u32, 10, &mut |id, ts| {
        removed.push((id, ts));
    });
    assert_eq!(removed, vec![(EdgeId(205), 5)]);
    csr.rebalance_row(1u32);
    assert_delete_model_invariants(&csr, 0..3);
}
