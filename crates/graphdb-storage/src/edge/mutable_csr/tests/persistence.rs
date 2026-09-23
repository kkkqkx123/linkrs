use super::super::super::{EdgeId, VertexId};
use super::super::MutableCsr;
use crate::edge::Nbr;

#[test]
fn test_dump_and_load() {
    let mut csr1 = MutableCsr::with_capacity(10, 100);

    csr1.insert_edge(0u32, VertexId::edge_endpoint_key(1, 0), EdgeId(100), 1)
        .unwrap();
    csr1.insert_edge(0u32, VertexId::edge_endpoint_key(2, 0), EdgeId(101), 1)
        .unwrap();
    csr1.insert_edge(1u32, VertexId::edge_endpoint_key(3, 0), EdgeId(102), 1)
        .unwrap();

    let data = csr1.dump();

    let mut csr2 = MutableCsr::new();
    let _ = csr2.load(&data);

    assert_eq!(csr2.vertex_capacity(), csr1.vertex_capacity());
    assert_eq!(csr2.edge_count(), csr1.edge_count());
}

#[test]
fn test_load_rejects_tampered_edge_count() {
    let mut csr1 = MutableCsr::with_capacity(10, 100);
    csr1.insert_edge(0u32, VertexId::edge_endpoint_key(1, 0), EdgeId(100), 1)
        .unwrap();
    csr1.insert_edge(0u32, VertexId::edge_endpoint_key(2, 0), EdgeId(101), 1)
        .unwrap();
    let data = csr1.dump();
    let mut ok = MutableCsr::new();
    ok.load(&data).expect("normal payload must load");

    let mut tampered = data.clone();
    let stored = u64::from_le_bytes(tampered[12..20].try_into().unwrap());
    tampered[12..20].copy_from_slice(&(stored + 1).to_le_bytes());
    let mut csr2 = MutableCsr::new();
    let err = csr2.load(&tampered).expect_err("tampered count must fail");
    assert!(err.to_string().contains("CRC mismatch"));

    // Re-seal the trailer so the CRC passes: the structural edge-count
    // validation underneath must still catch the tamper.
    let body_len = tampered.len() - 4;
    let resealed = crc32fast::hash(&tampered[..body_len]);
    tampered[body_len..].copy_from_slice(&resealed.to_le_bytes());
    let mut csr3 = MutableCsr::new();
    let err = csr3.load(&tampered).expect_err("resealed count must fail");
    assert!(err.to_string().contains("edge count mismatch"));
}

#[test]
fn test_overflow_dump_and_load() {
    let mut csr1 = MutableCsr::with_capacity(10, 100);

    for i in 1..=6 {
        let dst = VertexId::edge_endpoint_key(i as u32, 0);
        csr1.insert_edge(0u32, dst, EdgeId(i as u64), 1).unwrap();
    }

    let data = csr1.dump();

    let mut csr2 = MutableCsr::new();
    let _ = csr2.load(&data);

    assert_eq!(csr2.vertex_capacity(), csr1.vertex_capacity());
    assert_eq!(csr2.edge_count(), csr1.edge_count());
    assert_eq!(
        csr2.overflow_chunks.get(0).map_or(0, |chunks| {
            chunks.iter().map(|chunk| chunk.len()).sum::<usize>()
        }),
        2
    );
}

#[test]
fn test_topology_encoding_roundtrip_keeps_snapshot_reads() {
    let mut csr = MutableCsr::with_capacity(16, 64);
    for i in 0..20u64 {
        csr.insert_edge(
            (i % 4) as u32,
            VertexId::edge_endpoint_key(100 + i as u32, 0),
            EdgeId(i),
            10,
        )
        .unwrap();
    }
    assert!(csr.delete_edge(0u32, EdgeId(0), 20).unwrap());
    let before: Vec<Nbr> = {
        let mut all = csr.physical_edges_of(0);
        all.extend(csr.physical_edges_of(1));
        all
    };
    let live_before = csr.edges_of(1u32, 30);

    let payload = csr.dump();
    let mut loaded = MutableCsr::new();
    loaded.load(&payload).expect("encoded load must succeed");
    let mut after = loaded.physical_edges_of(0);
    after.extend(loaded.physical_edges_of(1));
    assert_eq!(before, after);
    assert_eq!(loaded.edges_of(1u32, 30), live_before);
    assert_eq!(loaded.edge_count(), csr.edge_count());

    let report = loaded.topology_encoding_report();
    assert_eq!(report.len(), 5);
    assert!(report.iter().any(|(name, _, _, _)| name == "neighbor"));
    assert!(report.iter().any(|(name, _, _, _)| name == "edge_id"));
}

#[test]
fn test_topology_encoding_rejects_garbage_marker() {
    let mut payload = Vec::new();
    payload.extend_from_slice(&2u32.to_le_bytes());
    payload.extend_from_slice(&[0u8; 32]);
    let mut csr = MutableCsr::new();
    assert!(csr.load(&payload).is_err());
}

#[test]
fn single_marker_dump_load_roundtrip() {
    let mut csr = MutableCsr::with_overflow_chunk_edges(2, 16, 8);
    for i in 0..30i64 {
        csr.insert_edge(0u32, VertexId::edge_endpoint_key((i + 1) as u32, 0), EdgeId(i as u64 + 1), 1)
            .unwrap();
    }
    for i in 1..10i64 {
        csr.insert_edge(
            1u32,
            VertexId::edge_endpoint_key((i + 100) as u32, 0),
            EdgeId(1000 + i as u64),
            2,
        )
        .unwrap();
    }
    assert!(csr.delete_edge(0u32, EdgeId(5), 3).unwrap());

    let payload = csr.dump();
    let marker = u32::from_le_bytes(payload[0..4].try_into().expect("header present"));
    assert_eq!(
        marker,
        super::super::serialization::MUTABLE_CSR_FORMAT_VERSION
    );

    let mut loaded = MutableCsr::new();
    loaded.load(&payload).expect("dump loads by marker");
    assert_eq!(loaded.edge_count(), csr.edge_count());

    let mut expected = Vec::new();
    csr.fill_physical_into(0u32, &mut expected);
    let mut actual = Vec::new();
    loaded.fill_physical_into(0u32, &mut actual);
    assert_eq!(actual, expected);

    let mut expected_one = Vec::new();
    csr.fill_physical_into(1u32, &mut expected_one);
    let mut actual_one = Vec::new();
    loaded.fill_physical_into(1u32, &mut actual_one);
    assert_eq!(actual_one, expected_one);
}

#[test]
fn retired_raw_marker_is_rejected() {
    let mut csr = MutableCsr::with_capacity(4, 16);
    csr.insert_edge(0u32, VertexId::edge_endpoint_key(1, 0), EdgeId(7), 1)
        .unwrap();

    // The retired direct-dump marker (9) is damage now, not an alternate
    // mode: rewrite a valid payload's marker and the load must refuse it.
    let mut payload = csr.dump();
    payload[0..4].copy_from_slice(&9u32.to_le_bytes());
    let body_len = payload.len() - 4;
    let resealed = crc32fast::hash(&payload[..body_len]);
    payload[body_len..].copy_from_slice(&resealed.to_le_bytes());
    let err = MutableCsr::new()
        .load(&payload)
        .expect_err("marker 9 must fail");
    assert!(err
        .to_string()
        .contains("Unsupported mutable CSR format version"));

    let mut bad_marker = csr.dump();
    bad_marker[0..4].copy_from_slice(&99u32.to_le_bytes());
    assert!(MutableCsr::new().load(&bad_marker).is_err());

    let encoded = csr.dump();
    assert!(MutableCsr::new()
        .load(&encoded[..encoded.len() - 1])
        .is_err());
    assert!(MutableCsr::new().load(&[]).is_err());
}
