//! Unit tests for `SingleMutableCsr`.

use super::super::{EdgeId, VertexId};
use super::SingleMutableCsr;

#[test]
fn test_basic_operations() {
    let mut csr = SingleMutableCsr::with_capacity(10);

    csr.insert_edge(0u32, VertexId::edge_endpoint_key(1, 0), EdgeId(100), 100)
        .unwrap();
    assert!(csr
        .insert_edge(0u32, VertexId::edge_endpoint_key(2, 0), EdgeId(101), 99)
        .is_err());
    assert!(csr
        .insert_edge(0u32, VertexId::edge_endpoint_key(2, 0), EdgeId(102), 101)
        .is_err());

    assert_eq!(csr.edge_count(), 1);
}

#[test]
fn test_second_live_edge_rejected_at_csr_layer() {
    let mut csr = SingleMutableCsr::with_capacity(4);
    csr.insert_edge(0u32, VertexId::edge_endpoint_key(10, 0), EdgeId(100), 100)
        .unwrap();
    let err = csr
        .insert_edge(0u32, VertexId::edge_endpoint_key(11, 0), EdgeId(101), 200)
        .expect_err("second live edge must be rejected");
    assert!(err.to_string().contains("conflict"));
    assert_eq!(csr.edge_count(), 1);
    assert!(csr.delete_edge(0, EdgeId(100), 150).unwrap());
    csr.insert_edge(0u32, VertexId::edge_endpoint_key(11, 0), EdgeId(101), 151)
        .unwrap();
    assert_eq!(csr.edge_count(), 1);
}

#[test]
fn test_physical_lookup_skips_tombstone() {
    let mut csr = SingleMutableCsr::with_capacity(4);
    csr.insert_edge(0u32, VertexId::edge_endpoint_key(10, 0), EdgeId(100), 100)
        .unwrap();
    assert!(csr.delete_edge(0, EdgeId(100), 150).unwrap());
    assert!(csr
        .get_edge_physical(0, VertexId::edge_endpoint_key(10, 0))
        .is_none());
    assert_eq!(csr.physical_edges_of(0).len(), 1);
}

#[test]
fn test_exact_edge_id_required_for_delete() {
    let mut csr = SingleMutableCsr::with_capacity(4);
    csr.insert_edge(0u32, VertexId::edge_endpoint_key(10, 0), EdgeId(100), 100)
        .unwrap();
    assert!(!csr.delete_edge(0, EdgeId(999), 150).unwrap());
    assert!(!csr
        .delete_edge(0, crate::edge::INVALID_EDGE_ID, 150)
        .unwrap());
    assert!(csr.delete_edge(0, EdgeId(100), 150).unwrap());
}

#[test]
fn test_delete_missing_id_on_tombstone_returns_not_found() {
    let mut csr = SingleMutableCsr::with_capacity(4);
    csr.insert_edge(0u32, VertexId::edge_endpoint_key(10, 0), EdgeId(100), 100)
        .unwrap();
    assert!(csr.delete_edge(0, EdgeId(100), 150).unwrap());
    assert!(!csr.delete_edge(0, EdgeId(999), 160).unwrap());
    assert!(csr.delete_edge(0, EdgeId(100), 160).is_err());
    assert!(!csr.delete_edge(0, EdgeId(100), 150).unwrap());
}

#[test]
fn test_delete_by_dst_reports_count() {
    let mut csr = SingleMutableCsr::with_capacity(4);
    csr.insert_edge(0u32, VertexId::edge_endpoint_key(10, 0), EdgeId(100), 100)
        .unwrap();
    assert_eq!(
        csr.delete_edge_by_dst(0, VertexId::edge_endpoint_key(11, 0), 150),
        0
    );
    assert_eq!(
        csr.delete_edge_by_dst(0, VertexId::edge_endpoint_key(10, 0), 150),
        1
    );
    assert_eq!(
        csr.delete_edge_by_dst(0, VertexId::edge_endpoint_key(10, 0), 150),
        0
    );
}

#[test]
fn test_dump_and_load() {
    let mut csr1 = SingleMutableCsr::with_capacity(10);

    // Use insert_edge to populate data
    csr1.insert_edge(0u32, VertexId::edge_endpoint_key(10, 0), EdgeId(100), 100)
        .unwrap();
    csr1.insert_edge(1u32, VertexId::edge_endpoint_key(20, 0), EdgeId(101), 100)
        .unwrap();
    csr1.insert_edge(2u32, VertexId::edge_endpoint_key(30, 0), EdgeId(102), 100)
        .unwrap();

    let data = csr1.dump();

    let mut csr2 = SingleMutableCsr::new();
    csr2.load(&data).unwrap();

    assert_eq!(csr2.vertex_capacity(), csr1.vertex_capacity());
    assert_eq!(csr2.edge_count(), csr1.edge_count());
}

#[test]
fn test_load_rejects_tampered_edge_count() {
    let mut csr1 = SingleMutableCsr::with_capacity(10);
    csr1.insert_edge(0u32, VertexId::edge_endpoint_key(10, 0), EdgeId(100), 100)
        .unwrap();
    csr1.insert_edge(1u32, VertexId::edge_endpoint_key(20, 0), EdgeId(101), 100)
        .unwrap();
    let data = csr1.dump();
    let mut ok = SingleMutableCsr::new();
    ok.load(&data).expect("normal payload must load");

    let mut tampered = data.clone();
    let stored = u64::from_le_bytes(tampered[4..12].try_into().unwrap());
    tampered[4..12].copy_from_slice(&(stored + 1).to_le_bytes());
    let mut csr2 = SingleMutableCsr::new();
    let err = csr2.load(&tampered).expect_err("tampered count must fail");
    assert!(err.to_string().contains("CRC mismatch"));

    // Re-seal the trailer so the CRC passes: the structural edge-count
    // validation underneath must still catch the tamper.
    let body_len = tampered.len() - 4;
    let resealed = crc32fast::hash(&tampered[..body_len]);
    tampered[body_len..].copy_from_slice(&resealed.to_le_bytes());
    let mut csr3 = SingleMutableCsr::new();
    let err = csr3.load(&tampered).expect_err("resealed count must fail");
    assert!(err.to_string().contains("edge count mismatch"));
}

#[test]
fn test_dump_and_load_roundtrip() {
    let mut csr1 = SingleMutableCsr::with_capacity(10);
    csr1.insert_edge(0u32, VertexId::edge_endpoint_key(10, 0), EdgeId(100), 100)
        .unwrap();

    let data = csr1.dump();
    let mut csr2 = SingleMutableCsr::new();
    csr2.load(&data).unwrap();

    assert!(csr2
        .get_edge(0, VertexId::edge_endpoint_key(10, 0), 99)
        .is_some());
    assert!(csr2
        .get_edge(0, VertexId::edge_endpoint_key(10, 0), 100)
        .is_some());
    assert_eq!(csr2.edges_of(0, 99).len(), 1);
    assert_eq!(csr2.edges_of(0, 100).len(), 1);
}

#[test]
fn test_load_rejects_truncated_and_trailing_data() {
    let mut csr1 = SingleMutableCsr::with_capacity(4);
    csr1.insert_edge(0u32, VertexId::edge_endpoint_key(10, 0), EdgeId(100), 100)
        .unwrap();
    let data = csr1.dump();

    // Truncated payload.
    let mut csr2 = SingleMutableCsr::new();
    assert!(csr2.load(&data[..data.len() - 8]).is_err());

    // Trailing bytes.
    let mut trailing = data.clone();
    trailing.push(0xff);
    assert!(csr2.load(&trailing).is_err());
}

#[test]
fn test_offset_delete_propagates_conflict() {
    let mut csr = SingleMutableCsr::with_capacity(4);
    csr.insert_edge(0u32, VertexId::edge_endpoint_key(10, 0), EdgeId(100), 100)
        .unwrap();
    assert!(csr.delete_edge(0, EdgeId(100), 150).unwrap());
    assert!(!csr.delete_edge(0, EdgeId(100), 150).unwrap());
    assert!(csr.delete_edge(0, EdgeId(100), 160).is_err());
    // Offset path surfaces the same conflict instead of folding it.
    assert!(csr.delete_edge_by_offset(0, 0, 160).is_err());
    assert!(!csr.delete_edge_by_offset(0, 1, 160).unwrap());
}

#[test]
fn test_resurrect_allows_any_timestamp() {
    let mut csr = SingleMutableCsr::with_capacity(4);
    csr.insert_edge(0u32, VertexId::edge_endpoint_key(10, 0), EdgeId(100), 100)
        .unwrap();
    assert!(csr.delete_edge(0, EdgeId(100), 150).unwrap());
    csr.insert_edge(0u32, VertexId::edge_endpoint_key(11, 0), EdgeId(101), 140)
        .unwrap();
    assert_eq!(csr.edge_count(), 1);
}

#[test]
fn test_resurrect_with_equal_timestamp() {
    let mut csr = SingleMutableCsr::with_capacity(4);
    csr.insert_edge(0u32, VertexId::edge_endpoint_key(10, 0), EdgeId(100), 100)
        .unwrap();
    assert!(csr.delete_edge(0, EdgeId(100), 150).unwrap());
    csr.insert_edge(0u32, VertexId::edge_endpoint_key(11, 0), EdgeId(101), 150)
        .unwrap();
    assert_eq!(csr.edge_count(), 1);
}

#[test]
fn test_single_reclaim_reports_and_clears_slot() {
    let mut csr = SingleMutableCsr::with_capacity(4);
    csr.insert_edge(0u32, VertexId::edge_endpoint_key(10, 0), EdgeId(100), 100)
        .unwrap();
    assert!(csr.delete_edge(0, EdgeId(100), 150).unwrap());
    assert_eq!(csr.reclaimable_count(0, 100), 0);
    assert_eq!(csr.reclaimable_count(0, 150), 1);
    assert_eq!(csr.vertex_census(0), (0, 1, 1));
    let mut reported = Vec::new();
    assert_eq!(
        csr.compact_vertex_with_reporting(0, 150, &mut |id, ts| reported.push((id, ts))),
        1
    );
    assert_eq!(reported, vec![(EdgeId(100), 150)]);
    assert_eq!(csr.vertex_census(0), (0, 0, 0));
    assert!(!csr.has_physical_entries(0));
    csr.insert_edge(0u32, VertexId::edge_endpoint_key(11, 0), EdgeId(101), 160)
        .unwrap();
    assert_eq!(csr.edge_count(), 1);
}

#[test]
fn test_single_remove_and_revert_by_id() {
    let mut csr = SingleMutableCsr::with_capacity(4);
    csr.insert_edge(0u32, VertexId::edge_endpoint_key(10, 0), EdgeId(100), 100)
        .unwrap();
    assert!(csr.rollback_insert(0, EdgeId(100)));
    assert_eq!(csr.edge_count(), 0);
    assert!(!csr.has_physical_entries(0));
    csr.insert_edge(0u32, VertexId::edge_endpoint_key(10, 0), EdgeId(101), 110)
        .unwrap();
    assert!(csr.delete_edge(0, EdgeId(101), 120).unwrap());
    assert!(csr.revert_delete_by_edge_id(0, EdgeId(101), 130));
    assert_eq!(csr.edges_of(0, 130).len(), 1);
}

#[test]
fn test_single_topology_encoding_roundtrip() {
    let mut csr = SingleMutableCsr::with_capacity(8);
    csr.insert_edge(0u32, VertexId::edge_endpoint_key(10, 0), EdgeId(100), 100)
        .unwrap();
    csr.insert_edge(3u32, VertexId::edge_endpoint_key(11, 0), EdgeId(101), 100)
        .unwrap();
    let payload = csr.dump();
    let mut loaded = SingleMutableCsr::new();
    loaded.load(&payload).expect("encoded load must succeed");
    assert_eq!(loaded.edge_count(), 2);
    assert_eq!(loaded.edges_of(0, 200).len(), 1);
    assert_eq!(loaded.edges_of(3, 200).len(), 1);
    assert_eq!(loaded.edges_of(1, 200).len(), 0);
}

#[test]
fn test_single_topology_encoding_rejects_garbage() {
    let mut payload = Vec::new();
    payload.extend_from_slice(&5u64.to_le_bytes());
    payload.extend_from_slice(&[0u8; 24]);
    let mut csr = SingleMutableCsr::new();
    assert!(csr.load(&payload).is_err());
}

#[test]
fn test_single_sparse_slots_stay_lazy_behind_present_bitmap() {
    let mut csr = SingleMutableCsr::with_capacity(8192);
    csr.insert_edge(0u32, VertexId::edge_endpoint_key(10, 0), EdgeId(100), 100)
        .unwrap();
    csr.insert_edge(
        7000u32,
        VertexId::edge_endpoint_key(11, 0),
        EdgeId(101),
        100,
    )
    .unwrap();
    assert_eq!(csr.edge_count(), 2);
    assert_eq!(csr.allocated_segments(), 2);
    assert!(csr.sparse_memory_bytes() < 8192 * 32);
    assert!(csr
        .get_edge(1, VertexId::edge_endpoint_key(10, 0), 200)
        .is_none());
    assert_eq!(csr.edges_of(1, 200).len(), 0);
    assert!(!csr.has_physical_entries(1));
}
