# E2E Test Migration Plan: Python to Rust

## Overview

This document outlines the plan to migrate E2E tests from Python to Rust, improving maintainability, performance, and consistency with the main codebase.

## Current Issues

1. **Language Inconsistency**: Python tests in a Rust project
2. **Test Organization Chaos**: Multiple test locations with overlapping responsibilities
3. **Infrastructure Duplication**: Python client reimplements Rust client functionality
4. **Manual Server Lifecycle Management**: Tests manually start/stop servers
5. **Duplicate Tests**: Same functionality tested in multiple places

## Proposed Structure

```
tests/
├── integration/
│   ├── mod.rs
│   ├── common/
│   │   ├── mod.rs
│   │   ├── test_server.rs      # Server lifecycle management
│   │   ├── test_scenario.rs    # Fluent test API (existing)
│   │   ├── assertions.rs       # Common assertions (existing)
│   │   ├── data_fixtures.rs    # Test data generation (existing)
│   │   └── debug_helpers.rs    # Debug utilities (existing)
│   ├── e2e/
│   │   ├── mod.rs              # E2E test module
│   │   ├── social_network.rs   # Social network scenario tests
│   │   ├── optimizer.rs        # Query optimizer tests
│   │   ├── extended_types.rs   # Extended type tests
│   │   └── schema_manager.rs   # Schema manager tests
│   ├── transaction/            # Transaction tests (existing)
│   ├── ddl/                    # DDL tests (existing)
│   ├── dcl/                    # DCL tests (existing)
│   ├── sync/                   # Sync tests (existing)
│   └── cache/                  # Cache tests (existing)
├── common/                     # Shared utilities (consolidate existing)
│   ├── mod.rs
│   ├── lib.rs                  # Re-exports
│   ├── test_scenario.rs
│   ├── test_server.rs
│   ├── assertions.rs
│   ├── data_fixtures.rs
│   └── debug_helpers.rs
├── e2e_verify.py               # DELETE - replaced by Rust
├── server_startup_test.py      # DELETE - replaced by Rust
└── e2e/                        # DELETE - all Python tests
```

## Implementation Plan

### Phase 1: Create Test Infrastructure

1. **Create `tests/common/test_server.rs`**:
   - Server lifecycle management (start/stop)
   - Port allocation (dynamic to avoid conflicts)
   - Health check utilities
   - Authentication helpers

2. **Create `tests/integration/e2e/mod.rs`**:
   - Test module organization
   - Shared setup/teardown
   - Test configuration

3. **Update `tests/common/mod.rs`**:
   - Re-export common utilities
   - Consolidate existing helpers

### Phase 2: Migrate Test Suites

1. **Social Network Tests** (`test_social_network.py` → `social_network.rs`):
   - Basic connection and schema management
   - Data operations (insert, fetch)
   - Query operations (MATCH, GO, LOOKUP)
   - EXPLAIN/PROFILE commands
   - Transaction management

2. **Optimizer Tests** (`test_optimizer.py` → `optimizer.rs`):
   - Index selection
   - Join optimization
   - Aggregation strategies
   - TopN optimization
   - EXPLAIN format tests

3. **Extended Types Tests** (`test_extended_types.py` → `extended_types.rs`):
   - Geography types
   - Vector search
   - Full-text search

4. **Schema Manager Tests** (`test_schema_manager_init.py` → `schema_manager.rs`):
   - Basic operations
   - Error handling

### Phase 3: Create Test Runner

1. **Create `tests/integration/e2e/main.rs`**:
   - Binary target for running E2E tests
   - Command-line arguments for test selection
   - JUnit XML report generation
   - JSON report generation

2. **Add to `Cargo.toml`**:
   - New binary target for E2E tests
   - Feature flags for different test suites

### Phase 4: Cleanup

1. Remove `tests/e2e/` directory
2. Remove `tests/e2e_verify.py`
3. Remove `tests/server_startup_test.py`
4. Update documentation
5. Update CI/CD configuration

## Benefits

1. **Consistency**: All tests in Rust, same language as main codebase
2. **Performance**: Rust tests run faster than Python
3. **Type Safety**: Compile-time checks for test code
4. **Better Tooling**: Use Rust test framework features (cargo test, etc.)
5. **Simplified CI/CD**: No Python environment needed
6. **Code Reuse**: Share types and utilities with main codebase
7. **Maintenance**: Single language to maintain
8. **Integration**: Better integration with existing Rust test infrastructure

## Migration Strategy

### For Each Test File:

1. Create Rust equivalent in `tests/integration/e2e/`
2. Use `TestScenario` fluent API for test setup
3. Use `TestServer` for server lifecycle management
4. Use common assertions and fixtures
5. Ensure all tests pass
6. Delete Python original

### Test Parity:

- Ensure all test cases from Python have Rust equivalents
- Maintain same test names/IDs for traceability
- Keep same test organization (suites, test cases)
- Preserve test documentation

## Timeline

- Phase 1: 1-2 days (test infrastructure)
- Phase 2: 3-5 days (migrate test suites)
- Phase 3: 1 day (test runner)
- Phase 4: 1 day (cleanup)

Total: 6-9 days

## Risk Mitigation

1. **Run Python and Rust tests in parallel** during migration
2. **Gradual migration** - one test suite at a time
3. **Keep Python tests** until all Rust tests pass
4. **Test runner** can run both old and new tests during transition
