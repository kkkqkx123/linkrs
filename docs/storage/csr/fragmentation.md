# Fragmentation & Compaction in MutableCsr

## Problem: Why Fragmentation?

### Two-Level Storage Design

MutableCsr uses a two-level approach to avoid O(n) reshuffling:

```
Initial State (primary blocks contiguous):
+---------------------------------+
|V0: [E0, E1] | V1: [E2] | V2: []|  Primary blocks
+---------------------------------+

After V0 fills up (overflow allocated):
+---------------------------------+
|V0: [E0, E1] | V1: [E2] | V2: []|  Primary
+---------------------------------+
                    +---> [E3] (overflow appended)

After V0 overflows again (re-expansion):
+---------------------------------+
|V0: [E0, E1] | V1: [E2] | V2: []|  Primary (unchanged)
+---------------------------------+
     +--------------------> [E3] <- zombie (unreachable)
                    +---> [E3, E4, E5] (new overflow)
```

### Root Cause

Each vertex expansion allocates new space at **end of `nbr_list`**:
1. Old overflow block address becomes unreachable
2. `overflow_starts[v]` updated to new location
3. Old space never reclaimed -> internal fragmentation

### Cumulative Effect

After many vertex expansions:
- `nbr_list` contains both live and dead edges
- Serialization dumps **entire** list including zombie blocks
- Queries unaffected (always use current `overflow_starts` pointer)
- Memory wasted but correctness preserved

---

## Measuring Fragmentation

### Fragmentation Ratio

**Definition**:
```
fragmentation_ratio = nbr_list.len() / active_edges
```

**Examples**:
- `1.0`: No wasted space (perfectly packed)
- `2.0`: 50% wasted (2x the space needed)
- `5.0`: 80% wasted (very fragmented)

**Location**: `MutableCsr::fragmentation_ratio()`

```rust
pub fn fragmentation_ratio(&self) -> f32 {
    let active_edges = self.edge_count.load(Ordering::Relaxed) as usize;
    if active_edges == 0 {
        return 0.0;
    }
    self.nbr_list.len() as f32 / active_edges as f32
}
```

### Diagnostics

```rust
let ratio = csr.fragmentation_ratio();
if ratio > 2.5 {
    println!("High fragmentation: {:.2}x", ratio);
}
```

---

## Compaction: Recovery

### Purpose

Merge primary + overflow blocks into **flat CSR** layout:
- Removes all zombie blocks
- Removes soft-deleted edges (where `delete_ts < u32::MAX`)
- Restores `fragmentation_ratio()` to ~1.0 + reserve
- Reduces serialization size

### Method Signature

```rust
pub fn compact_with_ts(&mut self, _ts: u32, reserve_ratio: f32) -> usize
```

**Parameters**:
- `_ts`: Reserved for future use (currently **ignored** by MutableCsr). All edges with `delete_ts == u32::MAX` are kept regardless of ts.
- `reserve_ratio`: Reserve fraction for future growth
  - `0.25` = reserve 25% extra capacity
  - Reduces need for immediate re-expansion

**Returns**: Number of edges removed (soft-deleted)

### Algorithm

```
compact_with_ts(ts, reserve_ratio):
  1. Allocate new edge list
  2. For each vertex v:
     a. Iterate primary block [offset[v], offset[v] + degree[v])
     b. Iterate overflow block (if exists)
     c. Filter: keep only edges where delete_ts == u32::MAX (active)
     d. Append valid edges to new list
  3. Build new offsets, degrees, capacities with reserve
  4. Clear all overflow pointers
  5. Replace nbr_list with compacted version
  6. Return count of removed edges
```

### Complexity

- **Time**: O(V + E) — visit all vertices and edges once
- **Space**: O(E) — allocate new edge list
- **Lock**: Exclusive write access required (not concurrent)

### Example

```rust
// Before compaction
let ratio = csr.fragmentation_ratio();  // 3.2x

// Compact: remove deleted edges, keep 25% reserve
let removed = csr.compact_with_ts(1000, 0.25);
println!("Removed {} edges", removed);

// After compaction
let ratio = csr.fragmentation_ratio();  // ~1.25x (25% reserve)
```

---

## When to Compact

### Using fragmentation_ratio()

```rust
// Check and compact if needed
if csr.fragmentation_ratio() > 2.5 {
    csr.compact_with_ts(current_ts, 0.25);
}

// Before persistent snapshot
if csr.fragmentation_ratio() > 1.5 {
    csr.compact_with_ts(snapshot_ts, 0.1);
}
```

### Scenarios

| Scenario | When | Action |
|----------|------|--------|
| High-throughput writes | Rare | Monitor ratio, compact during off-peak |
| Batch deletion | After massive delete | Compact to reclaim space |
| Before serialization | Snapshot time | Compact to reduce disk size |
| Periodic maintenance | Scheduled task | e.g., hourly if ratio > 3.0 |
| Memory pressure | OOM near threshold | Emergency compact |

---

## Trade-offs

### Costs of NOT Compacting

| Impact | Effect |
|--------|--------|
| Disk usage | Serialized snapshots bloated by 2-5x |
| Network | Large transfers of fragmented CSR |
| Cache efficiency | Dead edges waste CPU cache lines |
| Query latency | Slight overhead scanning dead blocks |

### Costs of Compacting

| Impact | Effect |
|--------|--------|
| CPU time | O(V + E) full scan and rewrite |
| Lock duration | Exclusive write access (blocks other writers) |
| Memory peak | Temporarily 2x space during rewrite |
| Latency spike | Queries blocked during compaction |

### Recommendation

- **High-concurrency OLTP**: Rarely compact (throughput cost too high)
- **OLAP / Analytics**: Compact before snapshot export (size matters)
- **Batch loads**: Compact after bulk insertions (avoid initial overflow)
- **Retention-heavy workloads**: Compact monthly if soft-delete ratio > 50%

---

## Compaction in Other Variants

### MutableCsr (Multiple)
- Full compaction supported
- Removes soft-deleted edges (delete_ts < u32::MAX)
- Merges overflow blocks into flat layout

### SingleMutableCsr
- No-op (returns 0)
- Rationale: O(1) direct access, no overflow, no fragmentation

### MultiSingleMutableCsr
- Removes tombstoned edges by shifting valid edges within fixed blocks
- Does not change block capacity (max_edges_per_vertex is fixed)

### LabeledMutableCsr
- Full compaction via `Vec::retain()` on nbr_list
- Rebuilds label ranges after compaction (partial: labels reset to 0)

### Immutable Csr
- No-op (read-only snapshot already flat)
- Rationale: Immutable data, no mutations possible

### None
- No-op (zero edges)

---

## Soft-Delete Semantics

### Create & Delete Timestamps

```rust
pub struct Nbr {
    pub neighbor: VertexId,
    pub edge_id: EdgeId,
    pub prop_offset: u32,
    pub create_ts: Timestamp,      // When added
    pub delete_ts: Timestamp,      // When soft-deleted (u32::MAX = active)
}
```

### Visibility Window

Edge is visible at timestamp `T` if:
```rust
create_ts <= T && T < delete_ts
```

### Soft-Delete Process

1. **Delete operation**: Set `delete_ts = current_ts`
2. **Query**: Filters out edges where `delete_ts <= query_ts`
3. **Compaction**: Removes edges where `delete_ts < u32::MAX` (hard-delete)

**Benefits**:
- Fast deletion (no reallocation)
- MVCC support (multiple snapshots see different state)
- Time-travel queries (query past state)
- Undo capability (revert can reset `delete_ts`)

---

## Serialization & Fragmentation

### dump() includes fragmentation

```rust
fn dump(&self) -> Vec<u8> {
    // Serializes entire nbr_list including zombie blocks!
    // Vertex capacities, overflow metadata all preserved
}
```

**Impact**:
- Fragmented CSR serializes at 2-5x size
- Network transfer cost increases
- Disk storage inflated

**Mitigation**:
- Compact before serialization if `ratio > 1.5`

### load() reconstructs fragmented state

Deserializes the exact fragmentation state from the snapshot. After loading, consider checking `fragmentation_ratio()` and compacting if > 2.0.

---

## Future Optimizations

### Lazy Compaction
- Mark blocks for compaction but defer actual work
- Batch compactions during idle time

### Incremental Compaction
- Compact one vertex at a time
- Amortize O(V + E) cost over many operations

### Adaptive Thresholds
- Monitor workload patterns
- Auto-tune compaction threshold
- Trigger early if write rate drops
