# Streaming Execution System Completion Plan

**Analysis Date**: 2026/07/07  
**Current Status**: Framework complete, implementation incomplete (3.6/10)  
**Target**: Production-ready streaming executor  
**Timeline**: 4 weeks (Phase 1: 1 week, Phase 2: 2 weeks, Phase 3: 1 week)

---

## Executive Summary

Streaming execution framework exists with core components (DataChunk, Scheduler, WorkerPool, Engine) but **cannot be used** due to:
1. **Critical**: Hard-coded column mappings (5 columns only)
2. **Critical**: HashJoin implementation ignores join conditions
3. **Critical**: No integration with main executor system
4. **Important**: Missing 8+ essential operators (GroupBy, Distinct, Window, Set operations)
5. **Important**: No production testing

This plan fixes issues in priority order: enable basic usage → add missing operators → integrate with system → optimize.

---

## Phase 1: Core Functionality (Week 1, ~40 hours)

### Goal: Make streaming execution usable for basic queries

### 1.1 Fix Column Hard-Coding (8 hours) 🔴 CRITICAL

**Problem**: All operators support only 5 columns named "c0"-"c4"

**Root Cause**: 
```rust
// executor.rs - lines 256, 299, 365, 454, 526
let col_names = vec!["c0".to_string(), "c1".to_string(), "c2".to_string(), 
                      "c3".to_string(), "c4".to_string()];  // ❌ Hard-coded
```

**Solution**:

1. **Extend DataChunk with dynamic column names** (2 hours)
   - File: `crates/graphdb-query/src/query/executor/streaming/chunk.rs`
   - Change:
     ```rust
     pub struct DataChunk {
         pub rows: Vec<Vec<Value>>,
         pub schema: Arc<Schema>,  // Already exists but not used
     }
     
     impl DataChunk {
         /// Get column names from schema
         pub fn col_names(&self) -> Vec<String> {
             self.schema.columns.iter().map(|c| c.name.clone()).collect()
         }
     }
     ```

2. **Update ValueRowContext to accept dynamic columns** (2 hours)
   - File: `crates/graphdb-query/src/query/executor/streaming/executor.rs` (lines 38-86)
   - Change:
     ```rust
     impl StreamingExecutor {
         fn get_col_names(&self) -> Option<Vec<String>> {
             // Get from first chunk's schema, cache in executor state
             // Falls back to ["c0", "c1", ...] if schema unavailable
         }
     }
     ```

3. **Replace all hard-coded column usage** (4 hours)
   - Filter operator: lines 256-262
   - Project operator: lines 299-305
   - Aggregate operator: lines 365-371
   - Sort operator: lines 454-460
   - HashJoin operator: lines 526-532

**Test**: 
- Add test with 10+ columns
- Add test with custom column names
- Verify schema is preserved through pipeline

**Files to Modify**:
- `chunk.rs` - Extend Schema usage
- `executor.rs` - Replace 5 hard-coded instances
- Add test: `executor.rs` - test_dynamic_columns

---

### 1.2 Fix HashJoin Implementation (6 hours) 🔴 CRITICAL

**Problem**: 
```rust
// executor.rs lines 539-541
if let Some(key) = row.first() {  // ❌ Always uses first column
    let key_str = format!("{}", key);
    hash_table.entry(key_str).or_insert_with(Vec::new).push(row);
}
// ❌ join_condition parameter is never used
```

**Solution**:

1. **Implement join key extraction from condition** (3 hours)
   - File: `crates/graphdb-query/src/query/executor/streaming/executor.rs` (HashJoin variant)
   - Add helper function:
     ```rust
     fn extract_join_keys(
         left_row: &[Value],
         right_row: &[Value],
         col_names: &[String],
         join_condition: &Option<Expression>,
     ) -> String {
         // Evaluate join_condition to get join key
         // Handle multi-column keys: "key1|key2|key3"
         // Handle NULL: skip this row
     }
     ```

2. **Use join condition for row matching** (2 hours)
   - Replace simple equality with condition evaluation
   - Handle NULL values correctly
   - Support both equi-joins and theta-joins

3. **Handle multi-column join keys** (1 hour)
   - Current: single string key
   - Improved: support "col1=col2 AND col3=col4" syntax

**Test**:
- test_hash_join_with_multi_column_key
- test_hash_join_with_complex_condition
- test_hash_join_with_null_handling

**Files to Modify**:
- `executor.rs` - HashJoin next() implementation (lines 518-597)

---

### 1.3 Integrate Streaming with Main Executor System (12 hours) 🔴 CRITICAL

**Problem**: StreamingExecutionEngine exists but is never used. Main executor system in ExecutorEnum has no variant for it.

**Solution**:

1. **Add StreamingExecutionEngine to ExecutorEnum** (4 hours)
   - File: `crates/graphdb-query/src/query/executor/base/executor_enum.rs`
   - Add variant:
     ```rust
     pub enum ExecutorEnum {
         // ... existing variants ...
         Streaming(Box<StreamingExecutionEngine>),
     }
     ```
   - Implement Executor trait for variant
   - Handle open(), next_batch(), close()

2. **Create StreamingExecutorFactory** (6 hours)
   - File: New file `crates/graphdb-query/src/query/executor/factory/builders/streaming_builder.rs`
   - Convert ExecutionPlan nodes to StreamingExecutor
   - Support: Scan, Filter, Project, Limit, Sort, Aggregate, HashJoin
   - Fallback to Materialized mode for unsupported operators

3. **Add execution mode selection logic** (2 hours)
   - File: `crates/graphdb-query/src/query/executor/factory/mod.rs`
   - Choose ExecutionMode based on:
     - Query complexity
     - Plan structure (is it pipeline-friendly?)
     - Estimated data size
   - Default: Materialized (safe fallback)
   - Support: Config option to force streaming mode

**Files to Modify/Create**:
- `executor_enum.rs` - Add StreamingExecutionEngine variant
- `factory/builders/streaming_builder.rs` - NEW
- `factory/mod.rs` - Add mode selection logic
- `factory/plan_executor.rs` - Update to handle Streaming variant

---

### 1.4 Comprehensive Testing (14 hours)

**Unit Tests** (4 hours):
- File: `executor.rs` - Add test cases
  - test_scan_with_dynamic_columns (not just 5)
  - test_filter_with_many_columns
  - test_project_with_custom_column_names
  - test_aggregate_with_grouping
  - test_sort_with_multiple_keys
  - test_hash_join_multi_column
  - test_hash_join_null_handling

**Integration Tests** (6 hours):
- File: New `tests/streaming_integration_test.rs`
  - test_scan_and_filter_pipeline
  - test_scan_filter_project_pipeline
  - test_multi_partition_execution
  - test_backpressure_handling
  - test_worker_pool_coordination
  - test_scheduler_task_dependencies
  - test_error_propagation

**End-to-End Tests** (4 hours):
- File: `tests/streaming_e2e_test.rs`
  - test_simple_select_from_vertices
  - test_select_with_where_clause
  - test_select_with_aggregation
  - test_select_with_join
  - test_select_with_limit
  - test_select_with_order_by

**Success Criteria**:
- All new tests pass
- No existing tests broken
- Coverage >80% for streaming module
- No clippy warnings in streaming code

---

### Phase 1 Summary

| Task | Hours | Status | Files |
|------|-------|--------|-------|
| Fix column hard-coding | 8 | Ready | chunk.rs, executor.rs |
| Fix HashJoin | 6 | Ready | executor.rs |
| System integration | 12 | Design needed | 3 files |
| Testing | 14 | Ready | 3 test files |
| **Total** | **40** | **Design 70%** | **6 files** |

**Definition of Done**:
- ✅ All Phase 1 tests pass
- ✅ Streaming executor can execute basic queries
- ✅ No hard-coded column references
- ✅ HashJoin uses join condition
- ✅ Integrated with ExecutorEnum

---

## Phase 2: Missing Operators & Features (Week 2-3, ~60 hours)

### Goal: Add missing operators to support more query patterns

### 2.1 Add GroupBy Operator (12 hours)

**Current State**: Only Aggregate operator, which combines grouping + aggregation.

**Problem**: Cannot do independent grouping before aggregation.

**Implementation**:
- File: `executor.rs` - Add GroupBy variant
- Maintain group state with minimal buffering
- Emit groups as they complete
- Handle streaming GROUP BY

**Similar to**: Filter/Project - single-input operator
**Memory**: O(number of distinct groups)

---

### 2.2 Add Distinct Operator (6 hours)

**Implementation**:
- File: `executor.rs` - Add Distinct variant
- Use HashSet to track seen rows
- Stream through pipeline
- Emit only first occurrence of each row

**Memory**: O(number of distinct rows in window)

---

### 2.3 Add NestedLoopJoin Operator (8 hours)

**Current State**: Only HashJoin available.

**Use Cases**:
- Theta-joins (non-equi joins)
- Small right-side tables
- Backup for unsupported join types

**Implementation**: Simple nested loops with condition evaluation

---

### 2.4 Add Window Functions Support (16 hours)

**Include**: ROW_NUMBER, RANK, DENSE_RANK, LAG, LEAD, etc.

**Challenge**: Window functions need full partition before execution

**Implementation**:
- New operator: WindowFunction
- Buffer by PARTITION BY clause
- Compute window functions within partition
- Stream results

---

### 2.5 Set Operations (12 hours)

**Operators**: Union, UnionAll, Intersect, Except

**Implementation**:
- Each as separate operator variant
- Handle two input streams
- Merge/deduplicate appropriately

---

### 2.6 Operator Testing (6 hours)

- Unit tests for each new operator
- Integration tests for operator combinations
- Edge cases: empty inputs, NULL handling, etc.

---

## Phase 3: Robustness & Optimization (Week 4, ~40 hours)

### Goal: Make streaming executor production-ready

### 3.1 Streaming Aggregation (12 hours)

**Current Issue**: All data buffered to memory before aggregation

**Improvement**: 
- Maintain hash table of partial aggregates
- Emit complete groups as they become final
- Bounded memory: O(distinct groups)
- Requires sorted input or explicit GROUP BY boundaries

**Implementation**:
- New operator: StreamingAggregate (different from stateful Aggregate)
- Use for ORDER BY ... GROUP BY patterns
- Fallback to buffering Aggregate for out-of-order data

---

### 3.2 External Sort (Basic) (10 hours)

**Current Issue**: Sort buffers entire dataset

**Improvement**:
- Phase 1: Sort chunks independently  
- Phase 2: Merge sorted chunks
- Spill to disk if > memory threshold

**Implementation**:
- File: New `executor/streaming/external_sort.rs`
- Integrate with Sort operator
- Config parameter: `streaming.max_memory_per_sort`

---

### 3.3 Better Error Handling (8 hours)

**Issues**:
- Evaluation errors silently suppressed
- No diagnostic information
- Implicit type conversions

**Improvements**:
- Propagate evaluation errors with context
- Add error codes for different failure types
- Log error details for debugging

---

### 3.4 Performance Profiling (6 hours)

**Add**:
- Execution time per operator
- Memory usage tracking
- Chunk processing statistics
- Worker thread utilization

**Files**: New `metrics/streaming_metrics.rs`

---

### 3.5 Comprehensive Integration Tests (4 hours)

- Real workloads from test suite
- Large dataset scenarios
- Stress tests (many partitions, workers)
- Edge cases and boundary conditions

---

## Implementation Priority Matrix

```
Priority 1 (Phase 1 - Must Do):
├─ Fix column hard-coding ← START HERE
├─ Fix HashJoin
└─ Integrate with ExecutorEnum

Priority 2 (Phase 2 - Should Do):
├─ Add GroupBy
├─ Add Distinct
├─ Window functions
└─ Set operations

Priority 3 (Phase 3 - Nice to Have):
├─ Streaming aggregation
├─ External sort
└─ Performance optimization
```

---

## Technical Decisions

### 1. Fallback Strategy for Unsupported Operators

**Decision**: When streaming encounters unsupported operator, convert to Materialized mode

**Rationale**:
- Graceful degradation
- No query failures
- User can force streaming with config flag

**Implementation**: StreamingExecutorFactory.build() returns Option<StreamingExecutor>

### 2. Column Metadata Propagation

**Decision**: Require all DataChunk to have valid Schema with column names

**Rationale**:
- No surprises about column mapping
- Enables expression evaluation
- Self-documenting

**Enforcement**: Assert schema.columns.len() == rows[0].len()

### 3. Memory Management

**Decision**: Use threshold-based spilling (Phase 3)

**Current**: Naive buffering (Phase 1)
**Improved**: Check memory + fall back to disk (Phase 3)

**Config**: 
```
streaming:
  max_memory_per_op: 512MB  # Per aggregate/sort/join
  enable_spill_to_disk: false  # Phase 3
```

---

## Code Locations & File Mapping

### Core Streaming Module
```
crates/graphdb-query/src/query/executor/streaming/
├── mod.rs              # Module exports
├── base.rs             # ExecutionMode enum ✅
├── chunk.rs            # DataChunk (modify for columns)
├── executor.rs         # StreamingExecutor (8 operators, fix 3)
├── engine.rs           # StreamingExecutionEngine ✅
├── partition.rs        # PartitionView ✅
├── scheduler.rs        # PipelineScheduler ✅
├── worker.rs           # WorkerPool ✅
└── builder.rs          # StreamingExecutorBuilder (extend)
```

### Integration Points (New/Modify)
```
crates/graphdb-query/src/query/executor/
├── base/executor_enum.rs           # Add Streaming variant
├── factory/
│   ├── mod.rs                      # Add mode selection
│   ├── builders/
│   │   ├── streaming_builder.rs    # NEW - Plan → Executor
│   │   ├── data_processing_builder.rs  # Modify if needed
│   │   └── transformation_builder.rs   # Modify if needed
│   └── plan_executor.rs            # Handle Streaming variant
```

### Tests (New)
```
tests/
├── streaming_integration_test.rs    # NEW - Integration tests
├── streaming_e2e_test.rs           # NEW - End-to-end tests
└── existing tests continue to pass
```

---

## Risk Assessment

### High Risk

| Risk | Mitigation |
|------|-----------|
| Hard-coded columns breaks existing code | Provide schema with default names if missing |
| Integration breaks main executor | Extensive testing before merge |
| Memory blowup with large aggregates | Config limits + spill-to-disk (Phase 3) |

### Medium Risk

| Risk | Mitigation |
|------|-----------|
| Performance regression | Benchmark before & after |
| Missing operators needed by queries | Fallback to Materialized mode |
| Thread safety issues | Use Arc<Mutex<>> for shared state |

### Mitigation Strategy

1. **Feature flag**: `streaming_execution` (default off until Phase 2)
2. **Comprehensive testing**: >80% code coverage
3. **Config knobs**: Users can disable/tune behavior
4. **Clear fallback**: Automatic degradation to Materialized

---

## Definition of Done

### Phase 1 Done Criteria
- [ ] All hard-coded column references removed
- [ ] HashJoin uses join_condition correctly
- [ ] StreamingExecutionEngine integrated into ExecutorEnum
- [ ] 20+ new tests pass
- [ ] Existing tests pass
- [ ] Code review approved
- [ ] Streaming mode executes basic SELECT queries

### Phase 2 Done Criteria
- [ ] All 4 new operators implemented and tested
- [ ] Window function support working
- [ ] 30+ new tests pass
- [ ] Performance reasonable (<10% slower than Materialized)
- [ ] Fallback mechanism verified

### Phase 3 Done Criteria
- [ ] Streaming aggregation working
- [ ] External sort implemented
- [ ] Memory usage bounded and configurable
- [ ] Comprehensive profiling data available
- [ ] Production-ready for limited use cases

---

## Rollout Plan

### Week 1 (Phase 1)
1. Start with column hard-coding fix (no behavior change)
2. Fix HashJoin
3. Add integration layer
4. Extensive testing
5. **Commit**: Streaming execution usable for basic queries

### Week 2-3 (Phase 2)
1. Add missing operators iteratively
2. Test each operator thoroughly
3. **Commit**: Support wider range of queries

### Week 4 (Phase 3)
1. Optimize memory usage
2. Add profiling
3. **Commit**: Production-ready

---

## Related Files NOT Modified

These files should NOT need changes:
- Expression evaluator (already integrates)
- Value type system (compatible)
- Storage layer (already provides data)
- API layer (can add streaming later)

---

## Success Metrics

### Code Quality
- ✅ No clippy warnings
- ✅ >80% test coverage for streaming module
- ✅ No hard-coded magic numbers/strings
- ✅ Clear error messages

### Performance
- ✅ <5% overhead vs Materialized for simple queries
- ✅ Memory usage <10x data size
- ✅ Scales to multi-partition execution

### Usability
- ✅ Transparent mode selection
- ✅ Graceful fallback to Materialized
- ✅ Config options for advanced users

---

## Notes for Implementation

### Important: Hard-Coded Pattern
Before Phase 1, search for all occurrences:
```bash
grep -n "c0.*c1.*c2.*c3.*c4" crates/graphdb-query/src/query/executor/streaming/
```

Should find ~5 locations - all in executor.rs.

### Important: Schema Population
Ensure every DataChunk operation maintains valid schema:
```rust
pub fn from_rows(rows: Vec<Vec<Value>>) -> Self {
    let col_count = if rows.is_empty() { 0 } else { rows[0].len() };
    // Create schema with col_0, col_1, ... col_N
}
```

### Important: Testing Strategy
1. Unit tests: Individual operators with small datasets
2. Integration tests: Operator chains with realistic data
3. E2E tests: Full query execution through ExecutorEnum
4. Property tests: Random queries, verify against Materialized mode

---

## Appendix: Operator Implementation Checklist

For each operator, verify:
- [ ] Accepts dynamic column names from schema
- [ ] Preserves schema through next() calls
- [ ] Handles empty inputs correctly
- [ ] Handles NULL values correctly
- [ ] Bounded memory usage (or documented as stateful)
- [ ] Unit tests with 5+ columns
- [ ] Integration tests in pipeline
- [ ] Error cases tested

---

## Questions & Decisions Needed

Before starting implementation:

1. **Fallback Behavior**: Auto-convert to Materialized or error?
   - **Answer**: Auto-convert (safer)

2. **Column Naming**: If no schema, use "c0", "c1", ... or error?
   - **Answer**: Use defaults, but warn

3. **Feature Flag**: Hide streaming until Phase 1 complete?
   - **Answer**: Yes, behind `streaming_execution` flag

4. **Operator Completeness**: Implement all in Phase 1 or iteratively?
   - **Answer**: Iteratively (Core 7 in Phase 1, Rest in Phase 2)

---

**Document Version**: 1.0  
**Last Updated**: 2026-07-07  
**Next Review**: After Phase 1 completion
