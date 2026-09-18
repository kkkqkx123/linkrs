# CSR Dispatch & Selection Logic

> **Historical document.** The dispatch set below predates the restructure.
> `MultiSingle`, `Labeled` and the immutable `Csr` have been removed, and
> `EdgeStrategy` now rejects single-direction schemas at construction.
> Current selection and dispatch — see [overview.md](overview.md).

## Overview

CSR selection happens at relationship creation time via the `EdgeStrategy` enum. All strategies map to different CSR implementations, and all are wrapped in a single `CsrVariant` enum for runtime dispatch without virtual function overhead.

## Entry Points

### CsrVariant::from_strategy() - Primary Factory

**Location**: `crates/graphdb-storage/src/storage/edge/csr_variant.rs:140`

```rust
pub fn from_strategy(
    strategy: EdgeStrategy,
    vertex_capacity: usize,
    edge_capacity: usize,
) -> StorageResult<Self> {
    match strategy {
        EdgeStrategy::Multiple => Ok(CsrVariant::Multiple(MutableCsr::with_capacity(
            vertex_capacity, edge_capacity,
        ))),
        EdgeStrategy::Single => Ok(CsrVariant::Single(SingleMutableCsr::with_capacity(
            vertex_capacity,
        ))),
        EdgeStrategy::MultiSingle { max_edges } => {
            Ok(CsrVariant::MultiSingle(MultiSingleMutableCsr::with_capacity(
                vertex_capacity, max_edges,
            )))
        }
        EdgeStrategy::Labeled => Ok(CsrVariant::Labeled(LabeledMutableCsr::with_capacity(
            vertex_capacity, edge_capacity,
        ))),
        EdgeStrategy::None => Ok(CsrVariant::None { vertex_capacity }),
    }
}
```

### Decision Flow

```
EdgeStrategy enum
    │
    ├─ Multiple ────→ MutableCsr (two-level CSR with overflow)
    │
    ├─ Single ──────→ SingleMutableCsr (O(1) direct array)
    │
    ├─ MultiSingle ─→ MultiSingleMutableCsr (fixed-capacity slots)
    │
    ├─ Labeled ─────→ LabeledMutableCsr (label-grouped edges)
    │
    └─ None ────────→ CsrVariant::None (placeholder, zero edges)
```

## CsrVariant Enum

**Location**: `crates/graphdb-storage/src/storage/edge/csr_variant.rs:125`

```rust
pub enum CsrVariant {
    Multiple(MutableCsr),
    Single(SingleMutableCsr),
    MultiSingle(MultiSingleMutableCsr),
    Labeled(LabeledMutableCsr),
    None { vertex_capacity: usize },
}
```

All 5 variants are fully wired via `from_strategy()`. The immutable `Csr` (read-only frozen segments) is managed separately by `CsrSegment` in `edge_table/`, not part of `CsrVariant`.

## Dispatch Logic: Macros

Dispatch uses two macros to eliminate boilerplate:

```rust
/// Dispatch mutable method calls to the underlying CSR variant.
/// Returns $default for None variant.
macro_rules! dispatch {
    ($self:expr, $method:ident($($arg:expr),+) -> $default:expr) => {
        match $self {
            CsrVariant::Multiple(csr) => csr.$method($($arg),+),
            CsrVariant::Single(csr) => csr.$method($($arg),+),
            CsrVariant::MultiSingle(csr) => csr.$method($($arg),+),
            CsrVariant::Labeled(csr) => csr.$method($($arg),+),
            CsrVariant::None { .. } => $default,
        }
    };
}

/// One macro serves both mutable and immutable call sites: mutability lives
/// with the receiver, so no second macro is kept.
```

### Pattern 1: Mutable Operations

**Location**: `csr_variant.rs:308`

```rust
impl MutableCsrTrait for CsrVariant {
    fn insert_edge(&mut self, src_vid: u32, dst: VertexId,
                   edge_id: EdgeId, prop_offset: u32, ts: Timestamp) -> bool {
        dispatch!(self, insert_edge(src_vid, dst, edge_id, prop_offset, ts) -> false)
    }
    fn delete_edge(&mut self, src_vid: u32, edge_id: EdgeId, ts: Timestamp) -> bool {
        dispatch!(self, delete_edge(src_vid, edge_id, ts) -> false)
    }
    fn delete_edge_by_dst(&mut self, src_vid: u32, dst: VertexId, ts: Timestamp) -> usize {
        dispatch!(self, delete_edge_by_dst(src_vid, dst, ts) -> 0)
    }
    // ... all other mutable methods
}
```

**Key Points**:
- Macro-expanded match dispatch (no vtable)
- `None` variant always returns the `$default` value (reject all writes)
- All 4 mutable variants (`Multiple`, `Single`, `MultiSingle`, `Labeled`) forward to their implementations

### Pattern 2: Read Operations

**Location**: `csr_variant.rs:336`

```rust
fn get_edge(&self, src_vid: u32, dst: VertexId, ts: Timestamp) -> Option<Nbr> {
    dispatch_immutable!(self, get_edge(src_vid, dst, ts) -> None)
}
fn edges_of(&self, src_vid: u32, ts: Timestamp) -> Vec<Nbr> {
    dispatch_immutable!(self, edges_of(src_vid, ts) -> Vec::new())
}
```

### Pattern 3: Base Operations (Serialization, Memory)

**Location**: `csr_variant.rs:212`

Serialization uses variant-specific tags:

```
Tag   Variant
0     None
1     Multiple
2     Single
3     MultiSingle
4     Labeled
```

`dump()` prepends the tag byte; `load()` dispatches based on the first byte.

### Pattern 4: Iterator Dispatch

**Location**: `csr_variant.rs:361`

```rust
pub fn iter(&self, ts: Timestamp) -> CsrIterator<'_> {
    match self {
        CsrVariant::Multiple(csr) => CsrIterator::Multiple(csr.iter(ts)),
        CsrVariant::Single(csr) => CsrIterator::Single(csr.iter(ts)),
        CsrVariant::MultiSingle(csr) => CsrIterator::MultiSingle(csr.iter(ts)),
        CsrVariant::Labeled(csr) => CsrIterator::Labeled(csr.iter(ts)),
        CsrVariant::None { .. } => CsrIterator::None,
    }
}

pub enum CsrIterator<'a> {
    Multiple(MutableCsrIterator<'a>),
    Single(SingleMutableCsrIterator<'a>),
    MultiSingle(MultiSingleMutableCsrIterator<'a>),
    Labeled(LabeledMutableCsrIterator<'a>),
    None,
}
```

## Integration: EdgeSchema → EdgeTable → CsrVariant

### EdgeTable Creation

```
EdgeSchema.oe_strategy ──┐
                         ├─→ CsrVariant::from_strategy() → out_csr
EdgeSchema.oe_capacity ──┘

EdgeSchema.ie_strategy ──┐
                         ├─→ CsrVariant::from_strategy() → in_csr
EdgeSchema.ie_capacity ──┘

EdgeTable { out_csr, in_csr, prop_table, ... }
```

### Query Execution

```
Query("traverse edges")
    │
    ├─→ EdgeTable.edges_of(src, ts)
    │   │
    │   └─→ out_csr.edges_of(src, ts)  // dispatch via CsrVariant
    │       │
    │       ├─ Multiple ──→ scan primary + overflow
    │       ├─ Single ────→ O(1) direct access
    │       ├─ MultiSingle → scan fixed slots
    │       ├─ Labeled ───→ scan label-grouped blocks
    │       └─ None ──────→ empty vec
    │
    └─→ Fetch edge properties from PropertyTable
```

## Compaction & Maintenance

Full-table rebuild is not part of the storage interface: production
compaction goes through per-row `compact_vertex_with_reporting`, and unit
tests exercise the same reporting entry point directly.

**Behavior per variant**:
| Variant | Behavior |
|---------|----------|
| `Multiple` | Full compaction: merges overflow, removes soft-deleted edges |
| `Single` | No-op (no fragmentation, direct array) |
| `MultiSingle` | Removes tombstoned edges by shifting within fixed slots |
| `Labeled` | Removes tombstoned edges, rebuilds label ranges |
| `None` | No-op (zero edges) |

Fragmentation diagnostics (only meaningful for `Multiple`):

```rust
pub fn fragmentation_ratio(&self) -> f32 {
    match self {
        CsrVariant::Multiple(csr) => csr.fragmentation_ratio(),
        _ => 0.0,
    }
}
```

## Design Principles

### 1. Zero Vtable Overhead
- Enum dispatch via `match` is inlineable
- No runtime indirection for method calls
- Compiler can optimize based on variant

### 2. Unified Interface
- All variants implement `CsrBase` + `MutableCsrTrait`
- Single enum type simplifies code (no generic parameters)
- Type safety at compile time via pattern matching

### 3. Graceful Degradation
- `None` variant accepts strategy but rejects operations
- Consistent `false`/`None`/empty behavior for rejected ops

### 4. Extensibility
- Adding new variant only requires:
  1. New struct implementing traits
  2. Match arm macro invocation in dispatch macros
  3. Serialization tag in `dump()`/`load()`
- Macros minimize boilerplate across 4 mutable variants
