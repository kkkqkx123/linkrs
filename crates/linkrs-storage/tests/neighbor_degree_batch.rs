//! Fused two-hop degree read tests.
//!
//! The native neighbor-degree primitive must agree with the portable
//! neighbor-plus-degree composition, including parallel edges and self-loops.

mod common;

use linkrs_core::types::VertexId;
use linkrs_core::{Edge, EdgeDirection, Value};
use linkrs_storage::{GraphStorage, StorageReader, StorageWriter};

fn vid(id: i64) -> VertexId {
    VertexId::try_from_int64(id).expect("vertex id")
}

fn edge(src: i64, dst: i64, rank: i64) -> Edge {
    Edge::new(
        vid(src),
        vid(dst),
        "KNOWS".to_string(),
        rank,
        vec![("since".into(), Value::Int(2020))]
            .into_iter()
            .collect(),
    )
}

fn fixture() -> GraphStorage {
    let mut storage = common::create_in_memory_storage();
    common::setup_basic_schema(&mut storage);
    for id in 1..=3 {
        storage
            .insert_vertex(
                "test_space",
                common::create_person_vertex(id, &format!("v{id}"), 20),
            )
            .expect("insert vertex");
    }
    for e in [
        edge(1, 2, 0),
        edge(1, 2, 1),
        edge(2, 1, 0),
        edge(2, 3, 0),
        edge(3, 1, 0),
        edge(1, 1, 0),
    ] {
        storage.insert_edge("test_space", e).expect("insert edge");
    }
    storage
}

fn portable(
    storage: &GraphStorage,
    seeds: &[VertexId],
    direction: EdgeDirection,
    edge_types: &[String],
) -> Vec<usize> {
    let neighbors = storage
        .neighbor_dst_ids_batch("test_space", seeds, direction, edge_types)
        .expect("neighbors");
    let lengths: Vec<usize> = neighbors.iter().map(Vec::len).collect();
    let flat: Vec<VertexId> = neighbors.into_iter().flatten().collect();
    let degrees = storage
        .out_degree_batch("test_space", &flat, direction, edge_types)
        .expect("degrees");
    let mut totals = Vec::with_capacity(seeds.len());
    let mut offset = 0;
    for length in lengths {
        totals.push(degrees[offset..offset + length].iter().sum());
        offset += length;
    }
    totals
}

#[test]
fn neighbor_degree_matches_portable_composition() {
    let storage = fixture();
    let seeds = vec![vid(1), vid(2), vid(3), vid(999)];
    let edge_types = vec!["KNOWS".to_string()];

    let out = storage
        .neighbor_degree_batch("test_space", &seeds, EdgeDirection::Out, &edge_types)
        .expect("out neighbor degrees");
    assert_eq!(out, vec![7, 4, 3, 0]);
    assert_eq!(
        out,
        portable(&storage, &seeds, EdgeDirection::Out, &edge_types)
    );

    let in_degrees = storage
        .neighbor_degree_batch("test_space", &seeds, EdgeDirection::In, &edge_types)
        .expect("in neighbor degrees");
    assert_eq!(in_degrees, vec![6, 6, 2, 0]);
    assert_eq!(
        in_degrees,
        portable(&storage, &seeds, EdgeDirection::In, &edge_types)
    );
}

#[test]
fn neighbor_degree_without_edge_type_filter_matches_portable_composition() {
    let storage = fixture();
    let seeds = vec![vid(1), vid(2), vid(3)];
    let edge_types: Vec<String> = Vec::new();
    let fused = storage
        .neighbor_degree_batch("test_space", &seeds, EdgeDirection::Out, &edge_types)
        .expect("all-type neighbor degrees");
    assert_eq!(
        fused,
        portable(&storage, &seeds, EdgeDirection::Out, &edge_types)
    );
}
