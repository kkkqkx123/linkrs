# Cargo Check Error Analysis Report

## Summary

- **Total Errors**: 29
- **Total Warnings**: 16
- **Total Issues**: 45
- **Unique Error Patterns**: 8
- **Unique Warning Patterns**: 15
- **Files with Issues**: 18

## Error Statistics

**Total Errors**: 29

### Error Type Breakdown

- **error[E0282]**: 13 errors
- **error[E0432]**: 8 errors
- **error[E0433]**: 6 errors
- **error[E0603]**: 2 errors

### Files with Errors (Top 10)

- `src\query\executor\data_access\vector_index.rs`: 9 errors
- `src\query\executor\data_access\vector_search.rs`: 7 errors
- `src\sync\manager.rs`: 6 errors
- `src\api\core\query_api.rs`: 2 errors
- `src\query\metadata\vector_provider.rs`: 1 errors
- `src\query\executor\factory\executor_factory.rs`: 1 errors
- `src\config\mod.rs`: 1 errors
- `src\vector\mod.rs`: 1 errors
- `src\query\executor\base\execution_context.rs`: 1 errors

## Warning Statistics

**Total Warnings**: 16

### Warning Type Breakdown

- **warning**: 16 warnings

### Files with Warnings (Top 10)

- `src\query\validator\statements\insert_vertices_validator.rs`: 4 warnings
- `crates\vector-client\src\embedding\service.rs`: 3 warnings
- `src\api\server\graph_service.rs`: 2 warnings
- `src\query\planning\statements\dml\insert_planner.rs`: 1 warnings
- `src\query\executor\expression\functions\builtin\aggregate.rs`: 1 warnings
- `src\sync\vector_sync.rs`: 1 warnings
- `src\query\executor\result_processing\agg_function_manager.rs`: 1 warnings
- `src\storage\event_storage.rs`: 1 warnings
- `crates\inversearch\src\config\validator.rs`: 1 warnings
- `src\api\core\query_api.rs`: 1 warnings

## Detailed Error Categorization

### error[E0282]: type annotations needed: cannot infer type

**Total Occurrences**: 13  
**Unique Files**: 4

#### `src\query\executor\data_access\vector_search.rs`: 6 occurrences

- Line 71: type annotations needed: cannot infer type
- Line 153: type annotations needed: cannot infer type
- Line 423: type annotations needed: cannot infer type
- ... 3 more occurrences in this file

#### `src\sync\manager.rs`: 4 occurrences

- Line 247: type annotations needed: cannot infer type
- Line 250: type annotations needed
- Line 269: type annotations needed: cannot infer type
- ... 1 more occurrences in this file

#### `src\query\executor\data_access\vector_index.rs`: 2 occurrences

- Line 103: type annotations needed: cannot infer type
- Line 215: type annotations needed: cannot infer type

#### `src\api\core\query_api.rs`: 1 occurrences

- Line 49: type annotations needed: cannot infer type

### error[E0432]: unresolved import `crate::vector::VectorCoordinator`: no `VectorCoordinator` in `vector`

**Total Occurrences**: 8  
**Unique Files**: 8

#### `src\query\metadata\vector_provider.rs`: 1 occurrences

- Line 12: unresolved import `crate::vector::VectorCoordinator`: no `VectorCoordinator` in `vector`

#### `src\config\mod.rs`: 1 occurrences

- Line 7: unresolved import `crate::vector::config`: could not find `config` in `vector`

#### `src\query\executor\base\execution_context.rs`: 1 occurrences

- Line 16: unresolved import `crate::vector::VectorCoordinator`: no `VectorCoordinator` in `vector`

#### `src\query\executor\factory\executor_factory.rs`: 1 occurrences

- Line 20: unresolved import `crate::vector::VectorCoordinator`: no `VectorCoordinator` in `vector`

#### `src\api\core\query_api.rs`: 1 occurrences

- Line 13: unresolved imports `crate::vector::VectorConfig`, `crate::vector::VectorCoordinator`, `crate::vector::VectorIndexManager`: no `VectorConfig` in `vector`, no `VectorCoordinator` in `vector`, no `VectorIndexManager` in `vector`

#### `src\sync\manager.rs`: 1 occurrences

- Line 13: unresolved import `crate::vector::VectorCoordinator`: no `VectorCoordinator` in `vector`, help: a similar name exists in the module: `VectorSyncCoordinator`

#### `src\query\executor\data_access\vector_search.rs`: 1 occurrences

- Line 21: unresolved import `crate::vector::VectorCoordinator`: no `VectorCoordinator` in `vector`

#### `src\query\executor\data_access\vector_index.rs`: 1 occurrences

- Line 15: unresolved import `crate::vector::VectorCoordinator`: no `VectorCoordinator` in `vector`

### error[E0433]: failed to resolve: could not find `config` in `vector`: could not find `config` in `vector`

**Total Occurrences**: 6  
**Unique Files**: 1

#### `src\query\executor\data_access\vector_index.rs`: 6 occurrences

- Line 20: failed to resolve: could not find `config` in `vector`: could not find `config` in `vector`
- Line 86: failed to resolve: could not find `config` in `vector`: could not find `config` in `vector`
- Line 89: failed to resolve: could not find `config` in `vector`: could not find `config` in `vector`
- ... 3 more occurrences in this file

### error[E0603]: struct import `VectorPointData` is private: private struct import

**Total Occurrences**: 2  
**Unique Files**: 2

#### `src\sync\manager.rs`: 1 occurrences

- Line 12: struct import `VectorPointData` is private: private struct import

#### `src\vector\mod.rs`: 1 occurrences

- Line 17: struct import `VectorPointData` is private: private struct import

## Detailed Warning Categorization

### warning: unused import: `crate::api::core::error::CoreError`

**Total Occurrences**: 16  
**Unique Files**: 10

#### `src\query\validator\statements\insert_vertices_validator.rs`: 4 occurrences

- Line 163: unused variable: `row_idx`: help: if this is intentional, prefix it with an underscore: `_row_idx`
- Line 164: unused variable: `tag_idx`: help: if this is intentional, prefix it with an underscore: `_tag_idx`
- Line 186: unused variable: `prop_idx`: help: if this is intentional, prefix it with an underscore: `_prop_idx`
- ... 1 more occurrences in this file

#### `crates\vector-client\src\embedding\service.rs`: 3 occurrences

- Line 40: field `usage` is never read
- Line 51: fields `prompt_tokens` and `total_tokens` are never read
- Line 118: method `add_auth` is never used

#### `src\api\server\graph_service.rs`: 2 occurrences

- Line 1: unused import: `crate::api::core::error::CoreError`
- Line 2: unused import: `crate::api::core::types::QueryRequest`

#### `src\sync\vector_sync.rs`: 1 occurrences

- Line 9: unused import: `warn`

#### `src\query\executor\result_processing\agg_function_manager.rs`: 1 occurrences

- Line 467: unused variable: `i`: help: if this is intentional, prefix it with an underscore: `_i`

#### `src\api\core\query_api.rs`: 1 occurrences

- Line 83: unused variable: `ctx`: help: if this is intentional, prefix it with an underscore: `_ctx`

#### `src\query\planning\statements\dml\insert_planner.rs`: 1 occurrences

- Line 6: unused import: `crate::query::metadata::MetadataContext`

#### `src\query\executor\expression\functions\builtin\aggregate.rs`: 1 occurrences

- Line 375: unused variable: `i`: help: if this is intentional, prefix it with an underscore: `_i`

#### `crates\inversearch\src\config\validator.rs`: 1 occurrences

- Line 16: unused import: `std::fmt`

#### `src\storage\event_storage.rs`: 1 occurrences

- Line 162: unused variable: `old_vertex`: help: if this is intentional, prefix it with an underscore: `_old_vertex`

