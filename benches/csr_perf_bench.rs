//! CSR performance bench: insert throughput, scan bandwidth, memory,
//! checkpoint/load cost, delete throughput.
//!
//! Plain-main bench (harness = false). Run with:
//!   cargo bench --bench csr_perf_bench
//!
//! Covers uniform and power-law degree distributions, with and without
//! overflow and tombstones, so each optimization tier can be compared on
//! the same machine and dataset.

use std::hint::black_box;
use std::time::Instant;

use graphdb::core::types::{EdgeId, VertexId};
use graphdb::storage::edge::mutable_csr::MutableCsr;

const VERTICES: u32 = 4096;

fn build_uniform(csr: &mut MutableCsr, degree: u32) {
    let mut edge_id = 0u64;
    for src in 0..VERTICES {
        for k in 0..degree {
            let dst = VertexId::edge_endpoint_key((src ^ (k * 7919)) % VERTICES, k as i64 % 3);
            csr.insert_edge(src, dst, EdgeId(edge_id), 1)
                .expect("insert");
            edge_id += 1;
        }
    }
    black_box(edge_id);
}

fn build_power_law(csr: &mut MutableCsr) {
    let mut edge_id = 0u64;
    for src in 0..VERTICES {
        let degree = if src % 128 == 0 {
            512
        } else if src % 8 == 0 {
            32
        } else {
            5
        };
        for k in 0..degree {
            let dst = VertexId::edge_endpoint_key((src + k * 31) % VERTICES, 0);
            csr.insert_edge(src, dst, EdgeId(edge_id), 1)
                .expect("insert");
            edge_id += 1;
        }
    }
    black_box(edge_id);
}

fn bench_insert_uniform() -> f64 {
    let mut csr = MutableCsr::with_capacity(VERTICES as usize, 65536);
    let start = Instant::now();
    build_uniform(&mut csr, 16);
    let secs = start.elapsed().as_secs_f64();
    let edges = csr.edge_count() as f64;
    println!(
        "insert uniform d=16 : {:>10.0} edges/s ({:.3}s, {} edges)",
        edges / secs,
        secs,
        edges as u64
    );
    edges / secs
}

fn bench_insert_power_law() -> f64 {
    let mut csr = MutableCsr::with_capacity(VERTICES as usize, 65536);
    let start = Instant::now();
    build_power_law(&mut csr);
    let secs = start.elapsed().as_secs_f64();
    let edges = csr.edge_count() as f64;
    println!(
        "insert power-law    : {:>10.0} edges/s ({:.3}s, {} edges)",
        edges / secs,
        secs,
        edges as u64
    );
    edges / secs
}

fn bench_scan(csr: &MutableCsr, label: &str) {
    let mut visited = 0usize;
    let start = Instant::now();
    for src in 0..VERTICES {
        csr.visit_physical(src, |_| {
            visited += 1;
            true
        });
    }
    let secs = start.elapsed().as_secs_f64();
    let bytes = visited * std::mem::size_of::<graphdb::storage::edge::Nbr>();
    println!(
        "scan {:<14}: {:>7.3} GB/s ({} edges, {:.3}s)",
        label,
        bytes as f64 / secs / 1e9,
        visited,
        secs
    );
    black_box(visited);
}

fn bench_checkpoint_load(csr: &MutableCsr) {
    let start = Instant::now();
    let bytes = csr.dump();
    let dump_secs = start.elapsed().as_secs_f64();
    println!(
        "checkpoint dump     : {:>8.3}s, {:>10} bytes ({} edges)",
        dump_secs,
        bytes.len(),
        csr.edge_count()
    );
    let mut loaded = MutableCsr::new();
    let start = Instant::now();
    loaded.load(&bytes).expect("load");
    let load_secs = start.elapsed().as_secs_f64();
    println!(
        "load                : {:>8.3}s, {} edges",
        load_secs,
        loaded.edge_count()
    );
    assert_eq!(loaded.edge_count(), csr.edge_count());
    black_box(loaded);
}

fn bench_memory(csr: &MutableCsr) {
    let total = csr.used_memory_size();
    let per_edge = total as f64 / csr.edge_count().max(1) as f64;
    println!(
        "memory              : {:>10} bytes total, {:>7.1} bytes/edge",
        total, per_edge
    );
}

fn bench_deletes(csr: &mut MutableCsr) {
    let ids: Vec<EdgeId> = (0..1000u64).map(EdgeId).collect();
    let start = Instant::now();
    let mut deleted = 0usize;
    for id in &ids {
        if csr.delete_edge(0, *id, 2).unwrap_or(false) {
            deleted += 1;
        }
    }
    let secs = start.elapsed().as_secs_f64();
    println!(
        "delete by edge_id   : {:>10.0} ops/s ({} deleted, {:.3}s)",
        ids.len() as f64 / secs,
        deleted,
        secs
    );
    let start = Instant::now();
    let mut reverted = 0usize;
    for id in &ids {
        if csr.revert_delete_by_edge_id(0, *id, 2) {
            reverted += 1;
        }
    }
    let secs = start.elapsed().as_secs_f64();
    println!(
        "revert by edge_id   : {:>10.0} ops/s ({} reverted, {:.3}s)",
        ids.len() as f64 / secs,
        reverted,
        secs
    );
    black_box((deleted, reverted));
}

fn bench_point_lookup_narrow(csr: &MutableCsr) {
    let start = Instant::now();
    let mut hits = 0usize;
    for src in 0..VERTICES {
        let dst = VertexId::edge_endpoint_key((src ^ (0 * 7919)) % VERTICES, 0);
        if csr.get_edge(src, dst, 1).is_some() {
            hits += 1;
        }
    }
    let secs = start.elapsed().as_secs_f64();
    println!(
        "point lookup narrow : {:>10.0} ops/s ({} hits, {:.3}s)",
        VERTICES as f64 / secs,
        hits,
        secs
    );
    black_box(hits);
}

fn bench_point_lookup_wide(csr: &MutableCsr) {
    let hub = 0u32;
    let keys: Vec<VertexId> = (0..512u32)
        .map(|k| VertexId::edge_endpoint_key((hub + k * 31) % VERTICES, 0))
        .collect();
    let start = Instant::now();
    let mut hits = 0usize;
    for dst in &keys {
        if csr.get_edge(hub, *dst, 1).is_some() {
            hits += 1;
        }
    }
    let secs = start.elapsed().as_secs_f64();
    println!(
        "point lookup wide  : {:>10.0} ops/s ({} hits, {:.3}s)",
        keys.len() as f64 / secs,
        hits,
        secs
    );
    black_box(hits);
}

fn bench_full_iter(csr: &MutableCsr) {
    let start = Instant::now();
    let count = csr.iter(1).count();
    let secs = start.elapsed().as_secs_f64();
    println!(
        "full iter           : {:>10.0} edges/s ({} edges, {:.3}s)",
        count as f64 / secs,
        count,
        secs
    );
    black_box(count);
}

fn main() {
    println!("machine: {}", machine_name());
    bench_insert_uniform();
    bench_insert_power_law();

    let mut csr = MutableCsr::with_capacity(VERTICES as usize, 65536);
    build_uniform(&mut csr, 4);
    bench_scan(&csr, "no-overflow");
    bench_point_lookup_narrow(&csr);
    bench_full_iter(&csr);
    bench_memory(&csr);

    let mut wide = MutableCsr::with_capacity(VERTICES as usize, 65536);
    build_power_law(&mut wide);
    bench_scan(&wide, "with-overflow");
    bench_point_lookup_wide(&wide);
    bench_full_iter(&wide);

    for src in 0..64u32 {
        let _ = wide.delete_edge_by_dst(
            src,
            VertexId::edge_endpoint_key((src + 31) % VERTICES, 0),
            2,
        );
    }
    bench_scan(&wide, "with-tombstones");
    bench_checkpoint_load(&wide);
    bench_deletes(&mut wide);
    println!("done");
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
