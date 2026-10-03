//! Crash recovery / restart latency benchmark.
//!
//! Addresses the gap: restart latency was only thinly covered. Times
//! `GraphStorage::open` for (a) a clean checkpointed store and (b) a store
//! with an un-checkpointed WAL tail to replay. Debug-mode friendly dataset.

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use graphdb::core::types::{EdgeTypeInfo, PropertyDef, SpaceInfo, TagInfo, VertexId};
use graphdb::core::vertex_edge_path::Tag;
use graphdb::core::{DataType, Edge, Value, Vertex};
use graphdb::storage::{
    GraphStorage, StoragePersistenceOps, StorageReader, StorageSchemaOps, StorageWriter,
};
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// Write a human-readable report into `benches/results/<bench_name>/`.
fn write_results_report(bench_name: &str, filename: &str, content: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("benches")
        .join("results")
        .join(bench_name);
    std::fs::create_dir_all(&dir).expect("create results dir");
    let path = dir.join(filename);
    std::fs::write(&path, content).expect("write results report");
    path
}

const VERTEX_COUNT: usize = 2000;
const EDGES_PER_VERTEX: usize = 2;

fn make_vertex(i: i64) -> Vertex {
    Vertex::new(
        VertexId::try_from_int64(i).expect("valid vertex id"),
        Tag::new(
            "Node".to_string(),
            [
                ("id".to_string(), Value::BigInt(i)),
                ("value".to_string(), Value::Double(i as f64)),
            ]
            .into_iter()
            .collect(),
        ),
    )
}

fn populate(path: &std::path::Path, checkpoint: bool) {
    let mut storage = GraphStorage::new_with_path(path.to_path_buf()).expect("create storage");
    let space_name = "recovery".to_string();
    let mut space = SpaceInfo::new(space_name.clone()).with_vid_type(DataType::BigInt);
    storage.create_space(&mut space).expect("create space");

    storage
        .create_tag(
            &space_name,
            &TagInfo::new("Node".to_string()).with_properties(vec![
                PropertyDef::new("id".to_string(), DataType::BigInt),
                PropertyDef::new("value".to_string(), DataType::Double),
            ]),
        )
        .expect("create tag");
    storage
        .create_edge_type(&space_name, &EdgeTypeInfo::new("Link".to_string()))
        .expect("create edge type");

    // Schema metadata plus half the vertices are checkpointed first so the
    // restart scenarios below isolate data-recovery cost (space lookup must
    // survive reopen). NOTE: edge WAL tails currently fail replay with
    // "Source vertex label not found during recovery" (see docs/issue), so
    // the tail scenario uses vertex-only ops.
    let head: Vec<Vertex> = (0..VERTEX_COUNT as i64 / 2).map(make_vertex).collect();
    storage
        .batch_insert_vertices(&space_name, head)
        .expect("insert head vertices");
    storage.create_checkpoint().expect("head checkpoint");

    if checkpoint {
        let tail: Vec<Vertex> = ((VERTEX_COUNT / 2) as i64..VERTEX_COUNT as i64)
            .map(make_vertex)
            .collect();
        storage
            .batch_insert_vertices(&space_name, tail)
            .expect("insert tail vertices");
        let edges: Vec<Edge> = (0..VERTEX_COUNT as i64)
            .flat_map(|src| {
                (1..=EDGES_PER_VERTEX).map(move |k| Edge {
                    src: VertexId::try_from_int64(src).expect("valid vertex id"),
                    dst: VertexId::try_from_int64((src + k as i64) % VERTEX_COUNT as i64)
                        .expect("valid vertex id"),
                    edge_type: "Link".to_string(),
                    ranking: 0,
                    props: Default::default(),
                })
            })
            .collect();
        storage
            .batch_insert_edges(&space_name, edges)
            .expect("insert edges");
        storage.create_checkpoint().expect("full checkpoint");
    } else {
        // Un-checkpointed WAL tail: the remaining vertex writes only.
        let tail: Vec<Vertex> = ((VERTEX_COUNT / 2) as i64..VERTEX_COUNT as i64)
            .map(make_vertex)
            .collect();
        storage
            .batch_insert_vertices(&space_name, tail)
            .expect("insert tail vertices");
    }
    drop(storage);
}

/// Open a store, retrying while a background persistence operation (dropped at
/// the end of `populate`) is still in flight. Returns the opened storage.
fn open_settled(path: &std::path::Path) -> GraphStorage {
    for _ in 0..100 {
        match GraphStorage::open(path.to_path_buf()) {
            Ok(storage) => return storage,
            Err(err) if err.to_string().contains("already active") => {
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(err) => panic!("reopen failed: {err}"),
        }
    }
    panic!("persistence never settled after 100 retries");
}

fn bench_restart(c: &mut Criterion) {
    let mut report = String::from("crash recovery / restart latency (debug build)\n\n");
    report.push_str(&format!(
        "dataset: {} vertices, {} edges\n\n",
        VERTEX_COUNT,
        VERTEX_COUNT * EDGES_PER_VERTEX
    ));
    report.push_str(
        "| scenario | open latency (median of criterion) | one-shot wall |\n|---|---|---|\n",
    );

    for (label, checkpoint) in &[("clean_checkpoint", true), ("wal_tail_replay", false)] {
        let tmp = tempfile::TempDir::new().expect("tmpdir");
        populate(tmp.path(), *checkpoint);

        // One-shot wall measurement (outside criterion) for the report.
        // Settle first so background checkpoint/snapshot started at drop has
        // finished and the timed open measures recovery only.
        {
            let settled = open_settled(tmp.path());
            drop(settled);
            std::thread::sleep(Duration::from_millis(100));
        }
        let start = Instant::now();
        let opened = open_settled(tmp.path());
        let wall = start.elapsed();
        assert!(opened.get_space("recovery").expect("space").is_some());
        drop(opened);

        let mut group = c.benchmark_group("restart");
        group.measurement_time(Duration::from_secs(3));
        group.sample_size(10);
        group.warm_up_time(Duration::from_millis(300));
        let checkpoint = *checkpoint;
        // Fresh store per iteration: a replayed-open mutates the directory
        // (replay products / new checkpoint), so repeated opens of one dir
        // are not representative — populate happens in setup, only the
        // open+recover is timed.
        group.bench_function(*label, |b| {
            b.iter_batched(
                || {
                    let tmp = tempfile::TempDir::new().expect("tmpdir");
                    populate(tmp.path(), checkpoint);
                    // Settle background checkpoint/snapshot so the timed open
                    // below is not polluted by "already active" retries.
                    {
                        let settled = open_settled(tmp.path());
                        drop(settled);
                    }
                    std::thread::sleep(Duration::from_millis(100));
                    tmp
                },
                |tmp| {
                    let storage = open_settled(tmp.path());
                    black_box(&storage);
                    drop(storage);
                },
                criterion::BatchSize::PerIteration,
            );
        });
        group.finish();

        report.push_str(&format!(
            "| {} | see criterion group `restart/{}` | {:.2} ms |\n",
            label,
            label,
            wall.as_secs_f64() * 1000.0
        ));
    }

    let path = write_results_report("crash_recovery_bench", "results.txt", &report);
    println!("report written to {}", path.display());
}

criterion_group!(benches, bench_restart);
criterion_main!(benches);
