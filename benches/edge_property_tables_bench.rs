//! Edge property tables: benchmark-first decision suite.
//!
//! One plain-main bench (harness = false) covering the seven open decisions
//! about edge property tables. Small data on purpose: only the contrast
//! matters, not absolute throughput.
//!
//! Convention: 11 iterations, median reported, machine + core count printed.
//! Run with:
//!   cargo bench --bench edge_property_tables_bench

use std::collections::HashMap;
use std::hint::black_box;
use std::time::Instant;

use tempfile::TempDir;

use linkrs::core::types::{EdgeId, EdgeTypeInfo, PropertyDef, SpaceInfo, TagInfo, VertexId};
use linkrs::core::vertex_edge_path::Tag;
use linkrs::core::{DataType, Edge, Value, Vertex};
use linkrs::storage::edge::edge_table::config::EdgeTableConfig;
use linkrs::storage::edge::{EdgeSchema, EdgeStore, MutableCsr, Nbr};
use linkrs::storage::{
    GraphStorage, StoragePersistenceOps, StorageReader, StorageSchemaOps, StorageWriter,
};

const ITERATIONS: usize = 11;

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

fn median_u64(samples: &mut [u64]) -> u64 {
    samples.sort_unstable();
    samples[samples.len() / 2]
}

fn median_f64(samples: &mut [f64]) -> f64 {
    samples.sort_by(|a, b| a.partial_cmp(b).expect("finite"));
    samples[samples.len() / 2]
}

fn dir_bytes(dir: &std::path::Path) -> u64 {
    let mut total = 0u64;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(path) = stack.pop() {
        let entries = match std::fs::read_dir(&path) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if let Ok(meta) = entry.metadata() {
                total += meta.len();
            }
        }
    }
    total
}

// ── B1: primary_sorted persistence ──────────────────────────────
// Out-of-order row: sorted-flag fast path (bisect prefix) vs cold-load
// behavior (flag reset to false on load -> linear scan). Persist cost is
// one byte per vertex vs the CSR dump size.

const B1_EDGES: u32 = 20_000;
/// Inner repeats per timing sample: one 20k-row scan is ~µs, below the
/// timer granularity needed for a stable median.
const B1_REPEAT: usize = 20;

fn build_csr_descending() -> MutableCsr {
    let mut csr = MutableCsr::with_capacity(8, B1_EDGES as usize * 2);
    for endpoint in (0..B1_EDGES).rev() {
        csr.insert_edge(
            0,
            VertexId::edge_endpoint_key(endpoint, 0),
            EdgeId(endpoint as u64),
            100,
        )
        .expect("insert");
    }
    csr
}

fn bench_b1() {
    println!("\n## B1: primary_sorted persistence (cold-load penalty vs 1B/vtx cost)");
    let csr = build_csr_descending();
    println!(
        "row edges = {B1_EDGES}, is_primary_sorted(descending inserts) = {}",
        csr.is_primary_sorted(0)
    );

    let lo = Some((B1_EDGES / 4, 0));
    let hi = Some((B1_EDGES / 4 + B1_EDGES / 100, 0));
    let mut buf: Vec<Nbr> = Vec::new();
    csr.fill_threshold_into(0, lo, hi, &mut buf);
    let expect = buf.len();

    // Cold load: dump/load resets the flag to false (current behavior).
    let dump = csr.dump();
    let mut cold = MutableCsr::new();
    cold.load(&dump).expect("reload");
    println!(
        "after dump/load: is_primary_sorted = {} (reset=false is current behavior)",
        cold.is_primary_sorted(0)
    );
    let mut buf2: Vec<Nbr> = Vec::new();
    cold.fill_threshold_into(0, lo, hi, &mut buf2);
    assert_eq!(buf2.len(), expect, "cold path must return the same rows");

    // Warm: same bytes but flag true (what persisting the flag would give).
    // Rebuild sorted order by ascending inserts for a fair bisect sample.
    let mut warm = MutableCsr::with_capacity(8, B1_EDGES as usize * 2);
    for endpoint in 0..B1_EDGES {
        warm.insert_edge(
            0,
            VertexId::edge_endpoint_key(endpoint, 0),
            EdgeId(endpoint as u64),
            100,
        )
        .expect("insert");
    }
    assert!(warm.is_primary_sorted(0));
    let mut buf3: Vec<Nbr> = Vec::new();
    warm.fill_threshold_into(0, lo, hi, &mut buf3);
    assert_eq!(buf3.len(), expect);

    let mut cold_us = Vec::with_capacity(ITERATIONS);
    for _ in 0..ITERATIONS {
        let start = Instant::now();
        for _ in 0..B1_REPEAT {
            cold.fill_threshold_into(0, lo, hi, &mut buf2);
            black_box(buf2.len());
        }
        cold_us.push(start.elapsed().as_micros() as u64);
    }
    let mut warm_us = Vec::with_capacity(ITERATIONS);
    for _ in 0..ITERATIONS {
        let start = Instant::now();
        for _ in 0..B1_REPEAT {
            warm.fill_threshold_into(0, lo, hi, &mut buf3);
            black_box(buf3.len());
        }
        warm_us.push(start.elapsed().as_micros() as u64);
    }
    let cold_med = median_u64(&mut cold_us);
    let warm_med = median_u64(&mut warm_us);
    let penalty = if warm_med > 0 {
        (cold_med as f64 - warm_med as f64) / warm_med as f64 * 100.0
    } else {
        0.0
    };
    let flag_bytes = cold.vertex_capacity() as f64;
    let share = flag_bytes / dump.len() as f64 * 100.0;
    println!("threshold window rows = {expect} (1% of row)");
    println!("cold (flag=false, linear) median = {cold_med} us ({B1_REPEAT}x repeats)");
    println!("warm (flag=true, bisect) median = {warm_med} us ({B1_REPEAT}x repeats)");
    println!("cold penalty = {penalty:.1}% (threshold: >5% -> persist)");
    println!(
        "persist cost = {flag_bytes:.0}B flag vs {}B dump = {share:.3}% (threshold: <1% of checkpoint -> persist)",
        dump.len()
    );
}

// ── B2: tombstone-reuse hint persistence ────────────────────────
// Delete half a row, then re-insert with the reuse cutoff refreshed
// (warm) vs cleared to the disabled sentinel (cold restart). Reports
// re-insert time, reuse count and hint hit rate.

const B2_ROWS: u32 = 2000;
const B2_FULL: u32 = 4;

fn bench_b2() {
    println!("\n## B2: tombstone-reuse hint cold start (insert p99 proxy + reuse rate)");
    // Each row is filled exactly to its 4-slot primary block, then half
    // deleted: the row stays full (degree == capacity), so the next insert
    // must take the tombstone-reuse path when the cutoff allows it.
    let build = || {
        let mut csr = MutableCsr::with_capacity(B2_ROWS as usize + 8, 16_384);
        for src in 0..B2_ROWS {
            for k in 0..B2_FULL {
                csr.insert_edge(
                    src,
                    VertexId::edge_endpoint_key(k, 0),
                    EdgeId((src as u64) * 16 + k as u64),
                    100,
                )
                .expect("insert");
            }
        }
        for src in 0..B2_ROWS {
            for k in (0..B2_FULL).step_by(2) {
                csr.delete_edge_by_dst(src, VertexId::edge_endpoint_key(k, 0), 100);
            }
        }
        csr
    };

    let mut warm_ms = Vec::with_capacity(ITERATIONS);
    let mut warm_reuse = 0u64;
    let mut warm_rate = 0.0f32;
    let mut warm_overflow = 0u64;
    for _ in 0..ITERATIONS {
        let mut csr = build();
        csr.refresh_tombstone_reuse_cutoff(150);
        let start = Instant::now();
        for src in 0..B2_ROWS {
            for k in (0..B2_FULL).step_by(2) {
                csr.insert_edge(
                    src,
                    VertexId::edge_endpoint_key(100 + k, 0),
                    EdgeId(1_000_000 + (src as u64) * 16 + k as u64),
                    300,
                )
                .expect("reinsert");
            }
        }
        black_box(());
        warm_ms.push(start.elapsed().as_secs_f64() * 1000.0);
        warm_reuse = csr.tombstone_reuse_count();
        warm_rate = csr.tombstone_reuse_hint_hit_rate();
        warm_overflow = csr.overflow_chunk_allocs();
    }
    let mut cold_ms = Vec::with_capacity(ITERATIONS);
    let mut cold_reuse = 0u64;
    let mut cold_rate = 0.0f32;
    let mut cold_overflow = 0u64;
    for _ in 0..ITERATIONS {
        let mut csr = build();
        csr.clear_tombstone_reuse_cutoff();
        let start = Instant::now();
        for src in 0..B2_ROWS {
            for k in (0..B2_FULL).step_by(2) {
                csr.insert_edge(
                    src,
                    VertexId::edge_endpoint_key(100 + k, 0),
                    EdgeId(1_000_000 + (src as u64) * 16 + k as u64),
                    300,
                )
                .expect("reinsert");
            }
        }
        black_box(());
        cold_ms.push(start.elapsed().as_secs_f64() * 1000.0);
        cold_reuse = csr.tombstone_reuse_count();
        cold_rate = csr.tombstone_reuse_hint_hit_rate();
        cold_overflow = csr.overflow_chunk_allocs();
    }
    let warm_med = median_f64(&mut warm_ms);
    let cold_med = median_f64(&mut cold_ms);
    let degradation = if warm_med > 0.0 {
        (cold_med - warm_med) / warm_med * 100.0
    } else {
        0.0
    };
    println!("re-insert {} edges into half-tombstoned full rows", B2_ROWS);
    println!("warm (cutoff fresh) median = {warm_med:.3} ms, reuse={warm_reuse}, hint_hit_rate={warm_rate:.2}, overflow_allocs={warm_overflow}");
    println!("cold (cutoff cleared, restart) median = {cold_med:.3} ms, reuse={cold_reuse}, hint_hit_rate={cold_rate:.2}, overflow_allocs={cold_overflow}");
    println!("cold degradation = {degradation:.1}% (threshold: >10% -> persist hint)");
    println!("persist cost reference: one table-level cutoff, 8 bytes");
}

// ── B3: default scan pagination ─────────────────────────────────
// Full `scan_edges_by_type` materialization vs paginated loop over the
// same type. Peak bytes approximated as Vec<Edge> reservation.

const B3_SPACE: &str = "b3";
const B3_TAG: &str = "Node";
const B3_EDGE: &str = "Link";
const B3_VERTICES: usize = 2000;
const B3_EDGES: usize = 20_000;
const B3_PAGE: usize = 5000;

fn build_b3() -> GraphStorage {
    let mut storage = GraphStorage::new().expect("storage init");
    let mut space = SpaceInfo::new(B3_SPACE.to_string()).with_vid_type(DataType::BigInt);
    storage.create_space(&mut space).expect("create space");
    storage
        .create_tag(
            B3_SPACE,
            &TagInfo::new(B3_TAG.to_string()).with_properties(vec![PropertyDef::new(
                "value".to_string(),
                DataType::BigInt,
            )]),
        )
        .expect("create tag");
    storage
        .create_edge_type(
            B3_SPACE,
            &EdgeTypeInfo::new(B3_EDGE.to_string())
                .with_src_tag(B3_TAG.to_string())
                .with_dst_tag(B3_TAG.to_string()),
        )
        .expect("create edge type");
    let vertices: Vec<Vertex> = (0..B3_VERTICES)
        .map(|i| {
            Vertex::new(
                VertexId::try_from_int64(i as i64).expect("vid"),
                Tag::new(
                    B3_TAG.to_string(),
                    [("value".into(), Value::BigInt(i as i64))]
                        .into_iter()
                        .collect(),
                ),
            )
        })
        .collect();
    storage
        .batch_insert_vertices(B3_SPACE, vertices)
        .expect("vertices");
    let edges: Vec<Edge> = (0..B3_EDGES)
        .map(|i| Edge {
            src: VertexId::try_from_int64((i % B3_VERTICES) as i64).expect("vid"),
            dst: VertexId::try_from_int64(((i + 1) % B3_VERTICES) as i64).expect("vid"),
            edge_type: B3_EDGE.to_string(),
            ranking: (i / B3_VERTICES) as i64,
            props: HashMap::new(),
        })
        .collect();
    for chunk in edges.chunks(5000) {
        storage
            .batch_insert_edges(B3_SPACE, chunk.to_vec())
            .expect("edges");
    }
    storage
}

fn bench_b3() {
    println!("\n## B3: default scan vs explicit paging (time + peak bytes)");
    // The default scan already drains the paginated cursor, so both legs
    // share one implementation; the count assert below is the parity gate
    // and the memory ratio gates the bounded working set.
    let storage = build_b3();
    let expect = storage
        .scan_edges_by_type(B3_SPACE, B3_EDGE)
        .expect("scan")
        .len();
    assert_eq!(expect, B3_EDGES);

    let mut full_us = Vec::with_capacity(ITERATIONS);
    let mut full_bytes = 0usize;
    for _ in 0..ITERATIONS {
        let start = Instant::now();
        let edges = storage.scan_edges_by_type(B3_SPACE, B3_EDGE).expect("scan");
        black_box(edges.len());
        full_us.push(start.elapsed().as_micros() as u64);
        full_bytes = edges.capacity() * std::mem::size_of::<Edge>();
    }
    let mut page_us = Vec::with_capacity(ITERATIONS);
    let mut page_bytes = 0usize;
    for _ in 0..ITERATIONS {
        let start = Instant::now();
        let mut count = 0usize;
        let mut peak = 0usize;
        let mut offset = 0;
        loop {
            let page = storage
                .scan_edges_by_type_paginated(B3_SPACE, B3_EDGE, offset, B3_PAGE)
                .expect("paginated");
            if page.is_empty() {
                break;
            }
            peak = peak.max(page.capacity() * std::mem::size_of::<Edge>());
            count += page.len();
            offset += page.len();
            black_box(page.len());
        }
        assert_eq!(count, expect);
        page_us.push(start.elapsed().as_micros() as u64);
        page_bytes = peak;
    }
    let full_med = median_u64(&mut full_us);
    let page_med = median_u64(&mut page_us);
    let overhead = if full_med > 0 {
        (page_med as f64 - full_med as f64) / full_med as f64 * 100.0
    } else {
        0.0
    };
    let ratio = full_bytes as f64 / page_bytes.max(1) as f64;
    println!("edges = {expect}, page = {B3_PAGE}");
    println!("default scan (single cursor drain) median = {full_med} us, peak ~ {full_bytes}B");
    println!("explicit paging median = {page_med} us, peak ~ {page_bytes}B");
    println!("paging-loop overhead = {overhead:.1}% (informational: per-page cursor setup; the default scan drains one cursor)");
    println!(
        "memory ratio full/paged = {ratio:.2}x (gate: >2x keeps the paged working set bounded)"
    );
}

// ── B4: out/in dual-write fused single traversal ────────────────
// One insert batch on a Both table (2x topology writes) vs the same
// batch on an OutOnly table (1x). The gap is the prize for fusing the
// shared authority/property checks into one traversal.

const B4_EDGES: usize = 5000;

fn table_schema(out_only: bool) -> EdgeSchema {
    use linkrs::core::types::EdgeStrategy;
    use linkrs::storage::edge::RecordForm;
    EdgeSchema {
        label_id: 0,
        label_name: "bench".to_string(),
        src_label: 0,
        dst_label: 0,
        properties: Vec::new(),
        oe_strategy: EdgeStrategy::Multiple,
        ie_strategy: if out_only {
            EdgeStrategy::None
        } else {
            EdgeStrategy::Multiple
        },
        schema_version: 1,
        record_form: RecordForm::Columnar,
    }
}

fn bench_b4() {
    println!("\n## B4: dual-write cost (Both vs OutOnly batch commit)");
    let keys: Vec<(u32, u32)> = (0..B4_EDGES as u32).map(|i| (i, i + 1)).collect();
    let mut both_ms = Vec::with_capacity(ITERATIONS);
    let mut single_ms = Vec::with_capacity(ITERATIONS);
    for _ in 0..ITERATIONS {
        let mut table = EdgeStore::with_config(table_schema(false), EdgeTableConfig::default())
            .expect("both table");
        let mut batch = EdgeStore::staging_batch();
        for (src, dst) in &keys {
            batch.stage_insert(*src, *dst, 0, &[], 100);
        }
        let start = Instant::now();
        let applied = table.commit_staging_batch(batch).expect("commit");
        black_box(applied);
        both_ms.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    for _ in 0..ITERATIONS {
        let mut table = EdgeStore::with_config(table_schema(true), EdgeTableConfig::default())
            .expect("out-only table");
        let mut batch = EdgeStore::staging_batch();
        for (src, dst) in &keys {
            batch.stage_insert(*src, *dst, 0, &[], 100);
        }
        let start = Instant::now();
        let applied = table.commit_staging_batch(batch).expect("commit");
        black_box(applied);
        single_ms.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    let both_med = median_f64(&mut both_ms);
    let single_med = median_f64(&mut single_ms);
    let probe =
        EdgeStore::with_config(table_schema(false), EdgeTableConfig::default()).expect("probe");
    let breakdown = probe.storage_breakdown();
    println!("batch = {B4_EDGES} edges, topology_write_amplification Both=2 OutOnly=1");
    println!("Both commit median = {both_med:.3} ms");
    println!("OutOnly commit median = {single_med:.3} ms");
    println!(
        "dual-write prize = {:.1}% of Both commit time",
        (both_med - single_med) / both_med * 100.0
    );
    println!(
        "empty-table storage split: out={}B in={}B authority={}B owner={}B property={}B",
        breakdown.out_bytes,
        breakdown.in_bytes,
        breakdown.authority_bytes,
        breakdown.owner_bytes,
        breakdown.property_bytes
    );
    println!("step-1 gate (merge shared checks, no layout change): merge iff no regression");
}

// ── B5: region-level incremental checkpoint ─────────────────────
// Full flush bytes vs a small-churn flush on a persistent storage.
// A small second flush proves owner-group granularity already pays.

const B5_EDGES: usize = 20_000;
const B5_CHURN: usize = 400;

fn build_b5_vertices(storage: &mut GraphStorage, space: &str, tag: &str, n: usize) {
    let vertices: Vec<Vertex> = (0..n)
        .map(|i| {
            Vertex::new(
                VertexId::try_from_int64(i as i64).expect("vid"),
                Tag::new(
                    tag.to_string(),
                    [("value".into(), Value::BigInt(i as i64))]
                        .into_iter()
                        .collect(),
                ),
            )
        })
        .collect();
    storage
        .batch_insert_vertices(space, vertices)
        .expect("vertices");
}

fn bench_b5() {
    println!("\n## B5: incremental checkpoint (full flush vs small-churn flush)");
    let dir = TempDir::new().expect("tempdir");
    let space = "b5";
    let tag = "Node";
    let edge = "Link";
    let mut storage =
        GraphStorage::new_with_path(dir.path().to_path_buf()).expect("persistent storage");
    let mut space_info = SpaceInfo::new(space.to_string()).with_vid_type(DataType::BigInt);
    storage.create_space(&mut space_info).expect("space");
    storage
        .create_tag(
            space,
            &TagInfo::new(tag.to_string()).with_properties(vec![PropertyDef::new(
                "value".to_string(),
                DataType::BigInt,
            )]),
        )
        .expect("tag");
    storage
        .create_edge_type(
            space,
            &EdgeTypeInfo::new(edge.to_string())
                .with_src_tag(tag.to_string())
                .with_dst_tag(tag.to_string()),
        )
        .expect("edge type");
    build_b5_vertices(&mut storage, space, tag, 2000);
    let edges: Vec<Edge> = (0..B5_EDGES)
        .map(|i| Edge {
            src: VertexId::try_from_int64((i % 2000) as i64).expect("vid"),
            dst: VertexId::try_from_int64(((i + 1) % 2000) as i64).expect("vid"),
            edge_type: edge.to_string(),
            ranking: (i / 2000) as i64,
            props: HashMap::new(),
        })
        .collect();
    for chunk in edges.chunks(5000) {
        storage
            .batch_insert_edges(space, chunk.to_vec())
            .expect("edges");
    }
    let start = Instant::now();
    storage.create_checkpoint().expect("full checkpoint");
    let full_ms = start.elapsed().as_secs_f64() * 1000.0;
    let full_bytes = dir_bytes(dir.path());

    let churn: Vec<Edge> = (0..B5_CHURN)
        .map(|i| Edge {
            src: VertexId::try_from_int64(i as i64).expect("vid"),
            dst: VertexId::try_from_int64(((i + 7) % 2000) as i64).expect("vid"),
            edge_type: edge.to_string(),
            ranking: 100_000 + i as i64,
            props: HashMap::new(),
        })
        .collect();
    storage
        .batch_insert_edges(space, churn)
        .expect("churn edges");
    let start = Instant::now();
    storage.create_checkpoint().expect("churn checkpoint");
    let churn_ms = start.elapsed().as_secs_f64() * 1000.0;
    let churn_total = dir_bytes(dir.path());
    let churn_delta = churn_total.saturating_sub(full_bytes);
    println!("base edges = {B5_EDGES}, churn edges = {B5_CHURN}");
    println!("full checkpoint = {full_ms:.2} ms, dir bytes = {full_bytes}");
    println!("churn checkpoint = {churn_ms:.2} ms, dir delta = {churn_delta}B");
    println!(
        "churn/full byte ratio = {:.2}% (threshold: fallback path >20% of flush -> region work)",
        churn_delta as f64 / full_bytes.max(1) as f64 * 100.0
    );
}

// ── B6: commit-order fault matrix (recovery gate) ───────────────
// Not a perf bench: recovery time + audit zero-drift across a clean
// flush/close vs a WAL-open crash (drop without flush). 100% pass
// is the merge gate.

const B6_EDGES: usize = 10_000;

fn build_b6_storage_chunked(
    path: Option<std::path::PathBuf>,
    chunk: usize,
) -> (GraphStorage, Option<TempDir>) {
    let (mut storage, dir) = match path {
        Some(path) => (
            GraphStorage::new_with_path(path).expect("persistent storage"),
            None,
        ),
        None => {
            let dir = TempDir::new().expect("tempdir");
            (
                GraphStorage::new_with_path(dir.path().to_path_buf()).expect("persistent storage"),
                Some(dir),
            )
        }
    };
    let space = "b6";
    let tag = "Node";
    let edge = "Link";
    let mut space_info = SpaceInfo::new(space.to_string()).with_vid_type(DataType::BigInt);
    storage.create_space(&mut space_info).expect("space");
    storage
        .create_tag(
            space,
            &TagInfo::new(tag.to_string()).with_properties(vec![PropertyDef::new(
                "value".to_string(),
                DataType::BigInt,
            )]),
        )
        .expect("tag");
    storage
        .create_edge_type(
            space,
            &EdgeTypeInfo::new(edge.to_string())
                .with_src_tag(tag.to_string())
                .with_dst_tag(tag.to_string()),
        )
        .expect("edge type");
    let vertices: Vec<Vertex> = (0..2000)
        .map(|i| {
            Vertex::new(
                VertexId::try_from_int64(i as i64).expect("vid"),
                Tag::new(
                    tag.to_string(),
                    [("value".into(), Value::BigInt(i as i64))]
                        .into_iter()
                        .collect(),
                ),
            )
        })
        .collect();
    storage
        .batch_insert_vertices(space, vertices)
        .expect("vertices");
    let edges: Vec<Edge> = (0..B6_EDGES)
        .map(|i| Edge {
            src: VertexId::try_from_int64((i % 2000) as i64).expect("vid"),
            dst: VertexId::try_from_int64(((i + 1) % 2000) as i64).expect("vid"),
            edge_type: edge.to_string(),
            ranking: (i / 2000) as i64,
            props: HashMap::new(),
        })
        .collect();
    for chunk in edges.chunks(chunk) {
        storage
            .batch_insert_edges(space, chunk.to_vec())
            .expect("edges");
    }
    (storage, dir)
}

fn bench_b6() {
    println!("\n## B6: commit-order recovery matrix (time + audit drift)");
    let dir_a = TempDir::new().expect("tempdir");
    let path_a = dir_a.path().to_path_buf();
    {
        let (storage, _) = build_b6_storage_chunked(Some(path_a.clone()), B6_EDGES);
        storage.create_checkpoint().expect("checkpoint");
    }
    let start = Instant::now();
    let reopened = GraphStorage::open(path_a.clone()).expect("reopen clean");
    let recover_a_ms = start.elapsed().as_secs_f64() * 1000.0;
    let count_a = reopened
        .scan_edges_by_type("b6", "Link")
        .expect("audit scan")
        .len();

    // Crash-tail shapes run quiesced (see
    // tests/edge_wal_tail_recovery.rs): without Drop-time quiescing,
    // background checkpoint tasks race the next open.

    // Live audit timing on the same shape.
    let (storage_c, _dir_c) = build_b6_storage_chunked(None, B6_EDGES);
    let start = Instant::now();
    let count_c = storage_c
        .scan_edges_by_type("b6", "Link")
        .expect("scan")
        .len();
    let audit_ms = start.elapsed().as_secs_f64() * 1000.0;

    // Case C: two-commit entry gate. Two 5k commits (same-pair
    // multi-rank edges), then checkpoint + open. This used to fail with
    // "Mutable CSR edge count mismatch: stored=625, recomputed=938"
    // (stale live slots after reserve moves); the fault matrix starts here.
    let dir_d = TempDir::new().expect("tempdir");
    let path_d = dir_d.path().to_path_buf();
    {
        let (storage, _) = build_b6_storage_chunked(Some(path_d.clone()), 5000);
        storage.create_checkpoint().expect("checkpoint");
    }
    let count_d = GraphStorage::open(path_d.clone())
        .expect("two-commit reopen")
        .scan_edges_by_type("b6", "Link")
        .expect("two-commit audit")
        .len();

    println!("rows = {B6_EDGES}");
    println!("clean-checkpoint reopen = {recover_a_ms:.2} ms, audit edges = {count_a}");
    println!("live audit scan = {audit_ms:.2} ms, edges = {count_c}");
    println!("two-commit checkpoint reopen audit edges = {count_d}");
    let pass = count_a == B6_EDGES && count_c == B6_EDGES && count_d == B6_EDGES;
    println!(
        "audit zero-drift pass = {pass} (gate: 100% required; recovery must fit one checkpoint period)"
    );
    assert!(pass, "recovery audit must show zero drift");
}

// ── B7: narrow rows + authority trimming ────────────────────────
// Per-edge byte split (topology, authority, mapping, rows) on a
// zero-rank property-less table vs a ranked table with one property.
// Decides whether a narrow-row prototype is worth building.

const B7_EDGES: usize = 10_000;

fn bench_b7() {
    println!("\n## B7: per-edge byte split (narrow-row candidacy)");
    use linkrs::core::types::EdgeStrategy;
    use linkrs::storage::edge::RecordForm;
    use linkrs::storage::StoragePropertyDef;
    let flat_schema = EdgeSchema {
        label_id: 0,
        label_name: "flat".to_string(),
        src_label: 0,
        dst_label: 0,
        properties: Vec::new(),
        oe_strategy: EdgeStrategy::Multiple,
        ie_strategy: EdgeStrategy::Multiple,
        schema_version: 1,
        record_form: RecordForm::Columnar,
    };
    let ranked_schema = EdgeSchema {
        label_id: 1,
        label_name: "ranked".to_string(),
        src_label: 0,
        dst_label: 0,
        properties: vec![StoragePropertyDef {
            name: "weight".into(),
            data_type: DataType::Double,
            nullable: false,
            default_value: Some(Value::Double(0.0)),
        }],
        oe_strategy: EdgeStrategy::Multiple,
        ie_strategy: EdgeStrategy::Multiple,
        schema_version: 1,
        record_form: RecordForm::Columnar,
    };
    let mut flat =
        EdgeStore::with_config(flat_schema, EdgeTableConfig::default()).expect("flat table");
    let mut ranked =
        EdgeStore::with_config(ranked_schema, EdgeTableConfig::default()).expect("ranked table");
    for i in 0..B7_EDGES as u32 {
        flat.insert_edge(i, i + 1, 0, &[], 100)
            .expect("flat insert");
        ranked
            .insert_edge(
                i,
                i + 1,
                (i % 7) as i64,
                &[("weight".into(), Value::Double(i as f64))],
                100,
            )
            .expect("ranked insert");
    }
    let (f_topo, f_auth, f_map, f_rows) = flat.bytes_per_edge_breakdown();
    let (r_topo, r_auth, r_map, r_rows) = ranked.bytes_per_edge_breakdown();
    let f_total = f_topo + f_auth + f_map + f_rows;
    let r_total = r_topo + r_auth + r_map + r_rows;
    println!("edges per table = {B7_EDGES}");
    println!(
        "flat   (rank=0, no props): topo={f_topo:.1}B authority={f_auth:.1}B mapping={f_map:.1}B rows={f_rows:.1}B total={f_total:.1}B"
    );
    println!(
        "ranked (ranked + 1 prop): topo={r_topo:.1}B authority={r_auth:.1}B mapping={r_map:.1}B rows={r_rows:.1}B total={r_total:.1}B"
    );
    let row_share = r_rows / r_total * 100.0;
    println!("row share of ranked total = {row_share:.1}% (narrow-row prize ceiling)");
    println!("gate: narrow layout needs >15% total saving with zero scan regression");
}

fn main() {
    println!("== edge property tables: benchmark-first decision suite ==");
    println!(
        "machine: {} ({} visible cores)",
        machine_name(),
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(0)
    );
    println!("iterations = {ITERATIONS} (median), small data: contrast only");
    bench_b1();
    bench_b2();
    bench_b3();
    bench_b4();
    bench_b5();
    bench_b6();
    bench_b7();
    println!("\nresult: per-section pass/fail gates printed above");
}
