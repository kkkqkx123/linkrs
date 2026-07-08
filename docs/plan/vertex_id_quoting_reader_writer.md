# VertexId Quoting Bug in Reader and Writer

## Problem

`VertexId::to_string()` wraps string IDs in quotes (`"person00001"`), but `as_str()` returns the raw string. When `to_string()` is used as a key for id_indexer lookups, it causes a key mismatch with vertices stored using `as_str()`, resulting in "Vertex not found" errors.

## Root Cause

In `crates/graphdb-core/src/core/types/storage_ids.rs:155-165`, the `Display` trait for `VertexId` adds quotes around string IDs:

```rust
impl fmt::Display for VertexId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(i) = self.as_int64() {
            write!(f, "{}", i)
        } else if let Some(s) = self.as_str() {
            write!(f, "\"{}\"", s)  // <-- adds quotes
        } else {
            write!(f, "{:?}", self.as_bytes())
        }
    }
}
```

## Affected Code Paths

### reader.rs (FIXED)

`get_vertex()` at line 37-42 used `id.to_string()` for vertex lookup:

```rust
// BEFORE (buggy):
let id_str = id.to_string();  // produces "\"p1\"" for string IDs
ctx.get_vertex(label_id, &id_str, ts)

// AFTER (fixed):
} else if let Some(id_str) = id.as_str() {
    ctx.get_vertex(label_id, id_str, ts)  // uses raw "p1"
} else {
    let id_str = id.to_string();
    ctx.get_vertex(label_id, &id_str, ts)
}
```

### writer.rs (FIXED)

Multiple functions used `to_string()` for vertex ID lookups:

1. `upsert_vertex()` at line 161 - vertex lookup
2. `upsert_vertex()` at line 185 - property update
3. `delete_vertex()` at line 232 - vertex deletion
4. `delete_tags()` at line 340 - tag deletion
5. `insert_vertex_data()` at line 622 - C-API vertex insertion
6. `update_vertex_prop()` at line 822 - property update

All fixed to use `as_str()` with `to_string()` fallback.

## Key Principle

| Operation | Key to Use | Why |
|-----------|-----------|-----|
| Vertex insert | `as_str()` | Stores raw string in id_indexer |
| Vertex lookup | `as_str()` | Matches stored key |
| Vertex update | `as_str()` | Matches stored key |
| Vertex delete | `as_str()` | Matches stored key |
| Display/Logging | `to_string()` | Quoted format is for human readability |

## Testing

After fixing, the following tests improved:
- Social network tests: GQL file loads successfully (no more "Vertex not found" on edge insertion)
- All edge insertion tests pass (30 friend, 20 works_at, 20 lives_in edges)

## Remaining Issues

The social network tests still fail because `MATCH (p:person) RETURN count(p)` returns 0 rows. This is a separate issue - the `scan_vertices` function returns no vertices even though the data was loaded. This may be related to:
1. Space name not being properly tracked after `USE` statement
2. MVCC visibility issue (read timestamp not covering written data)
3. `scan_vertices` not finding vertices in the correct space
