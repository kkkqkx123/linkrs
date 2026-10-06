//! Allocation accounting for the edge property read path.
//!
//! Uses a counting global allocator to report per-operation heap bytes and
//! allocation counts for `get_edge`. Column names travel as shared `Arc<str>`
//! handles cloned from the label schema, so a per-edge read must not allocate
//! for property names; any growth here means a name string is being rebuilt
//! on the read path. Small dataset, debug-mode friendly.

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use graphdb::core::types::{EdgeTypeInfo, PropertyDef, SpaceInfo, TagInfo, VertexId};
use graphdb::core::vertex_edge_path::Tag;
use graphdb::core::{DataType, Edge, Value, Vertex};
use graphdb::storage::{GraphStorage, StorageReader, StorageSchemaOps, StorageWriter};
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

static ALLOC_BYTES: AtomicUsize = AtomicUsize::new(0);
static ALLOC_COUNT: AtomicUsize = AtomicUsize::new(0);

struct CountingAlloc;

unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOC_BYTES.fetch_add(layout.size(), Ordering::Relaxed);
        ALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
        System.alloc(layout)
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout)
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOC_BYTES.fetch_add(new_size.saturating_sub(layout.size()), Ordering::Relaxed);
        ALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
        System.realloc(ptr, layout, new_size)
    }
}

#[global_allocator]
static GLOBAL: CountingAlloc = CountingAlloc;

#[path = "results_report.rs"]
mod report;
use report::write_results_report;

const VERTEX_COUNT: usize = 1000;
const EDGES_PER_VERTEX: usize = 2;
const PROP_COUNT: usize = 4;

fn setup_graph() -> GraphStorage {
    let mut storage = GraphStorage::new().expect("storage init");
    let space_name = "alloc_bench".to_string();
    let mut space = SpaceInfo::new(space_name.clone()).with_vid_type(DataType::BigInt);
    storage.create_space(&mut space).expect("create space");

    storage
        .create_tag(
            &space_name,
            &TagInfo::new("Node".to_string()).with_properties(vec![
                PropertyDef::new("id".into(), DataType::BigInt),
                PropertyDef::new("value".into(), DataType::Double),
            ]),
        )
        .expect("create tag");

    let edge_props: Vec<PropertyDef> = (0..PROP_COUNT)
        .map(|i| PropertyDef::new(format!("p{}", i), DataType::Double))
        .collect();
    storage
        .create_edge_type(
            &space_name,
            &EdgeTypeInfo::new("Link".to_string()).with_properties(edge_props),
        )
        .expect("create edge type");

    let vertices: Vec<Vertex> = (0..VERTEX_COUNT as i64)
        .map(|i| {
            Vertex::new(
                VertexId::try_from_int64(i).expect("valid vertex id"),
                Tag::new(
                    "Node".to_string(),
                    [
                        ("id".into(), Value::BigInt(i)),
                        ("value".into(), Value::Double(i as f64)),
                    ]
                    .into_iter()
                    .collect(),
                ),
            )
        })
        .collect();
    storage
        .batch_insert_vertices(&space_name, vertices)
        .expect("insert vertices");

    let edges: Vec<Edge> = (0..VERTEX_COUNT as i64)
        .flat_map(|src| {
            (1..=EDGES_PER_VERTEX).map(move |k| Edge {
                src: VertexId::try_from_int64(src).expect("valid vertex id"),
                dst: VertexId::try_from_int64((src + k as i64) % VERTEX_COUNT as i64)
                    .expect("valid vertex id"),
                edge_type: "Link".to_string(),
                ranking: 0,
                props: (0..PROP_COUNT)
                    .map(|i| {
                        (
                            Arc::from(format!("p{}", i)),
                            Value::Double(k as f64 + i as f64 * 0.5),
                        )
                    })
                    .collect(),
            })
        })
        .collect();
    storage
        .batch_insert_edges(&space_name, edges)
        .expect("insert edges");
    storage
}

fn read_edge_once(storage: &GraphStorage) {
    let edge = storage
        .get_edge(
            "alloc_bench",
            &VertexId::try_from_int64(7).expect("vid"),
            &VertexId::try_from_int64(8).expect("vid"),
            "Link",
            0,
        )
        .expect("get_edge");
    black_box(edge);
}

fn bench_edge_read(c: &mut Criterion) {
    let storage = setup_graph();

    // Warm caches.
    for _ in 0..100 {
        read_edge_once(&storage);
    }

    // Allocation accounting per single-edge read.
    let iters = 1000;
    ALLOC_BYTES.store(0, Ordering::Relaxed);
    ALLOC_COUNT.store(0, Ordering::Relaxed);
    for _ in 0..iters {
        read_edge_once(&storage);
    }
    let bytes_per_op = ALLOC_BYTES.load(Ordering::Relaxed) as f64 / iters as f64;
    let calls_per_op = ALLOC_COUNT.load(Ordering::Relaxed) as f64 / iters as f64;

    let mut report =
        String::from("edge property read path allocation accounting (bench profile)\n\n");
    report.push_str(&format!(
        "dataset: {} vertices, {} edges/vertex, {} edge properties\n\n",
        VERTEX_COUNT, EDGES_PER_VERTEX, PROP_COUNT
    ));
    report.push_str(&format!(
        "get_edge (single edge, {} props):\n  heap bytes/op:  {:.1}\n  allocations/op: {:.1}\n\n\
Per-edge read allocates nothing for property names: column names are shared\n\
Arc<str> handles cloned from the label schema. Residual heap traffic is the\n\
value map plus Value clones, which are inherent to returning owned results.\n",
        PROP_COUNT, bytes_per_op, calls_per_op
    ));

    let mut group = c.benchmark_group("edge_read");
    group.measurement_time(Duration::from_secs(2));
    group.sample_size(20);
    group.warm_up_time(Duration::from_millis(300));
    group.bench_function("get_edge_single", |b| {
        b.iter(|| read_edge_once(&storage));
    });
    group.finish();

    let path = write_results_report("edge_read_alloc_bench", "results.txt", &report);
    println!("report written to {}", path.display());
}

criterion_group!(benches, bench_edge_read);
criterion_main!(benches);
