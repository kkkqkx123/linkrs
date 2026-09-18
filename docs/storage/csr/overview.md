# CSR Architecture Overview

> 当前结构以本文件为准。variants.md、dispatch.md、quick_reference.md 的正文
> 含已删除的历史形态（`MultiSingle`、`Labeled`、immutable `Csr`、`prop_offset`），
> 阅读时以代码与本文对齐。

## What is CSR?

**CSR** (Compressed Sparse Row) is the column-oriented edge storage format used
for adjacency: an offset array per row, a flattened neighbor list, a degree
array per row, giving O(V + E) space instead of O(V²).

## Current Layout

One edge label maps to one sharded edge table (`EdgeStore`): a node-group
sharded topology per direction (`CsrShardSet`), a centralized authority
timestamp table (`MVCCManager`), a columnar property store
(`CsrWithProperties`), an edge-owner map, and per-group segment statistics.
Grouping partitions bound endpoints by interval; each group keeps one
`CsrVariant` plus leaf-region dirt driving incremental checkpoints.

## CSR Variants

| Variant | Use Case | Structure |
|---------|----------|-----------|
| `Multiple` (`MutableCsr`) | general multi-edge rows | primary rows with density-target gaps plus graded per-vertex overflow chunks |
| `Single` (`SingleMutableCsr`) | one edge per vertex | direct slot array, empty slots use the never-alive sentinel |
| `None` | direction stores nothing | placeholder |

`EdgeStrategy` selects the variant per direction, and `EdgeSchema::validate`
requires both directions to use the same non-`None` strategy (single-direction
tables are rejected at construction). The `dispatch!` macro in `csr_variant.rs`
routes trait calls to the concrete shape without `dyn`.

## Trait Hierarchy

### CsrBase
`vertex_capacity()`, `edge_count()`, `dump()`, `dump_into()`, `load()`.
`CsrShardSet` fails closed on whole-direction dump/load: persistence moves
through the per-group incremental protocol only.

### MutableCsrTrait
Insert, delete by id / by dst / by offset, reverts, physical and visible reads,
per-vertex reclaim probing (`reclaimable_count`, `vertex_reclaim_probe`),
row-level compaction with removal reporting, and memory accounting.
Timestamp-filtered row reads (`get_edge`, `edges_of`, `get_edge_physical`) are
test-only primitives; production visibility goes through the version
authority (`MVCCManager`) via the merged lookup in the table layer.

## Timestamp & Visibility

```
Nbr { endpoint: u32, rank: i64, edge_id: EdgeId, create_ts, delete_ts }
```

Row stamps are physical projections for collection only. Visibility authority
is `edge_timestamps`: an edge is visible when `create_ts <= ts < delete_ts`.
Gap-fill uses `Nbr::dead_gap()` (invalid edge id, never-alive window) so an
overrun scan reports absence instead of a ghost live edge. Properties keep
their own per-column version chains for time-travel reads, collapsed to
current values on checkpoint.

## Fragmentation Management

Primary rows keep density-target gaps for everyday writes; deleted entries
wait for the collection cutoff. Waste is therefore gaps plus tombstone slots:
`fragmentation_ratio()` reports the wasted share of reserved capacity
(0.0–1.0) and is observation only. Collection triggers consult per-vertex
reclaimable counts; group merges stay behind the caller fragmentation gate.
Recovery paths compact with per-edge removal reporting and always pass a
watermark cutoff; rows below the watermark keep their tombstones.

## File Organization

```
crates/graphdb-storage/src/edge/
├── csr_shared.rs             # shared slot decision for delete paths
├── csr_trait.rs              # CsrBase, MutableCsrTrait
├── csr_variant.rs            # Multiple/Single/None enum, dispatch macro
├── csr_with_properties.rs    # columnar property store
├── edge.rs (parent)          # Nbr, EdgeSchema, EdgeRecord, strategies
├── edge_table/               # sharded table, staging commit, checkpoint,
│   │                         # compaction, MVCC, WAL, schema state machines
├── fragmentation_stats.rs    # wasted-share caliber stats, group merge gate
├── mutable_csr/              # Multiple variant (row, overflow, merge, read)
├── node_group.rs             # group shard set, dirt, append log, manifest
├── property_schema.rs        # property schema entries
└── single_mutable_csr.rs     # Single variant
```

## Related Docs

- [Fragmentation & Compaction](fragmentation.md) — measurement and recovery details
- [docs/plan/csr-design-review.md](../../plan/csr-design-review.md) — current design review
