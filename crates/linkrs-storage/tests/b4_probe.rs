//! TEMPORARY B4 probe: staged single-edge insert vs batch insert, small scale.
//! Deleted after diagnosis. Run: cargo test -p linkrs-storage --test b4_probe --profile=test -- --nocapture
use std::time::Instant;

use tempfile::TempDir;

use linkrs_core::types::{EdgeTypeInfo, PropertyDef, SpaceInfo, TagInfo, TransactionId, VertexId};
use linkrs_core::vertex_edge_path::Tag;
use linkrs_core::{DataType, Edge, Value, Vertex};
use linkrs_storage::{
    GraphStorage, StorageCommitOps, StorageOperationContext, StorageOperationContextOps,
    StorageSchemaOps, StorageWriter,
};

const SPACE: &str = "b4p";
const TAG: &str = "Node";
const EDGE: &str = "Link";

fn setup(nv: usize) -> (GraphStorage, TempDir) {
    let dir = TempDir::new().expect("temp dir");
    let mut storage =
        GraphStorage::new_with_path(dir.path().to_path_buf()).expect("storage");
    let mut space = SpaceInfo::new(SPACE.to_string()).with_vid_type(DataType::BigInt);
    storage.create_space(&mut space).expect("space");
    storage
        .create_tag(
            SPACE,
            &TagInfo::new(TAG.to_string()).with_properties(vec![PropertyDef::new(
                "value".to_string(),
                DataType::BigInt,
            )]),
        )
        .expect("tag");
    storage
        .create_edge_type(
            SPACE,
            &EdgeTypeInfo::new(EDGE.to_string())
                .with_src_tag(TAG.to_string())
                .with_dst_tag(TAG.to_string()),
        )
        .expect("edge type");
    for chunk in (0..nv as i64).collect::<Vec<_>>().chunks(10_000) {
        let vertices: Vec<Vertex> = chunk
            .iter()
            .map(|i| {
                Vertex::new(
                    VertexId::try_from_int64(*i).expect("vid"),
                    Tag::new(
                        TAG.to_string(),
                        [("value".into(), Value::BigInt(*i))].into_iter().collect(),
                    ),
                )
            })
            .collect();
        let done = chunk[0] as usize;
        storage
            .batch_insert_vertices(SPACE, vertices)
            .expect("batch vertices");
        if done % 10_000 == 0 {
            println!("setup vertex {done}/{nv}");
        }
    }
    println!("setup done");
    (storage, dir)
}

fn make_edge(i: usize, nv: usize, salt: i64) -> Edge {
    Edge {
        src: VertexId::try_from_int64(i as i64 % nv as i64).expect("src"),
        dst: VertexId::try_from_int64((i as i64 + 1) % nv as i64).expect("dst"),
        edge_type: EDGE.to_string(),
        ranking: salt + i as i64,
        props: Default::default(),
    }
}

fn scale(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

#[test]
fn probe_staged_vs_batch() {
    let nv = scale("B4P_VERTICES", 50_000);
    let ne = scale("B4P_EDGES", 200);
    let use_session = std::env::var("B4P_SESSION").is_ok();
    let (storage, _dir) = setup(nv);

    // Path 1: explicit-txn staged single inserts (B4 shape).
    println!("acquiring timestamp");
    let _session = if use_session {
        println!("bulk-load session engaged (triggers suppressed)");
        Some(storage.begin_bulk_load().expect("session"))
    } else {
        None
    };
    let ts = storage
        .version_manager()
        .acquire_insert_timestamp()
        .expect("ts");
    println!("timestamp acquired");
    let txid = TransactionId::from(1u64);
    let mut txn = storage.bind_operation_context(
        StorageOperationContext::transaction_with_timestamps(txid, ts, Some(ts), false, false),
    );
    let start = Instant::now();
    let mut marks = Vec::new();
    for i in 0..ne {
        txn.insert_edge(SPACE, make_edge(i, nv, 0)).expect("insert");
        if i % (ne / 10).max(1) == 0 || i + 1 == ne {
            marks.push((i + 1, start.elapsed()));
        }
    }
    for (n, dt) in &marks {
        println!("staged {n}/{ne} elapsed={dt:?}");
    }
    let staged_us = start.elapsed().as_micros();
    drop(txn);
    if let Some(session) = _session {
        session.abandon();
    }
    println!(
        "STAGED single: {ne} edges in {staged_us}us = {:.1}us/edge",
        staged_us as f64 / ne as f64
    );

    // Path 2: batch insert same count (fresh storage to avoid dup edges).
    let (mut storage2, _dir2) = setup(nv);
    let edges: Vec<Edge> = (0..ne).map(|i| make_edge(i, nv, 0)).collect();
    let start = Instant::now();
    storage2
        .batch_insert_edges(SPACE, edges)
        .expect("batch");
    let batch_us = start.elapsed().as_micros();
    println!(
        "BATCH: {ne} edges in {batch_us}us = {:.1}us/edge",
        batch_us as f64 / ne as f64
    );

    // Path 3: auto-commit single inserts.
    let start = Instant::now();
    for i in 0..ne {
        storage2
            .insert_edge(SPACE, make_edge(i, nv, 10_000_000))
            .expect("insert");
    }
    let single_us = start.elapsed().as_micros();
    println!(
        "AUTOCOMMIT single: {ne} edges in {single_us}us = {:.1}us/edge",
        single_us as f64 / ne as f64
    );
}
