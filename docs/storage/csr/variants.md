# CSR Variants: Implementation Details

## 1. MutableCsr (Multiple Variant)

**File**: `crates/graphdb-storage/src/storage/edge/mutable_csr.rs`

### Purpose
Standard multi-edge CSR for general cases where vertices can have many outgoing edges.

### Layout

```
Memory Layout:
+-----------------------------+
|  Vertex 0  | Vertex 1 | ... |  Primary blocks (contiguous)
+-----------------------------+
         |
         +--> Overflows (append-only at end)
              +----------------+
              |  Overflow V0   |
              |  Overflow V1   |
              |  ...           |
              +----------------+
```

### Data Structures

```rust
pub struct MutableCsr {
    nbr_list: Vec<Nbr>,              // All edges, flat
    adj_offsets: Vec<u32>,           // Where each vertex's edges start
    primary_capacities: Vec<u32>,    // Preallocated size per vertex
    degrees: Vec<u32>,               // Actual edge count per vertex
    overflow_starts: Vec<u32>,       // Where overflow block begins (NO_OVERFLOW = none)
    overflow_counts: Vec<u32>,       // Edge count in overflow
    overflow_capacities: Vec<u32>,   // Overflow capacity per vertex
    edge_count: AtomicU64,           // Total active edge count
    vertex_capacity: usize,
    total_edge_capacity: usize,
}
```

### Two-Level Storage Strategy

**Primary Block**:
- Fixed pre-allocated space for each vertex (default: 4 edges)
- Located at `adj_offsets[v]` with size `primary_capacities[v]`
- Fast insertion if space available

**Overflow Block**:
- Created when primary fills up
- Appended to the end of `nbr_list`
- Grows dynamically via `expand_vertex_capacity()` (doubles capacity)

### Insertion Logic

```
insert_edge(src, dst, edge_id, prop_offset, ts):
  1. Check duplicate: neighbor + active (delete_ts == u32::MAX)
  2. If primary has space and no overflow allocated:
     -> Write to primary block
  3. Else:
     -> If no overflow or overflow full: expand_vertex_capacity()
     -> Write to overflow block
  4. Update degree counter, increment edge_count
```

### Fragmentation

**When does it occur?**
- Each `expand_vertex_capacity()` for a vertex allocates new space at the end of `nbr_list`
- Old overflow block becomes unreachable -> internal fragmentation

**Detection**:
```rust
csr.fragmentation_ratio()  // Returns nbr_list.len() / active_edges
```

### Compaction

**Operation**:
- O(V + E) time, O(E) space
- Merges primary + overflow into flat CSR
- Removes soft-deleted edges (delete_ts < u32::MAX)
- Reserves `reserve_ratio` free space for future growth per vertex

### Operations Complexity

| Operation | Complexity | Notes |
|-----------|-----------|-------|
| `insert_edge` | O(1) amortized | Spills to overflow when full |
| `delete_edge` (by ID) | O(degree) | Scans primary + overflow |
| `get_edge` | O(degree) | Scans both levels |
| `edges_of` | O(degree) | Returns all valid edges |
| `compact_with_ts` | O(V + E) | Defragments storage |

---

## 2. SingleMutableCsr (Single Variant)

**File**: `crates/graphdb-storage/src/storage/edge/single_mutable_csr.rs`

### Purpose
Optimized for one-to-one relationships where each vertex has **at most one outgoing edge**.

### Use Cases
- "Spouse" relationships
- "Current employer"
- Any strict single-edge semantic

### Layout

```
Direct array indexing:
+---+---+---+---+
| V0| V1| V2| V3|  nbr_list (one Nbr per vertex)
+---+---+---+---+
 0   1   2   3
```

### Data Structures

```rust
pub struct SingleMutableCsr {
    nbr_list: Vec<Nbr>,          // One edge per vertex (may be inactive)
    edge_count: AtomicU64,       // Count of active edges
    vertex_capacity: usize,
}
```

### Operations Complexity

| Operation | Complexity |
|-----------|-----------|
| `insert_edge` | O(1) |
| `delete_edge` | O(1) |
| `get_edge` | O(1) |
| `edges_of` | O(1) |

### Concurrency Limitation

**Critical**: This CSR does NOT support concurrent writes at the same timestamp.

**Behavior**:
- Each vertex can have at most 1 logically valid edge
- Newer timestamps **overwrite** older ones
- If two updates arrive with same/non-monotonic timestamp, the later one is **silently rejected**

**Example**:
```
T1: insert_edge(v0, v1, ts=100) succeeds
T2: insert_edge(v0, v2, ts=99)  rejected (99 < 100)
T3: insert_edge(v0, v3, ts=100) rejected (100 == 100, not >)
```

**Workarounds**:
1. Ensure timestamp monotonicity at upper layers (WAL, transaction log)
2. Use `MutableCsr` if concurrent writes needed

---

## 3. MultiSingleMutableCsr (MultiSingle Variant)

**File**: `crates/graphdb-storage/src/storage/edge/multi_single_mutable_csr.rs`

### Purpose
Each vertex has multiple edges, but limited to a fixed capacity per vertex.

### Use Case
- Memory-constrained scenarios
- Known upper bound on edges per vertex

### Layout

```
Flat array with stride = edges_per_vertex:
+---------+---------+---------+
| V0[0..N)| V1[0..N)| V2[0..N)|  ...
+---------+---------+---------+
counts = [2, 1, 3, ...]
```

### Data Structures

```rust
pub struct MultiSingleMutableCsr {
    edges: Vec<Nbr>,                 // Flat array with vertex stride
    edges_per_vertex: usize,         // Fixed capacity per vertex
    counts: Vec<u32>,               // Active edge count per vertex
    edge_count: AtomicU64,
    vertex_capacity: usize,
}
```

### Insertion Behavior
- Returns `false` if vertex's edge count reaches `edges_per_vertex`
- No overflow, strict capacity
- Updates existing edge with same dst if ts is newer

### Operations Complexity

| Operation | Complexity | Notes |
|-----------|-----------|-------|
| `insert_edge` | O(degree) | Duplicate check + linear scan |
| `delete_edge` (by ID) | O(degree) | Scans vertex block |
| `get_edge` | O(degree) | Linear scan |
| `edges_of` | O(degree) | Returns valid edges |
| `compact_with_ts` | O(degree) | Shifts within fixed blocks |

---

## 4. LabeledMutableCsr (Labeled Variant)

**File**: `crates/graphdb-storage/src/storage/edge/labeled_mutable_csr.rs`

### Purpose
Multi-label CSR where edges from the same source-destination pair may have different labels.

### Use Case
- Multi-label graphs (e.g., "friend", "colleague", "family" on same pair)
- Efficient label-filtered traversal

### Layout

```
Label-grouped storage:
+---------------------------------+
| nbr_list (flattened by label)  |
+---------------------------------+

Per-vertex mapping:
label_ranges[v] = [
  { label: 1, offset: 0, count: 3 },
  { label: 5, offset: 3, count: 2 },
  ...
]
```

### Data Structures

```rust
pub struct LabeledMutableCsr {
    nbr_list: Vec<Nbr>,                    // All edges, flat
    label_ranges: Vec<Vec<LabelRange>>,    // Label -> (offset, count) per vertex
    degrees: Vec<u32>,                     // Total edges per vertex
    edge_count: AtomicU64,
    vertex_capacity: usize,
}

struct LabelRange {
    label: LabelId,
    offset: u32,
    count: u32,
}
```

### Operations Complexity

| Operation | Complexity | Notes |
|-----------|-----------|-------|
| `insert_edge` (with label) | O(K) | K = distinct labels at vertex (linear scan, then sort) |
| `get_edge` (by label) | O(K + degree) | Linear scan on label ranges |
| `edges_of` (all) | O(degree) | Return all label groups |

Note: The basic `MutableCsrTrait::insert_edge()` (without label) assigns label 0.

---

## 5. Immutable Csr

**File**: `crates/graphdb-storage/src/storage/edge/csr.rs`

### Purpose
Read-only, compact snapshot for:
- Static analysis
- Batch-loaded data
- Persistent storage format (frozen segments)

### Layout

```
Flat CSR (no overflow, no fragmentation):
+--------------------------------+
|  All edges, contiguous          |  edges (Vec<ImmutableNbr>)
+--------------------------------+

Offset array:
+----------------+
| Offsets[V]     |  offsets (Vec<u32>)
| Last entry     |  = total edge count
+----------------+
```

### Data Structures

```rust
pub struct Csr {
    offsets: Vec<u32>,           // Where each vertex's edges start
    edges: Vec<ImmutableNbr>,   // Contiguous edge data
    edge_count: AtomicU64,
    vertex_capacity: usize,
}
```

### Key Differences from Mutable CSR

| Aspect | Mutable | Immutable |
|--------|---------|-----------|
| Storage | Vec (growable) with overflow | Vec (compact, no overflow) |
| Neighbor type | `Nbr` (with delete_ts) | `ImmutableNbr` (single timestamp) |
| Fragmentation | Yes (overflow blocks) | No (flat layout) |
| Mutations | `insert_edge`, `delete_edge` | Build-only (`batch_put_edges_with_timestamps`) |
| Memory | Higher (capacity > usage) | Lower (capacity == usage) |
| Lookup | O(degree) with timestamp | O(degree) snapshot |

### Construction

```rust
// From mutable entries
let csr = Csr::from_nbr_entries(&entries, vertex_capacity);

// With batch builder
csr.batch_put_edges_with_timestamps(
    &src_list, &dst_list, &edge_ids, &prop_offsets, &timestamps,
);
```

### Operations

| Operation | Behavior |
|-----------|----------|
| `get_edge` | Direct array lookup, no timestamp filtering |
| `edges_of` | Returns all edges slice, O(degree) |
| `dump` / `load` | Serialization of flat layout |
| `insert_edge` | Not available (read-only) |

---

## 6. None (Placeholder Variant)

**File**: `crates/graphdb-storage/src/storage/edge/csr_variant.rs`

### Purpose
Placeholder for relationships with **no edges stored**.

### Data Structure

```rust
None { vertex_capacity: usize }  // Only stores capacity, no edges
```

### Behavior

| Operation | Result |
|-----------|--------|
| `edge_count()` | 0 |
| `insert_edge()` | `false` (rejected) |
| `delete_edge()` | `false` (rejected) |
| `get_edge()` | `None` |
| `edges_of()` | Empty vec |
| `iter()` | Empty iterator |
| Memory | `sizeof(usize)` |

### Serialization

```
dump(): [0u8, vertex_capacity (8 bytes)] // Tag 0 = None
load(): Deserializes vertex_capacity, recreates None variant
```

---

## Trait Implementation Matrix

| Trait Method | Multiple | Single | MultiSingle | Labeled | Immutable (Csr) | None |
|--------------|----------|--------|-------------|---------|-----------------|------|
| `vertex_capacity` | Yes | Yes | Yes | Yes | Yes | Yes |
| `edge_count` | Yes | Yes | Yes | Yes | Yes | Yes (0) |
| `dump` / `load` | Yes | Yes | Yes | Yes | Yes | Yes |
| `insert_edge` | Yes | Yes | Yes | Yes | N/A | No |
| `delete_edge` | Yes | Yes | Yes | Yes | N/A | No |
| `get_edge` | Yes | Yes | Yes | Yes | Yes | No |
| `edges_of` | Yes | Yes | Yes | Yes | Yes | Yes (empty) |
| `compact_with_ts` | Yes | No-op | Yes | Yes | N/A | No-op |
| `used_memory_size` | Yes | Yes | Yes | Yes | Yes | Yes |

---

## Comparison: When to Use Which

| Scenario | Variant | Reason |
|----------|---------|--------|
| "Friends" (multi-edge, general) | `Multiple` | Default, handles any case |
| "Spouse" (one-to-one) | `Single` | O(1) access, memory efficient |
| "Followers" (bounded multi-edge, ~1K per vertex) | `MultiSingle` | Fixed memory, predictable layout |
| "Collaborates on project/paper/team" (multi-label) | `Labeled` | Efficient label filtering |
| Analytical snapshot, batch-loaded data | `Immutable` (Csr) | Flat, compact, read-only |
| Schema exists but no actual edges stored | `None` | Zero overhead |
