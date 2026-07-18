# Remaining Integration Test Failures (2026-07-18)

After fixing the initial `Chunk 0 has N columns, expected 0` error (which masked many failures) and the DML hang (infinite loop in SinkOperator), **36 of 61 tests pass**, 25 remain.

## 1. DataChunk Layout Mismatch (Phase A-D Regression)

**Failure**: `test_match_path` — `DataChunk::new_with_layout: row width 4 does not match layout width 3`

**Cause**: Phase A-D changed blocking operators to use `Arc::clone(&base.output_layout)` instead of constructing a layout via `SlotLayout::from_names(...)`. The `ExpandAll` / path-expansion operators now produce 4-column rows (vid, tags, properties, edge) but the plan's `output_layout` has only 3 columns. The assertion in `DataChunk::new_with_layout` fires.

**Fix**: Either widen the planned `output_layout` for MATCH queries, or skip strict layout enforcement in operators that produce variable-width results.

**Status**: Needs deeper analysis of the planner's output layout calculation for path queries.

## 2. Index Space ID Mismatch

**Tests**: `test_index_scan_for_equality`, `test_index_scan_for_range`, `test_lookup_index`

**Error**: `Index idx_person_name belongs to space 0, not space 1`

**Cause**: CREATE INDEX resolves the space ID from the current execution context, but the execution plan stores the space ID at plan-construction time. The space created by `setup_test_space` gets ID 1, but the CREATE INDEX plan resolves to space 0 (default space). Likely a pre-existing issue with how USE SPACE propagates through the query pipeline, or related to the auto-commit transaction scope not carrying the session context properly.

**Status**: Uncertain — could be pre-existing or caused by auto-commit scope changes. Needs investigation of space resolution in execution runtime.

## 3. Profile / EXPLAIN Empty Space Name

**Tests**: `test_basic_profile`, `test_profile_query`

**Error**: `StorageScanVertices open cursor failed for space ''` — space name is empty string.

**Cause**: PROFILE queries run against a space but the space name is not propagated to the scan operator. Likely related to how the execution runtime resolves the current space for profile/explain execution plans. May be pre-existing or related to auto-commit scope changes.

**Status**: Uncertain — needs investigation of how space name flows through to StorageScanVertices in the profile path.

## 4. Transaction Controller Not Available

**Tests**: `test_transaction_commit`, `test_transaction_rollback`

**Error**: `Transaction controller not available in execution runtime`

**Cause**: Explicit BEGIN/COMMIT/ROLLBACK statements require a `TransactionController` to be present in the execution runtime. Our auto-commit scope (added for DML in Phase A-D fixes) wraps individual DML statements in an auto-commit scope but does not set up a controller for explicit transaction management. The `BEGIN` statement tries to create/pull a transaction controller from the runtime and fails.

**Fix**: The auto-commit scope addition in `execute_query_with_request` needs to also support explicit transaction lifecycle. When a user issues `BEGIN`, the runtime must set up a transaction controller that persists across subsequent statements until `COMMIT`/`ROLLBACK`.

**Status**: Confirmed regression from auto-commit scope fix.

## 5. Fetch Vertex — Missing Vertex IDs

**Tests**: `test_fetch_vertex` (both schema_manager and social_network)

**Error**: `GetVertices requires vertex IDs`

**Cause**: The FETCH PROP query expects vertex IDs to be passed to the scan operator, but the operator receives an empty set. Likely a pre-existing issue where the planner doesn't resolve the vertex ID expression correctly, or the space doesn't have the expected data.

**Status**: Uncertain — likely pre-existing.

## 6. Geography Tests (Pre-existing)

**Tests**: `test_geography_vertex_counts`, `test_distance_calculation`, `test_explain_geography_query`, `test_point_creation`, `test_within_distance`, `test_wkt_creation`

**Error**: Various geography-related assertions.

**Cause**: The geography (S2/H3) feature likely has incomplete implementation or test fixture issues. Not related to Phase A-D changes.

**Status**: Pre-existing.

## 7. Vector Test (Needs Qdrant)

**Test**: `test_vector_insertion`

**Cause**: Requires a running Qdrant instance. Confirmed pre-existing.

**Status**: Pre-existing, needs external service.

## 8. Data-Driven Optimizer Tests (Uncertain)

**Tests**: `test_optimizer_aggregate`, `test_optimizer_vertex_count`, `test_social_network_edge_counts`, `test_social_network_filter`, `test_social_network_go_traversal`, `test_social_network_lookup_index`, `test_social_network_vertex_counts`

**Cause**: These tests load GQL from data files and assert against expected results. Failures could be caused by layout mismatches, space resolution issues, or plan changes from Phase A-D.

**Status**: Uncertain — need individual investigation.

## Summary

| Category | Count | Likely Root Cause |
|---|---|---|
| Layout mismatch (match_path) | 1 | Phase A-D regression |
| Transaction controller | 2 | Auto-commit scope regression |
| Index space ID | 3 | Uncertain / pre-existing |
| Profile empty space | 2 | Uncertain |
| Fetch vertex | 2 | Likely pre-existing |
| Geography | 5 | Pre-existing |
| Vector (qdrant) | 1 | Pre-existing |
| Data-driven optimizer | 7 | Uncertain |
| **Total** | **25** | |
