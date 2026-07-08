# Phase 3b & 4 - Remaining Dead Code Integrations Plan

**Status**: 22/32 Phase 3 warnings eliminated (69% complete)  
**Current**: 22 dead_code warnings remaining  
**Target**: Eliminate all 44 original warnings (currently at 29/44 = 66% overall)

---

## Phase 3b: Remaining 12 Moderate Integrations

### Tier 1: High-Priority Observable Items (2-3 items, 2-3 hours)

#### 1. `bytes_per_edge(CsrVariant)` - VERIFY IF FALSE POSITIVE
- **Current location**: `crates/graphdb-storage/src/storage/edge/csr_variant.rs:201`
- **Issue**: Clippy reports unused, but method is used via CSR trait
- **Action**: Verify if this is a true positive
- **Effort**: 30 min investigation
- **Integration point**: May need explicit call in diagnostics or can be left as-is if false positive
- **Status**: PENDING VERIFICATION

#### 2. `has_more()` - EdgeTableScanIterator Method
- **Current location**: `crates/graphdb-storage/src/storage/edge/edge_table/core.rs:1099`
- **Issue**: Iterator has `has_more()` method, but `scan_paginated()` doesn't use iterator
- **Integration options**:
  - Option A: Refactor `scan_paginated()` to return iterator with `has_more()`
  - Option B: Create wrapper function that uses iterator's `has_more()`
  - Option C: Use in pagination debug/monitoring
- **Effort**: 1-2 hours
- **Recommended**: Option C - add to pagination monitoring in API layer
- **Status**: AWAITING DESIGN DECISION
- **Note**: May be Phase 4 (cross-crate API layer)

#### 3. `update_segment_checksums()` - Segment Lifecycle
- **Current location**: `crates/graphdb-storage/src/storage/edge/edge_table/core.rs:351`
- **Purpose**: Update checksums after segment modifications
- **Integration points**:
  - Merge operations (already checksums computed)
  - Compaction pipeline (add explicit checksum update)
  - Freeze operations
- **Effort**: 1-2 hours
- **Recommended approach**: Call during freeze → merge → compaction pipeline
- **Status**: READY TO INTEGRATE
- **Priority**: HIGH

---

### Tier 2: Medium-Priority Refactoring Items (4-5 items, 4-6 hours)

#### 4. `SnapshotBuilder` - Builder Pattern Integration
- **Current location**: `crates/graphdb-storage/src/storage/edge/edge_table/stats.rs`
- **Current usage**: Never constructed (dead code)
- **Purpose**: Builder pattern for snapshot export
- **Integration points**:
  - `export_snapshot()` method (create public API)
  - HTTP `/snapshot` endpoint
  - Snapshot export pipeline
- **Effort**: 3-4 hours
- **Scope**: Moderate - may be Phase 4 feature
- **Status**: DESIGN NEEDED
- **Priority**: MEDIUM

#### 5. `FragmentationStats` - Compaction Decision Path
- **Current location**: `crates/graphdb-storage/src/storage/edge/edge_table/stats.rs`
- **Purpose**: Calculate space efficiency and reclamation potential
- **Integration**: Use in `maybe_compact_for_flush()` to decide compaction threshold
- **Method chain needed**:
  - `fragmentation_ratio()` - % of unused capacity
  - `unused_capacity()` - bytes available for reclamation
  - `space_efficiency()` - % of used vs total space
  - `reclamation_potential()` - bytes saved if compacted
  - `should_compact()` - boolean decision
- **Effort**: 2-3 hours
- **Status**: READY FOR IMPLEMENTATION
- **Priority**: MEDIUM

#### 6. `MutableCsr` Compaction Methods - CSR-Only Path
- **Methods involved**:
  - `edges_iter()` - Edge iteration
  - `should_compact()` - Compaction decision
  - `wasted_bytes_estimate()` - Space calculation
  - `get_fragmentation_stats()` - Stats gathering
  - `should_compact_with_threshold()` - Threshold-based decision
- **Integration**: Create `compact_csr_only()` method for in-place CSR compaction
- **Effort**: 2-3 hours
- **Status**: AWAITING ARCHITECTURE DISCUSSION
- **Priority**: MEDIUM-LOW (may be Phase 4)

#### 7. `register/unregister_active_snapshot` - MVCC Integration
- **Current location**: `crates/graphdb-storage/src/storage/engine/background_freeze.rs`
- **Purpose**: Track active snapshots in MVCC manager
- **Integration**: Wire into `MVCCTable` trait for EdgeTable
- **Method chain**:
  - MVCC table creation → `register_active_snapshot()`
  - Snapshot cleanup → `unregister_active_snapshot()`
- **Effort**: 1-2 hours
- **Status**: DESIGN NEEDED
- **Priority**: MEDIUM

#### 8. `Conservative/LSMTiered` Variants - False Positive Check
- **Current location**: `crates/graphdb-storage/src/storage/engine/config.rs:31`
- **Issue**: Clippy reports never constructed, but used in configs
- **Investigation**: 
  - Check if only used in test code
  - Verify if conditional compilation affects visibility
  - May be false positive due to feature gate
- **Effort**: 30 min - 1 hour
- **Status**: INVESTIGATION NEEDED
- **Priority**: LOW (likely false positive)

---

### Tier 3: Lower-Priority Remaining (5+ items, 4-8 hours)

#### 9. `scan_paginated` - HTTP Pagination API
- **Current location**: `crates/graphdb-storage/src/storage/edge/edge_table/core.rs:661`
- **Purpose**: Return paginated edge records with has_more flag
- **Integration**: Expose in `graphdb-api` crate as HTTP endpoint
- **Effort**: 2-3 hours
- **Status**: REQUIRES CROSS-CRATE WORK (Phase 4)
- **Priority**: LOW

#### 10. `set_schema` / `version_history_ref` - Schema DDL Handler
- **Current location**: `crates/graphdb-storage/src/storage/edge/edge_table/core.rs`
- **Purpose**: Schema version tracking and DDL operations
- **Integration**: Schema manager DDL handler
- **Effort**: 1-2 hours
- **Status**: DESIGN NEEDED (Phase 4)
- **Priority**: LOW

#### 11. `data_store_arc()` - Cross-Crate API
- **Current location**: `crates/graphdb-storage/src/storage/engine/graph_storage/mod.rs`
- **Purpose**: Expose data store reference to API layer
- **Effort**: 30 min - 1 hour
- **Status**: READY (Phase 4)
- **Priority**: LOW

#### 12. `get_id(NameIndexer)` - Schema Lookup
- **Current location**: Schema/index related
- **Purpose**: Get ID by name in schema
- **Integration**: Schema manager query path
- **Effort**: 1 hour
- **Status**: DESIGN NEEDED
- **Priority**: LOW

---

## Phase 4: Complex Feature Integrations

### Overview
10 items requiring new features or cross-crate integration (8-16 hours estimated)

### Items (from Phase 4 priority list):

#### 1. `EdgeDeletionBloomFilter` - Fast-Path Filter
- **Purpose**: Quick tombstone detection without full scan
- **Integration**: `MVCCManager::is_tombstoned()` fast-path
- **Effort**: 3-4 hours
- **Scope**: Internal optimization

#### 2. `SchemaVersionHistory` - Schema Tracking
- **Purpose**: Track schema changes over time
- **Integration**:
  - Load at startup
  - Record on DDL
  - Expose via HTTP `/schema/history`
- **Effort**: 2-3 hours
- **Scope**: New feature, HTTP API

#### 3. `LabeledMutableCsr` Methods - Labeled Edge Support
- **Methods**: `edges_of_label()`, `get_edge_by_label()`
- **Integration**: Query engine traversal when `EdgeStrategy::Labeled`
- **Effort**: 2-3 hours
- **Scope**: Query optimization

#### 4. `SnapshotBuilder` Export API (continued from 3b)
- **Purpose**: Builder pattern for snapshots
- **Integration**: HTTP `/export/snapshot` endpoint
- **Effort**: 2-3 hours
- **Scope**: Cross-crate (API layer)

#### 5. `VertexEdgesIter` - Allocation-Free Iteration
- **Purpose**: Avoid Vec allocation in hot path
- **Integration**: `edges_of()` alternative
- **Effort**: 1-2 hours
- **Scope**: Performance optimization

#### 6. `FreezeConfig/PropertyGraphConfig` Methods - Config API
- **Methods**: `development()`, `production_large()`, `validate()`
- **Integration**: Expose in admin API for dynamic config
- **Effort**: 1-2 hours
- **Scope**: Admin API

#### 7. `TieredTombstoneManager.hot_max_size` - LSM Tombstones
- **Purpose**: Tombstone manager LSM tiering
- **Integration**: LSM tiering pipeline
- **Effort**: 1-2 hours
- **Scope**: Internal optimization

#### 8-10. Additional Phase 4 items
- Various API exposures and cross-crate integrations
- Estimated: 4-6 hours combined

---

## Work Session Plan

### Session 1 (Current/Next - 2-3 hours):
- ✅ Phase 3a: 10 items completed
- Status: **COMPLETE**
- Commits: User to commit staged changes

### Session 2 (Phase 3b Part 1 - 2 hours):
**High-Priority Tier 1**:
1. Verify `bytes_per_edge` false positive
2. Decide on `has_more()` integration approach
3. Implement `update_segment_checksums()` integration

**Estimated result**: -3 warnings (22 → 19)

### Session 3 (Phase 3b Part 2 - 3 hours):
**Medium-Priority Tier 2**:
1. Design and implement `FragmentationStats` integration
2. Implement `MutableCsr` compaction methods
3. Optional: `register/unregister_active_snapshot` if time allows

**Estimated result**: -4 warnings (19 → 15)

### Session 4 (Phase 3b Cleanup - 2 hours):
**Remaining & False Positives**:
1. Investigate `Conservative/LSMTiered` variants
2. Handle remaining observable items
3. Document integration decisions

**Estimated result**: -3 warnings (15 → 12)

### Sessions 5+ (Phase 4 - 8-16 hours across multiple sessions):
**Complex Feature Integration**:
- BloomFilter, SchemaVersionHistory, cross-crate APIs
- New HTTP endpoints
- Performance optimizations

**Estimated result**: -12 warnings (12 → 0)

---

## Decision Points Requiring User Input

| Item | Decision Needed | Options |
|------|-----------------|---------|
| `has_more()` | Integration strategy | A: Refactor scan_paginated; B: Wrapper; C: Monitoring |
| `SnapshotBuilder` | Scope | Phase 3b or Phase 4? |
| `MutableCsr` methods | CSR-only path | Create new method or integrate into existing? |
| `register/unregister` | MVCC trait design | How to integrate into EdgeTable? |
| `bytes_per_edge` | Investigation | Run detailed analysis or mark as false positive? |

---

## File Organization

```
docs/plan/
├── phase3b_integration_guide.md      (detailed implementation steps)
├── phase4_feature_plan.md            (feature-by-feature breakdown)
├── integration_checklist.md           (per-item verification steps)
├── architecture_decisions.md          (design decisions)
└── timeline.md                       (estimated schedule)
```

---

## Summary Metrics

| Phase | Items | Warnings | Est. Hours | Status |
|-------|-------|----------|-----------|--------|
| **3a** | 10 | -10 | 3 | ✅ COMPLETE |
| **3b** | 12 | -10 | 6-8 | 🔄 PENDING |
| **4** | 10 | -2 | 8-16 | ⏳ TODO |
| **Total** | 32 | -22 | 17-27 | 🎯 IN PROGRESS |

**Overall Progress**: 44 → 22 warnings (50% complete)

---

## Critical Path

1. ✅ Phase 3a (DONE)
2. 🔄 Phase 3b (NEXT - 3 sessions)
3. ⏳ Phase 4 (AFTER Phase 3b)

**Estimated completion**: 2-4 weeks depending on availability
