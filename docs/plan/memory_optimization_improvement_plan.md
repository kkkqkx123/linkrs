# linkrs Memory Optimization: Gap Analysis & Phased Improvement Plan

## 1. Current Architecture Gap Analysis

### 1.1 What linkrs Already Does Well

| Capability | Implementation | Assessment |
|-----------|---------------|------------|
| **Memory budget & accounting** | `MemoryAccounting` with 5 categories + atomic CAS reservation (`resource_budget.rs:163-331`) | Production-grade, well-structured |
| **MVCC / snapshots** | `TieredTombstoneManager` hot/cold layers + `MVCCTable` trait (`mvcc.rs:76-222`) | More sophisticated than Ladybug's version counter |
| **Column compression** | Dictionary/RLE/BitPack/FSST/ALP with `CompressionSelector` (`encoding/mod.rs:63-171`) | Richer algorithm set than Ladybug |
| **Block compression** | zstd on flush (storage architecture refactoring Phase 4) | Ladybug lacks this |
| **Arena allocation** | Bumpalo-based `Arena` + `ArenaPool` (`utils/arena.rs:79-132`) | Good for short-lived query data |
| **Background freeze** | `BackgroundFreezeManager` with Conservative/Adaptive/LSM strategies (`background_freeze.rs:170-255`) | Unique to linkrs |
| **Cache eviction** | Moka weight-based with `EvictionCallback` (`record_cache.rs:21-325`) | More explicit than Ladybug |

### 1.2 Critical Gaps (Prioritized)

#### Gap A: No Page-Level Memory Control

**Current state**: Data lives in plain `Vec`/`HashMap`/`CsrSegment` structures allocated on the heap. There is no abstraction that maps logical data units to OS pages. When memory pressure occurs, the only lever is Moka cache eviction — there is no mechanism to reclaim physical memory from the bulk of the data (vertex tables, edge segments, column stores).

**Impact**: Under memory pressure, the process either:
- Rejects requests via `MemoryAccounting::try_reserve` hard-limit error (`resource_budget.rs:217-268`)
- Gets OOM-killed by the OS when heap grows beyond available RAM

**Ladybug equivalent**: VMRegion with `MAP_NORESERVE` + `MADV_DONTNEED` provides fine-grained physical memory reclamation at OS page granularity.

#### Gap B: No Disk Spill Mechanism

**Current state**: When `MemoryAccounting` hits the hard limit, new allocations fail with `CapacityExceeded` (`resource_budget.rs:225-233`). There is no fallback path that writes cold data to disk temporarily.

**Impact**: Large analytical queries that scan cold edges/vertices cannot proceed once the memory ceiling is reached. The system degrades abruptly rather than gracefully.

**Ladybug equivalent**: Spiller writes cold `ChunkedNodeGroup` data to temporary files when evictable memory drops below 50% of the pool.

#### Gap C: No Page State Machine for Concurrent Access

**Current state**: Concurrent read/write coordination relies on `RwLock<HashMap<...>>` at the table level (`GraphStorageContext`). There is no per-page lock state tracking, no optimistic read, no version-based conflict detection for in-memory data access.

**Impact**: Read operations block on write locks even when accessing disjoint data within the same table. Write serialization limits throughput in mixed workloads.

**Ladybug equivalent**: `PageState` atomic CAS state machine (LOCKED/UNLOCKED/MARKED/EVICTED) with version-based optimistic reads.

#### Gap D: No LSM Segment Memory Reclaim

**Current state**: After `BackgroundFreezeManager` freezes delta CSR to immutable segments, and after LSM merge creates larger segments, the resulting segments remain fully resident in memory. There is no tiered residency — hot segments and cold segments consume the same physical memory.

**Impact**: On graphs with billions of edges, the cumulative size of merged segments can exceed available RAM even though only a fraction is actively queried.

**Ladybug equivalent**: VMRegion's `MADV_DONTNEED` discards cold pages from physical memory while preserving the virtual address mapping for transparent reload.

#### Gap E: Cache/Memory Accounting Decoupling

**Current state**: `CacheManager::refresh_memory_usage()` (`cache_manager.rs:45-52`) reports Moka's weighted size to `MemoryAccounting::report_usage`. However, this is a snapshot report, not a real-time synchronized accounting. When Moka evicts entries, the `EvictionCallback` only updates stats — it does not call `MemoryAccounting::release()`.

**Impact**: Memory accounting may over-report cache usage between refresh intervals, causing false pressure detection.

---

## 2. Phased Improvement Plan

### Phase 1: Memory-Aware Segment Tiering (High Impact, Low Risk)

**Timeline**: ~2 weeks | **Effort**: ~600 LOC

**Goal**: Introduce the concept of "residency tiers" for LSM segments so that cold segment data can be evicted from physical memory under pressure, with transparent reload from disk.

**Rationale**: Segments are the natural unit of memory management in linkrs's LSM architecture. Unlike introducing a full page table (which would require redesigning every data structure), segment tiering builds on existing abstractions (`CsrSegment`, freeze/merge, persistence).

#### 1.1 New Types

```rust
// crates/graphdb-storage/src/storage/edge/edge_table/residency.rs

/// Residency state of a segment's data in physical memory.
pub enum SegmentResidency {
    /// Data is resident and directly accessible
    Resident,
    /// Data has been evicted; backing file offset is recorded for reload
    Evicted { file_offset: u64, file_len: u64 },
}

/// Per-segment memory tracking.
pub struct SegmentResidencyEntry {
    residency: RwLock<SegmentResidency>,
    estimated_memory: usize,
    last_access: AtomicU64,  // monotonic counter for LRU ordering
}
```

#### 1.2 Integration Points

- **`CsrSegment`**: Add `residency: SegmentResidencyEntry` field. When data is accessed, check residency — if `Evicted`, reload from the segment's persisted file.
- **`merge_selected_segments_with_deletion_filter`** (`merge.rs:24-132`): After merge, mark source segments as eligible for eviction (their data is now redundant).
- **`GraphStorageContext::trigger_background_freeze`**: After freeze, the new segment starts as `Resident`. Older merged segments become eviction candidates.

#### 1.3 Eviction Policy

- When `MemoryAccounting` detects soft-limit crossing, trigger segment eviction.
- Evict in LRU order (by `last_access` counter), skipping segments pinned by active queries.
- For evicted segments: write data to a temporary spill file (reusing existing flush infrastructure), record offset/len, then `madvise(MADV_DONTNEED)` on the backing memory pages.

#### 1.4 Reload Mechanism

- On segment access in `Evicted` state: read from spill file into a new buffer, update residency to `Resident`.
- Reload is atomic at the segment level — concurrent readers of an evicted segment coordinate via `RwLock`.

---

### Phase 2: Disk Spiller for Query Scratch Space (High Impact, Medium Risk)

**Timeline**: ~2 weeks | **Effort**: ~500 LOC

**Goal**: When memory pressure exceeds the soft limit during large query execution, spill intermediate results (sorted edge lists, property batches, join buffers) to temporary files instead of failing.

**Rationale**: The current hard-stop behavior (`CapacityExceeded`) is acceptable for OLTP but catastrophic for OLAP. A spill mechanism enables graceful degradation.

#### 2.1 New Module

```rust
// crates/graphdb-storage/src/storage/engine/spiller.rs

/// Spill manager for query scratch data.
pub struct Spiller {
    spill_dir: PathBuf,
    accounting: Arc<MemoryAccounting>,
    active_spills: RwLock<Vec<SpillFile>>,
    /// Soft ratio at which spill triggers (configurable)
    spill_threshold_ratio: f64,
}

struct SpillFile {
    path: PathBuf,
    /// Original memory category this data belonged to
    category: MemoryCategory,
    /// Bytes that were reserved before spill
    spilled_bytes: u64,
}
```

#### 2.2 Integration with MemoryAccounting

- Add `try_reserve_with_spill` method that, on hard-limit failure, invokes `spiller.spill_cold_data()` to free memory, then retries reservation.
- Spill candidates: cold LSM segments (Phase 1), cached vertex data (force-evict from Moka), frozen-but-unmerged delta fragments.

#### 2.3 Spill Data Flow

1. detect pressure (`MemoryAccounting` soft limit crossed)
2. select coldest spill candidate (segments first — they have natural serialization)
3. serialize to temp file (reusing existing column encoding → file format)
4. release memory (`MADV_DONTNEED` on segment backing store)
5. record spill metadata
6. on access: deserialize from temp file, re-register memory usage

#### 2.4 Cleanup

- `Spiller` drops all temp files on `Drop`.
- Temp files are also cleaned up at startup (stale spill files from crashed process).

---

### Phase 3: Page State Machine + Optimistic Read (Medium Impact, Medium Risk)

**Timeline**: ~3 weeks | **Effort**: ~400 LOC

**Goal**: Introduce per-segment lock state tracking with optimistic reads to reduce read-write contention on edge/vertex data.

**Rationale**: `RwLock<HashMap<LabelId, VertexTable>>` serializes all writes against all reads at the table level. For read-heavy workloads (graph traversal, property lookup), optimistic reads would significantly improve throughput.

#### 3.1 Design Constraints

- **Scope**: Only applied to frozen segments (immutable data). Delta CSR already uses a different concurrency model (single-writer).
- **No full buffer pool**: Unlike Ladybug, linkrs does not need a VMRegion-style page table because data is organized as segments, not fixed-size pages. The segment is the unit of concurrency.

#### 3.2 New Types

```rust
// crates/graphdb-storage/src/storage/edge/edge_table/page_state.rs

/// Lock state for a frozen segment.
/// Uses atomic CAS — no mutex for read path.
pub struct SegmentLockState {
    /// Packed: [state: u8 | dirty: u1bit | version: 55bit]
    state_and_version: AtomicU64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum SegmentState {
    /// Readable by optimistic readers; writers must CAS to Locked
    Unlocked = 0,
    /// Exclusively held by a writer (merge/freeze)
    Locked = 1,
    /// Marked for eviction but still readable (second-chance)
    Marked = 2,
    /// Evicted to disk; must reload before access
    Evicted = 3,
}
```

#### 3.3 Optimistic Read API

```rust
impl SegmentLockState {
    /// Attempt an optimistic read. Returns the data slice accessor if successful.
    /// If state changed during read (detected via version mismatch), caller retries.
    pub fn try_optimistic_read<F, R>(&self, data: &[u8], func: F) -> Option<R>
    where
        F: FnOnce(&[u8]) -> R,
    {
        let version_before = self.read_version();
        if self.is_write_locked(version_before) {
            return None; // falling back to pessimistic path
        }
        let result = func(data);
        let version_after = self.read_version();
        if version_before == version_after {
            Some(result)
        } else {
            None // retry
        }
    }
}
```

#### 3.4 Integration

- Each `CsrSegment` gets a `SegmentLockState` at creation.
- Read path (edge traversal, property lookup): optimistic read first, fall back to `RwLock` read if CAS fails.
- Write path (merge, freeze): CAS from `Unlocked` → `Locked`, perform write, CAS back with version increment.
- Eviction (Phase 1): CAS from `Unlocked` → `Marked` (second chance), then `Marked` → `Eviced` if still unlocked after second pass.

---

### Phase 4: Memory Accounting & Cache Synchronization (Low Impact, Low Risk)

**Timeline**: ~1 week | **Effort**: ~150 LOC

**Goal**: Tighten the integration between Moka cache eviction and `MemoryAccounting` to eliminate accounting lag.

#### 4.1 Changes

- In `RecordCache::build_vertex_cache` / `build_id_index_cache` (`record_cache.rs:110-185`): wire the `eviction_listener` to call `MemoryAccounting::release(MemoryCategory::Cache, estimated_entry_size)`.
- In `CacheManager::refresh_memory_usage` (`cache_manager.rs:45-52`): replace `report_usage` (swap-based) with a delta-based correction that accounts for entries evicted since last refresh.
- Add `MemoryAccounting::release_category(category, bytes)` public method for use by eviction callbacks.

#### 4.2 Configuration

- Add `cache_eviction_sync: bool` flag to `ResourceConfig` (default: true).
- When enabled, every Moka eviction immediately decrements the accounting counter.

---

### Phase 5: FreeSpaceManager for Segment Allocation (Low Impact, Low Risk)

**Timeline**: ~1.5 weeks | **Effort**: ~300 LOC

**Goal**: Track freed segment slots (after merge or deletion) and reuse them instead of allocating new `Vec` storage. Reduces memory fragmentation in the segment store.

#### 5.1 Design

```rust
// crates/graphdb-storage/src/storage/edge/edge_table/free_space.rs

/// Tracks freed segment indices for reuse.
pub struct SegmentFreeList {
    /// Free segment slots grouped by capacity tier
    free_slots: Vec<Vec<usize>>, // tiered by segment size class
}

impl SegmentFreeList {
    pub fn allocate(&mut self, required_capacity: usize) -> Option<usize> {
        // Best-fit search across tiers
    }
    pub fn free(&mut self, slot: usize, capacity: usize) {
        // Return slot to appropriate tier
    }
}
```

---

## 3. Phasing & Dependencies

```
Phase 1 (Segment Tiering) ──────┐
                                 ├──→ Phase 3 (Page State Machine)
Phase 2 (Disk Spiller) ─────────┘         │
                                          │
Phase 4 (Cache Sync) ─────────────────────┘ (independent, can start anytime)
Phase 5 (FreeSpaceManager) ───────────────────────────────────────────────── (independent)
```

| Phase | Depends On | Risk | Expected Memory Savings |
|-------|-----------|------|------------------------|
| 1: Segment Tiering | None | Medium (new disk I/O path) | 30-60% of cold segment data |
| 2: Disk Spiller | Phase 1 (segments already spillable) | Medium (spill/reload correctness) | Enables workloads 2-5x beyond RAM |
| 3: Page State Machine | Phase 1 (needs residency tracking) | Low (additive to existing RwLock) | 10-30% read throughput improvement |
| 4: Cache Sync | None | Low (callback wiring) | Fixes accounting accuracy |
| 5: FreeSpaceManager | None | Low (slot tracking) | 5-15% fragmentation reduction |

---

## 4. Out of Scope (with Rationale)

| Ladybug Feature | Reason to Skip |
|----------------|---------------|
| **Full VMRegion with MAP_NORESERVE** | linkrs uses variable-size segments, not fixed-size pages. Segment tiering (Phase 1) achieves the same physical memory control without requiring a complete data layout redesign. |
| **MADV_DONTDUMP** | Not relevant for a Rust server; core dump debugging is uncommon in production. |
| **MmAllocator (STL-compatible)** | Rust's Bumpalo Arena already serves this purpose for temporary allocations. |
| **OptimisticAllocator** | linkrs's MVCC already handles rollback via tombstone versions; no need for optimistic page allocation. |
| **Page-level LRU EvictionQueue** | Segment-level LRU is more natural for linkrs's data model. |

---

## 5. Key Files Modified Per Phase

| Phase | Primary Files | New Files |
|-------|--------------|-----------|
| 1 | `edge_table/segment.rs`, `edge_table/merge.rs`, `engine/background_freeze.rs`, `engine/graph_storage/context/mod_freeze.rs` | `edge_table/residency.rs`, `engine/segment_eviction.rs` |
| 2 | `engine/resource_budget.rs`, `engine/cache_manager.rs` | `engine/spiller.rs` |
| 3 | `edge_table/segment.rs`, `edge_table/access.rs` (read path) | `edge_table/page_state.rs` |
| 4 | `cache/record_cache.rs`, `engine/cache_manager.rs`, `engine/resource_budget.rs` | None (modifications only) |
| 5 | `edge_table/segment.rs`, `edge_table/merge.rs` | `edge_table/free_space.rs` |
