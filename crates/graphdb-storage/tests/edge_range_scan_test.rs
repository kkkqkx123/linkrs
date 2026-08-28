//! Edge cursor `edge_src_id_range` correctness.
//!
//! The CSR indexes edges by internal vertex index; the src-id range in
//! `ScanOptions` is expressed over EXTERNAL vertex ids. This regression test
//! pins the mapping: a partitioned edge scan must return exactly the edges
//! whose external src id falls in the range, and the ranges must be complete
//! and disjoint (union == full scan).

use std::sync::Arc;

use graphdb_storage::core::types::{EdgeTypeInfo, PropertyDef, SpaceInfo, TagInfo, VertexId};
use graphdb_storage::core::vertex_edge_path::Tag;
use graphdb_storage::core::{DataType, Edge, Vertex};
use graphdb_storage::{GraphStorage, ScanOptions, StorageReader, StorageSchemaOps, StorageWriter};
use parking_lot::RwLock;

const SPACE: &str = "ers";
const TAG: &str = "Node";
const EDGE: &str = "Link";
const VERTEX_COUNT: i64 = 2000;

fn setup_storage() -> Arc<RwLock<GraphStorage>> {
    let mut storage = GraphStorage::new().expect("storage init");
    let mut space = SpaceInfo::new(SPACE.to_string()).with_vid_type(DataType::BigInt);
    storage.create_space(&mut space).unwrap();
    storage
        .create_tag(
            SPACE,
            &TagInfo::new(TAG.to_string()).with_properties(vec![PropertyDef::new(
                "value".to_string(),
                DataType::BigInt,
            )]),
        )
        .unwrap();
    storage
        .create_edge_type(
            SPACE,
            &EdgeTypeInfo::new(EDGE.to_string())
                .with_src_tag(TAG.to_string())
                .with_dst_tag(TAG.to_string()),
        )
        .unwrap();

    let mut start = 0i64;
    while start < VERTEX_COUNT {
        let end = (start + 500).min(VERTEX_COUNT);
        let vertices: Vec<Vertex> = (start..end)
            .map(|i| {
                Vertex::new(
                    VertexId::from_int64(i),
                    vec![Tag::new(TAG.to_string(), Default::default())],
                )
            })
            .collect();
        storage.batch_insert_vertices(SPACE, vertices).unwrap();
        start = end;
    }

    let mut edges = Vec::with_capacity((VERTEX_COUNT * 2) as usize);
    for src in 0..VERTEX_COUNT {
        for k in 1..=2i64 {
            edges.push(Edge {
                src: VertexId::from_int64(src),
                dst: VertexId::from_int64((src + k) % VERTEX_COUNT),
                edge_type: EDGE.to_string(),
                ranking: 0,
                props: Default::default(),
            });
        }
    }
    for chunk in edges.chunks(10_000) {
        storage.batch_insert_edges(SPACE, chunk.to_vec()).unwrap();
    }
    Arc::new(RwLock::new(storage))
}

fn drain(storage: &Arc<RwLock<GraphStorage>>, range: Option<std::ops::Range<i64>>) -> Vec<Edge> {
    let mut opts = ScanOptions::new();
    opts.edge_type = Some(EDGE.to_string());
    opts.edge_src_id_range = range;
    let reader = storage.read();
    let mut cursor = reader
        .create_edge_cursor(SPACE, &opts)
        .expect("open edge cursor");
    let mut out = Vec::new();
    loop {
        let batch = cursor.next_batch(128).expect("edge batch");
        if batch.is_empty() {
            break;
        }
        out.extend(batch);
    }
    out
}

#[test]
fn edge_src_range_returns_only_in_range_srcs() {
    let storage = setup_storage();
    let edges = drain(&storage, Some(0..500));
    assert_eq!(edges.len(), 1000, "500 srcs x 2 edges");
    assert!(
        edges
            .iter()
            .all(|e| e.src.as_int64().is_some_and(|s| (0..500).contains(&s))),
        "all edges must have external src in [0, 500)"
    );
}

#[test]
fn edge_src_ranges_are_complete_and_disjoint() {
    let storage = setup_storage();
    let total = drain(&storage, None);
    assert_eq!(total.len(), 4000);

    let p0 = drain(&storage, Some(0..500));
    let p1 = drain(&storage, Some(500..1000));
    let p2 = drain(&storage, Some(1000..1500));
    let p3 = drain(&storage, Some(1500..2000));

    let mut combined: Vec<Edge> = p0.into_iter().chain(p1).chain(p2).chain(p3).collect();
    combined.sort_by(|a, b| format!("{a:?}").cmp(&format!("{b:?}")));
    let mut expected = total;
    expected.sort_by(|a, b| format!("{a:?}").cmp(&format!("{b:?}")));
    assert_eq!(combined, expected, "partitioned scan must equal full scan");
}

#[test]
fn edge_src_range_without_start_filter_returns_nothing_for_high_range() {
    let storage = setup_storage();
    let edges = drain(&storage, Some(3000..4000));
    assert!(edges.is_empty());
}
