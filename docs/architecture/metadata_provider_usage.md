# Metadata Provider Usage Example

## Overview

This document demonstrates how to use the new Metadata Provider system to pre-resolve index metadata during query planning.

## Architecture

The metadata provider system follows the PostgreSQL FDW (Foreign Data Wrapper) pattern:

```
Parser → Validator → Planner (with MetadataProvider) → Executor
                          ↓
                    Pre-resolve metadata
                          ↓
                    Store in Plan Node
```

## Key Components

### 1. MetadataProvider Trait

Located at: `src/query/metadata/provider.rs`

```rust
pub trait MetadataProvider: Send + Sync {
    fn get_index_metadata(&self, space_id: u64, index_name: &str) 
        -> Result<IndexMetadata, MetadataProviderError>;
    
    fn get_tag_metadata(&self, space_id: u64, tag_name: &str) 
        -> Result<TagMetadata, MetadataProviderError>;
    
    fn list_indexes(&self, space_id: u64) 
        -> Result<Vec<IndexMetadata>, MetadataProviderError>;
}
```

### 2. MetadataContext

Located at: `src/query/metadata/context.rs`

Stores pre-resolved metadata during planning phase:

```rust
let mut context = MetadataContext::new();
context.set_index_metadata("my_index".to_string(), index_metadata);
let metadata = context.get_index_metadata("my_index");
```

### 3. VectorIndexMetadataProvider

Located at: `src/query/metadata/vector_provider.rs`

Concrete implementation that queries VectorCoordinator:

```rust
let coordinator = Arc::new(VectorCoordinator::new(manager));
let provider = VectorIndexMetadataProvider::new(coordinator);
let metadata = provider.get_index_metadata(1, "my_index")?;
```

## Usage Example

### Step 1: Create Metadata Provider

```rust
use std::sync::Arc;
use graphdb::query::metadata::{MetadataProvider, VectorIndexMetadataProvider};
use graphdb::vector::VectorCoordinator;

// Create vector coordinator
let manager = Arc::new(VectorIndexManager::new_in_memory());
let coordinator = Arc::new(VectorCoordinator::new(manager));

// Create metadata provider
let metadata_provider = Arc::new(VectorIndexMetadataProvider::new(coordinator));
```

### Step 2: Build Metadata Context During Planning

```rust
use graphdb::query::metadata::MetadataContext;

// Create metadata context
let mut metadata_context = MetadataContext::new();

// Pre-resolve index metadata
let index_metadata = metadata_provider.get_index_metadata(space_id, "my_index")?;
metadata_context.set_index_metadata("my_index".to_string(), index_metadata);

// Create planner with metadata context
let planner = VectorSearchPlanner::with_metadata_context(Arc::new(metadata_context));
```

### Step 3: Generate Plan with Pre-resolved Metadata

```rust
// The planner will now use the pre-resolved metadata
let plan = planner.transform(&validated_statement, query_context)?;

// The resulting VectorSearchNode will have:
// - tag_name: "person" (pre-resolved)
// - field_name: "embedding" (pre-resolved)
// - metadata_version: 0 (optional version tracking)
```

### Step 4: Executor Uses Pre-resolved Metadata

```rust
// Executor receives the plan with pre-resolved metadata
// No need to query VectorCoordinator at runtime
let executor = VectorSearchExecutor::new(id, node, storage, expr_context, coordinator);

// Execution is faster because:
// 1. No runtime metadata lookup
// 2. No string matching
// 3. Direct access to tag_name and field_name
```

## Benefits

### Performance Improvement

**Before (Runtime Resolution)**:
```
Executor: Query indexes → String match → Extract metadata → Execute search
          ~5-10ms overhead per query
```

**After (Pre-resolution)**:
```
Planner: Query indexes → Cache metadata → Store in plan
Executor: Use pre-resolved metadata → Execute search
          ~0ms overhead
```

### Early Error Detection

**Before**: Index not found errors discovered at execution time
**After**: Index not found errors discovered at planning time

### Better Optimization

With pre-resolved metadata, the optimizer can:
- Choose the best index for a query
- Estimate costs more accurately
- Apply index-specific optimizations

## Backward Compatibility

The system maintains backward compatibility:

```rust
// Old code still works (executor resolves at runtime)
let planner = VectorSearchPlanner::new();
let plan = planner.transform(&validated, qctx)?;
// tag_name and field_name will be empty strings
// Executor will resolve them at runtime

// New code uses pre-resolution
let context = Arc::new(MetadataContext::new());
let planner = VectorSearchPlanner::with_metadata_context(context);
let plan = planner.transform(&validated, qctx)?;
// tag_name and field_name are pre-resolved
```

## Testing

Run the metadata provider tests:

```bash
cargo test --lib metadata
```

## Future Enhancements

1. **Schema Metadata Provider**: Integrate with RedbSchemaManager
2. **Metadata Caching**: Add TTL-based cache invalidation
3. **Version Tracking**: Detect metadata changes and re-plan queries
4. **Multi-Index Optimization**: Choose best index for multi-index queries

## References

- PostgreSQL FDW Architecture: https://www.postgresql.org/docs/current/fdwhandler.html
- Design Document: `docs/architecture/planner_executor_metadata_resolution.md`
