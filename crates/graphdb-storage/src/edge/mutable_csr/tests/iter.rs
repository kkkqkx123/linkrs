use super::super::super::{EdgeId, VertexId};
use super::super::MutableCsr;

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
