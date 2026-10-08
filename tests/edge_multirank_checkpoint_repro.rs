use std::collections::HashMap;

use linkrs::core::types::{EdgeTypeInfo, PropertyDef, SpaceInfo, TagInfo, VertexId};
use linkrs::core::vertex_edge_path::Tag;
use linkrs::core::{DataType, Edge, Value, Vertex};
use linkrs::storage::{
    GraphStorage, StoragePersistenceOps, StorageReader, StorageSchemaOps, StorageWriter,
};

fn build(path: std::path::PathBuf, nverts: usize, nedges: usize, multi_rank: bool) {
    build_chunked(path, nverts, nedges, multi_rank, 5000)
}

fn build_chunked(
    path: std::path::PathBuf,
    nverts: usize,
    nedges: usize,
    multi_rank: bool,
    chunk: usize,
) {
    let mut storage = GraphStorage::new_with_path(path).expect("storage");
    let mut space = SpaceInfo::new("s".to_string()).with_vid_type(DataType::BigInt);
    storage.create_space(&mut space).expect("space");
    storage
        .create_tag(
            "s",
            &TagInfo::new("N".to_string()).with_properties(vec![PropertyDef::new(
                "value".to_string(),
                DataType::BigInt,
            )]),
        )
        .expect("tag");
    storage
        .create_edge_type(
            "s",
            &EdgeTypeInfo::new("L".to_string())
                .with_src_tag("N".to_string())
                .with_dst_tag("N".to_string()),
        )
        .expect("edge type");
    let vertices: Vec<Vertex> = (0..nverts)
        .map(|i| {
            Vertex::new(
                VertexId::try_from_int64(i as i64).expect("vid"),
                Tag::new(
                    "N".to_string(),
                    [("value".to_string(), Value::BigInt(i as i64))]
                        .into_iter()
                        .map(|(name, value)| (std::sync::Arc::from(name.as_str()), value))
                        .collect(),
                ),
            )
        })
        .collect();
    storage.batch_insert_vertices("s", vertices).expect("verts");
    let edges: Vec<Edge> = (0..nedges)
        .map(|i| Edge {
            src: VertexId::try_from_int64((i % nverts) as i64).expect("vid"),
            dst: VertexId::try_from_int64(((i + 1) % nverts) as i64).expect("vid"),
            edge_type: "L".to_string(),
            ranking: if multi_rank { (i / nverts) as i64 } else { 0 },
            props: HashMap::new(),
        })
        .collect();
    for chunk in edges.chunks(chunk) {
        storage
            .batch_insert_edges("s", chunk.to_vec())
            .expect("edges");
    }
    storage.create_checkpoint().expect("checkpoint");
}

fn roundtrip(nverts: usize, nedges: usize, multi_rank: bool) -> Result<usize, String> {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let path = dir.path().to_path_buf();
    build(path.clone(), nverts, nedges, multi_rank);
    match GraphStorage::open(path) {
        Ok(reopened) => match reopened.scan_edges_by_type("s", "L") {
            Ok(edges) => Ok(edges.len()),
            Err(e) => Err(format!("audit failed: {e}")),
        },
        Err(e) => Err(format!("open failed: {e}")),
    }
}

fn roundtrip_chunked(
    nverts: usize,
    nedges: usize,
    multi_rank: bool,
    chunk: usize,
) -> Result<usize, String> {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let path = dir.path().to_path_buf();
    build_chunked(path.clone(), nverts, nedges, multi_rank, chunk);
    match GraphStorage::open(path) {
        Ok(reopened) => match reopened.scan_edges_by_type("s", "L") {
            Ok(edges) => Ok(edges.len()),
            Err(e) => Err(format!("audit failed: {e}")),
        },
        Err(e) => Err(format!("open failed: {e}")),
    }
}

#[test]
fn repro_matrix() {
    for (name, nv, ne, mr) in [
        ("unique-small", 1000, 1000, false),
        ("multirank-small", 200, 1000, true),
        ("unique-large", 10000, 10000, false),
        ("multirank-500v5r", 500, 2500, true),
        ("multirank-2000v2r", 2000, 4000, true),
        ("multirank-large", 2000, 10000, true),
    ] {
        match roundtrip(nv, ne, mr) {
            Ok(n) => {
                println!("{name}: open ok, edges={n} (expect {ne})");
                assert_eq!(n, ne, "{name}: edge count drift after reopen");
            }
            Err(e) => panic!("{name}: {e}"),
        }
    }
    match roundtrip_chunked(2000, 10000, true, 10000) {
        Ok(n) => {
            println!("multirank-large-single-chunk: open ok, edges={n}");
            assert_eq!(n, 10000);
        }
        Err(e) => panic!("multirank-large-single-chunk: {e}"),
    }
    match roundtrip_chunked(2000, 11000, true, 11000) {
        Ok(n) => {
            println!("multirank-11k-single-chunk: open ok, edges={n}");
            assert_eq!(n, 11000);
        }
        Err(e) => panic!("multirank-11k-single-chunk: {e}"),
    }
    // Two commits + two checkpoints, second commit uses fresh pairs.
    {
        use linkrs::storage::StorageWriter;
        let dir = tempfile::TempDir::new().expect("tempdir");
        let path = dir.path().to_path_buf();
        build_chunked(path.clone(), 2000, 10000, true, 10000);
        let mut storage = GraphStorage::open(path.clone()).expect("reopen1");
        let tail: Vec<Edge> = (0..1000)
            .map(|i| Edge {
                src: VertexId::try_from_int64((i % 2000) as i64).expect("vid"),
                dst: VertexId::try_from_int64(((i + 2) % 2000) as i64).expect("vid"),
                edge_type: "L".to_string(),
                ranking: 0,
                props: HashMap::new(),
            })
            .collect();
        storage.batch_insert_edges("s", tail).expect("tail");
        storage.flush().expect("sync");
        storage.create_checkpoint().expect("checkpoint2");
        drop(storage);
        match GraphStorage::open(path) {
            Ok(reopened) => match reopened.scan_edges_by_type("s", "L") {
                Ok(edges) => {
                    println!("two-commit-fresh-pairs: open ok, edges={}", edges.len());
                    assert_eq!(edges.len(), 11000);
                }
                Err(e) => panic!("two-commit-fresh-pairs: audit failed: {e}"),
            },
            Err(e) => panic!("two-commit-fresh-pairs: open failed: {e}"),
        }
    }
}
