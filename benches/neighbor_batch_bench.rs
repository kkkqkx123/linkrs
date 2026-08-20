//! batched neighbor-read accessor benchmark.
//!
//! Compares the de-materialized batch accessors (`neighbor_dst_ids_batch`,
//! `out_degree_batch`) against the full `get_node_edges` path per vertex.
//! The edge type carries properties so the reference path pays per-edge
//! property decoding, which the batch accessors skip.
//!
//! Run with:
//!   cargo bench --bench neighbor_batch_bench

use graphdb::core::types::{EdgeTypeInfo, PropertyDef, SpaceInfo, TagInfo, VertexId};
use graphdb::core::vertex_edge_path::Tag;
use graphdb::core::{DataType, Edge, Value, Vertex};
use graphdb::storage::{GraphStorage, StorageReader, StorageSchemaOps, StorageWriter};
use std::collections::HashMap;
use std::time::Instant;

const SPACE: &str = "nb";
const TAG: &str = "Node";
const EDGE: &str = "Link";
const VERTEX_COUNT: u64 = 20_000;
const EDGES_PER_VERTEX: i64 = 3;

fn setup() -> GraphStorage {
    let mut storage = GraphStorage::new().expect("storage init");
    let mut space = SpaceInfo::new(SPACE.to_string()).with_vid_type(DataType::BigInt);
    storage.create_space(&mut space).expect("create space");
    storage
        .create_tag(
            SPACE,
            &TagInfo::new(TAG.to_string()).with_properties(vec![PropertyDef::new(
                "value".to_string(),
                DataType::BigInt,
            )]),
        )
        .expect("create tag");
    storage
        .create_edge_type(
            SPACE,
            &EdgeTypeInfo::new(EDGE.to_string())
                .with_properties(vec![
                    PropertyDef::new("weight".to_string(), DataType::BigInt),
                    PropertyDef::new("label".to_string(), DataType::String),
                ])
                .with_src_tag(TAG.to_string())
                .with_dst_tag(TAG.to_string()),
        )
        .expect("create edge type");

    let mut start = 0usize;
    while start < VERTEX_COUNT as usize {
        let end = (start + 10_000).min(VERTEX_COUNT as usize);
        let vertices: Vec<Vertex> = (start..end)
            .map(|i| {
                Vertex::new(
                    VertexId::from_int64(i as i64),
                    vec![Tag::new(
                        TAG.to_string(),
                        vec![("value".to_string(), Value::BigInt(i as i64))]
                            .into_iter()
                            .collect(),
                    )],
                )
            })
            .collect();
        storage
            .batch_insert_vertices(SPACE, vertices)
            .expect("insert vertices");
        start = end;
    }

    let mut edges = Vec::with_capacity((VERTEX_COUNT * EDGES_PER_VERTEX as u64) as usize);
    for src in 0..VERTEX_COUNT as i64 {
        for k in 1..=EDGES_PER_VERTEX {
            let mut props = HashMap::new();
            props.insert("weight".to_string(), Value::BigInt(k * 7));
            props.insert("label".to_string(), Value::string(format!("e{src}_{k}")));
            edges.push(Edge {
                src: VertexId::from_int64(src),
                dst: VertexId::from_int64((src + k) % VERTEX_COUNT as i64),
                edge_type: EDGE.to_string(),
                ranking: 0,
                props,
            });
        }
    }
    for chunk in edges.chunks(50_000) {
        storage
            .batch_insert_edges(SPACE, chunk.to_vec())
            .expect("insert edges");
    }
    storage
}

fn median_us(samples: &[u64]) -> u64 {
    let mut v = samples.to_vec();
    v.sort_unstable();
    v[v.len() / 2]
}

fn main() {
    println!("== neighbor batch accessor benchmark ==");
    let storage = setup();
    println!("vertices={VERTEX_COUNT}, edges/vertex={EDGES_PER_VERTEX}");

    let seeds: Vec<VertexId> = (0..VERTEX_COUNT as i64).map(VertexId::from_int64).collect();
    let no_types: Vec<String> = Vec::new();
    let iterations = 20;

    // Reference: full per-vertex get_node_edges (materializes Edge + props).
    let mut per_vertex_us = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        let start = Instant::now();
        for seed in &seeds {
            let _ = storage.get_node_edges(SPACE, seed, graphdb::core::EdgeDirection::Out);
        }
        per_vertex_us.push(start.elapsed().as_micros() as u64 * 1_000 / seeds.len() as u64);
    }
    let ref_ns = median_us(&per_vertex_us);

    // Batch neighbor read.
    let mut batch_us = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        let start = Instant::now();
        let result = storage.neighbor_dst_ids_batch(
            SPACE,
            &seeds,
            graphdb::core::EdgeDirection::Out,
            &no_types,
        );
        let total: usize = result.map(|r| r.iter().map(Vec::len).sum()).unwrap_or(0);
        assert_eq!(total, (VERTEX_COUNT * EDGES_PER_VERTEX as u64) as usize);
        batch_us.push(start.elapsed().as_micros() as u64 * 1_000 / seeds.len() as u64);
    }
    let batch_ns = median_us(&batch_us);

    // Batch out-degree read.
    let mut deg_us = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        let start = Instant::now();
        let degrees =
            storage.out_degree_batch(SPACE, &seeds, graphdb::core::EdgeDirection::Out, &no_types);
        assert_eq!(
            degrees.map(|d| d.iter().sum::<usize>()).unwrap_or(0),
            seeds.len() * EDGES_PER_VERTEX as usize
        );
        deg_us.push(start.elapsed().as_micros() as u64 * 1_000 / seeds.len() as u64);
    }
    let deg_ns = median_us(&deg_us);

    println!("get_node_edges       : {ref_ns} ns/vertex");
    println!("neighbor_dst_ids_batch: {batch_ns} ns/vertex");
    println!("out_degree_batch     : {deg_ns} ns/vertex");
    println!(
        "neighbor speedup     : {:.1}x (target >= 3x)",
        ref_ns as f64 / batch_ns.max(1) as f64
    );
    println!(
        "out-degree speedup   : {:.1}x",
        ref_ns as f64 / deg_ns.max(1) as f64
    );
}
