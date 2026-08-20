//! Storage read baseline: wide-table full/projected scan vs
//! narrow-table random property access.
//!
//! Plain-main bench (harness = false) so results can be consumed directly.
//! Run with:
//!   cargo bench --bench storage_read_baseline

use graphdb::core::types::{PropertyDef, SpaceInfo, TagInfo, VertexId};
use graphdb::core::vertex_edge_path::Tag;
use graphdb::core::{DataType, Value, Vertex};
use graphdb::storage::{GraphStorage, ScanOptions, StorageReader, StorageSchemaOps, StorageWriter};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use std::time::Instant;

const WIDE_PROPS: [(&str, DataType); 8] = [
    ("v0", DataType::BigInt),
    ("v1", DataType::BigInt),
    ("v2", DataType::BigInt),
    ("v3", DataType::BigInt),
    ("v4", DataType::BigInt),
    ("s0", DataType::String),
    ("s1", DataType::String),
    ("s2", DataType::String),
];

const NARROW_PROPS: [(&str, DataType); 2] = [("v", DataType::BigInt), ("s", DataType::String)];

fn new_space(storage: &mut GraphStorage, name: &str, vid_type: DataType) {
    let mut space = SpaceInfo::new(name.to_string()).with_vid_type(vid_type);
    storage.create_space(&mut space).expect("create space");
}

fn build_table(vertex_count: u64, props: &[(&str, DataType)], label: &str) -> GraphStorage {
    let mut storage = GraphStorage::new().expect("storage init");
    let space = format!("read_{}", label);
    new_space(&mut storage, &space, DataType::BigInt);
    storage
        .create_tag(
            &space,
            &TagInfo::new(label.to_string()).with_properties(
                props
                    .iter()
                    .map(|(n, t)| PropertyDef::new((*n).to_string(), (*t).clone()))
                    .collect(),
            ),
        )
        .expect("create tag");

    let mut start = 0usize;
    while start < vertex_count as usize {
        let end = (start + 10_000).min(vertex_count as usize);
        let vertices: Vec<Vertex> = (start..end)
            .map(|i| {
                let mut map = Vec::new();
                for (idx, (name, ty)) in props.iter().enumerate() {
                    let value = match ty {
                        DataType::BigInt | DataType::Int => Value::BigInt(i as i64 + idx as i64),
                        _ => Value::string(format!("s{}_v{}", idx, i)),
                    };
                    map.push((name.to_string(), value));
                }
                Vertex::new(
                    VertexId::from_int64(i as i64),
                    vec![Tag::new(label.to_string(), map.into_iter().collect())],
                )
            })
            .collect();
        storage
            .batch_insert_vertices(&space, vertices)
            .expect("batch insert");
        start = end;
    }
    storage
}

fn scan_all(
    storage: &GraphStorage,
    space: &str,
    limit: usize,
    projection: Option<&[String]>,
    batch: usize,
) -> u64 {
    let mut options = ScanOptions::new().with_offset(0).with_limit(limit);
    if let Some(proj) = projection {
        options = options.with_projection_named(proj.to_vec());
    }
    let mut cursor = storage
        .create_vertex_cursor(space, &options)
        .expect("open cursor");
    let mut batches = 0u64;
    while !cursor.next_batch(batch).expect("next batch").is_empty() {
        batches += 1;
    }
    batches
}

fn median_us(samples: &[f64]) -> f64 {
    let mut v = samples.to_vec();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[v.len() / 2]
}

fn bench_scan(
    storage: &GraphStorage,
    space: &str,
    limit: usize,
    projection: Option<&[String]>,
    batch: usize,
    iterations: usize,
) -> (f64, f64) {
    let mut samples = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        let start = Instant::now();
        let batches = scan_all(storage, space, limit, projection, batch);
        let elapsed = start.elapsed().as_secs_f64() * 1e6;
        samples.push(elapsed);
        assert!(batches > 0);
    }
    (
        median_us(&samples),
        limit as f64 / (median_us(&samples) / 1e6),
    )
}

fn bench_random_access(
    storage: &GraphStorage,
    space: &str,
    indices: &[i64],
    projected: bool,
    iterations: usize,
) -> f64 {
    let mut samples = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        let start = Instant::now();
        for &id in indices {
            let vid = VertexId::from_int64(id);
            if projected {
                let _ = storage
                    .get_vertex_projected(space, &vid, &["v".to_string()])
                    .expect("get projected");
            } else {
                let _ = storage.get_vertex(space, &vid).expect("get vertex");
            }
        }
        samples.push(start.elapsed().as_secs_f64() * 1e6);
    }
    median_us(&samples)
}

fn main() {
    println!("== storage read baseline ==");

    for (label, props, sizes) in [
        ("wide", &WIDE_PROPS[..], &[10_000u64, 100_000][..]),
        ("narrow", &NARROW_PROPS[..], &[10_000u64, 100_000][..]),
    ] {
        for &n in sizes {
            println!("\n-- {label} table, {n} rows --");
            let storage = build_table(n, props, label);
            let space = format!("read_{label}");
            let limit = n as usize;

            let (full_256_us, full_256_rows) = bench_scan(&storage, &space, limit, None, 256, 7);
            let (proj_256_us, proj_256_rows) =
                bench_scan(&storage, &space, limit, Some(&["v0".to_string()]), 256, 7);
            let (full_4096_us, full_4096_rows) = bench_scan(&storage, &space, limit, None, 4096, 7);

            let proj_ratio = full_256_us / proj_256_us;

            println!(
                "full scan  batch=256 : {:>9.0} us  ({:>12.0} rows/s)",
                full_256_us, full_256_rows
            );
            println!(
                "proj scan  batch=256 : {:>9.0} us  ({:>12.0} rows/s)",
                proj_256_us, proj_256_rows
            );
            println!(
                "full scan  batch=4096: {:>9.0} us  ({:>12.0} rows/s)",
                full_4096_us, full_4096_rows
            );
            println!("projected/full ratio   : {:.2}x", proj_ratio);

            if label == "narrow" {
                let mut rng = StdRng::seed_from_u64(42);
                let indices: Vec<i64> = (0..10_000).map(|_| rng.gen_range(0..n as i64)).collect();
                let full_us = bench_random_access(&storage, &space, &indices, false, 7);
                let proj_us = bench_random_access(&storage, &space, &indices, true, 7);
                let per_op_full = full_us / indices.len() as f64;
                let per_op_proj = proj_us / indices.len() as f64;
                println!(
                    "random get_vertex        : {:>9.0} us total, {:>8.2} us/op",
                    full_us, per_op_full
                );
                println!(
                    "random get_projected     : {:>9.0} us total, {:>8.2} us/op",
                    proj_us, per_op_proj
                );
                println!(
                    "projected/full ratio     : {:.2}x",
                    per_op_full / per_op_proj
                );
            }
        }
    }
}
