# CSR Quick Reference

## Variant Selection Guide

```
Do you have a one-to-one relationship (spouse, current_employer)?
  YES -> Use SingleMutableCsr (O(1) access, memory efficient)
         WARNING: Requires monotonic timestamp ordering
  NO  -> Continue

Do you have multi-label edges (same src->dst with different types)?
  YES -> Use LabeledMutableCsr (O(log K) label lookup)
  NO  -> Continue

Do you need a general multi-edge relationship (friends, follows)?
  YES -> Use MutableCsr (default, most flexible)
  NO  -> Continue

Do you have a known bounded edges per vertex (< 1K)?
  YES -> Use MultiSingleMutableCsr
  NO  -> Continue

Do you store any edges at all?
  NO  -> Use None (placeholder, zero memory)
  YES -> Already covered above

// Special case:
// - Read-only snapshot / batch-loaded data? -> Immutable Csr (not in CsrVariant)
```

## Code Examples

### Creating CSR from Strategy

```rust
use crate::storage::edge::{CsrVariant, EdgeStrategy};

// Create Multiple variant (general multi-edge)
let csr = CsrVariant::from_strategy(
    EdgeStrategy::Multiple,
    1000,      // vertex capacity
    10000,     // edge capacity
)?;

// Create Single variant (one-to-one)
let csr = CsrVariant::from_strategy(
    EdgeStrategy::Single,
    1000,      // ignored for Single
    10000,
)?;

// Create MultiSingle variant (bounded capacity)
let csr = CsrVariant::from_strategy(
    EdgeStrategy::MultiSingle { max_edges: 4 },
    1000, 10000,
)?;

// Create Labeled variant (multi-label)
let csr = CsrVariant::from_strategy(
    EdgeStrategy::Labeled,
    1000, 10000,
)?;

// Create None variant (placeholder)
let csr = CsrVariant::from_strategy(
    EdgeStrategy::None,
    1000, 10000,  // ignored for None
)?;
```

### Insert & Query

```rust
use crate::storage::edge::{MutableCsrTrait, EdgeId, VertexId};

let mut csr = /* ... */;

// Insert edge
let success = csr.insert_edge(
    0u32,                           // source vertex ID
    VertexId::from_int64(42),      // destination vertex ID
    EdgeId(100),                    // edge ID
    0u32,                           // property offset
    5,                              // timestamp
);

// Query single edge
let edge = csr.get_edge(0, VertexId::from_int64(42), 5);
match edge {
    Some(nbr) => println!("Found: {:?}", nbr.edge_id),
    None => println!("Not found"),
}

// Query all neighbors
let neighbors = csr.edges_of(0, 5);  // All edges from vertex 0 at ts=5
for nbr in neighbors {
    println!("Neighbor: {:?}", nbr.neighbor);
}
```

### Delete & Revert

```rust
// Delete by edge ID
csr.delete_edge(0u32, EdgeId(100), 5);

// Delete all edges to destination
csr.delete_edge_by_dst(0u32, VertexId::from_int64(42), 5);

// Delete by position in adjacency list
csr.delete_edge_by_offset(0u32, 0, 5);  // Delete 1st edge

// Undo deletion
csr.revert_delete_by_offset(0u32, 0, 5);
```

### Compaction & Maintenance

```rust
// Check fragmentation
let ratio = csr.fragmentation_ratio();
println!("Fragmentation: {:.2}x", ratio);

// Manual compact (Multiple variant only does real compaction)
let removed = csr.compact_with_ts(5, 0.25);
println!("Removed {} edges", removed);
```

### Iteration

```rust
let mut iter = csr.iter(5);  // Timestamp-aware iterator
while let Some((vertex_id, nbr)) = iter.next() {
    println!("Vertex {}: neighbor {:?}", vertex_id, nbr.neighbor);
}
```

### Serialization

```rust
// Dump to bytes
let data = csr.dump();

// Load from bytes
let mut csr2 = CsrVariant::from_strategy(EdgeStrategy::Multiple, 1000, 10000)?;
csr2.load(&data)?;

// Note: Fragmented CSR will deserialize fragmented
// Consider compact() after load() if needed
```

## Data Structures Reference

### Nbr (Neighbor)
```rust
pub struct Nbr {
    pub neighbor: VertexId,        // Target vertex
    pub edge_id: EdgeId,           // Edge identifier
    pub prop_offset: u32,          // Property storage offset
    pub create_ts: Timestamp,      // Creation timestamp
    pub delete_ts: Timestamp,      // Deletion timestamp (u32::MAX = active)
}

impl Nbr {
    pub fn is_valid_at(&self, ts: Timestamp) -> bool {
        self.create_ts <= ts && ts < self.delete_ts
    }
}
```

### ImmutableNbr (for Immutable Csr)
```rust
pub struct ImmutableNbr {
    pub neighbor: VertexId,
    pub edge_id: EdgeId,
    pub prop_offset: u32,
    pub timestamp: Timestamp,      // Single fixed timestamp
}
```

### EdgeSchema
```rust
pub struct EdgeSchema {
    pub label_id: LabelId,
    pub label_name: String,
    pub src_label: LabelId,
    pub dst_label: LabelId,
    pub properties: Vec<StoragePropertyDef>,
    pub oe_strategy: EdgeStrategy,  // Outgoing direction CSR
    pub ie_strategy: EdgeStrategy,  // Incoming direction CSR
}
```

### NbrWithoutEdgeId
```rust
pub struct NbrWithoutEdgeId {
    pub neighbor: VertexId,
    pub prop_offset: u32,
    pub create_ts: Timestamp,
    pub delete_ts: Timestamp,
}
```

## Trait Reference

### CsrBase (All variants)
```rust
pub trait CsrBase: Debug + Send + Sync {
    fn vertex_capacity(&self) -> usize;
    fn edge_count(&self) -> u64;
    fn dump(&self) -> Vec<u8>;
    fn load(&mut self, data: &[u8]) -> StorageResult<()>;
}
```

### MutableCsrTrait (Mutable variants)
```rust
pub trait MutableCsrTrait: CsrBase {
    fn insert_edge(...) -> bool;
    fn delete_edge(...) -> bool;
    fn delete_edge_by_dst(...) -> bool;
    fn delete_edge_by_offset(...) -> bool;
    fn revert_delete_by_offset(...) -> bool;
    fn get_edge(...) -> Option<Nbr>;
    fn edges_of(...) -> Vec<Nbr>;
    fn compact_with_ts(...) -> usize { 0 }  // default no-op
    fn used_memory_size(&self) -> usize;
}
```

## Files & Locations

| File | Contains |
|------|----------|
| `csr_variant.rs` | CsrVariant enum, dispatch macros, serialization tags |
| `csr_trait.rs` | CsrBase, MutableCsrTrait trait definitions |
| `mutable_csr.rs` | Multiple variant (two-level with overflow) |
| `single_mutable_csr.rs` | Single variant (O(1) direct array) |
| `multi_single_mutable_csr.rs` | MultiSingle variant (bounded slots) |
| `labeled_mutable_csr.rs` | Labeled variant (label-grouped edges) |
| `csr.rs` | Immutable variant (flat, read-only) |
| `fragmentation_stats.rs` | Metrics collection |
| `edge_table/mod.rs` | EdgeTable (out_csr + in_csr + segments + properties) |
| `property_table.rs` | Edge property storage |
| `bloom_filter.rs` | Bloom filter |

## Common Pitfalls

### 1. Forgetting Timestamp Filtering
```rust
// Wrong: May return deleted edges
let edges = csr.edges_of(0, u32::MAX);

// Correct: Respects soft-delete
let edges = csr.edges_of(0, current_ts);
```

### 2. Ignoring SingleMutableCsr Concurrency Limitation
```rust
// Wrong: Non-monotonic timestamps silently rejected
insert_edge(v, dst1, ts=100);
insert_edge(v, dst2, ts=99);   // Silently rejected!

// Correct: Ensure monotonic ordering or use MutableCsr
```

### 3. Not Compacting High-Fragmentation CSRs
```rust
// Wrong: Serialization bloats to 5x size
let data = csr.dump();  // If ratio > 2.0

// Correct: Compact before serialization
if csr.fragmentation_ratio() > 1.5 {
    csr.compact_with_ts(ts, 0.25);
}
let data = csr.dump();
```

### 4. Forgetting Offset is 0-indexed
```rust
// Wrong: Offset 1 means 2nd edge, not 1st
csr.delete_edge_by_offset(0, 1, ts);  // Deletes 2nd edge!

// Correct: Offset 0 = 1st edge
csr.delete_edge_by_offset(0, 0, ts);  // Deletes 1st edge
```

### 5. MultiSingle Capacity Limit
```rust
// Wrong: Assuming unlimited edges per vertex
for i in 0..100 {
    csr.insert_edge(0, dsts[i], ids[i], 0, 1);  // May fail silently
}

// Correct: Check return value and plan around max_edges
```

## Performance Characteristics

### Lookup Complexity

| Operation | Multiple | Single | MultiSingle | Labeled | Immutable (Csr) |
|-----------|----------|--------|-------------|---------|-----------------|
| `get_edge` | O(degree) | O(1) | O(degree) | O(K + degree) | O(degree) |
| `edges_of` | O(degree) | O(1) | O(degree) | O(degree) | O(degree) |
| `insert_edge` | O(1)* | O(1) | O(degree) | O(K) | N/A |
| `delete_edge` | O(degree) | O(1) | O(degree) | O(N) | N/A |
| `compact_with_ts` | O(V+E) | N/A | O(V*K) | O(N) | N/A |

*MutableCsr: O(1) amortized, worst-case O(degree) when expanding overflow

### Memory Characteristics

| Variant | Space | Fragmentation |
|---------|-------|----------------|
| Multiple | O(E + V) | Yes (overflow blocks) |
| Single | O(V) | No |
| MultiSingle | O(V * K) | No (fixed blocks) |
| Labeled | O(E + V*K) | Minimal |
| Immutable (Csr) | O(E + V) | No (flat) |
| None | O(1) | No |

## Testing

### Running Tests

```bash
# Test CSR variants
cargo test --lib storage::edge::csr_variant -- --nocapture

# Test all edge module
cargo test --lib storage::edge -- --nocapture

# Test with output
cargo test --lib -- --nocapture --test-threads=1
```

## Documentation

- [Overview](overview.md) - High-level CSR architecture
- [Variants](variants.md) - Deep dive into each CSR implementation
- [Dispatch](dispatch.md) - Runtime selection & polymorphism
- [Fragmentation](fragmentation.md) - Memory management & compaction
- **[Quick Reference](quick_reference.md)** - This file
