# Output Stream Module Design

## Overview

Unified output stream module for GraphDB, inspired by Go implementation in `ref/output/`.

## Core Concepts

### 1. Manager Pattern
- Central `Manager` controls output configuration
- Global default instance for convenience
- Per-instance customization support

### 2. Writer Abstraction
- Uses `std::io::Write` trait
- Supports: stdout, stderr, file, multi-writer
- Thread-safe with mutex protection

### 3. Format Support
- **Plain**: Human-readable text
- **JSON**: Machine-readable with pretty/compact modes
- **Table**: Structured tabular data

## Module Structure

```
src/utils/output/
├── mod.rs          # Public API exports
├── manager.rs      # OutputManager with global/default support
├── writer.rs       # Writer implementations (stdout, file, multi)
├── format.rs       # Format trait and implementations
├── json.rs         # JSON encoder/decoder
├── table.rs        # Table formatter
└── error.rs        # Error types
```

## Key Design Decisions

### 1. Dual API Pattern
```rust
// Global convenience functions
output::println("message");
output::print_json(&data);

// Instance-based for customization
let mut mgr = OutputManager::new()
    .with_format(Format::Json)
    .with_stdout(file);
mgr.println("message");
```

### 2. Builder Pattern for Configuration
```rust
let config = OutputConfig::new()
    .mode(OutputMode::Both)  // console, file, both
    .file_path("/path/to/file")
    .append(true)
    .format(Format::Json);
```

### 3. Format-Specific Formatters
```rust
pub trait Formatter {
    fn format<W: Write>(&self, data: &dyn Serialize, writer: &mut W) -> Result<()>;
}

impl Formatter for JsonFormatter { ... }
impl Formatter for TableFormatter { ... }
```

### 4. Multi-Writer Support
```rust
pub struct MultiWriter {
    writers: Vec<Box<dyn Write>>,
}

impl Write for MultiWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        for w in &mut self.writers {
            w.write_all(buf)?;
        }
        Ok(buf.len())
    }
}
```

## API Design

### Manager API
```rust
pub struct OutputManager {
    stdout: Box<dyn Write>,
    stderr: Box<dyn Write>,
    format: Format,
}

impl OutputManager {
    pub fn new() -> Self;
    pub fn with_stdout<W: Write>(self, w: W) -> Self;
    pub fn with_stderr<W: Write>(self, w: W) -> Self;
    pub fn with_format(self, format: Format) -> Self;
    
    // Output methods
    pub fn println(&mut self, msg: &str);
    pub fn print_error(&mut self, msg: &str);
    pub fn print_json<T: Serialize>(&mut self, data: &T);
    pub fn print_table(&mut self, headers: &[&str], rows: &[Vec<String>]);
}
```

### Global Functions
```rust
// Convenience functions using global manager
pub fn println(msg: &str);
pub fn print_error(msg: &str);
pub fn print_json<T: Serialize>(data: &T);
pub fn print_table(headers: &[&str], rows: &[Vec<String>]);

// With custom writer
pub fn println_to<W: Write>(writer: &mut W, msg: &str);
pub fn print_json_to<W: Write, T: Serialize>(writer: &mut W, data: &T);
```

## Integration Points

### 1. Query Result Output
```rust
// In query result processing
let result = execute_query(query);
output::print_json(&result);  // or output::print_table for CLI
```

### 2. Explain Plan Output
```rust
// Replace current format.rs
let plan = generate_plan();
output::print_table(&plan.headers(), &plan.rows());
// or
output::print_json(&plan);
```

### 3. Error Output
```rust
// Consistent error formatting
output::print_error(&err.to_string());
```

## Error Handling

```rust
pub enum OutputError {
    Io(io::Error),
    Json(serde_json::Error),
    InvalidConfig(String),
}

impl From<io::Error> for OutputError { ... }
impl From<serde_json::Error> for OutputError { ... }
```

## Thread Safety

- `OutputManager` uses `Mutex` for internal state
- `MultiWriter` uses `Mutex` for writer collection
- Writers must implement `Send + Sync`

## Performance Considerations

1. **Buffering**: Use `BufWriter` for file output
2. **Lazy Initialization**: Global manager initialized on first use
3. **Zero-Cost**: Direct trait calls, no dynamic dispatch overhead
