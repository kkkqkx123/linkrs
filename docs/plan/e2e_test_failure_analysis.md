# E2E Test Failure Analysis and Fixes

## Issues Found and Fixed

### 1. VertexId Quoting Bug in reader.rs and writer.rs

**Problem**: `VertexId::to_string()` wraps string IDs in quotes (`"person00001"`), but `as_str()` returns the raw string. When `to_string()` is used as a key for id_indexer lookups, it causes key mismatch.

**Files Fixed**:
- `crates/graphdb-storage/src/storage/engine/graph_storage/reader.rs:37-42` - `get_vertex()` vertex lookup
- `crates/graphdb-storage/src/storage/engine/graph_storage/writer.rs` - Multiple functions:
  - `upsert_vertex()` - vertex lookup and property update
  - `delete_vertex()` - vertex deletion
  - `delete_tags()` - tag deletion
  - `insert_vertex_data()` - C-API vertex insertion
  - `update_vertex_prop()` - property update

**Fix Pattern**: Always use `as_str()` for id_indexer keys, with `to_string()` as fallback only for non-string, non-integer vertex IDs.

### 2. Social Network GQL Data File Missing City Tag

**Problem**: The `lives_in` edge references city names like "Shanghai" as destination vertices, but there was no `city` tag or city vertices defined.

**File Fixed**: `tests/e2e/data/social_network_data.gql`
- Added `city` tag definition BEFORE `lives_in` edge (edge requires `FROM person TO city`)
- Added 4 city vertices: Beijing, Guangzhou, Shanghai, Shenzhen
- Added `FROM person TO city` clause to `lives_in` edge definition

### 3. Multi-Statement Query Parsing Issue

**Problem**: Test queries like `"USE e2e_social_network; MATCH (p:person) RETURN count(p)"` are sent as a single query string. The parser only parses the first statement (`USE`), and the `MATCH` part is silently ignored. The result is a `SpaceSwitched` result instead of the expected count result.

**Root Cause**: `Parser::parse()` calls `parse_statement()` which only parses one statement. The query pipeline manager does not split statements by semicolons.

**File Fixed**: `tests/e2e/data_driven.rs`
- Removed `USE e2e_XXX;` prefix from all test queries
- The `TestDb` already tracks the current space after `load_gql_file()` executes the `USE` statement

## Remaining Issues (20 tests still failing)

### Social Network Tests (7 failures)
- `test_social_network_vertex_counts` - `MATCH (p:person) RETURN count(p)` returns 0 rows
- `test_social_network_edge_counts` - same underlying issue
- `test_social_network_filter` - same underlying issue
- `test_social_network_lookup_index` - same underlying issue
- `test_social_network_go_traversal` - same underlying issue

The data is loaded (edges are inserted successfully), but `MATCH (p:person)` returns no vertices. The `FilterExecutor` shows `rows=0` for `p`, which means `scan_vertices` returns no vertices.

**Possible Causes**:
1. Space name not properly tracked - `TestDb::execute_query()` passes `current_space_name` to `QueryRequest`, but the space name tracking only works if the `USE` statement result has a `space_name` column
2. MVCC visibility issue - read timestamp not covering written data
3. `scan_vertices` not finding vertices in the correct space

### Optimizer Tests (2 failures)
- `test_optimizer_vertex_count` - same `count(p) = 0` issue
- `test_optimizer_aggregate` - same underlying issue

### Geography Tests (1 failure)
- `test_geography_vertex_counts` - same underlying issue

### E-commerce Tests (1 failure)
- `test_ecommerce_vertex_counts` - same underlying issue

### Vector Tests (4 failures)
- `test_vector_insertion` - `expected VectorDense(128), got Null` - array literal parsing not supported
- `test_vector_vertex_count` - same underlying issue
- `test_cosine_similarity` - same underlying issue
- `test_filtered_vector_search` - same underlying issue
- `test_explain_vector_query` - same underlying issue

### Fulltext Tests (4 failures)
- `test_basic_search` - fulltext feature may not be enabled
- `test_fulltext_index_creation` - same
- `test_boolean_search` - same
- `test_explain_fulltext` - same

### Transaction Tests (2 failures)
- `test_transaction_commit` - to be investigated
- `test_transaction_rollback` - to be investigated

## Architecture Issue: TestDb Space Tracking

The `TestDb::execute_query()` function tracks space switching by checking if the result has a `space_name` column. This works for `USE` statement results. However, there's a subtle issue:

When `load_gql_file()` executes statements one by one:
1. `CREATE SPACE e2e_social_network (vid_type=STRING)` - no `space_name` column in result
2. `USE e2e_social_network` - has `space_name` column, so `current_space_name` is updated
3. `CREATE TAG IF NOT EXISTS person(...)` - no `space_name` column
4. `INSERT VERTEX person(...)` - uses `current_space_name` from step 2

This should work correctly. The issue is elsewhere.

## Next Steps

1. Investigate why `scan_vertices` returns 0 vertices even though data was loaded
2. Check if the `space_name` is being passed correctly from `TestDb` to the query pipeline
3. Check if the MVCC read timestamp is covering the written data
4. Fix vector test data to not use array literals (parser limitation)
5. Enable fulltext feature for fulltext tests
6. Investigate transaction test failures
