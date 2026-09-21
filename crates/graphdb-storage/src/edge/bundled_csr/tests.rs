use super::super::{CsrBase, MutableCsrTrait};
use super::*;
use graphdb_core::types::{EdgeId, VertexId};
use graphdb_core::Value;

fn dst(endpoint: u32) -> VertexId {
    VertexId::edge_endpoint_key(endpoint, 0)
}

#[test]
fn insert_and_point_read_roundtrip() {
    let mut csr = BundledCsr::with_overflow_chunk_edges(8, 0, 4);
    csr.insert_edge_with_value(0, dst(1), EdgeId(10), Some(0xDEAD_BEEF))
        .expect("insert with value");
    csr.insert_edge(0, dst(2), EdgeId(11), 0)
        .expect("insert without value stores NULL");
    assert_eq!(csr.edge_count(), 2);
    assert_eq!(
        csr.value_by_edge_id(0, EdgeId(10)),
        Some((0xDEAD_BEEF, true))
    );
    assert_eq!(csr.value_by_edge_id(0, EdgeId(11)), Some((0, false)));
    assert_eq!(csr.value_by_endpoint(0, 1), Some((0xDEAD_BEEF, true)));
    assert!(csr.set_value_by_endpoint(0, 2, Some(7)));
    assert_eq!(csr.value_by_endpoint(0, 2), Some((7, true)));
}

#[test]
fn rank_and_duplicate_are_rejected() {
    let mut csr = BundledCsr::with_overflow_chunk_edges(8, 0, 4);
    assert!(csr
        .insert_edge_with_value(0, VertexId::edge_endpoint_key(1, 3), EdgeId(10), Some(1))
        .is_err());
    csr.insert_edge_with_value(0, dst(1), EdgeId(10), Some(1))
        .expect("first insert");
    assert!(csr.insert_edge(0, dst(1), EdgeId(11), 0).is_err());
}

#[test]
fn delete_clears_valid_and_revert_with_value_restores() {
    let mut csr = BundledCsr::with_overflow_chunk_edges(8, 0, 4);
    csr.insert_edge_with_value(0, dst(1), EdgeId(10), Some(99))
        .expect("insert");
    let (position, _) = csr.locate_edge(0, EdgeId(10)).expect("located");
    assert!(csr.delete_edge(0, EdgeId(10), 5).expect("deleted"));
    assert_eq!(csr.value_by_edge_id(0, EdgeId(10)), None);
    assert_eq!(csr.value_at_position(0, position), Some((99, false)));
    assert!(csr.revert_delete_at_position_with_value(0, position, EdgeId(10), 5, Some(99)));
    assert_eq!(csr.value_by_edge_id(0, EdgeId(10)), Some((99, true)));
}

#[test]
fn overflow_values_stay_aligned() {
    let mut csr = BundledCsr::with_overflow_chunk_edges(2, 0, 1);
    for i in 0..10u32 {
        csr.insert_edge_with_value(0, dst(100 + i), EdgeId(i as u64), Some(i as u64 * 10))
            .expect("overflow insert");
    }
    assert_eq!(csr.edge_count(), 10);
    for i in 0..10u32 {
        assert_eq!(
            csr.value_by_edge_id(0, EdgeId(i as u64)),
            Some((i as u64 * 10, true)),
            "overflow value drift at edge {}",
            i
        );
    }
    assert!(csr.set_value_by_edge_id(0, EdgeId(3), Some(0xFFFF)));
    assert_eq!(csr.value_by_edge_id(0, EdgeId(3)), Some((0xFFFF, true)));
    assert!(csr.rollback_insert(0, EdgeId(0)));
    assert_eq!(csr.edge_count(), 9);
    for i in 1..10u32 {
        let expect = if i == 3 { 0xFFFF } else { i as u64 * 10 };
        assert_eq!(
            csr.value_by_edge_id(0, EdgeId(i as u64)),
            Some((expect, true)),
            "value drift after physical remove at edge {}",
            i
        );
    }
}

#[test]
fn dump_load_preserves_values() {
    let mut csr = BundledCsr::with_overflow_chunk_edges(4, 0, 1);
    for i in 0..6u32 {
        let value = if i % 2 == 0 { Some(i as u64) } else { None };
        csr.insert_edge_with_value(0, dst(10 + i), EdgeId(i as u64), value)
            .expect("insert");
    }
    let bytes = csr.dump();
    let mut loaded = BundledCsr::new();
    loaded.load(&bytes).expect("load roundtrip");
    assert_eq!(loaded.edge_count(), 6);
    for i in 0..6u32 {
        let expect = if i % 2 == 0 {
            Some((i as u64, true))
        } else {
            Some((0, false))
        };
        assert_eq!(loaded.value_by_edge_id(0, EdgeId(i as u64)), expect);
    }
    assert!(loaded.load(&bytes[..bytes.len() - 1]).is_err());
    let mut trailing = bytes.clone();
    trailing.push(0xAA);
    assert!(loaded.load(&trailing).is_err());
}

#[test]
fn scalar_codec_roundtrip() {
    use graphdb_core::DataType;
    let cases = vec![
        (Value::Bool(true), DataType::Bool),
        (Value::Int(-12345), DataType::Int),
        (Value::BigInt(i64::MIN + 7), DataType::BigInt),
        (Value::Double(1.5), DataType::Double),
    ];
    for (value, dt) in cases {
        assert_eq!(decode_scalar(encode_scalar(&value), &dt), value);
    }
}

#[test]
fn borrowed_walks_agree_without_materializing() {
    let mut csr = BundledCsr::with_overflow_chunk_edges(2, 0, 1);
    for i in 0..6u32 {
        let value = if i % 2 == 0 { Some(i as u64) } else { None };
        csr.insert_edge_with_value(0, dst(10 + i), EdgeId(i as u64), value)
            .expect("insert");
    }
    // Borrowed visitor walk over primary plus overflow.
    let mut visited = Vec::new();
    csr.visit_physical(0, |nbr| {
        visited.push((nbr.edge_id, nbr.endpoint));
        true
    });
    // Borrowed row iterator over the same row.
    let iterated: Vec<_> = csr
        .iter_row(0)
        .map(|nbr| (nbr.edge_id, nbr.endpoint))
        .collect();
    assert_eq!(visited, iterated);
    // Value-carrying walk pairs each entry with its inline value.
    let mut valued = Vec::new();
    csr.visit_physical_with_values(0, |nbr, value| {
        valued.push((nbr.edge_id, value));
        true
    });
    let expect: Vec<_> = (0..6u32)
        .map(|i| {
            let value = if i % 2 == 0 { Some(i as u64) } else { None };
            (EdgeId(i as u64), value)
        })
        .collect();
    assert_eq!(valued, expect);
}

#[test]
fn bundled_sort_establishes_threshold_flag() {
    let mut csr = BundledCsr::with_overflow_chunk_edges(8, 0, 4);
    for (endpoint, edge) in [(30u32, 1u64), (10, 2), (20, 3)] {
        csr.insert_edge_with_value(0, dst(endpoint), EdgeId(edge), Some(edge * 10))
            .expect("insert");
    }
    assert!(!csr.topology.primary_sorted_flag(0));
    assert!(csr.sort_row(0));
    assert!(csr.topology.primary_sorted_flag(0));
    let mut ranged = Vec::new();
    csr.fill_threshold_into(0, Some(15), Some(25), &mut ranged);
    assert_eq!(ranged.len(), 1);
    assert_eq!(ranged[0].endpoint, 20);
    assert_eq!(csr.value_by_endpoint(0, 20), Some((30, true)));
}
