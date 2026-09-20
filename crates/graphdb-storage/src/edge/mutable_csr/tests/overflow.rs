use super::super::super::{EdgeId, Timestamp, VertexId};
use super::super::overflow::OVERFLOW_REPACK_CHUNKS_PER_VERTEX;
use super::super::row::{
    graded_overflow_chunk_edges, OVERFLOW_CHUNK_MAX, OVERFLOW_CHUNK_MIN,
};
use super::super::MutableCsr;

#[test]
fn test_overflow_insert() {
    let mut csr = MutableCsr::with_capacity(10, 100);

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

    assert_eq!(csr.edge_count(), 5);

    let edges = csr.edges_of(0u32, 1);
    assert_eq!(edges.len(), 5);

    assert!(csr
        .insert_edge(0u32, VertexId::from_int64(5), EdgeId(105), 1)
        .is_err());

    assert!(csr.delete_edge(0u32, EdgeId(104), 2).unwrap());
}

#[test]
fn test_overflow_storage_lookup() {
    let mut csr = MutableCsr::with_overflow_chunk_edges(10, 100, 2);
    for vid in 0..5u32 {
        for i in 0..6 {
            let dst = VertexId::from_int64((vid as i64 + 1) * 100 + i as i64);
            csr.insert_edge(vid, dst, EdgeId(vid as u64 * 10 + i as u64), 1)
                .unwrap();
        }
    }
    assert!(csr.get_overflow_chunks(0).is_some());
    assert!(csr.get_overflow_chunks(999).is_none());
}

#[test]
fn test_overflow_get_chunks_transparent() {
    let mut csr = MutableCsr::with_overflow_chunk_edges(10, 100, 2);
    for vid in 0..20u32 {
        for i in 0..6 {
            let dst = VertexId::from_int64((vid as i64 + 1) * 100 + i as i64);
            csr.insert_edge(vid, dst, EdgeId(vid as u64 * 10 + i as u64), 1)
                .unwrap();
        }
    }
    // All chunks should still be accessible via get_overflow_chunks
    for vid in 0..20u32 {
        let chunks = csr.get_overflow_chunks(vid).expect("should have overflow");
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].len(), 2);
    }
    for vid in 0..20u32 {
        let edges = csr.edges_of(vid, 1);
        assert_eq!(edges.len(), 6);
    }
}

#[test]
fn test_supernode_overflow_consolidates_repack_into_single_block() {
    let mut csr = MutableCsr::with_overflow_chunk_edges(1, 4, 32);
    for i in 0..4_096u64 {
        csr.insert_edge(0, VertexId::from_int64(i as i64 + 1), EdgeId(i + 1), 1)
            .unwrap();
    }

    let chunks = csr.overflow_chunks.get(0).expect("vertex 0 has overflow");
    // Repacks consolidate the chain: at most one consolidated block plus
    // the small graded tail chunks grown since the last repack.
    assert!(
        chunks.len() <= OVERFLOW_REPACK_CHUNKS_PER_VERTEX + 1,
        "overflow chain must stay bounded, got {}",
        chunks.len()
    );
    let consolidated = chunks.iter().filter(|chunk| chunk.capacity() > 32).count();
    assert!(
        consolidated <= 1,
        "at most one consolidated block per row, got {}",
        consolidated
    );
    assert!(chunks.iter().all(|chunk| chunk.len() <= chunk.capacity()));
    assert_eq!(csr.physical_edges_of(0).len(), 4_096);
    assert_eq!(csr.edges_of(0, 1).len(), 4_096);
}

#[test]
fn test_graded_overflow_tiers_bound_small_row_chunks() {
    assert_eq!(graded_overflow_chunk_edges(0), OVERFLOW_CHUNK_MIN);
    assert_eq!(graded_overflow_chunk_edges(1), OVERFLOW_CHUNK_MIN);
    assert_eq!(graded_overflow_chunk_edges(5), OVERFLOW_CHUNK_MIN);
    assert_eq!(graded_overflow_chunk_edges(64), 64);
    assert_eq!(graded_overflow_chunk_edges(65), 128);
    assert_eq!(graded_overflow_chunk_edges(1024), 1024);
    assert_eq!(graded_overflow_chunk_edges(1025), 2048);
    assert_eq!(graded_overflow_chunk_edges(1 << 20), OVERFLOW_CHUNK_MAX);

    // Small rows allocate small chunks: 300 edges stay far below the old
    // fixed 4096-edge reservation per chunk.
    let mut csr = MutableCsr::with_capacity(10, 100);
    for i in 1..=300i64 {
        csr.insert_edge(0u32, VertexId::from_int64(i), EdgeId(i as u64), 1)
            .unwrap();
    }
    let chunks = csr.get_overflow_chunks(0).expect("vertex 0 has overflow");
    assert_eq!(chunks[0].capacity(), OVERFLOW_CHUNK_MIN);
    assert!(
        chunks
            .iter()
            .all(|chunk| chunk.capacity() <= OVERFLOW_CHUNK_MAX),
        "graded chunks must stay at or below the cap for 300 live edges"
    );
    assert_eq!(csr.edges_of(0u32, 1).len(), 300);
    // Single repack bound: small-row chunk counts stay bounded.
    assert!(
        chunks.len() <= OVERFLOW_REPACK_CHUNKS_PER_VERTEX + 1,
        "overflow chunks must stay bounded, got {}",
        chunks.len()
    );
}

#[test]
fn test_overflow_repack_preserves_unexpired_tombstones() {
    let mut csr = MutableCsr::with_overflow_chunk_edges(10, 100, 2);
    for i in 0..8u64 {
        csr.insert_edge(0u32, VertexId::from_int64(100 + i as i64), EdgeId(i), 1)
            .unwrap();
    }
    assert!(csr.delete_edge(0u32, EdgeId(6), 10).unwrap());
    assert!(csr.delete_edge(0u32, EdgeId(7), 10).unwrap());
    let overflow_before: usize = csr
        .get_overflow_chunks(0)
        .map(|chunks| chunks.iter().map(|chunk| chunk.len()).sum())
        .unwrap_or(0);
    assert!(overflow_before > 0);

    let mut reported = Vec::new();
    csr.compact_overflow_for_vertex(0, Timestamp::MAX, &mut |id, ts| reported.push((id, ts)));
    assert!(reported.is_empty());
    let kept: usize = csr
        .get_overflow_chunks(0)
        .map(|chunks| chunks.iter().map(|chunk| chunk.len()).sum())
        .unwrap_or(0);
    assert_eq!(kept, overflow_before);

    let mut reported = Vec::new();
    csr.compact_overflow_for_vertex(0, 10, &mut |id, ts| reported.push((id, ts)));
    assert_eq!(reported.len(), 2);
    let kept: usize = csr
        .get_overflow_chunks(0)
        .map(|chunks| chunks.iter().map(|chunk| chunk.len()).sum())
        .unwrap_or(0);
    assert_eq!(kept, overflow_before - 2);
}

#[test]
fn test_overflow_repack_consolidates_to_single_chunk() {
    let mut csr = MutableCsr::with_overflow_chunk_edges(10, 100, 2);
    for i in 0..24u64 {
        csr.insert_edge(0u32, VertexId::from_int64(100 + i as i64), EdgeId(i), 1)
            .unwrap();
    }
    let before = csr.physical_edges_of(0);
    assert_eq!(before.len(), 24);

    let mut noop = |_: EdgeId, _: Timestamp| {};
    csr.compact_overflow_for_vertex(0, Timestamp::MAX, &mut noop);
    let chunks = csr
        .get_overflow_chunks(0)
        .expect("overflow remains after preserve-all repack");
    assert_eq!(
        chunks.len(),
        1,
        "repack must consolidate the row to one contiguous chunk"
    );
    // No entry lost or reordered by the consolidation.
    assert_eq!(csr.physical_edges_of(0), before);
    assert_eq!(csr.edges_of(0u32, 1).len(), 24);
}

#[test]
fn test_graded_chunks_monotonic_capped_and_floored() {
    // Pins the graded scheme shape: floor at the bottom, cap at the top,
    // monotonic in between. Any replacement scheme must keep these three
    // properties, proven by this test rather than by inspection.
    let mut prev = graded_overflow_chunk_edges(0);
    assert_eq!(prev, OVERFLOW_CHUNK_MIN);
    for live in 1..(1 << 22) {
        let cur = graded_overflow_chunk_edges(live);
        assert!(cur >= prev, "live={live} cur={cur} prev={prev}");
        assert!(cur >= OVERFLOW_CHUNK_MIN, "live={live} cur={cur}");
        assert!(cur <= OVERFLOW_CHUNK_MAX, "live={live} cur={cur}");
        prev = cur;
        if live >= OVERFLOW_CHUNK_MAX && cur == OVERFLOW_CHUNK_MAX {
            break;
        }
    }
    assert_eq!(prev, OVERFLOW_CHUNK_MAX);
}
