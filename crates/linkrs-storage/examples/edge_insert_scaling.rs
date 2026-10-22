//! Controlled micro-experiment: fix the number of vertices, vary the number of edges per vertex (epv), and measure the per-edge cost of a single edge insertion window by window.
//! Run: cargo run --release --example edge_insert_scaling -p linkrs-storage
//! Available environment variables: LINKRS_V (number of vertices, default 1000), LINKRS_EPV (edges per vertex, default 100).
//! Purpose: reproduce and locate the background-maintenance lock-contention stall that occurs during edge insertion when crossing the variable CSR memory threshold.

use linkrs_core::types::{
    EdgeTypeInfo, PropertyDef, SpaceInfo, TagInfo, VertexId,
};
use linkrs_core::vertex_edge_path::Tag;
use linkrs_core::{DataType, Edge, Value, Vertex};
use linkrs_storage::{GraphStorage, StorageSchemaOps, StorageWriter};
use std::collections::HashMap;
use std::time::Instant;

fn setup(vertex_count: u64, edges_per_vertex: usize) -> (GraphStorage, Vec<Edge>) {
    let mut storage = GraphStorage::new().expect("init");
    let mut space = SpaceInfo::new("bench".into()).with_vid_type(DataType::BigInt);
    storage
        .create_space(&mut space)
        .expect("space");
    storage
        .create_tag(
            "bench",
            &TagInfo::new("Node".to_string()).with_properties(vec![PropertyDef::new(
                "value".to_string(),
                DataType::BigInt,
            )]),
        )
        .expect("tag");
    for id in 0..vertex_count as i64 {
        storage
            .insert_vertex(
                "bench",
                Vertex::new(
                    VertexId::try_from_int64(id).expect("vid"),
                    Tag::new(
                        "Node".to_string(),
                        [("value".into(), Value::BigInt(id))].into_iter().collect(),
                    ),
                ),
            )
            .expect("vtx");
    }
    storage
        .create_edge_type(
            "bench",
            &EdgeTypeInfo::new("Link".to_string())
                .with_src_tag("Node".to_string())
                .with_dst_tag("Node".to_string()),
        )
        .expect("etype");
    let max_edges = edges_per_vertex.min((vertex_count as usize).saturating_sub(1));
    let edges: Vec<_> = (0..vertex_count as i64)
        .flat_map(|src| {
            (1..=max_edges as i64).map(move |k| {
                let dst = (src + k) % vertex_count as i64;
                Edge {
                    src: VertexId::try_from_int64(src).expect("v"),
                    dst: VertexId::try_from_int64(dst).expect("v"),
                    edge_type: "Link".to_string(),
                    ranking: 0,
                    props: HashMap::new(),
                }
            })
        })
        .collect();
    (storage, edges)
}

fn main() {
    let vertex_count: u64 = std::env::var("LINKRS_V")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1000);
    let epv: usize = std::env::var("LINKRS_EPV")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(100);
    let (mut storage, edges) = setup(vertex_count, epv);
    let window_size = 5000usize;
    let mut inserted = 0usize;
    let mut t0 = Instant::now();
    let mut win_max = 0f64;
    let mut win_over1ms = 0usize;
    for e in edges {
        let c0 = Instant::now();
        storage.insert_edge("bench", e).unwrap();
        let cdt = c0.elapsed().as_secs_f64() * 1e6;
        if cdt > win_max { win_max = cdt; }
        if cdt > 1000.0 { win_over1ms += 1; }
        inserted += 1;
        if inserted % window_size == 0 {
            let dt = t0.elapsed();
            let density = inserted as f64 / vertex_count as f64;
            println!(
                "edges={:>7} window_ms={:>9.1} us_per_edge={:>8.1} max={:>9.1}us >1ms={:>5} density={:.0}",
                inserted, dt.as_secs_f64()*1e3, dt.as_secs_f64()*1e6/window_size as f64, win_max, win_over1ms, density
            );
            t0 = Instant::now();
            win_max = 0.0;
            win_over1ms = 0;
        }
    }
}
