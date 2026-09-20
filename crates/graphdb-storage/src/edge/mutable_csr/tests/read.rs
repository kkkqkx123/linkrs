use super::super::super::{EdgeId, VertexId};
use super::super::MutableCsr;

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
