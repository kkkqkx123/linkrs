use super::super::super::{EdgeId, VertexId};
use super::super::row::PACKED_CSR_DENSITY;
use super::super::MutableCsr;

#[test]
fn test_steady_state_gap_fill_before_overflow() {
    let mut csr = MutableCsr::with_capacity(10, 100);
    for i in 1..=5i64 {
        csr.insert_edge(0u32, VertexId::edge_endpoint_key((i) as u32, 0), EdgeId(i as u64), 1)
            .unwrap();
    }
    // 4 primary slots plus one overflow entry.
    assert!(csr.get_overflow_chunks(0).is_some());
    let overflow_before: usize = csr
        .get_overflow_chunks(0)
        .map(|chunks| chunks.iter().map(|chunk| chunk.len()).sum())
        .unwrap_or(0);
    assert_eq!(overflow_before, 1);

    // Reclaim two primary tombstones at an eligible cutoff so trailing
    // gaps open without touching overflow.
    assert!(csr.delete_edge(0u32, EdgeId(1), 2).unwrap());
    assert!(csr.delete_edge(0u32, EdgeId(2), 2).unwrap());
    let removed = csr.compact_vertex_with_reporting(0, 3, &mut |_, _| {});
    assert_eq!(removed, 2);

    // Everyday writes fill the freed primary gaps first even though
    // overflow exists: overflow length stays put.
    csr.insert_edge(0u32, VertexId::edge_endpoint_key(10, 0), EdgeId(10), 3)
        .unwrap();
    csr.insert_edge(0u32, VertexId::edge_endpoint_key(11, 0), EdgeId(11), 3)
        .unwrap();
    let overflow_after: usize = csr
        .get_overflow_chunks(0)
        .map(|chunks| chunks.iter().map(|chunk| chunk.len()).sum())
        .unwrap_or(0);
    assert_eq!(overflow_after, overflow_before);
    assert_eq!(csr.edges_of(0u32, 3).len(), 5);
}

#[test]
fn test_rebalance_row_drains_overflow_into_gaps() {
    let mut csr = MutableCsr::with_overflow_chunk_edges(10, 100, 2);
    for i in 0..6u64 {
        csr.insert_edge(0u32, VertexId::edge_endpoint_key(100 + i as u32, 0), EdgeId(i), 1)
            .unwrap();
    }
    assert!(csr.get_overflow_chunks(0).is_some());
    // Reclaim primary tombstones so gaps open, then rebalance pulls the
    // overflow live entries back into the primary row.
    assert!(csr.delete_edge(0u32, EdgeId(0), 2).unwrap());
    assert!(csr.delete_edge(0u32, EdgeId(1), 2).unwrap());
    let removed = csr.compact_vertex_with_reporting(0, 3, &mut |_, _| {});
    assert_eq!(removed, 2);
    assert!(csr.rebalance_row(0));
    assert!(csr.get_overflow_chunks(0).is_none_or(Vec::is_empty));
    assert_eq!(csr.edges_of(0u32, 3).len(), 4);
}

#[test]
fn test_row_gap_and_density_observe_reserve() {
    let mut csr = MutableCsr::with_capacity(10, 100);
    csr.insert_edge(0u32, VertexId::edge_endpoint_key(1, 0), EdgeId(1), 1)
        .unwrap();
    // One live entry in a 4-slot block: three write gaps remain.
    assert_eq!(csr.row_gap(0), 3);
    assert!((csr.row_density(0) - 0.25).abs() < 1e-6);
    // Rebuilds size rows at the packed density target with gaps.
    let removed = csr.compact_with_ts_reporting(2, 1.0 - PACKED_CSR_DENSITY, &mut |_, _| {});
    assert_eq!(removed, 0);
    assert_eq!(csr.row_gap(0), 1);
    assert!((csr.row_density(0) - 0.5).abs() < 1e-6);
}

#[test]
fn test_sized_row_capacity_pins_density_formula() {
    // Pins `ceil(live / PACKED_CSR_DENSITY)`: changing the density target
    // must update this test, which is the point — density retunes are
    // explicit, never silent.
    assert_eq!(MutableCsr::sized_row_capacity(0), 0);
    // live=1: ceil(1/0.8)=2 beats the tiny-row floor, so capacity is 2.
    assert_eq!(MutableCsr::sized_row_capacity(1), 2);
    assert_eq!(
        MutableCsr::sized_row_capacity(8),
        (8.0f32 / PACKED_CSR_DENSITY).ceil() as usize
    );
    assert_eq!(
        MutableCsr::sized_row_capacity(100),
        (100.0f32 / PACKED_CSR_DENSITY).ceil() as usize
    );
    // Capacity always covers live entries and stays monotonic.
    let mut prev = 0usize;
    for live in 0..500usize {
        let cap = MutableCsr::sized_row_capacity(live);
        assert!(cap >= live, "live={live} cap={cap}");
        assert!(cap >= prev, "live={live} cap={cap} prev={prev}");
        prev = cap;
    }
}

#[test]
fn test_graded_sizing_beats_naive_doubling_for_small_rows() {
    // Control against naive geometric growth: doubling from the tiny-row
    // floor would reserve 8 slots for a 5-edge row and keep doubling past
    // every small growth step, while the density sizing reserves exactly the
    // packed target. Small rows must stay small; large rows converge to the
    // same linear reserve either way.
    use super::super::row::graded_overflow_chunk_edges;
    for live in [1usize, 5, 9, 33] {
        let graded = MutableCsr::sized_row_capacity(live);
        let doubled = live.next_power_of_two().max(4);
        assert!(
            graded <= doubled,
            "live={live} graded={graded} doubled={doubled}"
        );
        assert!(graded >= live, "live={live} graded={graded}");
    }
    for live in [100usize, 1000] {
        let graded = MutableCsr::sized_row_capacity(live);
        assert!((graded as f32 - live as f32 / PACKED_CSR_DENSITY).abs() < 2.0);
    }
    // Overflow grading shares the same discipline: monotonic, floored for
    // small rows, capped at one allocation unit for supernodes.
    let mut prev = 0usize;
    for live in [0usize, 1, 5, 8, 9, 100, 5000, 1_000_000] {
        let chunk = graded_overflow_chunk_edges(live);
        assert!(chunk >= prev, "live={live} chunk={chunk} prev={prev}");
        prev = chunk;
    }
}
