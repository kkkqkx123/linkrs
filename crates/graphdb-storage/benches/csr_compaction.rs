//! Benchmark for CSR compaction with and without calibrator.
//!
//! Measures compaction frequency, query latency impact, and memory savings
//! of the calibrator tree versus static thresholds.

use graphdb_core::types::{EdgeId, VertexId};
use graphdb_storage::edge::edge_table::calibrator::{
    CalibratorConfig, CalibratorTree, DensityStats,
};
use graphdb_storage::edge::mutable_csr::MutableCsr;
use std::time::Instant;

/// Benchmark compaction with static threshold versus calibrated threshold.
fn bench_compaction_with_calibrator() {
    let config = CalibratorConfig::default();
    let mut tree = CalibratorTree::with_region_count(8, config);

    for rid in 0..8u32 {
        let deleted = if rid < 2 { 60 } else { 5 };
        tree.update_region_stats(
            rid,
            DensityStats {
                edge_count: 100,
                deleted_count: deleted,
                fragmented_capacity: 10,
                access_count: 0,
                last_compact_ts: 0,
            },
        );
    }

    let threshold = tree.calibrated_threshold();
    println!(
        "Calibrated deletion ratio: {:.3} (base 0.5, multiplier {:.2})",
        threshold.effective_deletion_ratio(),
        threshold.multiplier
    );
    println!(
        "Calibrated fragmentation ratio: {:.3}",
        threshold.effective_fragmentation_ratio()
    );

    let mut csr = MutableCsr::with_capacity(4096, 8192);
    for vid in 0..1024u32 {
        for i in 0..4 {
            let dst = VertexId::from_int64((vid as i64) * 10 + i as i64);
            let _ = csr.insert_edge(vid, dst, EdgeId(vid as u64 * 10 + i as u64), 1);
        }
    }
    for vid in 0..512u32 {
        let _ = csr.delete_edge(vid, EdgeId(vid as u64 * 10), 2);
    }

    let cutoff = 3;
    let start = Instant::now();
    let removed = csr.compact_regions_with_ts_reporting_calibrated(
        cutoff,
        0.25,
        &mut |_, _| {},
        1024,
        Some(threshold.effective_deletion_ratio()),
    );
    let elapsed = start.elapsed();
    println!(
        "Calibrated compact: removed {} edges in {:?}, fragmentation {:.2}",
        removed,
        elapsed,
        csr.fragmentation_ratio()
    );

    let mut csr2 = MutableCsr::with_capacity(4096, 8192);
    for vid in 0..1024u32 {
        for i in 0..4 {
            let dst = VertexId::from_int64((vid as i64) * 10 + i as i64);
            let _ = csr2.insert_edge(vid, dst, EdgeId(vid as u64 * 10 + i as u64), 1);
        }
    }
    for vid in 0..512u32 {
        let _ = csr2.delete_edge(vid, EdgeId(vid as u64 * 10), 2);
    }
    let start2 = Instant::now();
    let removed2 = csr2.compact_regions_with_ts_reporting(3, 0.25, &mut |_, _| {}, 1024);
    let elapsed2 = start2.elapsed();
    println!(
        "Static compact: removed {} edges in {:?}, fragmentation {:.2}",
        removed2,
        elapsed2,
        csr2.fragmentation_ratio()
    );
}

/// Benchmark overflow index sequential run detection.
fn bench_overflow_index() {
    let mut csr = MutableCsr::with_overflow_chunk_edges(1024, 4096, 4096);
    for vid in 0..1000u32 {
        for i in 0..10 {
            let dst = VertexId::from_int64((vid as i64) * 100 + i as i64);
            let _ = csr.insert_edge(vid, dst, EdgeId(vid as u64 * 100 + i as u64), 1);
        }
    }
    let start = Instant::now();
    csr.rebuild_overflow_index();
    let elapsed = start.elapsed();
    let stats = csr.overflow_index_stats();
    println!(
        "Overflow index rebuild: {} runs, {} sequential vertices, {} sparse, saved {} bytes in {:?}",
        stats.sequential_runs,
        stats.sequential_vertices,
        stats.sparse_vertices,
        stats.metadata_bytes_saved,
        elapsed
    );
}

fn main() {
    println!("=== CSR Compaction Benchmark (Calibrator) ===");
    bench_compaction_with_calibrator();
    println!("\n=== Overflow Index Benchmark ===");
    bench_overflow_index();
}
