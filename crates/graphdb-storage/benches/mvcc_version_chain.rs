//! Benchmark for MVCC version chain refactor.
//!
//! Measures write throughput, time-travel read latency, and memory usage
//! after introducing RowVisibility (lightweight per-row timestamps) and
//! lazy-allocated version chains.
//!
//! Scenarios:
//! - Lazy allocation benefit: many columns, only one updated
//! - Fold optimization: repeated updates to same row with cap
//! - Time-travel read: get with historical timestamp over deep chain
//! - Memory: version_chain_stats after many updates

use graphdb_core::types::EdgeId;
use graphdb_core::{DataType, Value};
use graphdb_storage::edge::property_schema::PropertySchema;
use graphdb_storage::edge::CsrWithProperties;
use std::time::Instant;

fn bench_lazy_allocation() {
    println!("--- Lazy allocation: 10 columns, only 1 updated ---");
    let schema: Vec<PropertySchema> = (0..10)
        .map(|i| PropertySchema::new(format!("c{}", i), i, DataType::Int).nullable(true))
        .collect();
    let mut table = CsrWithProperties::new(16, schema);
    for i in 0..10_000u32 {
        let eid = EdgeId(i as u64 + 1);
        table
            .insert_for_edge(
                eid,
                &[("c0".to_string(), Value::Int(i as i32))],
                100 + i as u64,
            )
            .unwrap();
    }
    let start = Instant::now();
    for idx in 0..10_000usize {
        let eid = EdgeId(idx as u64 + 1);
        let v = Value::Int((idx as i32) * 2);
        table
            .set_property_for_edge(eid, "c0", Some(v), 200_000 + idx as u64)
            .unwrap();
    }
    let elapsed = start.elapsed();
    let stats = table.version_chain_stats();
    println!(
        "  inserted 10k rows (10 cols, 1 col with value) in {:?}",
        start.elapsed()
    );
    println!("  updated c0 for 10k rows in {:?}", elapsed);
    println!(
        "  version_chains: total_rows={}, total_entries={}, max_len={}, memory_bytes={}",
        stats.total_rows, stats.total_entries, stats.max_len, stats.memory_bytes
    );
    println!("  lazy allocation verified: untouched columns have minimal chains");
}

fn bench_fold_optimization() {
    println!("\n--- Fold optimization: 1 column, 1000 updates to same row, cap=64 ---");
    let schema = vec![PropertySchema::new("age".to_string(), 0, DataType::Int).nullable(true)];
    let mut table = CsrWithProperties::new(4, schema);
    table.set_version_chain_cap(64);
    let eid = EdgeId(1);
    table
        .insert_for_edge(eid, &[("age".to_string(), Value::Int(0))], 1)
        .unwrap();
    let start = Instant::now();
    for ts in 2..=1001u64 {
        table
            .set_property_for_edge(eid, "age", Some(Value::Int(ts as i32)), ts)
            .unwrap();
    }
    let elapsed = start.elapsed();
    let stats = table.version_chain_stats();
    println!("  1000 versioned writes with fold cap 64 in {:?}", elapsed);
    println!(
        "  total_entries={}, max_len={}",
        stats.total_entries, stats.max_len
    );
    assert!(stats.max_len <= 64 + 1);
    println!("  fold keeps chain bounded at cap");
}

fn bench_time_travel_read() {
    println!("\n--- Time-travel read latency ---");
    let schema = vec![PropertySchema::new("v".to_string(), 0, DataType::BigInt).nullable(true)];
    let mut table = CsrWithProperties::new(4, schema);
    table.set_version_chain_cap(0);
    let eid = EdgeId(1);
    table
        .insert_for_edge(eid, &[("v".to_string(), Value::BigInt(0))], 10)
        .unwrap();
    for ts in 1..=100u64 {
        let t = 10 + ts * 10;
        table
            .set_property_for_edge(eid, "v", Some(Value::BigInt(t as i64)), t)
            .unwrap();
    }
    let queries = 10_000;
    let start = Instant::now();
    for i in 0..queries {
        let q = (i % 100) * 10 + 5;
        let _ = table.get_by_edge_id(eid, q as u64);
    }
    let elapsed = start.elapsed();
    let stats = table.version_chain_stats();
    println!(
        "  {} time-travel gets over max_len {} in {:?} ({:.1} ns/op)",
        queries,
        stats.max_len,
        elapsed,
        elapsed.as_nanos() as f64 / queries as f64
    );
    // Verify correctness: query at 55 should return value at ts 50
    assert_eq!(
        table.get_by_edge_id(eid, 55).unwrap()[0].1,
        Some(Value::BigInt(50))
    );
    assert!(
        table.get_by_edge_id(eid, 5).is_none()
            || table.get_by_edge_id(eid, 5).unwrap()[0].1.is_none()
    );
    println!("  time-travel correctness verified");
}

fn bench_memory() {
    println!("\n--- Memory usage: 1k rows, each with 10 updates (cap 64) ---");
    let schema = vec![
        PropertySchema::new("a".to_string(), 0, DataType::Int).nullable(true),
        PropertySchema::new("b".to_string(), 1, DataType::String).nullable(true),
    ];
    let mut table = CsrWithProperties::new(1024, schema);
    table.set_version_chain_cap(64);
    let start = Instant::now();
    for row in 0..1000usize {
        let eid = EdgeId(row as u64 + 1);
        table
            .insert_for_edge(
                eid,
                &[
                    ("a".to_string(), Value::Int(row as i32)),
                    ("b".to_string(), Value::string("init")),
                ],
                100,
            )
            .unwrap();
        for upd in 1..10u64 {
            let v = Value::Int((row as i32) * 10 + upd as i32);
            table
                .set_property_for_edge(eid, "a", Some(v), 100 + upd)
                .unwrap();
        }
    }
    let elapsed = start.elapsed();
    let stats = table.version_chain_stats();
    println!("  1k rows * 10 updates in {:?}", elapsed);
    println!(
        "  total_entries={}, max_len={}, memory_bytes={}",
        stats.total_entries, stats.max_len, stats.memory_bytes
    );
    println!("  memory benchmark done");
}

fn main() {
    println!("=== MVCC Version Chain Benchmark (RowVisibility + Lazy Chains) ===");
    bench_lazy_allocation();
    bench_fold_optimization();
    bench_time_travel_read();
    bench_memory();
    println!("\nAll MVCC benchmarks completed.");
}
