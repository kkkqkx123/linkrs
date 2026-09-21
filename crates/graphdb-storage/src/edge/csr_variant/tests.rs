use super::super::{
    BundledCsr, CsrBase, EdgeId, EdgeStrategy, ImmutableCsr, MutableCsr, MutableCsrTrait, Nbr,
    PureTopologyCsr, VertexId,
};
use super::*;

#[test]
fn test_multiple_csr_variant() {
    let mut csr =
        CsrVariant::from_strategy_with_overflow(EdgeStrategy::Multiple, 10, 100, 4096).unwrap();

    csr.insert_edge(0u32, VertexId::from_int64(1), EdgeId(100), 1)
        .unwrap();
    assert_eq!(csr.edge_count(), 1);
}

#[test]
fn test_single_csr_variant() {
    let mut csr =
        CsrVariant::from_strategy_with_overflow(EdgeStrategy::Single, 10, 100, 4096).unwrap();

    csr.insert_edge(0u32, VertexId::from_int64(1), EdgeId(100), 1)
        .unwrap();
    assert_eq!(csr.edge_count(), 1);
}

#[test]
fn test_frozen_variant_dump_load_roundtrip() {
    let mut inner = MutableCsr::with_capacity(8, 64);
    inner
        .insert_edge(0u32, VertexId::from_int64(1), EdgeId(100), 1)
        .unwrap();
    let frozen = CsrVariant::Frozen(Box::new(super::super::ImmutableCsr::pack_from_mutable(
        &inner,
    )));
    assert_eq!(frozen.edge_count(), 1);
    let bytes = frozen.dump();
    assert_eq!(bytes[0], 3u8);

    let mut loaded =
        CsrVariant::from_strategy_with_overflow(EdgeStrategy::Multiple, 8, 64, 4096).unwrap();
    loaded.load(&bytes).unwrap();
    assert_eq!(loaded.edge_count(), 1);
    assert_eq!(loaded.edges_of(0, 1), frozen.edges_of(0, 1));
    assert!(loaded
        .insert_edge(1u32, VertexId::from_int64(2), EdgeId(101), 1)
        .is_err());
}

#[test]
fn test_removed_variant_tags_are_rejected() {
    for tag in [6u8, 7u8] {
        let mut payload = vec![tag];
        payload.extend_from_slice(&[0u8; 8]);
        let mut csr =
            CsrVariant::from_strategy_with_overflow(EdgeStrategy::Multiple, 10, 100, 4096).unwrap();
        assert!(csr.load(&payload).is_err());
    }
}

#[test]
fn test_none_csr_variant() {
    let mut csr =
        CsrVariant::from_strategy_with_overflow(EdgeStrategy::None, 10, 100, 4096).unwrap();

    // None variant should return the configured vertex capacity
    assert_eq!(csr.vertex_capacity(), 10);
    assert_eq!(csr.edge_count(), 0);
    assert!(csr.edges_of(0, 1).is_empty());

    // None variant should reject all insertions
    assert!(csr
        .insert_edge(0u32, VertexId::from_int64(1), EdgeId(100), 1)
        .is_err());
    assert_eq!(csr.edge_count(), 0);

    // None variant should reject all deletions
    assert!(csr.delete_edge(0, EdgeId(100), 1).is_err());
    assert_eq!(csr.delete_edge_by_dst(0, VertexId::from_int64(1), 1), 0);
    assert!(!csr.revert_delete_by_offset(0, 0, 1));

    // None variant should return None for get_edge
    assert!(csr.get_edge(0, VertexId::from_int64(1), 1).is_none());

    // Clear should be a no-op
    csr.clear();
    assert_eq!(csr.edge_count(), 0);
}

#[test]
fn test_none_csr_iter() {
    let csr = CsrVariant::from_strategy_with_overflow(EdgeStrategy::None, 10, 100, 4096).unwrap();
    let mut iter = csr.iter_all();

    // Iterator should produce no items
    assert!(iter.next().is_none());
}

#[test]
fn test_none_csr_dump_load() {
    let csr1 = CsrVariant::from_strategy_with_overflow(EdgeStrategy::None, 10, 100, 4096).unwrap();
    let data = csr1.dump();

    // Data should start with variant tag (0 for None)
    assert!(!data.is_empty());
    assert_eq!(data[0], 0u8);

    let mut csr2 =
        CsrVariant::from_strategy_with_overflow(EdgeStrategy::Multiple, 10, 100, 4096).unwrap();
    csr2.load(&data).unwrap();

    // After loading, should be None variant
    assert_eq!(csr2.edge_count(), 0);
    assert!(csr2
        .insert_edge(0, VertexId::from_int64(1), EdgeId(100), 1)
        .is_err());
}

#[test]
fn test_clone() {
    let mut csr1 =
        CsrVariant::from_strategy_with_overflow(EdgeStrategy::Multiple, 10, 100, 4096).unwrap();
    csr1.insert_edge(0u32, VertexId::from_int64(1), EdgeId(100), 1)
        .unwrap();

    let csr2 = csr1.clone();
    assert_eq!(csr2.edge_count(), 1);
}

#[test]
fn test_clone_none() {
    let csr1 = CsrVariant::from_strategy_with_overflow(EdgeStrategy::None, 10, 100, 4096).unwrap();
    let mut csr2 = csr1.clone();

    assert_eq!(csr2.edge_count(), 0);
    assert!(csr2
        .insert_edge(0, VertexId::from_int64(1), EdgeId(100), 1)
        .is_err());
}

#[test]
fn pure_row_iter_matches_allocating_read() {
    let mut inner = PureTopologyCsr::with_capacity(8, 16);
    for dst in [1u32, 2, 3] {
        inner
            .insert_edge(
                0,
                VertexId::edge_endpoint_key(dst, 0),
                EdgeId(dst as u64),
                0,
            )
            .unwrap();
    }
    let variant = CsrVariant::Pure(Box::new(inner));
    let via_iter: Vec<Nbr> = variant.iter_edges_of(0, 1).unwrap().collect();
    assert_eq!(via_iter, variant.edges_of(0, 1));
    assert_eq!(via_iter.len(), 3);
}

#[test]
fn bundled_row_iter_matches_allocating_read() {
    let mut inner = BundledCsr::with_capacity(8, 16);
    for dst in [5u32, 6] {
        inner
            .insert_edge(
                0,
                VertexId::edge_endpoint_key(dst, 0),
                EdgeId(dst as u64),
                0,
            )
            .unwrap();
    }
    let variant = CsrVariant::Bundled(Box::new(inner));
    let via_iter: Vec<Nbr> = variant.iter_edges_of(0, 1).unwrap().collect();
    assert_eq!(via_iter, variant.edges_of(0, 1));
    assert_eq!(via_iter.len(), 2);
}
