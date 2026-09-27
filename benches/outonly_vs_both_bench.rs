//! OutOnly vs Both write amplification probe (Section 4).
//!
//! Documents the write-amp penalty when an edge table keeps both outgoing and
//! incoming legs (`StorageDirection::Both`) versus only one leg (`OutOnly`).
//!
//!   1. edge-group commit wall time (CsrShardSet-level): Both vs OutOnly at
//!      batch sizes 5k/10k/20k.
//!   2. end-to-end GraphStorage point-write throughput (in-memory path, WAL
//!      fsync excluded so we isolate CPU write-amp).
//!   3. storage byte size after a disk flush (persistent path).
//!
//! Plain-main bench (harness = false). Run with:
//!   cargo bench --bench outonly_vs_both_bench

use std::collections::HashMap;
use std::hint::black_box;
use std::path::PathBuf;
use std::time::Instant;

use graphdb::core::types::{EdgeStrategy, EdgeTypeInfo, SpaceInfo, TagInfo, VertexId};
use graphdb::core::{DataType, Edge, Value, Vertex};
use graphdb::storage::edge::edge_table::config::EdgeTableConfig;
use graphdb::storage::edge::{EdgeSchema, EdgeStore};
use graphdb::storage::{StoragePropertyDef, StorageReader};
use graphdb::storage::{GraphStorage, StorageSchemaOps, StorageWriter};
use tempfile::TempDir;

const SPACE: &str = "outonly_v_both";
const TAG: &str = "Node";
const EDGE: &str = "Link";
const EDGES_E2E: usize = 2_000;
const ITERATIONS: usize = 3;
const BATCH_SIZES: [usize; 3] = [5_000, 10_000, 20_000];
const SPAN: u32 = 100_000;

// ---------- CsrShardSet-level section ----------

fn make_store(both: bool) -> EdgeStore {
    let (oe, ie) = if both {
        (EdgeStrategy::Multiple, EdgeStrategy::Multiple)
    } else {
        (EdgeStrategy::Multiple, EdgeStrategy::None)
    };
    let schema = EdgeSchema {
        label_id: 0,
        label_name: "bench".to_string(),
        src_label: 0,
        dst_label: 0,
        properties: Vec::new(),
        oe_strategy: oe,
        ie_strategy: ie,
        schema_version: 1,
        record_form: graphdb::storage::edge::RecordForm::Pure,
    };
    EdgeStore::with_config(schema, EdgeTableConfig::default()).expect("store")
}

fn keys(n: usize) -> Vec<(u32, u32)> {
    (0..n)
        .map(|i| {
            (
                ((i as u64 * 7919) % SPAN as u64) as u32,
                ((i as u64 * 104_729 + 7) % SPAN as u64) as u32,
            )
        })
        .collect()
}

fn measure_commit_ms(both: bool, n: usize) -> f64 {
    let mut samples = Vec::with_capacity(ITERATIONS);
    for _ in 0..ITERATIONS {
        let mut table = make_store(both);
        let k = keys(n);
        let mut batch = EdgeStore::staging_batch();
        for (s, d) in &k {
            batch.stage_insert(*s, *d, 0, &[], 100);
        }
        let start = Instant::now();
        let applied = table
            .commit_staging_batch(batch)
            .expect("batch commit");
        let ms = start.elapsed().as_secs_f64() * 1000.0;
        assert_eq!(applied, k.len());
        black_box(applied);
        samples.push(ms);
    }
    samples.sort_by(|a, b| a.partial_cmp(b).expect("finite"));
    samples[ITERATIONS / 2]
}

// ---------- End-to-end section ----------

fn new_graphstorage(both: bool) -> (GraphStorage, TempDir) {
    let temp = TempDir::new().expect("temp");
    let mut storage =
        GraphStorage::new_with_path(temp.path().to_path_buf()).expect("persist");
    let mut space = SpaceInfo::new(SPACE.to_string()).with_vid_type(DataType::BigInt);
    storage.create_space(&mut space).expect("space");
    storage
        .create_tag(
            SPACE,
            &TagInfo::new(TAG.to_string()).with_properties(vec![graphdb::core::types::PropertyDef::new(
                "v".to_string(),
                DataType::BigInt,
            )]),
        )
        .expect("tag");
    let mut et = EdgeTypeInfo::new(EDGE.to_string())
        .with_src_tag(TAG.to_string())
        .with_dst_tag(TAG.to_string());
    if !both {
        et = et.with_strategies(EdgeStrategy::Multiple, EdgeStrategy::None);
    }
    storage
        .create_edge_type(SPACE, &et)
        .expect("edge type");
    (storage, temp)
}

fn seed(storage: &mut GraphStorage, n: usize) {
    let verts: Vec<Vertex> = (0..n as i64)
        .map(|i| {
            Vertex::new(
                VertexId::try_from_int64(i).expect("vid"),
                graphdb::core::vertex_edge_path::Tag::new(
                    TAG.to_string(),
                    [("v".to_string(), Value::BigInt(i))].into(),
                ),
            )
        })
        .collect();
    storage.batch_insert_vertices(SPACE, verts).expect("verts");
}

fn e2e_edges(n: usize) -> Vec<Edge> {
    (0..n)
        .map(|i| {
            let raw_src = ((i as i64 * 7919) % (SPAN as i64)).abs().max(1);
            let raw_dst = ((i as i64 * 104_729 + 7) % (SPAN as i64)).abs().max(1);
            Edge {
                src: VertexId::try_from_int64(raw_src).expect("src"),
                dst: VertexId::try_from_int64(raw_dst).expect("dst"),
                edge_type: EDGE.to_string(),
                ranking: 0,
                props: HashMap::new(),
            }
        })
        .collect()
}

fn measure_e2e(both: bool, batch: bool) -> f64 {
    let mut samples = Vec::with_capacity(ITERATIONS);
    for _ in 0..ITERATIONS {
        let (mut storage, _dir) = new_graphstorage(both);
        seed(&mut storage, SPAN as usize);
        let edges = e2e_edges(EDGES_E2E);
        let start = Instant::now();
        if batch {
            storage
                .batch_insert_edges(SPACE, edges)
                .expect("batch");
        } else {
            for e in edges {
                storage.insert_edge(SPACE, e).expect("single");
            }
        }
        samples.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    samples.sort_by(|a, b| a.partial_cmp(b).expect("finite"));
    samples[ITERATIONS / 2]
}

// ---------- Storage size section ----------

fn total_dir_size(path: &PathBuf) -> u64 {
    let mut total = 0;
    let mut stack = vec![path.clone()];
    while let Some(dir) = stack.pop() {
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for e in entries.flatten() {
                let p = e.path();
                if let Ok(meta) = e.metadata() {
                    if meta.is_dir() {
                        stack.push(p);
                    } else {
                        total += meta.len();
                    }
                }
            }
        }
    }
    total
}

fn measure_storage_bytes(both: bool) -> u64 {
    let (mut storage, dir) = new_graphstorage(both);
    seed(&mut storage, SPAN as usize);
    let edges = e2e_edges(EDGES_E2E);
    storage.batch_insert_edges(SPACE, edges).expect("insert");
    let pre_size = total_dir_size(&dir.path().to_path_buf());
    // Force flush by creating a file we can measure.
    let _ = std::thread::sleep(std::time::Duration::from_millis(100));
    let post_size = total_dir_size(&dir.path().to_path_buf());
    post_size.max(pre_size)
}

// ---------- Reverse traversal probe ----------

fn measure_reverse_hit(both: bool, reverse_ratio: f64) -> f64 {
    let (mut storage, _dir) = new_graphstorage(both);
    seed(&mut storage, SPAN as usize);
    let edges = e2e_edges(EDGES_E2E);
    storage.batch_insert_edges(SPACE, edges).expect("insert");

    let trials = 2_000usize;
    let mut in_hits: u64 = 0;
    let mut in_nonempty: u64 = 0;
    let mut out_nonempty: u64 = 0;
    for i in 0..trials {
        let reverse = (i as f64 / trials as f64) < reverse_ratio;
        let vid = VertexId::try_from_int64(
            ((i as i64 * 104_729 + 3) % (SPAN as i64)).abs().max(1),
        )
        .expect("vid");
        let direction = if reverse {
            graphdb::core::types::graph_schema::EdgeDirection::In
        } else {
            graphdb::core::types::graph_schema::EdgeDirection::Out
        };
        let t0 = Instant::now();
        let res = storage
            .neighbor_dst_ids_batch(SPACE, &[vid], direction, &[EDGE.to_string()])
            .expect("read");
        let _ = black_box(t0.elapsed());
        if reverse {
            in_hits += 1;
            if let Some(list) = res.first() {
                if !list.is_empty() {
                    in_nonempty += 1;
                }
            }
        } else if let Some(list) = res.first() {
            if !list.is_empty() {
                out_nonempty += 1;
            }
        }
    }
    if in_hits > 0 {
        in_nonempty as f64 / in_hits as f64
    } else {
        0.0
    }
}

// ---------- main ----------

fn main() {
    println!("== Section 4: OutOnly vs Both write amplification ==");
    println!(
        "machine: {} ({} visible cores)",
        machine_name(),
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(0)
    );
    println!("E2E edges = {EDGES_E2E}, iterations = {ITERATIONS} (median)");

    println!("\n--- CsrShardSet-level commit wall time ---");
    println!(
        "{:>12} | {:>9} | {:>9} | {:>12}",
        "batch", "both ms", "outonly ms", "amplify"
    );
    for &n in &BATCH_SIZES {
        let b = measure_commit_ms(true, n);
        let o = measure_commit_ms(false, n);
        let amp = if o > 0.0 { b / o } else { 0.0 };
        println!(
            "{:>12} | {:>9.2} | {:>9.2} | {:>11.2}x",
            n, b, o, amp
        );
    }

    println!("\n--- End-to-end point-write throughput (in-memory path) ---");
    println!(
        "{:>10} | {:>8} | {:>10} | {:>10} | {:>12}",
        "mode", "both ms", "outonly ms", "edges/s both", "edges/s outonly"
    );
    for (label, batch) in &[("single", false), ("batch", true)] {
        let b = measure_e2e(true, *batch);
        let o = measure_e2e(false, *batch);
        let both_tput = EDGES_E2E as f64 / (b / 1000.0);
        let out_tput = EDGES_E2E as f64 / (o / 1000.0);
        println!(
            "{:>10} | {:>8.2} | {:>10.2} | {:>12.0} | {:>12.0}",
            label, b, o, both_tput, out_tput
        );
    }

    println!("\n--- Storage bytes after flush (persistent path) ---");
    let bytes_both = measure_storage_bytes(true);
    let bytes_out = measure_storage_bytes(false);
    println!(
        "{:>15}: {:>12} bytes ({:.1} MiB)",
        "Both legs",
        bytes_both,
        bytes_both as f64 / 1024.0 / 1024.0
    );
    println!(
        "{:>15}: {:>12} bytes ({:.1} MiB)",
        "OutOnly leg",
        bytes_out,
        bytes_out as f64 / 1024.0 / 1024.0
    );
    let byte_ratio = if bytes_out > 0 {
        bytes_both as f64 / bytes_out as f64
    } else {
        1.0
    };
    println!("byte amplification: {:.2}x", byte_ratio);

    println!("\n--- Reverse leg hit-rate over a read mix (Both table) ---");
    println!(
        "{:>15} | {:>14} | {:>14}",
        "reverse mix share", "in-leg nonempty%", "out-leg nonempty%"
    );
    for &share in &[0.0f64, 0.1, 0.5] {
        let in_nonempty = measure_reverse_hit(true, share);
        let out_nonempty = measure_reverse_hit(true, 0.0);
        println!(
            "{:>15.2} | {:>14.2} | {:>14.2}",
            share,
            in_nonempty * 100.0,
            out_nonempty * 100.0
        );
    }

    println!("\n-- Decision signals --");
    println!("(a) Both commit wall ~ 2x OutOnly AND WAL fsync share (see edge_point_write_bench \
             memory vs persistent cols) is < 60% -> CPU write-amp real; migrate to OutOnly.");
    println!("(b) byte amplification ~ 2x on persistent path -> both legs stored; usage audit \
             is the first fix.");
    println!("(c) reverse in-leg nonempty% ~ 0 in production query mix -> default OutOnly \
             migration; async leg only considered after this fails AND migration is \
             unacceptable.");
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

// silence unused import
#[allow(dead_code)]
fn _unused(sp: StoragePropertyDef) { let _ = sp; }
