# CSR (Compressed Sparse Row) Storage Documentation

Navigation for the edge-storage CSR implementation under
`crates/graphdb-storage/src/edge/`.

## Current

- **[Overview](overview.md)** — current architecture: sharded edge table,
  `Multiple`/`Single`/`None` variants, trait hierarchy, visibility authority,
  wasted-share fragmentation caliber
- **[Fragmentation & Compaction](fragmentation.md)** — measurement,
  watermark-gated removal reporting, compaction signatures

## Historical (pre-restructure; kept for reference)

- **[Variants](variants.md)** — deep dive that still lists the removed
  `MultiSingle` / `Labeled` / immutable `Csr` variants
- **[Dispatch Logic](dispatch.md)** — old selection/dispatch walkthrough
- **[Quick Reference](quick_reference.md)** — old code examples
  (`from_strategy`, `prop_offset`, `compact_with_ts` no longer exist)

## Source of truth

Code comments plus the design review:

- `docs/plan/csr-design-review.md` — current-state review vs Ladybug
- `docs/plan/csr-fix-phases.md` — remediation phases (all shipped)
- `docs/plan/csr-followup-fix.md` — remaining-gap follow-up plan

## Design decisions still true

- Enum-based `CsrVariant` dispatch, no vtable
- Soft-delete with timestamps on rows, but visibility authority is the
  centralized `MVCCManager` (`edge_timestamps`); row stamps are collection
  projections only
- Single variant for one-to-one relationships, sentinel-empty slots
- Per-endpoint-interval node-group sharding with dirty-region driven
  incremental checkpoints; whole-direction dumps are rejected fail-closed
