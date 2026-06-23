# Edge Rank Syntax `@0` Analysis

## Conclusion

The `@0` rank suffix on `INSERT EDGE` is **fully supported** across the entire query pipeline.
It should **NOT** be removed from GQL test files.

## Full Stack Support Evidence

### 1. Parser (`crates/graphdb-query/src/query/parser/parsing/dml_parser.rs:694-698`)

```rust
let rank = if ctx.match_token(TokenKind::At) {
    Some(self.parse_expression(ctx)?)
} else {
    None
};
```

Test: `"INSERT EDGE KNOWS(since) VALUES 1 -> 2 @0:('2020-01-01')"` — passes.

### 2. Validator (`crates/graphdb-query/src/query/validator/statements/insert_edges_validator.rs`)

- `validate_rank()` (line 200-229): accepts integer constants and variables
- `evaluate_rank()` (line 439-457): extracts `i64` from literal, defaults to 0

### 3. Planner (`crates/graphdb-query/src/query/planning/statements/dml/insert_planner.rs:91-96`)

Rank is passed as `Option<ContextualExpression>` through `EdgeInsertInfo.edges`.

### 4. Executor (`crates/graphdb-query/src/query/executor/factory/builders/data_modification_builder.rs:169-177`)

```rust
let rank = rank_expr
    .as_ref()
    .and_then(|e| e.get_expression())
    .and_then(|expr| Self::evaluate_literal(&expr))
    .and_then(|v| match v {
        crate::core::Value::BigInt(v) => Some(v),
        _ => None,
    })
    .unwrap_or(0);
```

**Note**: Only matches `Value::BigInt`, not `Value::Int`. `@0` parses as `Value::Int(0)` which falls through to default 0. This is a minor bug (rank always becomes 0 for Int literals) but does not cause failures.

### 5. Storage (`crates/graphdb-storage/src/storage/engine/graph_storage/writer.rs:421`)

`edge.ranking` is passed directly to `InsertEdgeParams`.

### 6. All 41 existing INSERT EDGE tests pass

Including:
- `test_insert_edge_with_rank` (parser)
- `test_insert_edge_with_rank` (execution)
- `test_insert_parser_edge_with_rank`
- `test_validate_rank_valid_integer`

## Real Issue: `Vertex not found` on Edge INSERT

The actual test failure is NOT caused by `@0`. The error occurs at statement #10110:

```
INSERT EDGE works_at(position, salary) VALUES "person00001" -> "comp064" @0: ("Engineer", 25369)
```

Error: `Vertex not found`

### Debugging Results

- All 10100 vertex INSERTs succeed (statements #1-#10109)
- `FETCH PROP ON person "person00001"` returns the vertex (internal_id=0)
- `FETCH PROP ON company "comp064"` returns the vertex (internal_id=63)
- But `INSERT EDGE works_at` still fails with "Vertex not found"

### Root Cause Hypothesis

This is a pre-existing bug in the MVCC timestamp/transaction isolation layer.
Each `execute_query` with `auto_commit: true` runs in its own transaction.
The vertex INSERT and edge INSERT use different write timestamps, and
`resolve_internal_id_from_str` → `get_internal_id` → `timestamps.is_valid(internal_id, ts)`
may fail due to timestamp ordering.

### Recommended Next Steps

1. **Keep `@0` in all GQL files** — it's correct syntax
2. **Investigate MVCC timestamp issue** — the `is_valid` check in `vertex_timestamp.rs:52-66`
3. **Consider batch loading** — use a single transaction for the entire GQL file instead of per-statement auto-commit
4. **Fix `Value::Int` rank handling** in `data_modification_builder.rs:173-176` to also accept `Value::Int`

## Files Modified

- `tests/e2e/data/optimizer_data.gql`: Added `FROM person TO company` to `CREATE EDGE` statements (DDL parser supports this syntax)
