# Output Stream Implementation Plan

## Phase 1: Core Infrastructure (Week 1)

### 1.1 Create Module Structure
```
src/utils/output/
├── mod.rs
├── error.rs
├── manager.rs
└── writer.rs
```

### 1.2 Implement Error Types
**File**: `src/utils/output/error.rs`
- Define `OutputError` enum
- Implement `From` traits for `io::Error`

### 1.3 Implement Writer Abstractions
**File**: `src/utils/output/writer.rs`
- `StdoutWriter`: Wrapper around stdout
- `FileWriter`: File output with buffering
- `MultiWriter`: Multiple writers with mutex protection

### 1.4 Implement Manager
**File**: `src/utils/output/manager.rs`
- `OutputManager` struct with stdout/stderr
- Global default instance
- Basic output methods (println, print_error)

### 1.5 Export Public API
**File**: `src/utils/output/mod.rs`
- Re-export all public types
- Provide global convenience functions

**Integration**: Update `src/utils/mod.rs` to include output module

---

## Phase 2: Format Support (Week 2)

### 2.1 Add Format Module
**File**: `src/utils/output/format.rs`
- Define `Format` enum (Plain, Json, Table)
- Define `Formatter` trait

### 2.2 Implement JSON Formatter
**File**: `src/utils/output/json.rs`
- `JsonFormatter` struct with indent/prefix options
- `print_json()` and `print_json_compact()` functions
- Global and instance-based APIs

### 2.3 Implement Table Formatter
**File**: `src/utils/output/table.rs`
- `TableFormatter` for tabular data
- Support headers and rows
- Configurable column widths

### 2.4 Update Manager
- Add `with_format()` builder method
- Add `print_formatted()` method

---

## Phase 3: Configuration & Stream (Week 3)

### 3.1 Add Configuration
**File**: `src/utils/output/config.rs`
- `OutputConfig` struct
- `OutputMode` enum (Console, File, Both)
- Validation methods

### 3.2 Implement Stream Output
**File**: `src/utils/output/stream.rs`
- `StreamOutput` for file + console output
- File creation with directory handling
- Append vs overwrite modes

### 3.3 Enhance Manager
- Add `from_config()` constructor
- Support file output mode

---

## Phase 4: Integration & Migration (Week 4)

### 4.1 Migrate explain/format.rs
- Replace string building with output manager
- Support JSON output for plans
- Keep backward compatibility

### 4.2 Add Result Output Methods
**File**: `src/core/query_result/result.rs`
- Add `to_output(&self, format: Format)` method
- Use output module for formatting

### 4.3 Migrate API Layer
**Files**: `src/api/mod.rs`, `src/api/server/http/handlers/*.rs`
- Replace `println!` with output module
- Support configurable output format

### 4.4 Add Tests
**File**: `tests/output_integration.rs`
- Test all output formats
- Test file output
- Test multi-writer

---

## Phase 5: Advanced Features (Optional)

### 5.1 Color Support
- Add `ColorManager` for terminal colors
- Support success/error/warning/info styles
- Auto-detect terminal capability

### 5.2 Progress Indicators
- Add progress bar support
- For long-running operations

### 5.3 Async Support
- Async output for high-throughput scenarios
- Channel-based writer

---

## Migration Strategy

### Gradual Replacement
1. New code uses output module
2. Existing code migrated file by file
3. Keep `println!` in tests for simplicity

### Backward Compatibility
- All new APIs are additive
- Existing `to_string()` methods remain
- Optional feature flag for output module

---

## Testing Checklist

- [ ] Unit tests for all formatters
- [ ] Integration tests for file output
- [ ] Thread safety tests
- [ ] Performance benchmarks
- [ ] Documentation examples
