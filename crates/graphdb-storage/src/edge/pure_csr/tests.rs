use super::super::{EdgePosition, MutableCsrTrait};
use super::*;
use graphdb_core::types::{EdgeId, Timestamp, VertexId};

#[test]
fn nonzero_rank_is_rejected() {
    let mut csr = PureTopologyCsr::with_capacity(4, 16);
    let err = csr
        .insert_edge(0, VertexId::edge_endpoint_key(1, 1), EdgeId(0), 0)
        .expect_err("nonzero rank has no meaning without a rank column");
    assert!(err.to_string().contains("rank must be 0"));
    assert_eq!(csr.edge_count(), 0);
}

#[test]
fn wide_row_absent_key_short_circuits() {
    let mut csr = PureTopologyCsr::with_capacity(16, 64);
    for dst in 0..12u32 {
        csr.insert_edge(
            0,
            VertexId::edge_endpoint_key(dst, 0),
            EdgeId(dst as u64),
            1,
        )
        .expect("insert");
    }
    assert!(
        csr.live_sets.get(0).is_some(),
        "wide row must carry an index"
    );
    let absent = VertexId::edge_endpoint_key(900, 0);
    assert!(csr.get_edge_physical(0, absent).is_none());
    assert!(csr.get_edge(0, absent, Timestamp::MAX).is_none());
    let present = VertexId::edge_endpoint_key(3, 0);
    assert!(csr.get_edge_physical(0, present).is_some());
    assert!(csr.get_edge(0, present, Timestamp::MAX).is_some());
}

#[test]
fn count_index_capacity_stay_consistent() {
    let mut csr = PureTopologyCsr::with_overflow_chunk_edges(8, 32, 4);
    for dst in 0..10u32 {
        csr.insert_edge(
            0,
            VertexId::edge_endpoint_key(dst, 0),
            EdgeId(dst as u64),
            0,
        )
        .expect("insert");
    }
    csr.delete_edge_by_offset(0, 0, 0).expect("delete");
    let live: usize = {
        let mut buf = Vec::new();
        csr.fill_physical_into(0, &mut buf);
        buf.into_iter()
            .filter(|nbr| nbr.edge_id != INVALID_EDGE_ID)
            .count()
    };
    assert_eq!(csr.edge_count(), live as u64);
    assert!(csr.total_edge_capacity >= csr.endpoints.len());
    assert!(csr.total_edge_capacity >= live);
    let mut rebuilt = 0usize;
    if let Some(chunks) = csr.overflow_chunks.get(0) {
        rebuilt += chunks.iter().map(|chunk| chunk.len()).sum::<usize>();
    }
    let (start, end) = csr.primary_window(0);
    rebuilt += end - start;
    assert!(csr.total_edge_capacity >= rebuilt);
    assert!(!csr.revert_delete_by_offset(0, 0, 0));
    assert!(!csr.revert_delete_by_edge_id(0, EdgeId(999), 0));
}

#[test]
fn positioned_revert_restores_deleted_slot() {
    let mut csr = PureTopologyCsr::with_capacity(8, 16);
    csr.insert_edge(0, VertexId::edge_endpoint_key(1, 0), EdgeId(10), 0)
        .expect("insert");
    csr.insert_edge(0, VertexId::edge_endpoint_key(2, 0), EdgeId(11), 0)
        .expect("insert");
    let position = EdgePosition::Primary { slot: 0 };
    assert!(csr
        .delete_edge_at_position(0, position, EdgeId(10), 0)
        .expect("positioned delete"));
    assert_eq!(csr.edge_count(), 1);
    assert!(csr.revert_delete_at_position(0, position, EdgeId(10), 0));
    assert_eq!(csr.edge_count(), 2);
    assert!(csr
        .get_edge_physical(0, VertexId::edge_endpoint_key(1, 0))
        .is_some());
}

#[test]
fn sort_row_orders_live_prefix_and_threshold_bisects() {
    let mut csr = PureTopologyCsr::with_capacity(4, 16);
    for (endpoint, edge) in [(30, 1), (10, 2), (20, 3)] {
        csr.insert_edge(0, VertexId::edge_endpoint_key(endpoint, 0), EdgeId(edge), 0)
            .expect("insert");
    }
    assert!(!csr.is_row_sorted(0));
    assert!(csr.sort_row(0));
    assert!(csr.is_row_sorted(0));
    let mut ranged = Vec::new();
    csr.fill_threshold_into(0, Some(15), Some(25), &mut ranged);
    assert_eq!(ranged.len(), 1);
    assert_eq!(ranged[0].endpoint, 20);
}

#[test]
fn primary_sorted_flag_tracks_writes_and_sort() {
    let mut csr = PureTopologyCsr::with_capacity(4, 16);
    assert!(csr.primary_sorted_flag(0));
    // Any primary key write clears the flag, even an in-order append.
    csr.insert_edge(0, VertexId::edge_endpoint_key(30, 0), EdgeId(1), 0)
        .expect("insert");
    assert!(!csr.primary_sorted_flag(0));
    // A single-entry row sorts trivially and re-establishes the flag.
    assert!(!csr.sort_row(0));
    assert!(csr.primary_sorted_flag(0));
    assert!(csr.is_primary_sorted(0));
    // Out-of-order second key clears again; the maintenance sort restores.
    csr.insert_edge(0, VertexId::edge_endpoint_key(10, 0), EdgeId(2), 0)
        .expect("insert");
    assert!(!csr.primary_sorted_flag(0));
    assert!(!csr.is_primary_sorted(0));
    assert!(csr.sort_row(0));
    assert!(csr.primary_sorted_flag(0));
    assert!(csr.is_primary_sorted(0));
    // Sentinel deletes keep the key order, so the flag survives.
    csr.insert_edge(0, VertexId::edge_endpoint_key(20, 0), EdgeId(3), 0)
        .expect("insert");
    assert!(csr.sort_row(0));
    assert!(csr.delete_edge(0, EdgeId(2), 0).expect("delete"));
    assert!(csr.primary_sorted_flag(0));
    assert!(csr.is_primary_sorted(0));
}

#[test]
fn threshold_matches_linear_results_on_both_flag_states() {
    let mut csr = PureTopologyCsr::with_capacity(4, 16);
    for (endpoint, edge) in [(50u32, 1u64), (10, 2), (30, 3), (20, 4), (40, 5)] {
        csr.insert_edge(0, VertexId::edge_endpoint_key(endpoint, 0), EdgeId(edge), 0)
            .expect("insert");
    }
    assert!(!csr.primary_sorted_flag(0));
    let mut unsorted_window = Vec::new();
    csr.fill_threshold_into(0, Some(15), Some(45), &mut unsorted_window);
    assert!(csr.sort_row(0));
    assert!(csr.primary_sorted_flag(0));
    let mut sorted_window = Vec::new();
    csr.fill_threshold_into(0, Some(15), Some(45), &mut sorted_window);
    let mut unsorted_keys: Vec<u32> = unsorted_window.iter().map(|nbr| nbr.endpoint).collect();
    let mut sorted_keys: Vec<u32> = sorted_window.iter().map(|nbr| nbr.endpoint).collect();
    unsorted_keys.sort_unstable();
    sorted_keys.sort_unstable();
    assert_eq!(unsorted_keys, sorted_keys);
    assert_eq!(sorted_keys, vec![20, 30, 40]);
}
