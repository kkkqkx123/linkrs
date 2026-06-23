# Test Issues Analysis Report

**Date**: 2026-06-13
**Test Command**: `cargo test --workspace --features fulltext-search,qdrant -- --nocapture`

## Summary

- **Total Tests**: 200+
- **Passed**: 197
- **Failed**: 3
- **Compilation Errors**: 0 (after fixes)
- **Warnings**: ~30 (mostly unused imports and dead code)

---

## Fixed Issues

### 1. Compilation Errors

#### graphdb-search: Incorrect `.await` on sync function
**Location**: `crates/graphdb-search/tests/fulltext_tests/transaction.rs:210,314`
**Issue**: `rollback_transaction()` returns `Result`, not `Future`, but test called `.await`
**Fix**: Removed `.await` calls

#### graphdb-search: Missing `qdrant` feature
**Location**: `crates/graphdb-search/src/search/isolation_test.rs:36,58`
**Issue**: `#[cfg(feature = "qdrant")]` used but `qdrant` feature didn't exist in graphdb-search
**Fix**: Added `qdrant = ["graphdb-sync/qdrant"]` to `crates/graphdb-search/Cargo.toml`

#### graphdb-storage: Private field access in tests
**Location**:
- `crates/graphdb-storage/src/storage/engine/graph_storage/schema_engine.rs:521,528`
- `crates/graphdb-storage/src/storage/engine/transaction/recovery.rs:871,879`
**Issue**: Tests accessed `ctx.persistent.data_store` directly, but `persistent` is private
**Fix**: Used existing `pub(crate) fn data_store()` accessor method

#### graphdb-query: Missing qdrant feature gates
**Location**: `crates/graphdb-query/src/query/optimizer/heuristic/visitor.rs:62,301-311`
**Issue**: Vector search imports and visitor methods not gated behind `qdrant` feature
**Fix**: Added `#[cfg(feature = "qdrant")]` to imports and visitor method implementations

---

## Remaining Failures

### Vector Search Tests (3 failures)

**Tests**:
- `e2e::extended_types::vector::test_cosine_similarity`
- `e2e::extended_types::vector::test_explain_vector_query`
- `e2e::extended_types::vector::test_filtered_vector_search`

**Error**:
```
Cannot start a runtime from within a runtime. This happens because a function (like `block_on`)
attempted to block the current thread while the thread is being used to drive asynchronous tasks.
```

**Root Cause**:
- `CreateVectorIndexExecutor::execute()` is a sync method
- It calls `tokio::runtime::Handle::current().block_on()` to bridge sync→async
- When tests use `#[tokio::test]`, this creates a nested runtime, which panics

**Location**: `crates/graphdb-query/src/query/executor/data_access/vector_index.rs:111`

**Analysis**:
This is a **design issue** requiring refactoring the executor framework to support async operations properly. The current `block_on` approach is a workaround that fails in async test contexts.

**Recommendation**:
- **Complete the integration**: Refactor the executor trait to support async execution
- This is a core feature (vector search), not dead code
- The implementation is incomplete, not redundant

---

## Dead Code Analysis

### graphdb-storage/tests/common/mod.rs

**Unused Functions** (warnings from lib test compilation):
- `create_in_memory_storage()`
- `create_persistent_storage()`
- `open_persistent_storage()`
- `create_employee_tag()`
- `create_works_at_edge_type()`
- `create_multi_tag_vertex()`
- `setup_multi_tag_schema()`
- `create_person_name_index()`
- `verify_test_data()`

**Analysis**:
These functions are **NOT dead code** - they are used by integration tests in:
- `tests/full_lifecycle.rs`
- `tests/compaction.rs`
- `tests/persistence_recovery.rs`
- `tests/wal_recovery.rs`
- `tests/scenario.rs`

The warnings appear only during lib test compilation because integration tests are in separate test targets.

**Recommendation**:
- **Keep the functions** - they are used by integration tests
- The warnings are expected and don't indicate a problem
- Consider adding `#[allow(dead_code)]` to suppress warnings if they're noisy

### Unused Imports in Tests

**Location**: Multiple test files
**Examples**:
- `crates/graphdb-storage/tests/compaction.rs:15`: unused `Value` import
- `crates/graphdb-storage/src/storage/engine/persistence_test.rs:6`: unused `EdgeStrategy`
- `crates/graphdb-storage/src/storage/engine/transaction/recovery.rs:836`: unused `IndexMetadataManager`

**Analysis**:
These are test-specific imports that may be used in some test functions but not others, or were used in removed tests.

**Recommendation**:
- **Clean up**: Remove unused imports to reduce warnings
- Low priority - doesn't affect correctness

### Unused Variable Warnings

**Location**:
- `crates/graphdb-search/src/search/manager.rs:372`: `user_index_name`
- `crates/graphdb-query/src/query/executor/expression/functions/builtin/aggregate.rs:534`: `sum_func`

**Analysis**:
- `user_index_name`: Likely reserved for future use or debugging
- `sum_func`: Likely a refactoring artifact

**Recommendation**:
- **Clean up**: Prefix with `_` or remove if truly unused
- Low priority

---

## Incomplete Integrations Analysis

### Vector Search (qdrant feature)

**Status**: Partially implemented
**Missing**:
1. Async executor support for `CreateVectorIndexExecutor`
2. Proper error handling for nested runtime scenarios
3. Integration tests that work with tokio runtime

**Recommendation**:
- **Complete the integration** - vector search is a core feature
- Refactor executor trait to support async operations
- This is the most important remaining work

### Full-text Search (fulltext-search feature)

**Status**: Complete
**Tests**: All passing
**Warnings**: Minor unused variable in manager

**Recommendation**:
- Clean up unused variable warning
- Feature is complete and working

### C-API

**Status**: Not tested in this run
**Note**: Requires `c-api` feature

**Recommendation**:
- Run separate test with `c-api` feature enabled

---

## Priority Actions

### High Priority
1. **Refactor vector index executor** to support async operations properly
   - Change `execute()` to `async fn execute()` or use proper async runtime handling
   - Update executor trait if needed
   - This unblocks 3 failing tests

### Medium Priority
2. **Clean up unused imports** in test files
   - Reduces warning noise
   - Improves code clarity

3. **Add `#[allow(dead_code)]`** to test helper functions in `common/mod.rs`
   - Suppresses incorrect dead code warnings
   - Documents intent

### Low Priority
4. **Prefix unused variables with `_`**
   - `user_index_name` in manager.rs
   - `sum_func` in aggregate.rs

---

## Test Coverage Assessment

### Well Tested
- Cache layer (73 tests)
- Cypher CREATE operations (26 tests)
- Social network E2E (40+ tests)
- Schema management (15+ tests)
- Optimizer (10+ tests)

### Needs Improvement
- Vector search (3 failing tests)
- Error handling edge cases
- Concurrent access patterns

### Not Tested
- C-API (requires separate feature flag)
- gRPC (requires separate feature flag)
- Jieba segmentation (requires separate feature flag)

---

## Conclusion

The test suite is in good shape overall. The main remaining work is:

1. **Fix the vector search executor** to properly handle async operations (3 test failures)
2. **Clean up warnings** to improve code quality (low priority)

The dead code warnings are mostly false positives from test helper functions used by integration tests, not actual dead code that should be removed.
