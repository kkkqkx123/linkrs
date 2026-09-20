//! CSR performance bench: insert throughput, scan bandwidth, memory,
//! checkpoint/load cost, delete throughput, and baseline probes.
//!
//! Plain-main bench (harness = false). Run with:
//!   cargo bench --bench csr_perf_bench
//!
//! Covers uniform and power-law degree distributions, with and without
//! overflow and tombstones, so each optimization tier can be compared on
//! the same machine and dataset.
//!
//! The baseline probe section reports allocation counts, repack events,
//! tombstone reuse hits, and live-set rebuild counts so later stages can
//! tune thresholds against measured data.

use std::hint::black_box;
use std::time::Instant;

use graphdb::core::types::{EdgeId, Timestamp, VertexId};
use graphdb::core::{DataType, Value};
use graphdb::storage::edge::mutable_csr::MutableCsr;
use graphdb::storage::edge::property_schema::PropertySchema;
use graphdb::storage::edge::CsrWithProperties;

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

fn bench_scan_alloc_vs_fill(csr: &MutableCsr, label: &str) {
    let start = Instant::now();
    let mut visited = 0usize;
    for src in 0..VERTICES {
        visited += csr.physical_edges_of(src).len();
    }
    let alloc_secs = start.elapsed().as_secs_f64();
    let mut buf = Vec::new();
    let start = Instant::now();
    let mut refilled = 0usize;
    for src in 0..VERTICES {
        csr.fill_physical_into(src, &mut buf);
        refilled += buf.len();
    }
    let fill_secs = start.elapsed().as_secs_f64();
    assert_eq!(visited, refilled);
    println!(
        "scan {:<14}: alloc-collect {:>7.1} ns/edge, caller-buffer {:>7.1} ns/edge",
        label,
        alloc_secs * 1e9 / visited.max(1) as f64,
        fill_secs * 1e9 / refilled.max(1) as f64,
    );
    black_box((visited, refilled));
}

fn bench_scan_filtered_alloc_vs_visit(csr: &MutableCsr, label: &str) {
    let start = Instant::now();
    let mut visited = 0usize;
    for src in 0..VERTICES {
        visited += csr.edges_of(src, 1).len();
    }
    let alloc_secs = start.elapsed().as_secs_f64();
    let start = Instant::now();
    let mut counted = 0usize;
    for src in 0..VERTICES {
        csr.visit_physical(src, |nbr| {
            if nbr.is_alive_at(1) {
                counted += 1;
            }
            true
        });
    }
    let visit_secs = start.elapsed().as_secs_f64();
    assert_eq!(visited, counted);
    println!(
        "scan {:<14}: filtered-alloc {:>7.1} ns/edge, visitor {:>7.1} ns/edge",
        label,
        alloc_secs * 1e9 / visited.max(1) as f64,
        visit_secs * 1e9 / counted.max(1) as f64,
    );
    black_box((visited, counted));
}

fn build_batch_groups(degree: u32) -> Vec<(u32, Vec<(u32, i64, EdgeId, Timestamp)>)> {
    let mut edge_id = 0u64;
    let mut groups = Vec::with_capacity(VERTICES as usize);
    for src in 0..VERTICES {
        let mut batch = Vec::with_capacity(degree as usize);
        for k in 0..degree {
            batch.push((k, 0, EdgeId(edge_id), 1));
            edge_id += 1;
        }
        groups.push((src, batch));
    }
    groups
}

fn bench_batch_put_vs_insert() {
    let groups = build_batch_groups(16);
    let total: usize = groups.iter().map(|(_, b)| b.len()).sum();
    let mut csr = MutableCsr::with_capacity(VERTICES as usize, 65536);
    let start = Instant::now();
    let inserted = csr.batch_put_edges(&groups, true).expect("batch put");
    let batch_secs = start.elapsed().as_secs_f64();
    assert_eq!(inserted, total);
    let mut single = MutableCsr::with_capacity(VERTICES as usize, 65536);
    let start = Instant::now();
    for (src, batch) in &groups {
        for (endpoint, rank, edge_id, ts) in batch {
            single
                .insert_edge(
                    *src,
                    VertexId::edge_endpoint_key(*endpoint, *rank),
                    *edge_id,
                    *ts,
                )
                .expect("insert");
        }
    }
    let single_secs = start.elapsed().as_secs_f64();
    assert_eq!(single.edge_count(), csr.edge_count());
    println!(
        "batch put d=16    : batch {:>10.0} edges/s, single {:>10.0} edges/s",
        total as f64 / batch_secs,
        total as f64 / single_secs,
    );
    black_box((inserted, single.edge_count()));
}

fn property_schema_set(cols: usize) -> Vec<PropertySchema> {
    (0..cols)
        .map(|i| PropertySchema::new(format!("c{i}"), i as i32, DataType::Double).nullable(true))
        .collect()
}

fn bench_properties(cols: usize) {
    const EDGES: u64 = 20_000;
    let mut props = CsrWithProperties::new(property_schema_set(cols));
    let values: Vec<(String, Value)> = (0..cols)
        .map(|i| (format!("c{i}"), Value::Double(i as f64)))
        .collect();
    let start = Instant::now();
    for eid in 0..EDGES {
        props
            .insert_for_edge(EdgeId(eid), &values, 1)
            .expect("prop insert");
    }
    let insert_secs = start.elapsed().as_secs_f64();
    let start = Instant::now();
    for eid in 0..EDGES {
        props
            .set_property_for_edge(EdgeId(eid), "c0", Some(Value::Double(1.5)), 2)
            .expect("prop update");
    }
    let update_secs = start.elapsed().as_secs_f64();
    let start = Instant::now();
    let mut hits = 0usize;
    for eid in 0..EDGES {
        if props
            .get_projected_physical_by_edge_id(EdgeId(eid), 2, None)
            .is_some()
        {
            hits += 1;
        }
    }
    let read_secs = start.elapsed().as_secs_f64();
    assert_eq!(hits, EDGES as usize);
    println!(
        "properties {:>2} cols: insert {:>7.1} ns/edge, update {:>7.1} ns/edge, read {:>7.1} ns/edge",
        cols,
        insert_secs * 1e9 / EDGES as f64,
        update_secs * 1e9 / EDGES as f64,
        read_secs * 1e9 / EDGES as f64,
    );
    black_box(hits);
}

// ---------------------------------------------------------------------------
// Baseline probe scenarios
// ---------------------------------------------------------------------------

/// Narrow row building: one edge per vertex, no overflow expected.
fn bench_insert_narrow() {
    let mut csr = MutableCsr::with_capacity(VERTICES as usize, 65536);
    let start = Instant::now();
    let mut edge_id = 0u64;
    for src in 0..VERTICES {
        let dst = VertexId::edge_endpoint_key((src + 1) % VERTICES, 0);
        csr.insert_edge(src, dst, EdgeId(edge_id), 1)
            .expect("insert");
        edge_id += 1;
    }
    let secs = start.elapsed().as_secs_f64();
    println!(
        "insert narrow d=1  : {:>10.0} edges/s ({:.3}s, {} edges)",
        edge_id as f64 / secs,
        secs,
        edge_id
    );
    println!(
        "  primary_block_allocs={}, overflow_chunk_allocs={}, live_set_rebuilds={}",
        csr.primary_block_allocs(),
        csr.overflow_chunk_allocs(),
        csr.live_set_rebuild_count()
    );
    black_box(edge_id);
}

/// Wide row point lookup with guaranteed hit: every key inserted exists.
fn bench_point_lookup_wide_hit(csr: &MutableCsr) {
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
        "point-lookup wide hit  : {:>10.0} ops/s ({} hits, {:.3}s)",
        keys.len() as f64 / secs,
        hits,
        secs
    );
    assert_eq!(hits, keys.len());
    black_box(hits);
}

/// Wide row point lookup with guaranteed miss: keys never inserted.
fn bench_point_lookup_wide_miss(csr: &MutableCsr) {
    let hub = 0u32;
    let keys: Vec<VertexId> = (0..512u32)
        .map(|k| VertexId::edge_endpoint_key((hub + k * 31 + 99999) % VERTICES, 0))
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
        "point-lookup wide miss : {:>10.0} ops/s ({} hits, {:.3}s)",
        keys.len() as f64 / secs,
        hits,
        secs
    );
    assert_eq!(hits, 0);
    black_box(hits);
}

/// Tombstone-dense row insertion: fill a row, delete half, then insert again
/// to exercise tombstone-reuse and overflow spill.
fn bench_tombstone_dense_insert() {
    const HUB: u32 = 0;
    const FILL: u32 = 256;
    let mut csr = MutableCsr::with_capacity(VERTICES as usize, 65536);
    // Fill the hub row
    for k in 0..FILL {
        let dst = VertexId::edge_endpoint_key(k, 0);
        csr.insert_edge(HUB, dst, EdgeId(k as u64), 1)
            .expect("insert");
    }
    // Delete the first half
    for k in 0..FILL / 2 {
        let _ = csr.delete_edge_by_dst(
            HUB,
            VertexId::edge_endpoint_key(k, 0),
            2,
        );
    }
    csr.reset_baseline_counters();
    // Insert new edges into the tombstone-dense row
    let start = Instant::now();
    let mut edge_id = FILL as u64;
    for k in FILL..FILL + 128 {
        let dst = VertexId::edge_endpoint_key(k + 10000, 0);
        csr.insert_edge(HUB, dst, EdgeId(edge_id), 3)
            .expect("insert");
        edge_id += 1;
    }
    let secs = start.elapsed().as_secs_f64();
    println!(
        "tombstone-dense insert: {:>10.0} ops/s ({} new edges, {:.3}s)",
        128.0 / secs,
        128,
        secs
    );
    println!(
        "  tombstone_reuse={}, overflow_allocs={}, repack={}",
        csr.tombstone_reuse_count(),
        csr.overflow_chunk_allocs(),
        csr.repack_count()
    );
    black_box(edge_id);
}

/// Overflow multi-block traversal: build a row that spans multiple overflow
/// chunks, then scan it.
fn bench_overflow_multi_block_traversal() {
    const HUB: u32 = 0;
    let mut csr = MutableCsr::with_capacity(VERTICES as usize, 65536);
    // Insert enough edges to force multi-block overflow.
    // Default chunk size is 4096, so we need >4096 edges in one row.
    let count = 8192u64;
    for k in 0..count {
        let dst = VertexId::edge_endpoint_key(k as u32, 0);
        csr.insert_edge(HUB, dst, EdgeId(k), 1)
            .expect("insert");
    }
    let chunks = csr
        .get_overflow_chunks(HUB)
        .map_or(0, |c| c.len());
    // Scan the wide row
    let start = Instant::now();
    let mut visited = 0usize;
    csr.visit_physical(HUB, |_| {
        visited += 1;
        true
    });
    let secs = start.elapsed().as_secs_f64();
    let bytes = visited * std::mem::size_of::<graphdb::storage::edge::Nbr>();
    println!(
        "overflow multi-block : {:>7.3} GB/s ({} edges, {} chunks, {:.3}s)",
        bytes as f64 / secs / 1e9,
        visited,
        chunks,
        secs
    );
    assert_eq!(visited as u64, count);
    black_box(visited);
}

/// Batch reserve vs full rebuild: compare batch_put with reserve_for_batch
/// against a full compact_with_ts_reporting rebuild on the same dataset.
fn bench_reserve_vs_rebuild() {
    let groups = build_batch_groups(32);
    let total: usize = groups.iter().map(|(_, b)| b.len()).sum();

    // Batch put (uses reserve_for_batch internally)
    let mut csr = MutableCsr::with_capacity(VERTICES as usize, 65536);
    let start = Instant::now();
    let _ = csr.batch_put_edges(&groups, false).expect("batch put");
    let batch_secs = start.elapsed().as_secs_f64();

    // Full rebuild via compaction (no tombstones to reclaim, pure rebuild)
    csr.reset_baseline_counters();
    let start = Instant::now();
    let removed = csr.compact_with_ts_reporting(Timestamp::MAX, 0.2, &mut |_, _| {});
    let rebuild_secs = start.elapsed().as_secs_f64();
    assert_eq!(removed, 0);
    assert_eq!(csr.edge_count() as usize, total);
    println!(
        "reserve vs rebuild  : batch {:>8.3}s, rebuild {:>8.3}s ({} edges)",
        batch_secs, rebuild_secs, total
    );
    println!(
        "  after rebuild: overflow_allocs={}, primary_allocs={}, live_set_rebuilds={}",
        csr.overflow_chunk_allocs(),
        csr.primary_block_allocs(),
        csr.live_set_rebuild_count()
    );
    black_box((batch_secs, rebuild_secs));
}

/// Routing hit vs fallback: on a wide row with a live set, point lookups
/// hit the index; on a narrow row without a set, they fall back to scan.
fn bench_routing_hit_fallback() {
    // Wide row: insert enough to trigger live set creation
    let mut wide_csr = MutableCsr::with_capacity(VERTICES as usize, 65536);
    for k in 0..64u32 {
        let dst = VertexId::edge_endpoint_key(k, 0);
        wide_csr
            .insert_edge(0, dst, EdgeId(k as u64), 1)
            .expect("insert");
    }
    // Verify live set exists for the wide row
    let has_set = wide_csr.has_live_set(&0);
    assert!(has_set, "wide row should have a live set");

    // Lookup existing keys (hits the live set)
    let keys: Vec<VertexId> = (0..64u32)
        .map(|k| VertexId::edge_endpoint_key(k, 0))
        .collect();
    let start = Instant::now();
    let mut hits = 0usize;
    for _ in 0..1000 {
        for dst in &keys {
            if wide_csr.get_edge(0, *dst, 1).is_some() {
                hits += 1;
            }
        }
    }
    let hit_secs = start.elapsed().as_secs_f64();

    // Narrow row: single edge, no live set
    let mut narrow_csr = MutableCsr::with_capacity(VERTICES as usize, 65536);
    narrow_csr
        .insert_edge(0, VertexId::edge_endpoint_key(0, 0), EdgeId(0), 1)
        .expect("insert");
    assert!(!narrow_csr.has_live_set(&0));

    let start = Instant::now();
    let mut fallback_hits = 0usize;
    for _ in 0..1000 {
        if narrow_csr
            .get_edge(0, VertexId::edge_endpoint_key(0, 0), 1)
            .is_some()
        {
            fallback_hits += 1;
        }
    }
    let fallback_secs = start.elapsed().as_secs_f64();

    println!(
        "routing: wide indexed {:>7.1} ns/lookup, narrow scan {:>7.1} ns/lookup",
        hit_secs * 1e9 / hits.max(1) as f64,
        fallback_secs * 1e9 / fallback_hits.max(1) as f64,
    );
    println!(
        "  wide {} lookups in {:.3}s, narrow {} lookups in {:.3}s",
        hits, hit_secs, fallback_hits, fallback_secs
    );
    black_box((hits, fallback_hits));
}

/// Freeze reclaim: tombstone-heavy rows compacted to reclaim space.
fn bench_freeze_reclaim() {
    let mut csr = MutableCsr::with_capacity(VERTICES as usize, 65536);
    build_uniform(&mut csr, 16);
    let before_edges = csr.edge_count();
    // Delete 80% of edges on half the vertices
    for src in 0..VERTICES / 2 {
        for k in 0..13u32 {
            let _ = csr.delete_edge_by_dst(
                src,
                VertexId::edge_endpoint_key((src ^ (k * 7919)) % VERTICES, k as i64 % 3),
                2,
            );
        }
    }
    let before_compact = csr.edge_count();
    let before_mem = csr.used_memory_size();

    // Compact to reclaim
    let start = Instant::now();
    let removed = csr.compact_with_ts_reporting(Timestamp::MAX, 0.2, &mut |_, _| {});
    let secs = start.elapsed().as_secs_f64();
    let after_mem = csr.used_memory_size();

    println!(
        "freeze reclaim      : removed {} edges in {:.3}s (before={} after={}, mem {}->{} bytes)",
        removed, secs, before_compact, csr.edge_count(), before_mem, after_mem
    );
    println!(
        "  after compact: overflow_allocs={}, primary_allocs={}, live_set_rebuilds={}",
        csr.overflow_chunk_allocs(),
        csr.primary_block_allocs(),
        csr.live_set_rebuild_count()
    );
    black_box((removed, before_edges));
}

fn main() {
    println!("machine: {}", machine_name());
    println!("--- insert throughput ---");
    bench_insert_uniform();
    bench_insert_power_law();
    bench_batch_put_vs_insert();

    println!("\n--- narrow row (no overflow) ---");
    let mut csr = MutableCsr::with_capacity(VERTICES as usize, 65536);
    build_uniform(&mut csr, 4);
    bench_scan(&csr, "no-overflow");
    bench_scan_alloc_vs_fill(&csr, "no-overflow");
    bench_scan_filtered_alloc_vs_visit(&csr, "no-overflow");
    bench_point_lookup_narrow(&csr);
    bench_full_iter(&csr);
    bench_memory(&csr);

    println!("\n--- wide row (with overflow) ---");
    let mut wide = MutableCsr::with_capacity(VERTICES as usize, 65536);
    build_power_law(&mut wide);
    bench_scan(&wide, "with-overflow");
    bench_scan_alloc_vs_fill(&wide, "with-overflow");
    bench_scan_filtered_alloc_vs_visit(&wide, "with-overflow");
    bench_point_lookup_wide(&wide);
    bench_full_iter(&wide);

    println!("\n--- baseline probes ---");
    bench_insert_narrow();
    bench_point_lookup_wide_hit(&wide);
    bench_point_lookup_wide_miss(&wide);
    bench_tombstone_dense_insert();
    bench_overflow_multi_block_traversal();
    bench_reserve_vs_rebuild();
    bench_routing_hit_fallback();
    bench_freeze_reclaim();

    println!("\n--- tombstones & persistence ---");
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

    println!("\n--- properties ---");
    bench_properties(2);
    bench_properties(8);

    println!("\n--- final counters ---");
    bench_counters_report(&wide);
    println!("done");
}

fn bench_counters_report(csr: &MutableCsr) {
    println!(
        "counters: overflow_chunk_allocs={}, primary_block_allocs={}, repack={}, tombstone_reuse={}, live_set_rebuilds={}",
        csr.overflow_chunk_allocs(),
        csr.primary_block_allocs(),
        csr.repack_count(),
        csr.tombstone_reuse_count(),
        csr.live_set_rebuild_count()
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
