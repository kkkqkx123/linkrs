use super::super::super::{EdgeId, VertexId};
use super::super::{EdgePosition, MutableCsr};

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
fn test_offset_delete_shares_conflict_semantics() {
    let mut csr = MutableCsr::with_capacity(10, 100);
    csr.insert_edge(0u32, VertexId::from_int64(1), EdgeId(100), 10)
        .unwrap();
    assert!(csr.delete_edge_by_offset(0u32, 0, 20).unwrap());
    assert!(!csr.delete_edge_by_offset(0u32, 0, 20).unwrap());
    assert!(csr.delete_edge_by_offset(0u32, 0, 30).is_err());
    assert_eq!(csr.edge_count(), 0);
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
