//! Standalone WAL benchmark: fsync latency baseline and WAL durability cost
//! under different sync policies, plus sequential replay-read throughput.
//!
//! WAL previously appeared only inside the ingest/rollback benches as a
//! "persistent - memory" difference. This suite isolates it: the cost of a
//! per-append fsync barrier, and what a sync policy buys end to end.
//!
//! Uses public entry points only: a raw append+`sync_all` file as the
//! synchronous-fsync baseline, and `GraphStorage::open_with_persistence` with
//! each `SyncPolicy`, measuring batch insert throughput end to end. The raw
//! baseline deliberately fsyncs metadata too; every log in the engine syncs
//! data only, so it reads as an upper bound rather than the shipped path.

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use linkrs_core::types::{EdgeTypeInfo, SpaceInfo, VertexId};
use linkrs_core::wal::SyncPolicy;
use linkrs_core::{DataType, Edge};
use linkrs_storage::{GraphStorage, StorageSchemaOps, StorageWriter};
use std::io::Write;
use std::path::Path;
use std::time::Duration;

#[path = "results_report.rs"]
mod report;
use report::write_results_report;

const PAYLOAD_LEN: usize = 128;
const EDGE_COUNT: usize = 2000;

fn make_payload(i: usize) -> Vec<u8> {
    (0..PAYLOAD_LEN).map(|b| ((i + b) % 251) as u8).collect()
}

fn make_edges() -> Vec<Edge> {
    (0..EDGE_COUNT as i64)
        .map(|i| Edge {
            src: VertexId::try_from_int64(i % 500).expect("valid vertex id"),
            dst: VertexId::try_from_int64((i * 7 + 3) % 500).expect("valid vertex id"),
            edge_type: "Link".to_string(),
            ranking: i,
            props: Default::default(),
        })
        .collect()
}

fn bench_fsync_latency(c: &mut Criterion) {
    let tmp = tempfile::TempDir::new().expect("tmpdir");

    let mut group = c.benchmark_group("wal_fsync");
    group.measurement_time(Duration::from_secs(3));
    group.sample_size(30);
    group.warm_up_time(Duration::from_millis(500));

    // Synchronous baseline: one sync_all per append, no group commit.
    group.bench_function("append_plus_sync_all_per_op", |b| {
        let path = tmp.path().join("raw_sync.bin");
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .expect("open raw file");
        let mut i = 0usize;
        b.iter(|| {
            file.write_all(&make_payload(i)).expect("write");
            file.sync_all().expect("sync");
            i += 1;
        });
    });

    // Append without sync (upper bound; group commit amortizes toward this).
    group.bench_function("append_no_sync", |b| {
        let path = tmp.path().join("raw_nosync.bin");
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .expect("open raw file");
        let mut i = 0usize;
        b.iter(|| {
            file.write_all(&make_payload(i)).expect("write");
            i += 1;
        });
    });

    group.finish();
}

fn open_store(path: &Path, sync_policy: Option<SyncPolicy>) -> GraphStorage {
    GraphStorage::open_with_persistence(path.to_path_buf(), true, sync_policy)
        .expect("open storage with persistence")
}

fn bench_sync_policy_ingest(c: &mut Criterion) {
    let mut report = String::from("WAL sync policy ingest throughput (bench profile)\n\n");
    report.push_str(&format!(
        "workload: batch_insert_edges of {} edges into a fresh store\n\n",
        EDGE_COUNT
    ));
    report.push_str("| sync policy | wall (one-shot) |\n|---|---|\n");

    let cases: &[(&str, Option<SyncPolicy>)] = &[
        ("never", Some(SyncPolicy::Never)),
        ("every_write", Some(SyncPolicy::EveryWrite)),
        ("batch_64", Some(SyncPolicy::batch(64))),
        ("batch_512", Some(SyncPolicy::batch(512))),
    ];

    for (label, policy) in cases {
        let tmp = tempfile::TempDir::new().expect("tmpdir");
        {
            let mut storage = open_store(tmp.path(), *policy);
            let space_name = "wal_bench".to_string();
            let mut space = SpaceInfo::new(space_name.clone()).with_vid_type(DataType::BigInt);
            storage.create_space(&mut space).expect("create space");
            storage
                .create_edge_type(&space_name, &EdgeTypeInfo::new("Link".to_string()))
                .expect("create edge type");
            storage
                .create_tag(
                    &space_name,
                    &linkrs_core::types::TagInfo::new("Node".to_string()).with_properties(vec![
                        linkrs_core::types::PropertyDef::new("id".into(), DataType::BigInt),
                    ]),
                )
                .expect("create tag");
            let vertices: Vec<linkrs_core::Vertex> = (0..500i64)
                .map(|i| {
                    linkrs_core::Vertex::new(
                        VertexId::try_from_int64(i).expect("valid vertex id"),
                        linkrs_core::vertex_edge_path::Tag::new(
                            "Node".to_string(),
                            [("id".into(), linkrs_core::Value::BigInt(i))]
                                .into_iter()
                                .collect(),
                        ),
                    )
                })
                .collect();
            storage
                .batch_insert_vertices(&space_name, vertices)
                .expect("insert vertices");

            let start = std::time::Instant::now();
            storage
                .batch_insert_edges(&space_name, make_edges())
                .expect("insert edges");
            let elapsed = start.elapsed();
            report.push_str(&format!(
                "| {} | {:.2} ms |\n",
                label,
                elapsed.as_secs_f64() * 1000.0
            ));
        }

        let mut group = c.benchmark_group("wal_sync_policy_ingest");
        group.measurement_time(Duration::from_secs(3));
        group.sample_size(10);
        group.warm_up_time(Duration::from_millis(300));
        let policy = *policy;
        group.bench_function(*label, |b| {
            b.iter_batched(
                // Fresh store per iteration; space and schema live in setup so
                // only the edge-ingest WAL cost is timed.
                || {
                    let tmp = tempfile::TempDir::new().expect("tmpdir");
                    let mut storage = open_store(tmp.path(), policy);
                    let space_name = "wal_bench".to_string();
                    let mut space =
                        SpaceInfo::new(space_name.clone()).with_vid_type(DataType::BigInt);
                    storage.create_space(&mut space).expect("create space");
                    storage
                        .create_tag(
                            &space_name,
                            &linkrs_core::types::TagInfo::new("Node".to_string()).with_properties(
                                vec![linkrs_core::types::PropertyDef::new(
                                    "id".to_string(),
                                    DataType::BigInt,
                                )],
                            ),
                        )
                        .expect("create tag");
                    storage
                        .create_edge_type(&space_name, &EdgeTypeInfo::new("Link".to_string()))
                        .expect("create edge type");
                    let vertices: Vec<linkrs_core::Vertex> = (0..500i64)
                        .map(|i| {
                            linkrs_core::Vertex::new(
                                VertexId::try_from_int64(i).expect("valid vertex id"),
                                linkrs_core::vertex_edge_path::Tag::new(
                                    "Node".to_string(),
                                    [("id".into(), linkrs_core::Value::BigInt(i))]
                                        .into_iter()
                                        .collect(),
                                ),
                            )
                        })
                        .collect();
                    storage
                        .batch_insert_vertices(&space_name, vertices)
                        .expect("insert vertices");
                    (tmp, storage)
                },
                |(_tmp, mut storage)| {
                    storage
                        .batch_insert_edges("wal_bench", make_edges())
                        .expect("insert edges");
                    black_box(&storage);
                },
                criterion::BatchSize::PerIteration,
            );
        });
        group.finish();
    }

    let path = write_results_report("wal_bench", "results.txt", &report);
    println!("report written to {}", path.display());
}

fn bench_replay_read(c: &mut Criterion) {
    // Sequential scan of a WAL-sized log: bounds crash-replay throughput.
    let tmp = tempfile::TempDir::new().expect("tmpdir");
    let log = tmp.path().join("replay.bin");
    let entries = 5000usize;
    {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&log)
            .expect("create log");
        for i in 0..entries {
            file.write_all(&make_payload(i)).expect("write");
        }
        file.sync_all().expect("sync");
    }

    let mut group = c.benchmark_group("wal_replay_read");
    group.measurement_time(Duration::from_secs(3));
    group.sample_size(20);
    group.warm_up_time(Duration::from_millis(300));
    group.bench_function("sequential_scan_5000_entries", |b| {
        b.iter(|| {
            let data = std::fs::read(&log).expect("read wal");
            black_box(data.len());
        });
    });
    group.finish();
}

criterion_group!(
    benches,
    bench_fsync_latency,
    bench_sync_policy_ingest,
    bench_replay_read
);
criterion_main!(benches);
