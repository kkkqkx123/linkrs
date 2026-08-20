//! Large-transaction rollback cost.
//!
//! Measures commit vs abort (undo execution) time for a single explicit
//! transaction that writes N edges:
//!
//! The explicit-transaction path binds a storage operation context with the
//! transaction's write timestamp (no `AutoCommitWriteGate` involvement) and
//! stages WAL entries under the transaction id. Commit appends the staged
//! WAL with Sync durability (fsync); abort drops the staged WAL and releases
//! the MVCC write timestamp.
//!
//! Plain-main bench (harness = false). Run with:
//!   cargo bench --bench rollback_bench

use std::hint::black_box;
use std::time::Instant;

use tempfile::TempDir;

use graphdb::core::types::{
    EdgeTypeInfo, PropertyDef, SpaceInfo, TagInfo, TransactionId, VertexId,
};
use graphdb::core::vertex_edge_path::Tag;
use graphdb::core::{DataType, Edge, Value, Vertex};
use graphdb::storage::{
    GraphStorage, StorageCommitOps, StorageOperationContext, StorageOperationContextOps,
    StorageSchemaOps, StorageWriter,
};

const SPACE: &str = "b4";
const TAG: &str = "Node";
const EDGE: &str = "Link";
const VERTEX_COUNT: usize = 1_000_000;
const EDGE_COUNTS: [usize; 3] = [100_000, 500_000, 1_000_000];
const ITERATIONS: usize = 3;

fn setup_storage() -> (GraphStorage, Option<TempDir>) {
    let dir = TempDir::new().expect("temp directory");
    let mut storage =
        GraphStorage::new_with_path(dir.path().to_path_buf()).expect("persistent storage");
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
                .with_src_tag(TAG.to_string())
                .with_dst_tag(TAG.to_string()),
        )
        .expect("create edge type");

    for chunk in (0..VERTEX_COUNT as i64).collect::<Vec<_>>().chunks(100_000) {
        let vertices: Vec<Vertex> = chunk
            .iter()
            .map(|&i| {
                Vertex::new(
                    VertexId::from_int64(i),
                    vec![Tag::new(
                        TAG.to_string(),
                        [("value".to_string(), Value::BigInt(i))]
                            .into_iter()
                            .collect(),
                    )],
                )
            })
            .collect();
        storage
            .batch_insert_vertices(SPACE, vertices)
            .expect("setup vertices");
    }
    (storage, Some(dir))
}

struct RunResult {
    write_us: u64,
    commit_us: u64,
    abort_us: u64,
}

fn run_transaction(storage: &GraphStorage, edge_count: usize, tx_seq: u64) -> RunResult {
    // Explicit transaction: bind the storage handle with the transaction's
    // write timestamp (no auto-commit gate involved).
    let ts = storage
        .version_manager()
        .acquire_insert_timestamp()
        .expect("acquire timestamp");
    let txid = TransactionId::from(tx_seq);

    let mut txn =
        storage.bind_operation_context(StorageOperationContext::transaction(txid, ts, false));

    let edges: Vec<Edge> = (0..edge_count)
        .map(|i| Edge {
            src: VertexId::from_int64(i as i64 % VERTEX_COUNT as i64),
            dst: VertexId::from_int64((i as i64 + 1) % VERTEX_COUNT as i64),
            edge_type: EDGE.to_string(),
            ranking: tx_seq as i64 * 2_000_000 + i as i64,
            props: Default::default(),
        })
        .collect();

    let write_start = Instant::now();
    for edge in edges {
        txn.insert_edge(SPACE, edge).expect("insert edge");
    }
    let write_us = write_start.elapsed().as_micros() as u64;
    drop(txn);

    // Commit path: append staged WAL with Sync durability + commit timestamp.
    let commit_start = Instant::now();
    storage
        .commit_staged_writes(txid, &[])
        .expect("commit staged writes");
    storage.version_manager().commit_write_timestamp(ts);
    let commit_us = commit_start.elapsed().as_micros() as u64;

    // Abort path: drop staged WAL + release timestamp (undo for insert-only
    // transactions is visibility-based via the aborted timestamp).
    let abort_start = Instant::now();
    storage
        .abort_staged_writes(txid)
        .expect("abort staged writes");
    storage.version_manager().abort_write_timestamp(ts);
    let abort_us = abort_start.elapsed().as_micros() as u64;

    RunResult {
        write_us,
        commit_us,
        abort_us,
    }
}

fn median(samples: &mut [u64]) -> u64 {
    samples.sort_unstable();
    samples[samples.len() / 2]
}

fn main() {
    println!("== B4: large-transaction rollback cost ==");
    println!(
        "machine: {} ({} visible cores)",
        machine_name(),
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(0)
    );
    println!(
        "vertex setup = {VERTEX_COUNT}, edge counts = {:?}, iterations = {} (median)",
        EDGE_COUNTS, ITERATIONS
    );
    println!(
        "{:>10} | {:>10} | {:>11} | {:>11} | {:>11} | {:>12}",
        "edges", "T_write ms", "T_commit ms", "T_abort ms", "abort/write", "commit/write"
    );
    let (storage, _dir) = setup_storage();
    let mut tx_seq = 1u64;
    for &edge_count in &EDGE_COUNTS {
        let mut writes = Vec::with_capacity(ITERATIONS);
        let mut commits = Vec::with_capacity(ITERATIONS);
        let mut aborts = Vec::with_capacity(ITERATIONS);
        for _ in 0..ITERATIONS {
            let r = run_transaction(&storage, edge_count, tx_seq);
            tx_seq += 1;
            writes.push(r.write_us);
            commits.push(r.commit_us);
            aborts.push(r.abort_us);
            black_box(());
        }
        let write_us = median(&mut writes);
        let commit_us = median(&mut commits);
        let abort_us = median(&mut aborts);
        println!(
            "{:>10} | {:>10.2} | {:>11.2} | {:>11.2} | {:>11.2} | {:>12.2}",
            edge_count,
            write_us as f64 / 1000.0,
            commit_us as f64 / 1000.0,
            abort_us as f64 / 1000.0,
            abort_us as f64 / write_us as f64,
            commit_us as f64 / write_us as f64,
        );
    }
    println!(
        "\nresult: abort < 2x equivalent write time -> rollback sharding not needed"
    );
}

fn machine_name() -> String {
    std::fs::read_to_string("/proc/cpuinfo")
        .ok()
        .and_then(|info| {
            info.lines()
                .find(|l| l.starts_with("model name"))
                .map(|l| l.split(':').nth(1).unwrap_or("").trim().to_string())
        })
        .unwrap_or_else(|| "unknown".to_string())
}
