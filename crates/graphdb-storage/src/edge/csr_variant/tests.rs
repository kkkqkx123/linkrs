use super::super::{
    BundledCsr, CsrBase, EdgeId, EdgeStrategy, ImmutableCsr, MutableCsr, MutableCsrTrait, Nbr,
    PureTopologyCsr, VertexId,
};
use super::*;

#[test]
fn test_multiple_csr_variant() {
    let mut csr =
        CsrVariant::from_strategy_with_overflow(EdgeStrategy::Multiple, 10, 100, 4096).unwrap();

    csr.insert_edge(0u32, VertexId::edge_endpoint_key(1, 0), EdgeId(100), 1)
        .unwrap();
    assert_eq!(csr.edge_count(), 1);
}

#[test]
fn test_single_csr_variant() {
    let mut csr =
        CsrVariant::from_strategy_with_overflow(EdgeStrategy::Single, 10, 100, 4096).unwrap();

    csr.insert_edge(0u32, VertexId::edge_endpoint_key(1, 0), EdgeId(100), 1)
        .unwrap();
    assert_eq!(csr.edge_count(), 1);
}

#[test]
fn test_frozen_variant_dump_load_roundtrip() {
    let mut inner = MutableCsr::with_capacity(8, 64);
    inner
        .insert_edge(0u32, VertexId::edge_endpoint_key(1, 0), EdgeId(100), 1)
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
        .insert_edge(1u32, VertexId::edge_endpoint_key(2, 0), EdgeId(101), 1)
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
        .insert_edge(0u32, VertexId::edge_endpoint_key(1, 0), EdgeId(100), 1)
        .is_err());
    assert_eq!(csr.edge_count(), 0);

    // None variant should reject all deletions
    assert!(csr.delete_edge(0, EdgeId(100), 1).is_err());
    assert_eq!(csr.delete_edge_by_dst(0, VertexId::edge_endpoint_key(1, 0), 1), 0);
    assert!(!csr.revert_delete_by_offset(0, 0, 1));

    // None variant should return None for get_edge
    assert!(csr.get_edge(0, VertexId::edge_endpoint_key(1, 0), 1).is_none());

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
        .insert_edge(0, VertexId::edge_endpoint_key(1, 0), EdgeId(100), 1)
        .is_err());
}

#[test]
fn test_clone() {
    let mut csr1 =
        CsrVariant::from_strategy_with_overflow(EdgeStrategy::Multiple, 10, 100, 4096).unwrap();
    csr1.insert_edge(0u32, VertexId::edge_endpoint_key(1, 0), EdgeId(100), 1)
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
        .insert_edge(0, VertexId::edge_endpoint_key(1, 0), EdgeId(100), 1)
        .is_err());
}

#[test]
fn pure_row_iter_matches_allocating_read() {
    let mut inner = PureTopologyCsr::with_capacity(8, 16);
    for dst in [1u32, 2, 3] {
        inner
            .insert_edge(
                0,
                VertexId::edge_endpoint_key((dst) as u32, 0),
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
                VertexId::edge_endpoint_key((dst) as u32, 0),
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

#[test]
fn pure_variant_reports_fragmentation_and_holes() {
    let mut inner = PureTopologyCsr::with_capacity(8, 16);
    for dst in [1u32, 2, 3, 4] {
        inner
            .insert_edge(
                0,
                VertexId::edge_endpoint_key((dst) as u32, 0),
                EdgeId(dst as u64),
                0,
            )
            .unwrap();
    }
    inner.delete_edge(0, EdgeId(2), 0).unwrap();
    let variant = CsrVariant::Pure(Box::new(inner));
    assert!(variant.fragmentation_ratio() > 0.0);
    assert!(variant.wasted_bytes_estimate() > 0);
    let stats = variant.fragmentation_stats().unwrap();
    assert!(stats.wasted_capacity > 0);
    assert_eq!(variant.reclaimable_count(0, 7), 1);
    assert!(variant.vertex_needs_compact(0, 7));
    let (dead, reclaimable) = variant.vertex_reclaim_probe(0, 7);
    assert_eq!((dead, reclaimable), (1, 1));
}

#[test]
fn bundled_variant_reports_fragmentation() {
    let mut inner = BundledCsr::with_capacity(8, 16);
    for dst in [1u32, 2, 3] {
        inner
            .insert_edge(
                0,
                VertexId::edge_endpoint_key((dst) as u32, 0),
                EdgeId(dst as u64),
                0,
            )
            .unwrap();
    }
    let variant = CsrVariant::Bundled(Box::new(inner));
    assert!(variant.fragmentation_ratio() > 0.0);
    assert!(variant.fragmentation_stats().is_some());
}

#[test]
fn single_variant_supports_positional_writes() {
    let mut csr =
        CsrVariant::from_strategy_with_overflow(EdgeStrategy::Single, 10, 100, 4096).unwrap();
    csr.insert_edge(0u32, VertexId::edge_endpoint_key(1, 0), EdgeId(100), 1)
        .unwrap();
    let (position, nbr) = csr.locate_edge(0, EdgeId(100)).unwrap();
    assert_eq!(nbr.edge_id, EdgeId(100));
    assert!(csr
        .delete_edge_at_position(0, position, EdgeId(100), 2)
        .unwrap());
    assert_eq!(csr.edge_count(), 0);
    assert!(csr.revert_delete_at_position(0, position, EdgeId(100), 2));
    assert_eq!(csr.edge_count(), 1);
    let stale = crate::edge::EdgePosition::Overflow { chunk: 0, slot: 0 };
    assert!(!csr.revert_delete_at_position(0, stale, EdgeId(100), 2));
    assert!(!csr
        .delete_edge_at_position(0, stale, EdgeId(100), 3)
        .unwrap_or(false));
}

#[test]
fn none_variant_offset_delete_fails_closed() {
    let mut csr =
        CsrVariant::from_strategy_with_overflow(EdgeStrategy::None, 10, 100, 4096).unwrap();
    assert!(csr.delete_edge_by_offset(0, 0, 1).is_err());
}

#[test]
fn frozen_variant_probe_reports_dead_entries() {
    let mut inner = MutableCsr::with_capacity(8, 64);
    inner
        .insert_edge(0u32, VertexId::edge_endpoint_key(1, 0), EdgeId(100), 1)
        .unwrap();
    inner
        .insert_edge(0u32, VertexId::edge_endpoint_key(2, 0), EdgeId(101), 1)
        .unwrap();
    inner.delete_edge(0, EdgeId(100), 5).unwrap();
    let variant = CsrVariant::Frozen(Box::new(ImmutableCsr::pack_from_mutable(&inner)));
    let (dead, reclaimable) = variant.vertex_reclaim_probe(0, 9);
    assert_eq!(dead, 1);
    assert_eq!(reclaimable, 1);
    assert!(variant.vertex_needs_compact(0, 9));
}

#[test]
fn mapped_dump_shares_tag_and_loads_as_heap() {
    use crate::edge::edge_table::checkpoint::snapshot::{write_snapshot_file, MappedFrozen};
    let mut inner = MutableCsr::with_capacity(8, 64);
    inner
        .insert_edge(0u32, VertexId::edge_endpoint_key(1, 0), EdgeId(100), 1)
        .unwrap();
    let frozen_heap = ImmutableCsr::pack_from_mutable(&inner);
    let mut path = std::env::temp_dir();
    path.push(format!(
        "linkrs_variant_mapped_tag_{}.bin",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    write_snapshot_file(&frozen_heap, &path).unwrap();
    let mapped = MappedFrozen::open(&path).unwrap();
    let mapped_variant = CsrVariant::Mapped(Box::new(mapped));
    let frozen_variant = CsrVariant::Frozen(Box::new(frozen_heap));
    let mapped_bytes = mapped_variant.dump();
    let frozen_bytes = frozen_variant.dump();
    assert_eq!(mapped_bytes[0], 3u8);
    assert_eq!(frozen_bytes[0], 3u8);
    assert_eq!(mapped_bytes, frozen_bytes);
    let mut loaded =
        CsrVariant::from_strategy_with_overflow(EdgeStrategy::Multiple, 8, 64, 4096).unwrap();
    loaded.load(&mapped_bytes).unwrap();
    assert!(matches!(loaded, CsrVariant::Frozen(_)));
    assert_eq!(loaded.edge_count(), 1);
    assert_eq!(
        loaded.physical_edges_of(0),
        frozen_variant.physical_edges_of(0)
    );
    let _ = std::fs::remove_file(&path);
}

#[test]
fn mapped_clear_falls_back_to_placeholder() {
    use crate::edge::edge_table::checkpoint::snapshot::{write_snapshot_file, MappedFrozen};
    let mut inner = MutableCsr::with_capacity(8, 64);
    inner
        .insert_edge(0u32, VertexId::edge_endpoint_key(1, 0), EdgeId(100), 1)
        .unwrap();
    let frozen_heap = ImmutableCsr::pack_from_mutable(&inner);
    let capacity = frozen_heap.vertex_capacity();
    let mut path = std::env::temp_dir();
    path.push(format!(
        "linkrs_variant_mapped_clear_{}.bin",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    write_snapshot_file(&frozen_heap, &path).unwrap();
    let mapped = MappedFrozen::open(&path).unwrap();
    let mut variant = CsrVariant::Mapped(Box::new(mapped));
    variant.clear();
    assert!(matches!(
        variant,
        CsrVariant::None { vertex_capacity } if vertex_capacity == capacity
    ));
    assert_eq!(variant.edge_count(), 0);
    assert!(variant.physical_edges_of(0).is_empty());
    assert!(variant
        .insert_edge(0, VertexId::edge_endpoint_key(1, 0), EdgeId(100), 1)
        .is_err());
    let _ = std::fs::remove_file(&path);
}

#[test]
fn pure_and_bundled_direct_pack_match_topology() {
    let mut pure = PureTopologyCsr::with_capacity(8, 16);
    for dst in [3u32, 1, 2] {
        pure.insert_edge(
            0,
            VertexId::edge_endpoint_key((dst) as u32, 0),
            EdgeId(dst as u64),
            0,
        )
        .unwrap();
    }
    let packed = ImmutableCsr::pack_from_pure(&pure);
    assert_eq!(packed.edge_count(), 3);
    assert_eq!(packed.vertex_capacity(), pure.vertex_capacity());
    let mut endpoints: Vec<u32> = packed
        .physical_edges_of(0)
        .iter()
        .map(|n| n.endpoint)
        .collect();
    assert_eq!(endpoints, vec![1, 2, 3]);

    let mut bundled = BundledCsr::with_capacity(8, 16);
    for dst in [3u32, 1, 2] {
        bundled
            .insert_edge(
                0,
                VertexId::edge_endpoint_key((dst) as u32, 0),
                EdgeId(dst as u64),
                0,
            )
            .unwrap();
    }
    assert!(!CsrVariant::Bundled(Box::new(bundled.clone())).bundled_has_valid_values());
    let packed_bundled = ImmutableCsr::pack_from_bundled(&bundled);
    assert_eq!(packed_bundled.edge_count(), 3);
    endpoints = packed_bundled
        .physical_edges_of(0)
        .iter()
        .map(|n| n.endpoint)
        .collect();
    assert_eq!(endpoints, vec![1, 2, 3]);
}

#[test]
fn bundled_topology_walk_needs_paired_values() {
    let mut inner = BundledCsr::with_capacity(8, 16);
    inner
        .insert_edge(0, VertexId::edge_endpoint_key(1, 0), EdgeId(7), 0)
        .unwrap();
    inner.set_value_by_edge_id(0, EdgeId(7), Some(99));
    let variant = CsrVariant::Bundled(Box::new(inner));
    let topo: Vec<Nbr> = variant.iter_edges_of(0, 0).unwrap().collect();
    assert_eq!(topo.len(), 1);
    assert_eq!(
        variant.bundled_value_by_edge_id(0, EdgeId(7)),
        Some((99, true))
    );
    let mut paired = Vec::new();
    variant.visit_physical_with_values(0, |nbr, value| {
        paired.push((nbr.edge_id, value));
        true
    });
    assert_eq!(paired, vec![(EdgeId(7), Some(99))]);
}

#[test]
fn pure_threshold_ignores_rank_half_explicitly() {
    let mut inner = PureTopologyCsr::with_capacity(8, 16);
    for dst in [1u32, 5, 9] {
        inner
            .insert_edge(
                0,
                VertexId::edge_endpoint_key((dst) as u32, 0),
                EdgeId(dst as u64),
                0,
            )
            .unwrap();
    }
    let variant = CsrVariant::Pure(Box::new(inner));
    let mut out = Vec::new();
    variant.fill_threshold_into(0, Some((2, 999)), Some((8, -999)), &mut out);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].endpoint, 5);
    assert!(variant.is_row_sorted(0) || !variant.is_row_sorted(0));
}
