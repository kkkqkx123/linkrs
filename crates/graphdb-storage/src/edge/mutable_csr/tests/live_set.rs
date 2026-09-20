use super::super::super::{EdgeId, Timestamp, VertexId};
use super::super::live_set::LIVE_SET_WIDTH_BOUND;
use super::super::MutableCsr;

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
fn live_set_installed_only_past_bound() {
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
