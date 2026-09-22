use super::*;
use crate::edge::{ImmutableCsr, MutableCsrTrait, Nbr};
use graphdb_core::types::{EdgeId, VertexId};
use std::path::PathBuf;

use super::format::{SNAPSHOT_CRC_LEN, SNAPSHOT_HEADER_LEN};

fn sample_frozen() -> ImmutableCsr {
    use crate::edge::MutableCsr;
    let mut csr = MutableCsr::with_capacity(8, 64);
    let key = |endpoint: u32| VertexId::edge_endpoint_key(endpoint, 0);
    csr.insert_edge(0, key(30), EdgeId(1), 1).unwrap();
    csr.insert_edge(0, key(10), EdgeId(2), 1).unwrap();
    csr.insert_edge(0, key(20), EdgeId(3), 2).unwrap();
    csr.delete_edge(0, EdgeId(3), 5).unwrap();
    csr.insert_edge(3, key(12), EdgeId(4), 2).unwrap();
    ImmutableCsr::pack_from_mutable(&csr)
}

fn serving_path(name: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "linkrs_serving_test_{name}_{}.bin",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    path
}

#[test]
fn mapped_reads_match_heap_frozen() {
    let frozen = sample_frozen();
    let path = serving_path("match");
    write_snapshot_file(&frozen, &path).unwrap();
    let mapped = MappedFrozen::open(&path).unwrap();
    assert_eq!(mapped.vertex_capacity(), frozen.vertex_capacity());
    assert_eq!(mapped.edge_count(), frozen.edge_count());
    for vid in 0..8u32 {
        assert_eq!(mapped.physical_edges_of(vid), frozen.physical_edges_of(vid));
        for ts in [1u64, 2, 4, 5, 6] {
            assert_eq!(mapped.edges_of(vid, ts), frozen.edges_of(vid, ts));
        }
        assert_eq!(mapped.vertex_census(vid), frozen.vertex_census(vid));
        assert_eq!(mapped.row_degree(vid), frozen.row_degree(vid));
    }
    let key = VertexId::edge_endpoint_key(10, 0);
    assert_eq!(mapped.get_edge(0, key, 6), frozen.get_edge(0, key, 6));
    assert_eq!(
        mapped.get_edge_physical(0, key),
        frozen.get_edge_physical(0, key)
    );
    assert_eq!(
        mapped.locate_edge(0, EdgeId(1)).map(|(_, nbr)| nbr),
        frozen.locate_edge(0, EdgeId(1)).map(|(_, nbr)| nbr)
    );
    assert!(mapped.primary_contains(0, EdgeId(1)));
    // Same authoritative bytes as the heap dump: flushes are identical.
    assert_eq!(mapped.dump(), frozen.dump());
    let _ = std::fs::remove_file(&path);
}

#[test]
fn mapped_iterators_match_heap_counts() {
    let frozen = sample_frozen();
    let path = serving_path("iter");
    write_snapshot_file(&frozen, &path).unwrap();
    let mapped = MappedFrozen::open(&path).unwrap();
    assert_eq!(mapped.iter(6).count(), frozen.iter(6).count());
    assert_eq!(mapped.iter_all().count(), frozen.iter_all().count());
    let mapped_row: Vec<Nbr> = mapped.iter_edges_of(0, 6).collect();
    let heap_row: Vec<Nbr> = frozen.iter_edges_of(0, 6).collect();
    assert_eq!(mapped_row, heap_row);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn hugepage_hint_keeps_serving_openable() {
    let frozen = sample_frozen();
    let path = serving_path("hugepage");
    write_snapshot_file(&frozen, &path).unwrap();
    // The open path carries a best-effort huge-page hint. It must never
    // fail the open: rejection falls back to base pages.
    let mapped = MappedFrozen::open(&path).unwrap();
    assert_eq!(mapped.physical_edges_of(0), frozen.physical_edges_of(0));
    let _ = std::fs::remove_file(&path);
}

#[test]
fn mapped_writes_are_rejected() {
    let frozen = sample_frozen();
    let path = serving_path("rejected");
    write_snapshot_file(&frozen, &path).unwrap();
    let mut mapped = MappedFrozen::open(&path).unwrap();
    let key = VertexId::edge_endpoint_key(99, 0);
    assert!(mapped.insert_edge(0, key, EdgeId(100), 9).is_err());
    assert!(mapped.delete_edge(0, EdgeId(1), 9).is_err());
    assert_eq!(mapped.delete_edge_by_dst(0, key, 9), 0);
    assert!(!mapped.rollback_insert(0, EdgeId(1)));
    assert!(!mapped.revert_delete_by_edge_id(0, EdgeId(1), 9));
    assert_eq!(mapped.reclaimable_count(0, 9), 0);
    assert!(!mapped.vertex_needs_compact(0, 9));
    let _ = std::fs::remove_file(&path);
}

#[test]
fn missing_file_falls_back_to_rebuild() {
    let frozen = sample_frozen();
    let path = serving_path("missing");
    let _ = std::fs::remove_file(&path);
    assert!(MappedFrozen::open(&path).is_err());
    let mapped = MappedFrozen::open_or_rebuild(&path, &frozen).unwrap();
    assert_eq!(mapped.physical_edges_of(0), frozen.physical_edges_of(0));
    let _ = std::fs::remove_file(&path);
}

#[test]
fn corrupt_serving_file_is_rejected() {
    let frozen = sample_frozen();
    let path = serving_path("corrupt");
    write_snapshot_file(&frozen, &path).unwrap();
    for mutate in [
        |bytes: &mut Vec<u8>| bytes[0] ^= 0xff,
        |bytes: &mut Vec<u8>| bytes[4] = 99,
        |bytes: &mut Vec<u8>| bytes.truncate(bytes.len() / 2),
        |bytes: &mut Vec<u8>| bytes.push(0),
    ] {
        let mut bytes = std::fs::read(&path).unwrap();
        mutate(&mut bytes);
        std::fs::write(&path, &bytes).unwrap();
        assert!(MappedFrozen::open(&path).is_err());
    }
    // A bad cache rebuilds cleanly from the authority.
    let mapped = MappedFrozen::open_or_rebuild(&path, &frozen).unwrap();
    assert_eq!(mapped.physical_edges_of(0), frozen.physical_edges_of(0));
    let _ = std::fs::remove_file(&path);
}

#[test]
fn payload_bit_flip_fails_checksum() {
    let frozen = sample_frozen();
    let path = serving_path("payload_crc");
    write_snapshot_file(&frozen, &path).unwrap();
    let mut bytes = std::fs::read(&path).unwrap();
    assert!(bytes.len() > SNAPSHOT_HEADER_LEN + SNAPSHOT_CRC_LEN);
    let mid = SNAPSHOT_HEADER_LEN + 1;
    bytes[mid] ^= 0x01;
    std::fs::write(&path, &bytes).unwrap();
    let err = MappedFrozen::open(&path).expect_err("payload corruption must fail");
    assert!(err.to_string().contains("CRC"));
    let mapped = MappedFrozen::open_or_rebuild(&path, &frozen).unwrap();
    assert_eq!(mapped.physical_edges_of(0), frozen.physical_edges_of(0));
    let _ = std::fs::remove_file(&path);
}

#[test]
fn empty_table_serves() {
    use crate::edge::MutableCsr;
    let frozen = ImmutableCsr::pack_from_mutable(&MutableCsr::with_capacity(4, 16));
    let path = serving_path("empty");
    write_snapshot_file(&frozen, &path).unwrap();
    let mapped = MappedFrozen::open(&path).unwrap();
    assert_eq!(mapped.edge_count(), 0);
    assert!(mapped.edges_of(0, 1).is_empty());
    assert_eq!(mapped.dump(), frozen.dump());
    let _ = std::fs::remove_file(&path);
}

#[test]
fn valued_sidecar_serves_carried_values() {
    use crate::edge::BundledCsr;
    let mut bundled = BundledCsr::with_capacity(8, 16);
    let key = |endpoint: u32| VertexId::edge_endpoint_key(endpoint, 0);
    bundled
        .insert_edge_with_value(0, key(10), EdgeId(1), Some(99))
        .unwrap();
    bundled
        .insert_edge_with_value(0, key(20), EdgeId(2), None)
        .unwrap();
    let frozen = ImmutableCsr::pack_from_bundled(&bundled);
    let path = serving_path("valued");
    write_snapshot_file(&frozen, &path).unwrap();
    let mapped = MappedFrozen::open(&path).unwrap();
    assert!(mapped.has_valued_entries());
    assert!(mapped.any_valid_values());
    assert_eq!(
        mapped.bundled_value_by_edge_id(0, EdgeId(1)),
        Some((99, true))
    );
    assert_eq!(
        mapped.bundled_value_by_edge_id(0, EdgeId(2)),
        Some((0, false))
    );
    assert_eq!(mapped.bundled_value_by_endpoint(0, 10), Some((99, true)));
    let mut seen = Vec::new();
    mapped.visit_physical_with_values(0, |nbr, value| {
        seen.push((nbr.edge_id, value));
        true
    });
    assert_eq!(seen, vec![(EdgeId(1), Some(99)), (EdgeId(2), None)]);
    // Same authoritative bytes as the heap dump: flushes are identical.
    assert_eq!(mapped.dump(), frozen.dump());
    // A heap reload from the mapped dump keeps the values.
    let mut reloaded = ImmutableCsr::new();
    reloaded.load(&mapped.dump()).unwrap();
    assert_eq!(reloaded.value_by_edge_id(0, EdgeId(1)), Some((99, true)));
    let _ = std::fs::remove_file(&path);
}
