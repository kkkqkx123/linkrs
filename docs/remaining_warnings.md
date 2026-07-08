# Remaining Warnings Analysis

Total: **44 `dead_code` warnings** in `-p graphdb-storage` (lib). All represent items not yet integrated into the runtime pipeline.

## Already Integrated (removed from warnings)

| Item | Method |
|------|--------|
| `deletion_threshold` (MergeConfig) | Wired into `merge_adaptive()` pipeline |
| `MergeMetrics.log()` | `println!` → `log::info!` |
| `MergeMetricsResult` in compaction | Captured and logged at call sites |
| `record_delta_size` | Called in `trigger_background_freeze` loop |
| `get_reason` / `get_config` | Called with `log::debug` in freeze loop |
| `edges_per_vertex` | Exposed via `CsrVariant::edges_per_vertex()`, used in `memory_size()` |
| `enable_lsm_tiering` | Read in `compact_maintenance` to branch LSM merge |
| `merge_segments_lsm_tiered` | Called when `enable_lsm_tiering` is true |
| `deletion_stats / DeletionStats` | Called in `compact_maintenance` and logged |
| `max_segments_per_direction` | Read in `auto_merge_segments` as emergency trigger |

## 44 Remaining Warnings by Category

### 🔴 Safe to Delete (no integration value)

| Item | File | Reason |
|------|------|--------|
| `NbrWithoutEdgeId` (struct + 5 methods) | `edge/mod.rs:286` | Old data structure superseded by `Nbr` |
| `CompactionReport` | `edge/mod.rs:62` | Return type of deleted `compact_with_stats` |
| `default_schema_version()` | `vertex/mod.rs:70` | Replaced by `schema_version` field |
| `version` (SegmentVersion field) | `edge/edge_table/segment.rs:103` | Never used for anything, increment/validate also dead |
| `max_segment_size_bytes` (FreezeConfig) | `engine/config.rs:91` | Duplicate of `MergeConfig.max_segment_size_bytes` |
| `min_active_snapshot_ts` (TombstoneStats) | `edge/edge_table/stats.rs:20` | Not exposed by any consumer |
| `edge_ids`, `csr_position_to_edge_ids_index` (FreezeDeltaResult) | `edge/edge_table/merge.rs:18-19` | Never read, only `frozen_count` is consumed |
| `config` (IdIndexer) | `vertex/id_indexer.rs:314` | Stored but never accessed |

### 🟢 Trivial Integration (1-3 lines each)

| Item | Integration |
|------|-------------|
| `segments_total_bytes()` | Call in `compact_and_freeze` and log result |
| `merge_stats()` + `MergeStats` struct | Call in `compact_maintenance` after merge loop |
| `merge_segments_lsm_tiered` already done | — |
| `has_more()` on iterator | Use in `scan_paginated` instead of inline bool |
| `data_store_arc()` | Used in API layer (cross-crate), add `pub(crate)` call |
| `get_id()` on NameIndexer | Schema lookup path in API |

### 🟡 Moderate Integration (signature/pipeline changes)

| Item | Integration Path |
|------|-----------------|
| `FreezeDecision` fields (5 fields) | Expose in HTTP `/stats` response |
| `FreezeGuard` (struct + 3 methods) | Use in `trigger_background_freeze` to replace manual `record_freeze` |
| `SnapshotBuilder` (struct + 5 methods) | Refactor `export_snapshot` to use builder pattern |
| `FragmentationStats` (struct + 6 methods) | Use in `maybe_compact_for_flush` to decide compaction |
| `deletion_percentage` (DeletionInfo) | Call in `deletion_stats()` pipeline |
| `register/unregister_active_snapshot` (MVCCManager) | Wire into `MVCCTable` trait for EdgeTable |
| `MutableCsr` methods (edges_iter, should_compact, wasted_bytes_estimate, get_fragmentation_stats, should_compact_with_threshold, compact_with_stats) | Use in CSR-only compaction path (`compact_csr_only`) |
| SegmentVersion `increment`/`validate` | Wire into segment lifecycle (freeze/merge) |
| `bytes_per_edge` | Already used in `merge_in_place` — clippy false positive |
| `strategy_name` (BackgroundFreezeManager + FreezeDecisionEngine) | Use in log messages in freeze loop |
| `get_config` (BackgroundFreezeManager) | Expose in admin API |
| `segments_reduced` (MergeMetricsResult field) | Log in compaction pipeline alongside metrics |
| LSMSegmentLevel methods (`size_range`, `merge_target_size`) | Used in LSM merge path (when enabled) |
| `Conservative`, `LSMTiered` (FreezeStrategyType variants) | Used when respective configs are set |

### 🔵 Complex Integration (new features / cross-crate)

| Item | Integration Path |
|------|-----------------|
| `EdgeDeletionBloomFilter` (struct + 6 methods) | Integrate into `MVCCManager::is_tombstoned()` as fast-path filter |
| `SchemaVersionHistory` (struct + 12 methods) | Load at startup, record on DDL, expose via HTTP `/schema/history` |
| `LabeledMutableCsr` methods (`edges_of_label`, `get_edge_by_label`) | Query engine traversal when `EdgeStrategy::Labeled` |
| `scan_paginated` | HTTP paginated scan API (graphdb-api crate) |
| `set_schema`, `version_history_ref` | Schema manager DDL handler |
| `validate_segment_integrity`, `segment_versions`, `update_segment_checksums` | Debug-build validation, flush pipeline |
| `VertexEdgesIter` | Alternative to `edges_of()` allocation in hot path |
| `FreezeConfig` methods (`development`, `production_large`, `validate`) | Config setup / validation in server startup |
| `PropertyGraphConfig` methods (`development`, `production_small`, `production_large`, `validate`) | Same as above |
| `hot_max_size` (TieredTombstoneManager) | LSM tiering tombstone pipeline |

## Recommendation Order

1. **🔴 Delete 8 items** — immediate warning reduction (~8)
2. **🟢 5 trivial integrations** — half-hour work (~5)
3. **🟡 13 moderate integrations** — planned across 2-3 PRs
4. **🔵 10 complex** — feature-track items

**Note**: The remaining 44 warnings are pure `dead_code`; there are zero code quality warnings (clippy `pedantic` etc. were not part of this pass). All `--all-targets` test compilation errors (104 pre-existing) are unrelated to the storage crate changes.
