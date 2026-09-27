//! Bundled vs Columnar for auto-Bundled decision (Section 5).
//!
//! Bundled is an inline record form that stores exactly one scalar edge
//! property inside the CSR row (20 bytes/edge vs 12 Pure, default
//! Columnar), saving an extra column vector and property-key indirection.
//! It has hard limits: exactly one scalar property, no Single-edge direction,
//! no rank, no MVCC version chain, no online schema change.
//! `RecordFormPreference::Auto` currently picks Pure/Columnar; Bundled is
//! opt-in. This bench probes whether automatic Bundled selection is worth
//! expanding by answering three questions on a schema that is eligible:
//!
//!   1. point-lookup latency on out-neighbor reads.
//!   2. full-scan throughput over repeated fan-out reads.
//!   3. resident footprint estimate (edge_count * edge size).
//!
//! Plain-main bench (harness = false). Run with:
//!   cargo bench --bench bundled_necessity_bench

use std::hint::black_box;
use std::path::PathBuf;
use std::time::Instant;

use graphdb::core::types::{EdgeStrategy, EdgeTypeInfo, PropertyDef, SpaceInfo, TagInfo, VertexId};
use graphdb::core::vertex_edge_path::Tag;
use graphdb::core::{DataType, Edge, Value, Vertex};
use graphdb::storage::edge::edge_table::config::EdgeTableConfig;
use graphdb::storage::edge::RecordFormPreference;
use graphdb::storage::edge::{EdgeSchema, EdgeStore};
use graphdb::storage::{GraphStorage, StorageSchemaOps, StorageWriter};
use tempfile::TempDir;

const SPACE: &str = "bundled_v_col";
const TAG: &str = "Node";
const EDGE: &str = "Link";
const PROP_NAME: &str = "weight";
const EDGE_COUNT: usize = 50_000;
const VERTEX_COUNT: i64 = 50_000;
const ITERATIONS: usize = 3;

fn bundled_eligible_props() -> Vec<graphdb::storage::StoragePropertyDef> {
    vec![graphdb::storage::StoragePropertyDef::new(
        PROP_NAME.to_string(),
        DataType::BigInt,
    )]
}

fn make_edge_store(preference: RecordFormPreference) -> EdgeStore {
    let schema = EdgeSchema {
        label_id: 0,
        label_name: "bench".to_string(),
        src_label: 0,
        dst_label: 0,
        properties: bundled_eligible_props(),
        oe_strategy: EdgeStrategy::Multiple,
        ie_strategy: EdgeStrategy::Multiple,
        schema_version: 1,
        record_form: graphdb::storage::edge::RecordForm::Columnar,
    };
    let cfg = EdgeTableConfig {
        record_form: preference,
        ..EdgeTableConfig::default()
    };
    EdgeStore::with_config(schema, cfg).expect("edge store")
}

fn build_and_materialize(preference: RecordFormPreference) -> EdgeStore {
    let mut store = make_edge_store(preference);
    let mut batch = EdgeStore::staging_batch();
    for i in 0..EDGE_COUNT {
        let src = ((i as u32 * 7919) % VERTEX_COUNT as u32).min(VERTEX_COUNT as u32 - 1);
        let dst = ((i as u32 * 104_729 + 7) % VERTEX_COUNT as u32).min(VERTEX_COUNT as u32 - 1);
        batch.stage_insert(
            src,
            dst,
            0,
            &[(PROP_NAME.to_string(), Value::BigInt((i as i64 * 1315423911i64) & 0x7FFF))],
            100,
        );
    }
    let applied = store
        .commit_staging_batch(batch)
        .expect("commit")
        ;
    assert_eq!(applied, EDGE_COUNT);
    store
}

fn measure_point_lookup_us(preference: RecordFormPreference) -> f64 {
    let mut samples = Vec::with_capacity(ITERATIONS);
    for _ in 0..ITERATIONS {
        let store = build_and_materialize(preference);
        let lookups = 5_000usize;
        let t0 = Instant::now();
        for i in 0..lookups {
            let src = ((i as u32 * 7919) % VERTEX_COUNT as u32).min(VERTEX_COUNT as u32 - 1);
            let res = store.merged_out_nbrs_with_limit(
                src,
                graphdb::core::types::Timestamp::MAX,
                256,
            );
            black_box(res.len());
        }
        samples.push(t0.elapsed().as_secs_f64() * 1_000_000.0 / lookups as f64);
    }
    samples.sort_by(|a, b| a.partial_cmp(b).expect("finite"));
    samples[ITERATIONS / 2]
}

fn measure_scan_throughput(preference: RecordFormPreference) -> f64 {
    let mut samples = Vec::with_capacity(ITERATIONS);
    for _ in 0..ITERATIONS {
        let store = build_and_materialize(preference);
        let lookups = 2_000usize;
        let t0 = Instant::now();
        let mut visited = 0usize;
        for i in 0..lookups {
            let src = ((i as u32 * 7919) % VERTEX_COUNT as u32).min(VERTEX_COUNT as u32 - 1);
            let res = store.merged_out_nbrs_with_limit(
                src,
                graphdb::core::types::Timestamp::MAX,
                4096,
            );
            visited += res.len();
            black_box(res);
        }
        let elapsed = t0.elapsed().as_secs_f64();
        samples.push(visited as f64 / elapsed.max(1e-9));
    }
    samples.sort_by(|a, b| a.partial_cmp(b).expect("finite"));
    samples[ITERATIONS / 2]
}

// ---------- Persistent path: checkpoint bytes + rank trigger probe ----------

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

fn build_persistent(both: bool) -> (GraphStorage, TempDir) {
    let temp = TempDir::new().expect("temp");
    let mut storage =
        GraphStorage::new_with_path(temp.path().to_path_buf()).expect("storage");
    let mut space = SpaceInfo::new(SPACE.to_string()).with_vid_type(DataType::BigInt);
    storage.create_space(&mut space).expect("space");
    storage
        .create_tag(
            SPACE,
            &TagInfo::new(TAG.to_string()).with_properties(vec![PropertyDef::new(
                "v".to_string(),
                DataType::BigInt,
            )]),
        )
        .expect("tag");
    let mut et = EdgeTypeInfo::new(EDGE.to_string())
        .with_src_tag(TAG.to_string())
        .with_dst_tag(TAG.to_string())
        .with_properties(vec![PropertyDef::new(PROP_NAME.to_string(), DataType::BigInt)]);
    if !both {
        et = et.with_strategies(EdgeStrategy::Multiple, EdgeStrategy::None);
    }
    storage.create_edge_type(SPACE, &et).expect("edge type");
    (storage, temp)
}

fn seed_persistent(storage: &mut GraphStorage) {
    let verts: Vec<Vertex> = (0..VERTEX_COUNT)
        .map(|i| {
            Vertex::new(
                VertexId::try_from_int64(i).expect("vid"),
                Tag::new(
                    TAG.to_string(),
                    [("v".to_string(), Value::BigInt(i))].into(),
                ),
            )
        })
        .collect();
    storage.batch_insert_vertices(SPACE, verts).expect("verts");
}

fn seed_edges(storage: &mut GraphStorage, rank_nonzero: bool) {
    let edges: Vec<Edge> = (0..EDGE_COUNT)
        .map(|i| Edge {
            src: VertexId::try_from_int64((i as i64 * 7919) % VERTEX_COUNT).expect("src"),
            dst: VertexId::try_from_int64((i as i64 * 104_729 + 7) % VERTEX_COUNT).expect("dst"),
            edge_type: EDGE.to_string(),
            ranking: if rank_nonzero { 42 } else { 0 },
            props: [(PROP_NAME.to_string(), Value::BigInt(i as i64 & 0x7FFF))]
                .into(),
        })
        .collect();
    storage.batch_insert_edges(SPACE, edges).expect("edges");
}

fn measure_persistent_bytes(both: bool, rank_nonzero: bool) -> (u64, u64) {
    let (mut storage, dir) = build_persistent(both);
    seed_persistent(&mut storage);
    seed_edges(&mut storage, rank_nonzero);
    let _ = std::thread::sleep(std::time::Duration::from_millis(100));
    let bytes = total_dir_size(&dir.path().to_path_buf());
    // rank_trigger = nonzero rank would break Bundled, so we count it as a
    // constraint trigger whenever nonzero ranks are written.
    let rank_triggered = rank_nonzero as u64;
    (bytes, rank_triggered)
}

// ---------- main ----------

fn main() {
    println!("== Section 5: Bundled vs Columnar ==");
    println!(
        "machine: {} ({} visible cores)",
        machine_name(),
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(0)
    );
    println!(
        "edge_count = {EDGE_COUNT}, vertices = {VERTEX_COUNT}, \
         property = single BIGINT '{PROP_NAME}', iterations = {ITERATIONS}"
    );

    println!("\n--- Verification: Auto keeps Columnar, explicit Bundled is accepted ---");
    let auto_store = make_edge_store(RecordFormPreference::Auto);
    let bundled_store = make_edge_store(RecordFormPreference::Bundled);
    println!(
        "Auto => {:?}, Bundled opt-in => {:?}",
        auto_store.schema().record_form,
        bundled_store.schema().record_form
    );

    println!("\n--- Point-lookup latency (median us per lookup) ---");
    let col_us = measure_point_lookup_us(RecordFormPreference::Columnar);
    let bun_us = measure_point_lookup_us(RecordFormPreference::Bundled);
    println!("Columnar : {:.2} us / lookup", col_us);
    println!("Bundled  : {:.2} us / lookup", bun_us);
    println!(
        "speedup  : {:.2}x",
        if col_us > 0.0 { col_us / bun_us } else { 0.0 }
    );

    println!("\n--- Full-scan throughput (edges/sec) ---");
    let col_tput = measure_scan_throughput(RecordFormPreference::Columnar);
    let bun_tput = measure_scan_throughput(RecordFormPreference::Bundled);
    println!("Columnar : {:.0} edges/s", col_tput);
    println!("Bundled  : {:.0} edges/s", bun_tput);
    println!(
        "speedup  : {:.2}x",
        if col_tput > 0.0 { bun_tput / col_tput } else { 0.0 }
    );

    println!("\n--- Edge count on materialized stores (verify parity) ---");
    let col_store = build_and_materialize(RecordFormPreference::Columnar);
    let bun_store = build_and_materialize(RecordFormPreference::Bundled);
    println!(
        "Columnar edge_count: {}, Bundled edge_count: {}",
        col_store.edge_count(),
        bun_store.edge_count()
    );

    println!("\n--- Persistent path: checkpoint bytes + rank-trigger probe ---");
    let (bytes_rank0, trigger0) = measure_persistent_bytes(true, false);
    let (bytes_rank42, trigger42) = measure_persistent_bytes(true, true);
    println!(
        "Both dir + rank=0  : {:>12} bytes ({:.1} MiB), rank_trigger={}",
        bytes_rank0,
        bytes_rank0 as f64 / 1024.0 / 1024.0,
        trigger0
    );
    println!(
        "Both dir + rank=42 : {:>12} bytes ({:.1} MiB), rank_trigger={}",
        bytes_rank42,
        bytes_rank42 as f64 / 1024.0 / 1024.0,
        trigger42
    );

    println!("\n--- Resident foot-print estimate (edge_count * assumed edge bytes) ---");
    // Pure topology CSR = 12B/edge (no prop). Bundled = 20B/edge (inline scalar).
    // Columnar = 12B/edge + 8B * N edges + indirection overhead; ~24B/edge floor.
    let col_est = col_store.edge_count() * 24;
    let bun_est = bun_store.edge_count() * 20;
    println!(
        "Columnar est resident: {:>12} bytes ({:.1} MiB)",
        col_est,
        col_est as f64 / 1024.0 / 1024.0
    );
    println!(
        "Bundled  est resident: {:>12} bytes ({:.1} MiB)",
        bun_est,
        bun_est as f64 / 1024.0 / 1024.0
    );
    if bun_est > 0 {
        println!("ratio: {:.2}x", col_est as f64 / bun_est as f64);
    }

    println!("\n-- Decision signals --");
    println!("(a) Bundled checkpoint bytes >= 2x smaller than Columnar AND resident \
             estimate follows -> space win real.");
    println!("(b) Point-lookup or full-scan >= 1.3x faster under Bundled -> hot-path \
             also benefits.");
    println!("(c) nonzero rank triggers Bundled incompatibility on this table -> auto-select \
             would force migration every rank write; keep explicit opt-in until rank-free \
             proven in production.");
    println!("(d) constraint trigger rate (rank_nonzero, version-chain depth, schema alter) \
             stays 0 over the production profile window -> only then consider adding \
             Bundled to Auto.");
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
fn _unused(_r: RecordFormPreference) {}
