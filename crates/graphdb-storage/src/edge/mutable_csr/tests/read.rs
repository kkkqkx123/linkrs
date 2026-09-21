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

#[test]
fn primary_sorted_flag_tracks_writes_and_sort() {
    let mut csr = MutableCsr::with_capacity(4, 64);
    assert!(csr.primary_sorted_flag(0));
    // Any primary key write clears the flag, even an in-order append.
    csr.insert_edge(0u32, VertexId::edge_endpoint_key(30, 0), EdgeId(1), 1)
        .unwrap();
    assert!(!csr.primary_sorted_flag(0));
    // A single-entry row sorts trivially and re-establishes the flag.
    assert!(!csr.sort_row(0u32));
    assert!(csr.primary_sorted_flag(0));
    assert!(csr.is_primary_sorted(0u32));
    // Out-of-order second key clears again; the maintenance sort restores.
    csr.insert_edge(0u32, VertexId::edge_endpoint_key(10, 0), EdgeId(2), 1)
        .unwrap();
    assert!(!csr.primary_sorted_flag(0));
    assert!(!csr.is_primary_sorted(0u32));
    assert!(csr.sort_row(0u32));
    assert!(csr.primary_sorted_flag(0));
    assert!(csr.is_primary_sorted(0u32));
    // Cold-only deletes keep the key order, so the flag survives.
    csr.insert_edge(0u32, VertexId::edge_endpoint_key(20, 0), EdgeId(3), 1)
        .unwrap();
    assert!(csr.sort_row(0u32));
    assert!(csr.delete_edge(0u32, EdgeId(2), 2).unwrap());
    assert!(csr.primary_sorted_flag(0));
    assert!(csr.is_primary_sorted(0u32));
}

#[test]
fn threshold_matches_linear_results_on_both_flag_states() {
    let mut csr = MutableCsr::with_capacity(4, 64);
    for (endpoint, edge) in [(50u32, 1u64), (10, 2), (30, 3), (20, 4), (40, 5)] {
        csr.insert_edge(
            0u32,
            VertexId::edge_endpoint_key(endpoint, 0),
            EdgeId(edge),
            1,
        )
        .unwrap();
    }
    assert!(!csr.primary_sorted_flag(0));
    let mut unsorted_window = Vec::new();
    csr.fill_threshold_into(0u32, Some((15, 0)), Some((45, 0)), &mut unsorted_window);
    assert!(csr.sort_row(0u32));
    assert!(csr.primary_sorted_flag(0));
    let mut sorted_window = Vec::new();
    csr.fill_threshold_into(0u32, Some((15, 0)), Some((45, 0)), &mut sorted_window);
    let mut unsorted_keys: Vec<(u32, i64)> = unsorted_window
        .iter()
        .map(|nbr| (nbr.endpoint, nbr.rank))
        .collect();
    let mut sorted_keys: Vec<(u32, i64)> = sorted_window
        .iter()
        .map(|nbr| (nbr.endpoint, nbr.rank))
        .collect();
    unsorted_keys.sort_unstable();
    sorted_keys.sort_unstable();
    assert_eq!(unsorted_keys, sorted_keys);
    assert_eq!(sorted_keys, vec![(20, 0), (30, 0), (40, 0)]);
}
