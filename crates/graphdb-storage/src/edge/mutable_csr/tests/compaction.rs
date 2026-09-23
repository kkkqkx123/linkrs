use super::super::super::{EdgeId, Timestamp, VertexId};
use super::super::MutableCsr;

#[test]
fn test_compact_with_ts_merges_overflow() {
    let mut csr = MutableCsr::with_capacity(10, 100);

    for i in 1..=6 {
        let dst = VertexId::edge_endpoint_key(i as u32, 0);
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
        let dst = VertexId::edge_endpoint_key(i as u32, 0);
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
        let dst = VertexId::edge_endpoint_key(i as u32, 0);
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
        csr.insert_edge(0u32, VertexId::edge_endpoint_key((i) as u32, 0), EdgeId(i as u64), 1)
            .unwrap();
    }
    csr.insert_edge(1u32, VertexId::edge_endpoint_key(1, 0), EdgeId(7), 1)
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
        csr.insert_edge(0u32, VertexId::edge_endpoint_key((i) as u32, 0), EdgeId(i as u64), 1)
            .unwrap();
    }
    let removed = csr.compact_with_ts_reporting(3, 0.0, &mut |_, _| {});
    assert_eq!(removed, 0);
    assert_eq!(csr.total_edge_capacity, 3);
    assert_eq!(csr.edges_of(0u32, 3).len(), 3);
}

#[test]
fn test_overflow_cleared_after_compact() {
    let mut csr = MutableCsr::with_overflow_chunk_edges(10, 100, 2);
    for vid in 0..20u32 {
        for i in 0..8 {
            let dst = VertexId::edge_endpoint_key((vid as u32 + 1) * 100 + i as u32, 0);
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
    csr.insert_edge(0u32, VertexId::edge_endpoint_key(1, 0), EdgeId(100), 1)
        .unwrap();
    csr.insert_edge(0u32, VertexId::edge_endpoint_key(2, 0), EdgeId(101), 1)
        .unwrap();
    csr.insert_edge(5u32, VertexId::edge_endpoint_key(6, 0), EdgeId(102), 1)
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
    csr.insert_edge(0u32, VertexId::edge_endpoint_key(1, 0), EdgeId(100), 1)
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
        let dst = VertexId::edge_endpoint_key(100 + i as u32, 0);
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
fn test_compact_reduces_fragmentation() {
    let mut csr = MutableCsr::with_capacity(10, 100);

    for i in 1..=6 {
        let dst = VertexId::edge_endpoint_key(i as u32, 0);
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
