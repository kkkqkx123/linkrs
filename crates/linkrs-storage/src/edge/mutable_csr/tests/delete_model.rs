use super::super::super::{EdgeId, Timestamp, VertexId};
use super::super::MutableCsr;

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
fn test_remove_after_delete_does_not_double_count() {
    let mut csr = MutableCsr::with_capacity(10, 100);
    csr.insert_edge(0u32, VertexId::edge_endpoint_key(1, 0), EdgeId(100), 1)
        .unwrap();
    csr.insert_edge(0u32, VertexId::edge_endpoint_key(2, 0), EdgeId(101), 1)
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
        csr.insert_edge(
            0u32,
            VertexId::edge_endpoint_key(100 + i as u32, 0),
            EdgeId(i),
            1,
        )
        .unwrap();
    }
    assert_eq!(csr.edge_count(), 6);
    assert!(csr.delete_edge(0u32, EdgeId(5), 2).unwrap());
    assert_eq!(csr.edge_count(), 5);
    assert!(csr.rollback_insert(0u32, EdgeId(5)));
    assert_eq!(csr.edge_count(), 5);
}

#[test]
fn test_delete_model_invariants_under_mixed_workload() {
    let mut csr = MutableCsr::with_overflow_chunk_edges(16, 128, 4);
    for i in 0..3u64 {
        csr.insert_edge(
            0u32,
            VertexId::edge_endpoint_key(10 + i as u32, 0),
            EdgeId(100 + i),
            1,
        )
        .unwrap();
    }
    for i in 0..20u64 {
        csr.insert_edge(
            1u32,
            VertexId::edge_endpoint_key(100 + i as u32, 0),
            EdgeId(200 + i),
            1,
        )
        .unwrap();
    }
    for i in 0..6u64 {
        csr.insert_edge(
            2u32,
            VertexId::edge_endpoint_key(200 + i as u32, 0),
            EdgeId(300 + i),
            1,
        )
        .unwrap();
    }
    assert_delete_model_invariants(&csr, 0..3);

    // Tombstone deletes across primary and overflow rows.
    assert!(csr.delete_edge(1u32, EdgeId(205), 5).unwrap());
    assert_eq!(
        csr.delete_edge_by_dst(2u32, VertexId::edge_endpoint_key(202, 0), 5),
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
