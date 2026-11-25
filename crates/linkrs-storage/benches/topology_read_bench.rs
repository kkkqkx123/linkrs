//! Batch topology and destination read microbenchmark.
//!
//! Measures the storage work behind one columnar expand batch: adjacency
//! materialization, per-edge endpoint translation, destination deduplication
//! and the flat property views used by the query engine. No parser, planner
//! or execution pipeline is involved.
//!
//! Run with:
//!   cargo bench -p linkrs-storage --bench topology_read_bench

use linkrs_core::types::{EdgeTypeInfo, PropertyDef, SpaceInfo, TagInfo, VertexId};
use linkrs_core::vertex_edge_path::Tag;
use linkrs_core::{DataType, Edge, EdgeDirection, Value, Vertex};
use linkrs_storage::{GraphStorage, StorageReader, StorageSchemaOps, StorageWriter};
use std::alloc::{GlobalAlloc, Layout, System};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

struct CountingAllocator;

static ALLOCATED_BYTES: AtomicU64 = AtomicU64::new(0);

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = System.alloc(layout);
        if !ptr.is_null() {
            ALLOCATED_BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout);
    }
}

#[global_allocator]
static GLOBAL_ALLOCATOR: CountingAllocator = CountingAllocator;

const SPACE: &str = "topo_bench";
const TAG: &str = "Node";
const EDGE: &str = "Link";
const VERTEX_COUNT: i64 = 20_000;
const EDGES_PER_VERTEX: usize = 16;
const SEED_COUNT: usize = 2048;
const BATCH_SIZES: [usize; 3] = [512, 2048, 4096];
const UNIQUE_RATES: [usize; 3] = [100, 50, 10];
const ITERATIONS: usize = 20;

fn setup() -> GraphStorage {
    let mut storage = GraphStorage::new().expect("storage init");
    let mut space = SpaceInfo::new(SPACE.to_string()).with_vid_type(DataType::BigInt);
    storage.create_space(&mut space).expect("create space");
    storage
        .create_tag(
            SPACE,
            &TagInfo::new(TAG.to_string())
                .with_properties(vec![PropertyDef::new("id".into(), DataType::BigInt)]),
        )
        .expect("create tag");
    storage
        .create_edge_type(
            SPACE,
            &EdgeTypeInfo::new(EDGE.to_string())
                .with_properties(vec![PropertyDef::new("weight".into(), DataType::BigInt)])
                .with_src_tag(TAG.to_string())
                .with_dst_tag(TAG.to_string()),
        )
        .expect("create edge type");

    let vertices: Vec<Vertex> = (0..VERTEX_COUNT)
        .map(|id| {
            Vertex::new(
                VertexId::try_from_int64(id).expect("valid vertex id"),
                Tag::new(
                    TAG.to_string(),
                    [("id".into(), Value::BigInt(id))].into_iter().collect(),
                ),
            )
        })
        .collect();
    for chunk in vertices.chunks(10_000) {
        storage
            .batch_insert_vertices(SPACE, chunk.to_vec())
            .expect("insert vertices");
    }

    let mut edges = Vec::with_capacity(VERTEX_COUNT as usize * EDGES_PER_VERTEX);
    for src in 0..VERTEX_COUNT {
        for k in 1..=EDGES_PER_VERTEX {
            edges.push(Edge {
                src: VertexId::try_from_int64(src).expect("valid vertex id"),
                dst: VertexId::try_from_int64((src + k as i64) % VERTEX_COUNT)
                    .expect("valid vertex id"),
                edge_type: EDGE.to_string(),
                ranking: 0,
                props: [("weight".into(), Value::BigInt(k as i64))]
                    .into_iter()
                    .collect(),
            });
        }
    }
    for chunk in edges.chunks(50_000) {
        storage
            .batch_insert_edges(SPACE, chunk.to_vec())
            .expect("insert edges");
    }
    storage
}

fn vid(id: i64) -> VertexId {
    VertexId::try_from_int64(id).expect("valid vertex id")
}

/// The dense prefix keeps F1/F4/F5 comparable with earlier packed-read runs.
fn prefix_seeds(count: usize) -> Vec<VertexId> {
    (0..count).map(|i| vid(i as i64)).collect()
}

/// Seeds spread across the whole key space so every direction has work.
fn spread_seeds(count: usize) -> Vec<VertexId> {
    let stride = VERTEX_COUNT as usize / count;
    (0..count).map(|i| vid((i * stride) as i64)).collect()
}

fn median_us(samples: &[u128]) -> u128 {
    let mut values = samples.to_vec();
    values.sort_unstable();
    values[values.len() / 2]
}

/// Every reported figure is per measured call: median wall time, allocated
/// bytes, and the same bytes normalized by topology rows and unique dests.
fn report(label: &str, samples: Vec<u128>, bytes: u128, edges: usize, unique: usize) {
    let us = median_us(&samples);
    let bytes_per_edge = if edges == 0 { 0 } else { bytes / edges as u128 };
    let bytes_per_unique = if unique == 0 {
        0
    } else {
        bytes / unique as u128
    };
    println!(
        "{label:<30} {us:>8} us  {bytes:>10} B/call  {bytes_per_edge:>9} B/edge  {bytes_per_unique:>13} B/unique-dest  edges={edges} unique={unique}",
    );
}

fn measure<T>(mut operation: impl FnMut() -> T) -> (Vec<u128>, u128) {
    let mut samples = Vec::with_capacity(ITERATIONS);
    let mut bytes = 0u128;
    for _ in 0..ITERATIONS {
        let before = ALLOCATED_BYTES.load(Ordering::Relaxed);
        let start = Instant::now();
        let value = operation();
        samples.push(start.elapsed().as_micros());
        bytes += ALLOCATED_BYTES
            .load(Ordering::Relaxed)
            .saturating_sub(before) as u128;
        std::hint::black_box(value);
    }
    (samples, bytes / ITERATIONS as u128)
}

/// Emulate the query view: collapse repeated ids to one storage lookup, then
/// expand cached values back into input order. The split timings show whether
/// the storage read or the view assembly dominates.
fn destination_view(
    storage: &GraphStorage,
    ids: &[VertexId],
    projection: &[Arc<str>],
) -> (u64, u64, usize, usize) {
    let dedup_started = Instant::now();
    let mut unique = Vec::with_capacity(ids.len());
    let mut slots = Vec::with_capacity(ids.len());
    let mut index = HashMap::new();
    for id in ids {
        let slot = *index.entry(*id).or_insert_with(|| {
            unique.push(*id);
            unique.len() - 1
        });
        slots.push(slot);
    }
    let mut view_us = dedup_started.elapsed().as_micros() as u64;

    let read_started = Instant::now();
    let columns = storage
        .get_vertex_property_columns_batch(SPACE, TAG, &unique, projection)
        .expect("property column read");
    let read_us = read_started.elapsed().as_micros() as u64;
    let present: Vec<bool> = columns.iter().map(Option::is_some).collect();
    let missing = present.iter().filter(|found| !**found).count();
    let names = projection.len();

    let expansion_started = Instant::now();
    let mut values: Vec<Vec<Value>> = vec![Vec::with_capacity(unique.len()); names];
    for column in columns {
        let row = column.unwrap_or_else(|| vec![Value::Null(linkrs_core::NullType::Null); names]);
        for (slot, value) in row.into_iter().enumerate() {
            values[slot].push(value);
        }
    }
    let mut edge_present = vec![false; ids.len()];
    let mut edge_values: Vec<Vec<Value>> = vec![Vec::with_capacity(ids.len()); names];
    for (edge, slot) in slots.into_iter().enumerate() {
        edge_present[edge] = present[slot];
        for (column, value) in edge_values.iter_mut().zip(&values) {
            column.push(value[slot].clone());
        }
    }
    std::hint::black_box((edge_present, edge_values));
    view_us += expansion_started.elapsed().as_micros() as u64;
    (read_us, view_us, unique.len(), missing)
}

fn measure_destination_view(
    storage: &GraphStorage,
    ids: &[VertexId],
    projection: &[Arc<str>],
) -> (Vec<u128>, Vec<u128>, Vec<u128>, u128) {
    let mut totals = Vec::with_capacity(ITERATIONS);
    let mut reads = Vec::with_capacity(ITERATIONS);
    let mut views = Vec::with_capacity(ITERATIONS);
    let mut bytes = 0u128;
    for _ in 0..ITERATIONS {
        let before = ALLOCATED_BYTES.load(Ordering::Relaxed);
        let start = Instant::now();
        let (read_us, view_us, _, _) = destination_view(storage, ids, projection);
        totals.push(start.elapsed().as_micros());
        reads.push(read_us as u128);
        views.push(view_us as u128);
        bytes += ALLOCATED_BYTES
            .load(Ordering::Relaxed)
            .saturating_sub(before) as u128;
    }
    (totals, reads, views, bytes / ITERATIONS as u128)
}

fn main() {
    println!("== batch topology and destination read microbenchmark ==");
    let storage = setup();
    let edge_types = vec![EDGE.to_string()];
    let no_types: Vec<String> = Vec::new();
    let schema_filtered = storage
        .resolve_topology_schema(SPACE, &edge_types)
        .expect("filtered topology schema");
    let schema_all = storage
        .resolve_topology_schema(SPACE, &no_types)
        .expect("all topology schema");
    let projected: Vec<Arc<str>> = vec![Arc::from("id")];
    let seeds = spread_seeds(SEED_COUNT);
    let dense_seeds = prefix_seeds(SEED_COUNT);

    println!("vertices={VERTEX_COUNT}, edges/vertex={EDGES_PER_VERTEX}, iterations={ITERATIONS}");

    println!("-- F1 packed topology by parent batch --");
    for &count in &BATCH_SIZES {
        let parents = prefix_seeds(count);
        let children = count * EDGES_PER_VERTEX;
        let (samples, bytes) = measure(|| {
            storage
                .get_packed_node_edge_headers_batch(
                    SPACE,
                    &parents,
                    None,
                    EdgeDirection::Out,
                    &edge_types,
                    &schema_filtered,
                )
                .expect("packed read")
        });
        let first = storage
            .get_packed_node_edge_headers_batch(
                SPACE,
                &parents,
                None,
                EdgeDirection::Out,
                &edge_types,
                &schema_filtered,
            )
            .expect("packed read");
        assert_eq!(first.child_offsets.last().copied(), Some(children));
        report(
            &format!("F1 packed batch={count}"),
            samples,
            bytes,
            children,
            0,
        );

        let spread = spread_seeds(count);
        let (spread_samples, spread_bytes) = measure(|| {
            storage
                .get_packed_node_edge_headers_batch(
                    SPACE,
                    &spread,
                    None,
                    EdgeDirection::Out,
                    &edge_types,
                    &schema_filtered,
                )
                .expect("spread packed read")
        });
        report(
            &format!("F1 spread batch={count}"),
            spread_samples,
            spread_bytes,
            children,
            0,
        );

    }

    println!("-- F2 direction and edge-type filtering --");
    let children = SEED_COUNT * EDGES_PER_VERTEX;
    for (direction, name) in [
        (EdgeDirection::Out, "out"),
        (EdgeDirection::In, "in"),
        (EdgeDirection::Both, "both"),
    ] {
        for (types, schema, filtered) in [
            (&edge_types, &schema_filtered, "filtered"),
            (&no_types, &schema_all, "all"),
        ] {
            let (samples, bytes) = measure(|| {
                storage
                    .get_packed_node_edge_headers_batch(
                        SPACE, &seeds, None, direction, types, schema,
                    )
                    .expect("packed read")
            });
            let first = storage
                .get_packed_node_edge_headers_batch(SPACE, &seeds, None, direction, types, schema)
                .expect("packed read");
            let expected = if direction == EdgeDirection::Both {
                children * 2
            } else {
                children
            };
            assert_eq!(first.child_offsets.last().copied(), Some(expected));
            report(
                &format!("F2 {name} {filtered}"),
                samples,
                bytes,
                children,
                0,
            );
        }
    }

    println!("-- F3 destination uniqueness --");
    let packed = storage
        .get_packed_node_edge_headers_batch(
            SPACE,
            &dense_seeds,
            None,
            EdgeDirection::Out,
            &edge_types,
            &schema_filtered,
        )
        .expect("packed read");
    let mut destinations = Vec::with_capacity(packed.dst.len());
    let mut seen = HashSet::new();
    for id in &packed.dst {
        if seen.insert(*id) {
            destinations.push(*id);
        }
    }
    assert!(destinations.len() >= SEED_COUNT);
    for &rate in &UNIQUE_RATES {
        let unique_target = (SEED_COUNT * rate / 100).max(1);
        let ids: Vec<VertexId> = (0..SEED_COUNT)
            .map(|i| destinations[i % unique_target])
            .collect();
        let (samples, reads, views, bytes) = measure_destination_view(&storage, &ids, &projected);
        let unique = ids.iter().collect::<HashSet<_>>().len();
        report(
            &format!("F3 unique {rate}%"),
            samples,
            bytes,
            ids.len(),
            unique,
        );
        println!(
            "    storage_read={} us, view_assembly={} us",
            median_us(&reads),
            median_us(&views),
        );
    }

    println!("-- F4 destination read shapes --");
    let (samples, bytes) = measure(|| {
        storage
            .get_vertex_property_columns_batch(SPACE, TAG, &dense_seeds, &[])
            .expect("existence read")
    });
    report(
        "F4 existence",
        samples,
        bytes,
        dense_seeds.len(),
        dense_seeds.len(),
    );

    let (samples, bytes) = measure(|| {
        storage
            .get_vertex_property_columns_batch(SPACE, TAG, &dense_seeds, &projected)
            .expect("property column read")
    });
    report(
        "F4 property column",
        samples,
        bytes,
        dense_seeds.len(),
        dense_seeds.len(),
    );

    let (samples, bytes) = measure(|| {
        storage
            .get_vertices_batch(SPACE, TAG, &dense_seeds)
            .expect("full vertex read")
    });
    report(
        "F4 full entity",
        samples,
        bytes,
        dense_seeds.len(),
        dense_seeds.len(),
    );

    println!("-- F5 packed plus destination --");
    let dest_ids: Vec<VertexId> = packed.dst[..SEED_COUNT].to_vec();
    let (samples, bytes) = measure(|| {
        let topology = storage
            .get_packed_node_edge_headers_batch(
                SPACE,
                &dense_seeds,
                None,
                EdgeDirection::Out,
                &edge_types,
                &schema_filtered,
            )
            .expect("packed read");
        let columns = storage
            .get_vertex_property_columns_batch(SPACE, TAG, &dest_ids, &projected)
            .expect("property column read");
        (topology.child_offsets.len(), columns.len())
    });
    report(
        "F5 packed + destination",
        samples,
        bytes,
        children,
        dest_ids.len(),
    );

    println!("-- existing reference cases --");
    let (samples, bytes) = measure(|| {
        storage
            .get_node_edge_headers_batch(SPACE, &dense_seeds, None, EdgeDirection::Out, &edge_types)
            .expect("legacy header read")
    });
    report("per_parent_headers", samples, bytes, children, 0);

    let (samples, bytes) = measure(|| {
        storage
            .neighbor_dst_ids_batch(SPACE, &dense_seeds, EdgeDirection::Out, &no_types)
            .expect("neighbor read")
    });
    report("neighbor_ids (lean)", samples, bytes, children, 0);

    let (samples, bytes) = measure(|| {
        storage
            .out_degree_batch(SPACE, &dense_seeds, EdgeDirection::Out, &no_types)
            .expect("degree read")
    });
    report("out_degree (count only)", samples, bytes, children, 0);
}
