use super::super::super::{EdgeId, VertexId};
use super::super::MutableCsr;
use crate::edge::Nbr;

#[test]
fn test_fragmentation_ratio() {
    let mut csr = MutableCsr::with_capacity(10, 100);

    // No edges - ratio should be 0.0
    assert_eq!(csr.fragmentation_ratio(), 0.0);

    // Insert edges to trigger overflow
    for i in 1..=6 {
        let dst = VertexId::edge_endpoint_key(i as u32, 0);
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
        let dst = VertexId::edge_endpoint_key(i as u32, 0);
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
fn test_fragmentation_stats_report_dead_entries() {
    let mut csr = MutableCsr::with_capacity(10, 100);
    for i in 0..3u64 {
        let dst = VertexId::edge_endpoint_key(10 + i as u32, 0);
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
