# Fragmentation & Compaction in MutableCsr

## Problem: Why Fragmentation?

### Two-Level Storage Design

MutableCsr uses a two-level approach to avoid O(n) reshuffling:

```
Initial State (primary rows reserve gaps for everyday writes):
+---------------------------------+
|V0: [E0, E1] | V1: [E2] | V2: []|  Primary rows
+---------------------------------+

After V0 fills up (overflow chunks appended per vertex):
+---------------------------------+
|V0: [E0, E1] | V1: [E2] | V2: []|  Primary (unchanged)
+---------------------------------+
     +---> [E3, ...] chunk 1
     +---> [E7, ...] chunk 2        graded overflow blocks

Past a per-vertex chunk bound a row holding dead entries is repacked on
the write path; removal detaches emptied chunks immediately.
```

### Root Cause

Waste accumulates from two sources, not from unreachable blocks:
1. Primary rows reserve gaps up to the packed density target; unfilled gap
   slots stay unused until everyday writes or a merge consumes them.
2. Deleted entries remain physically present until the collection cutoff
   passes, so their slots count as waste in the meantime.

High-degree vertices spill into graded overflow chunks. Empty chunks are
detached immediately on the removal path, so no unreachable-block accounting
remains in the structure.

### Cumulative Effect

Over time:
- Tombstone slots and unfilled gaps dominate `wasted_capacity`
- Whole-table ratio is an observation metric only; collection triggers use
  per-vertex reclaimable counts
- Group merges tighten live entries and restore the density-target reserve

---

## Measuring Fragmentation

### Fragmentation Ratio

**Definition** (single wasted-share caliber shared by all triggers and panels):
```
fragmentation_ratio = wasted_capacity / total_capacity
```

**Examples**:
- `0.0`: No wasted space (perfectly packed)
- `0.5`: 50% wasted (group merge gate level)
- `1.0`: All reserved capacity is waste

**Location**: `MutableCsr::fragmentation_ratio()`

```rust
pub fn fragmentation_ratio(&self) -> f32 {
    let active_edges = self.edge_count.load(Ordering::Relaxed) as usize;
    if self.total_edge_capacity == 0 {
        return 0.0;
    }
    self.total_edge_capacity.saturating_sub(active_edges) as f32
        / self.total_edge_capacity as f32
}
```

### Diagnostics

```rust
let ratio = csr.fragmentation_ratio();
if ratio >= GROUP_FRAGMENTATION_THRESHOLD {
    println!("High fragmentation: {:.2}", ratio);
}
```

---

## Compaction: Recovery

### Purpose

Merge primary + overflow blocks into **flat CSR** layout:
- Removes all zombie blocks
- Removes soft-deleted edges (where `delete_ts < u32::MAX`)
- Restores `fragmentation_ratio()` to ~0.0 + reserve
- Reduces serialization size

### Method Signature

```rust
pub fn compact_with_ts_reporting(
    &mut self,
    cutoff: Timestamp,
    reserve_ratio: f32,
    on_edge_removed: &mut dyn FnMut(EdgeId, Timestamp),
) -> usize
```

**Parameters**:
- `cutoff`: GC watermark. Tombstones with `delete_ts < cutoff` are dropped and
  reported; tombstones at or above the cutoff are kept so snapshot history
  survives. `Timestamp::MAX` means "move only, discard nothing".
- `reserve_ratio`: Reserve fraction for future growth
  - `0.25` = reserve 25% extra capacity
  - Reduces need for immediate re-expansion
- `on_edge_removed`: one call per discarded tombstone carrying `(edge_id,
  delete_ts)` so the caller promotes the deletion centrally.

**Returns**: Number of edges removed (reclaimable soft-deleted)

### Algorithm

```
compact_with_ts_reporting(cutoff, reserve_ratio, on_edge_removed):
  1. Allocate new edge list
  2. For each vertex v:
     a. Iterate primary block [offset[v], offset[v] + degree[v])
     b. Iterate overflow blocks (if any)
     c. Keep live entries and tombstones at or above the cutoff
     d. Drop entries below the cutoff and report each one
     e. Append kept entries to new list
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
let ratio = csr.fragmentation_ratio();  // 0.8

// Compact: drop reclaimable tombstones below the cutoff, keep 25% reserve
let removed = csr.compact_with_ts_reporting(500, 0.25, &mut |edge_id, ts| {
    println!("promoted tombstone {:?} at {}", edge_id, ts);
});

// After compaction
let ratio = csr.fragmentation_ratio();  // ~0.2 (25% reserve)
```

---

## When to Compact

### Using fragmentation_ratio()

```rust
// Check and compact if needed
if csr.fragmentation_ratio() >= GROUP_FRAGMENTATION_THRESHOLD {
    csr.compact_with_ts(current_ts, 0.25);
}

// Before persistent snapshot
if csr.fragmentation_ratio() >= GROUP_FRAGMENTATION_THRESHOLD {
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

## Compaction in Current Variants

### MutableCsr (Multiple)
- Full compaction supported
- Drops tombstones below the cutoff and reports each removal
- Merges primary + overflow into flat layout with density-target reserve

### SingleMutableCsr
- Drops the single tombstone entry once it is below the cutoff
- Rationale: O(1) direct access, one slot per vertex, no overflow

### None
- No-op (zero edges)

---

## Soft-Delete Semantics

### Create & Delete Timestamps

```rust
pub struct Nbr {
    pub endpoint: u32,
    pub rank: i64,
    pub edge_id: EdgeId,
    pub create_ts: Timestamp,      // When added
    pub delete_ts: Timestamp,      // When soft-deleted (MAX = alive)
}
```

Row stamps are physical projections for collection only; query visibility is
decided by the version authority above this layer.

### Visibility Window

Edge is visible at timestamp `T` if:
```rust
create_ts <= T && T < delete_ts
```

### Soft-Delete Process

1. **Delete operation**: Set `delete_ts = current_ts`
2. **Query**: Filters out edges where `delete_ts <= query_ts`
3. **Collection**: Drops edges whose `delete_ts` is below the GC cutoff,
   reporting each removal so the tombstone layer learns it

**Benefits**:
- Fast deletion (no reallocation)
- MVCC support (multiple snapshots see different state)
- Time-travel queries (query past state)
- Undo capability (revert can reset `delete_ts`)

---

## Serialization & Fragmentation

### dump() persists per-group columnar payloads

```rust
fn dump_into(&self, out: &mut Vec<u8>) {
    // Borrows the live topology and encodes column by column, so the peak
    // stays at one materialized column plus the output buffer.
}
```

**Impact**:
- Fragmented groups persist their gaps until merged
- Per-group merge before checkpoint keeps payloads tight

**Mitigation**:
- Compact before serialization if `ratio >= GROUP_FRAGMENTATION_THRESHOLD`

### load() reconstructs fragmented state

Deserializes the exact fragmentation state from the snapshot. After loading, consider checking `fragmentation_ratio()` and compacting if above the group gate.

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
