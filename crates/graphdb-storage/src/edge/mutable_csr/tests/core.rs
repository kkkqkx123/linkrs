use super::super::super::{EdgeId, VertexId};
use super::super::MutableCsr;

#[test]
fn test_basic_insert_and_query() {
    let mut csr = MutableCsr::with_capacity(10, 100);

    csr.insert_edge(0u32, VertexId::from_int64(1), EdgeId(100), 1)
        .unwrap();
    csr.insert_edge(0u32, VertexId::from_int64(2), EdgeId(101), 1)
        .unwrap();
    csr.insert_edge(1u32, VertexId::from_int64(3), EdgeId(102), 1)
        .unwrap();

    assert!(csr
        .insert_edge(0u32, VertexId::from_int64(1), EdgeId(103), 1)
        .is_err());

    assert_eq!(csr.edge_count(), 3);
}

#[test]
fn test_resize() {
    let mut csr = MutableCsr::with_capacity(2, 10);

    csr.insert_edge(0u32, VertexId::from_int64(1), EdgeId(100), 1)
        .unwrap();
    csr.insert_edge(100u32, VertexId::from_int64(1), EdgeId(101), 1)
        .unwrap();

    assert!(csr.vertex_capacity() >= 101);
}

#[test]
fn test_zero_degree_rows_hold_no_slots() {
    let mut csr = MutableCsr::with_capacity(1024, 4096);
    assert_eq!(csr.total_edge_capacity, 0);

    // A single edge allocates exactly one primary block
    csr.insert_edge(0u32, VertexId::from_int64(1), EdgeId(100), 1)
        .unwrap();
    assert_eq!(csr.total_edge_capacity, 4);

    // Sparse high vertex ids allocate blocks only for themselves
    csr.insert_edge(10_000u32, VertexId::from_int64(2), EdgeId(101), 1)
        .unwrap();
    assert_eq!(csr.vertex_capacity(), 12_502);
    assert_eq!(csr.total_edge_capacity, 8);

    // Growth is proportional (1.25x), not power-of-two doubling
    assert_eq!(csr.vertex_capacity(), (10_001.0_f64 * 1.25).ceil() as usize);

    // Compact reclaims slots of rows whose edges were all removed
    csr.delete_edge(0u32, EdgeId(100), 2).unwrap();
    csr.compact_with_ts_reporting(3, 0.0, &mut |_, _| {});
    assert_eq!(csr.total_edge_capacity, 1);
    assert_eq!(csr.rows.primary_capacities[0], 0);
}
