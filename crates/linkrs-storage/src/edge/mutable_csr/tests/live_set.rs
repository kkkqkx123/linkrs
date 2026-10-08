use super::super::super::{EdgeId, Timestamp, VertexId};
use super::super::live_set::LIVE_SET_WIDTH_BOUND;
use super::super::MutableCsr;

#[test]
fn test_single_live_set_rejects_duplicates_across_tiers() {
    let mut csr = MutableCsr::with_capacity(10, 100);
    for i in 1..=6i64 {
        csr.insert_edge(
            0u32,
            VertexId::edge_endpoint_key((i) as u32, 0),
            EdgeId(i as u64),
            1,
        )
        .unwrap();
    }
    // Primary-tier duplicate rejected without any scan fallback.
    assert!(csr
        .insert_edge(0u32, VertexId::edge_endpoint_key(1, 0), EdgeId(100), 1)
        .is_err());
    // Overflow-tier duplicate rejected through the same single set.
    assert!(csr
        .insert_edge(0u32, VertexId::edge_endpoint_key(6, 0), EdgeId(101), 1)
        .is_err());
    assert_eq!(csr.edge_count(), 6);
}

#[test]
fn live_set_installed_only_past_bound() {
    let mut csr = MutableCsr::with_overflow_chunk_edges(4, 64, 64);
    for i in 0..=(LIVE_SET_WIDTH_BOUND as i64) {
        csr.insert_edge(
            0u32,
            VertexId::edge_endpoint_key((1000 + i) as u32, 0),
            EdgeId(500 + i as u64),
            1,
        )
        .unwrap();
    }
    assert_eq!(csr.live_key_count(0), LIVE_SET_WIDTH_BOUND + 1);
    assert!(csr.live_sets.get(&0).is_some());
    assert!(csr
        .insert_edge(0u32, VertexId::edge_endpoint_key(1000, 0), EdgeId(999), 1)
        .is_err());
    // Narrow rows stay set-free yet answer duplicate checks through scans.
    csr.insert_edge(1u32, VertexId::edge_endpoint_key(1, 0), EdgeId(1), 1)
        .unwrap();
    assert!(csr.live_sets.get(&1).is_none());
    assert_eq!(csr.live_key_count(1), 1);
    assert!(csr
        .get_edge(1, VertexId::edge_endpoint_key(1, 0), 1)
        .is_some());
    assert!(csr
        .insert_edge(1u32, VertexId::edge_endpoint_key(1, 0), EdgeId(2), 1)
        .is_err());
}

#[test]
fn wide_row_point_lookup_uses_location_index() {
    let mut csr = MutableCsr::with_capacity(4, 64);
    for i in 0..20i64 {
        csr.insert_edge(
            0u32,
            VertexId::edge_endpoint_key((i + 1) as u32, 0),
            EdgeId(i as u64 + 1),
            1,
        )
        .unwrap();
    }
    assert!(csr.live_sets.get(&0).is_some());

    let hit = csr
        .get_edge(0u32, VertexId::edge_endpoint_key(7, 0), 1)
        .expect("indexed edge present");
    assert_eq!(hit.edge_id, EdgeId(7));
    assert_eq!(
        csr.get_edge_physical(0u32, VertexId::edge_endpoint_key(7, 0))
            .expect("indexed physical hit")
            .edge_id,
        EdgeId(7)
    );
    assert!(csr
        .get_edge(0u32, VertexId::edge_endpoint_key(999, 0), Timestamp::MAX)
        .is_none());
    assert!(csr
        .get_edge_physical(0u32, VertexId::edge_endpoint_key(999, 0))
        .is_none());

    assert!(csr.delete_edge(0u32, EdgeId(7), 2).unwrap());
    assert!(csr
        .get_edge_physical(0u32, VertexId::edge_endpoint_key(7, 0))
        .is_none());
    assert_eq!(
        csr.get_edge(0u32, VertexId::edge_endpoint_key(7, 0), 1)
            .expect("pre-delete version stays visible historically")
            .edge_id,
        EdgeId(7)
    );

    assert!(csr.rollback_insert(0u32, EdgeId(8)));
    for endpoint in [1i64, 2, 3, 9, 20] {
        let found = csr
            .get_edge_physical(0u32, VertexId::edge_endpoint_key((endpoint) as u32, 0))
            .expect("remaining edges stay addressable after gap close");
        assert_eq!(found.edge_id, EdgeId(endpoint as u64));
    }

    let bytes = csr.dump();
    let mut loaded = MutableCsr::new();
    loaded.load(&bytes).unwrap();
    assert!(loaded.live_sets.get(&0).is_some());
    assert_eq!(
        loaded
            .get_edge_physical(0u32, VertexId::edge_endpoint_key(9, 0))
            .expect("reloaded index hit")
            .edge_id,
        EdgeId(9)
    );
}

#[test]
fn threshold_oscillation_rebuilds_exactly_and_frees_index_memory() {
    // Width jitter around the bound must not accumulate stale indexes: one
    // rebuild drops the set once the row narrows, the rebuild counter moves
    // by exactly one, and index heap memory returns to zero while the
    // indexed lookups agreed with scans throughout.
    let mut csr = MutableCsr::with_overflow_chunk_edges(4, 64, 64);
    for i in 0..=(LIVE_SET_WIDTH_BOUND as i64) {
        csr.insert_edge(
            0u32,
            VertexId::edge_endpoint_key((1000 + i) as u32, 0),
            EdgeId(500 + i as u64),
            1,
        )
        .unwrap();
    }
    assert!(csr.live_sets.get(&0).is_some());
    assert!(csr.has_live_set(&0));
    let heap_wide = csr.live_sets.heap_bytes_total();
    assert!(heap_wide > 0);
    let rebuilds_before = csr.live_set_rebuild_count();

    // Indexed and scan paths agree before narrowing. Timestamp 1 is the
    // insert time: the scan path checks `ts < delete_ts`, so MAX never hits
    // there by construction.
    for i in 0..=(LIVE_SET_WIDTH_BOUND as i64) {
        let key = VertexId::edge_endpoint_key((1000 + i) as u32, 0);
        assert_eq!(
            csr.get_edge(0u32, key, 1).expect("indexed hit").edge_id,
            csr.get_edge_physical(0u32, key)
                .expect("physical hit")
                .edge_id,
        );
    }

    assert!(csr.delete_edge(0u32, EdgeId(500), 2).unwrap());
    csr.rebuild_live_set_for_vertex(0);
    assert_eq!(csr.live_set_rebuild_count(), rebuilds_before + 1);
    assert!(csr.live_sets.get(&0).is_none());
    assert!(!csr.has_live_set(&0));
    assert_eq!(csr.live_sets.heap_bytes_total(), 0);
    assert_eq!(csr.live_key_count(0), LIVE_SET_WIDTH_BOUND);
    // The narrowed row still answers through scans at a post-delete time.
    assert!(csr
        .get_edge(0u32, VertexId::edge_endpoint_key(1001, 0), 3)
        .is_some());
    assert!(csr
        .get_edge(0u32, VertexId::edge_endpoint_key(1000, 0), 3)
        .is_none());
}

#[test]
fn wide_row_index_memory_stays_proportional_to_width() {
    // Index cost transparency: 200 live entries must cost no more than one
    // entry slot per key plus table slabs, never a capacity-proportional
    // reservation.
    let mut csr = MutableCsr::with_overflow_chunk_edges(4, 512, 64);
    for i in 0..200i64 {
        csr.insert_edge(
            0u32,
            VertexId::edge_endpoint_key((i + 1) as u32, 0),
            EdgeId(i as u64 + 1),
            1,
        )
        .unwrap();
    }
    assert!(csr.live_sets.get(&0).is_some());
    let entry = std::mem::size_of::<((u32, i64), super::super::write::EdgePosition)>() + 8;
    assert!(csr.live_sets.heap_bytes_total() <= 200 * entry);
    assert_eq!(csr.live_key_count(0), 200);
}

#[test]
fn huge_degree_index_agrees_with_scan_and_stays_proportional() {
    // Cost transparency at supernode scale: a multi-thousand-edge row keeps
    // one index entry per live key, indexed hits and misses agree with the
    // physical scan path, and narrowing drops the index with exactly one
    // rebuild while heap memory returns to zero.
    let mut csr = MutableCsr::with_overflow_chunk_edges(4, 4096, 64);
    for i in 0..2000i64 {
        csr.insert_edge(
            0u32,
            VertexId::edge_endpoint_key((i + 1) as u32, 0),
            EdgeId(i as u64 + 1),
            1,
        )
        .unwrap();
    }
    assert!(csr.live_sets.get(&0).is_some());
    assert_eq!(csr.live_key_count(0), 2000);
    let entry = std::mem::size_of::<((u32, i64), super::super::write::EdgePosition)>() + 8;
    assert!(csr.live_sets.heap_bytes_total() <= 2000 * entry);
    for probe in [1i64, 777, 2000] {
        let key = VertexId::edge_endpoint_key((probe) as u32, 0);
        assert_eq!(
            csr.get_edge(0u32, key, 1).map(|nbr| nbr.edge_id),
            csr.get_edge_physical(0u32, key).map(|nbr| nbr.edge_id),
        );
    }
    assert!(csr
        .get_edge(0u32, VertexId::edge_endpoint_key(999_999_u32, 0), 1)
        .is_none());
    assert!(csr
        .get_edge_physical(0u32, VertexId::edge_endpoint_key(999_999_u32, 0))
        .is_none());
    let rebuilds_before = csr.live_set_rebuild_count();
    for i in 0..1992i64 {
        assert!(csr.delete_edge(0u32, EdgeId(i as u64 + 1), 2).unwrap());
    }
    csr.rebuild_live_set_for_vertex(0);
    assert_eq!(csr.live_set_rebuild_count(), rebuilds_before + 1);
    assert!(csr.live_sets.get(&0).is_none());
    assert_eq!(csr.live_sets.heap_bytes_total(), 0);
    assert_eq!(csr.live_key_count(0), 8);
}
