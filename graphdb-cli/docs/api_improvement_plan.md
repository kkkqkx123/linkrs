# GraphDB CLI API Improvement Plan

This document outlines the phased approach to align the `graphdb-cli` client with the server APIs provided by `src/api/server`.

## Overview

Based on the analysis in `docs/api/server/client_kernel_api_matching_analysis.md`, the current client-server API matching rate is approximately **65%**. This plan aims to achieve **90%+** coverage through incremental improvements.

## Current State Summary

### Implemented (Working)
- Health check
- Authentication (login)
- Session creation
- Query execution
- List spaces
- Get space details
- List tags (partial)
- List edge types (partial)

### Missing (Gaps)
- Transaction management
- Schema DDL operations (create/drop)
- Batch operations
- Statistics APIs
- Query validation
- Vector operations
- Configuration management
- Streaming queries
- Proper logout on disconnect

---

## Phase 1: Critical Fixes and Transaction Support

**Priority**: High  
**Estimated Effort**: 2-3 days  
**Target Coverage**: 75%

### Goals
1. Fix critical disconnect/logout issue
2. Add transaction management support
3. Maintain backward compatibility

### Tasks

#### 1.1 Fix Disconnect/Logout
**File**: `src/client/http.rs`, `src/client/client_trait.rs`

- Update `disconnect()` to call `POST /v1/auth/logout`
- Add session_id tracking for logout
- Handle logout errors gracefully

#### 1.2 Add Transaction Types
**File**: `src/client/http.rs`

Add request/response types:
```rust
#[derive(Debug, Serialize)]
struct BeginTransactionRequest {
    session_id: i64,
    read_only: bool,
    timeout_seconds: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct TransactionResponse {
    transaction_id: u64,
    status: String,
}
```

#### 1.3 Extend GraphDbClient Trait
**File**: `src/client/client_trait.rs`

Add methods:
```rust
async fn begin_transaction(&self, read_only: bool) -> Result<u64>;
async fn commit_transaction(&self, txn_id: u64) -> Result<()>;
async fn rollback_transaction(&self, txn_id: u64) -> Result<()>;
```

#### 1.4 Implement Transaction Methods
**File**: `src/client/http.rs`

Implement:
- `begin_transaction()` - POST `/v1/transactions`
- `commit_transaction()` - POST `/v1/transactions/{id}/commit`
- `rollback_transaction()` - POST `/v1/transactions/{id}/rollback`

#### 1.5 Add Transaction State Management
**File**: `src/transaction/manager.rs` (existing)

- Integrate HTTP transaction operations
- Maintain transaction state consistency

### Acceptance Criteria
- [ ] `disconnect()` properly calls logout endpoint
- [ ] Can begin, commit, and rollback transactions via HTTP
- [ ] Transaction IDs are properly tracked
- [ ] Error handling works for all transaction operations
- [ ] Unit tests pass

---

## Phase 2: Schema DDL Operations

**Priority**: High  
**Estimated Effort**: 2-3 days  
**Target Coverage**: 85%

### Goals
1. Add schema creation and deletion capabilities
2. Support property definitions
3. Enable full schema management via CLI

### Tasks

#### 2.1 Add Schema Types
**File**: `src/client/http.rs`

Add types:
```rust
#[derive(Debug, Serialize)]
struct CreateSpaceRequest {
    name: String,
    vid_type: Option<String>,
    comment: Option<String>,
}

#[derive(Debug, Serialize)]
struct CreateTagRequest {
    name: String,
    properties: Vec<PropertyDefInput>,
}

#[derive(Debug, Serialize)]
struct PropertyDefInput {
    name: String,
    data_type: String,
    nullable: bool,
}
```

#### 2.2 Extend GraphDbClient Trait
**File**: `src/client/client_trait.rs`

Add methods:
```rust
async fn create_space(&self, name: &str, vid_type: Option<&str>, comment: Option<&str>) -> Result<()>;
async fn drop_space(&self, name: &str) -> Result<()>;
async fn create_tag(&self, space: &str, name: &str, properties: Vec<PropertyDef>) -> Result<()>;
async fn create_edge_type(&self, space: &str, name: &str, properties: Vec<PropertyDef>) -> Result<()>;
```

#### 2.3 Implement Schema Methods
**File**: `src/client/http.rs`

Implement:
- `create_space()` - POST `/v1/schema/spaces`
- `drop_space()` - DELETE `/v1/schema/spaces/{name}`
- `create_tag()` - POST `/v1/schema/spaces/{name}/tags`
- `create_edge_type()` - POST `/v1/schema/spaces/{name}/edge-types`

#### 2.4 Add CLI Commands
**File**: `src/command/meta_commands.rs`, `src/command/executor.rs`

Add commands:
- `CREATE SPACE <name>`
- `DROP SPACE <name>`
- `CREATE TAG <name> (prop1 type1, ...)`
- `CREATE EDGE <name> (prop1 type1, ...)`

### Acceptance Criteria
- [ ] Can create and drop spaces via CLI
- [ ] Can create tags and edge types
- [ ] Property definitions are properly serialized
- [ ] Schema changes are reflected immediately
- [ ] Integration tests pass

---

## Phase 3: Batch Operations and Statistics

**Priority**: Medium  
**Estimated Effort**: 3-4 days  
**Target Coverage**: 90%

### Goals
1. Enable efficient bulk data operations
2. Provide visibility into query performance
3. Support data import/export workflows

### Tasks

#### 3.1 Add Batch Operation Types
**File**: `src/client/http.rs`

Add types for:
- `CreateBatchRequest`
- `BatchResponse`
- `BatchItem` (Vertex/Edge)
- `BatchStatusResponse`

#### 3.2 Extend GraphDbClient Trait
**File**: `src/client/client_trait.rs`

Add methods:
```rust
async fn create_batch(&self, space_id: u64, batch_type: BatchType, batch_size: usize) -> Result<String>;
async fn add_batch_items(&self, batch_id: &str, items: Vec<BatchItem>) -> Result<usize>;
async fn execute_batch(&self, batch_id: &str) -> Result<BatchResult>;
async fn get_batch_status(&self, batch_id: &str) -> Result<BatchStatus>;
async fn cancel_batch(&self, batch_id: &str) -> Result<()>;
```

#### 3.3 Add Statistics Methods
**File**: `src/client/client_trait.rs`, `src/client/http.rs`

Add methods:
```rust
async fn get_session_statistics(&self, session_id: i64) -> Result<SessionStatistics>;
async fn get_query_statistics(&self) -> Result<QueryStatistics>;
async fn get_database_statistics(&self) -> Result<DatabaseStatistics>;
```

#### 3.4 Integrate with Import/Export
**File**: `src/io/import.rs`, `src/io/export.rs`

- Use batch API for bulk imports
- Add progress reporting via batch status

### Acceptance Criteria
- [ ] Can create and execute batch tasks
- [ ] Bulk imports use batch API
- [ ] Can view session and query statistics
- [ ] Batch operations show progress
- [ ] Performance tests show improvement

---

## Phase 4: Advanced Features

**Priority**: Low  
**Estimated Effort**: 4-5 days  
**Target Coverage**: 95%

### Goals
1. Add query validation
2. Support configuration management
3. Enable vector operations (if needed)

### Tasks

#### 4.1 Query Validation
**File**: `src/client/http.rs`

Add:
```rust
async fn validate_query(&self, query: &str) -> Result<ValidationResult>;
```

#### 4.2 Configuration Management
**File**: `src/client/http.rs`

Add:
```rust
async fn get_config(&self) -> Result<ServerConfig>;
async fn update_config(&self, section: &str, key: &str, value: serde_json::Value) -> Result<()>;
```

#### 4.3 Vector Operations (Optional)
**File**: `src/client/http.rs`

Add vector search methods if CLI needs this feature.

### Acceptance Criteria
- [ ] Can validate queries before execution
- [ ] Can view server configuration
- [ ] All new features have tests

---

## Implementation Guidelines

### Code Structure

```
graphdb-cli/src/
├── client/
│   ├── client_trait.rs    # Trait definitions
│   ├── http.rs            # HTTP implementation
│   └── types.rs           # Shared types (new file)
├── command/
│   ├── meta_commands.rs   # Command definitions
│   └── executor.rs        # Command execution
└── transaction/
    └── manager.rs         # Transaction state
```

### Error Handling

All new methods should:
1. Use the existing `CliError` type
2. Provide meaningful error messages
3. Include HTTP status codes when relevant
4. Handle network timeouts gracefully

### Testing Strategy

1. **Unit Tests**: Mock HTTP responses
2. **Integration Tests**: Test against running server
3. **Error Cases**: Test failure scenarios
4. **Performance Tests**: Measure batch operation throughput

### Backward Compatibility

- All changes must be backward compatible
- Existing methods should not change signatures
- New methods are additions only
- Deprecate old methods if needed (not remove)

---

## Timeline

| Phase | Duration | Start | End | Deliverables |
|-------|----------|-------|-----|--------------|
| Phase 1 | 2-3 days | Week 1 | Week 1 | Transaction support, logout fix |
| Phase 2 | 2-3 days | Week 2 | Week 2 | Schema DDL operations |
| Phase 3 | 3-4 days | Week 3 | Week 3 | Batch operations, statistics |
| Phase 4 | 4-5 days | Week 4 | Week 4 | Advanced features |

**Total Estimated Time**: 11-15 days

---

## Risk Assessment

| Risk | Impact | Mitigation |
|------|--------|------------|
| Server API changes | High | Verify API contracts before implementation |
| Breaking changes | Medium | Maintain backward compatibility |
| Performance issues | Low | Benchmark batch operations |
| Test coverage | Medium | Require tests for all new code |

---

## Success Metrics

1. **API Coverage**: Reach 90%+ coverage of server APIs
2. **Test Coverage**: Maintain >80% code coverage
3. **Performance**: Batch operations 10x faster than individual inserts
4. **User Satisfaction**: All critical features available via CLI

---

## Related Documents

- `docs/api/server/http_api_specification.md` - Server API documentation
- `docs/api/server/client_kernel_api_matching_analysis.md` - Gap analysis
- `docs/client/design/cli_design.md` - CLI design principles
