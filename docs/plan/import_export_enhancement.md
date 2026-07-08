# Data Import/Export Enhancement Design Plan

## 1. Overview

### 1.1 Background

The current GraphDB CLI already implements basic data import/export functionality:
- `\import csv/json/jsonl <file> <tag|edge> <name>` — Import from local files
- `\export csv/json/jsonl <file> <query>` — Export query results to files
- `\copy <target> from|to '<file>'` — psql-style copy commands
- Batch API (HTTP REST + gRPC + Embedded `BatchInserter`) — Programmatic bulk operations

However, compared to mature databases (Neo4j, PostgreSQL, MongoDB), several critical capabilities are missing:
- Database-level dump/restore (backup & recovery)
- Full space-level export (entire graph, not just query results)
- API-layer import/export endpoints (REST beyond batch)
- Schema import/export
- Remote data source support (S3, Azure Blob, GCS)
- Parallel/multi-threaded import
- Graph serialization formats (RDF, GraphML)

### 1.2 Reference Implementations

| Database | Feature | Implementation |
|----------|---------|----------------|
| Neo4j | `neo4j-admin database import full` | High-speed CSV/Parquet import into new database |
| Neo4j | `neo4j-admin database dump/restore` | Binary backup/restore |
| Neo4j | APOC `apoc.export.csv.all` | Export entire graph to CSV/JSON/GraphML/RDF |
| Neo4j | `s3://`, `azb://`, `gs://` | Remote data source support |
| PostgreSQL | `pg_dump` / `pg_restore` | Full database logical backup |
| PostgreSQL | `\copy` | Client-side CSV import/export |
| MongoDB | `mongodump` / `mongorestore` | Binary format backup/restore |
| MongoDB | `mongoimport` / `mongoexport` | JSON/CSV import/export |

### 1.3 Goals

1. Provide reliable backup/restore (dump/restore) for production use
2. Support full-space export (entire graph, not query-limited)
3. Expose import/export via HTTP API (not just CLI)
4. Support schema export/import for migration scenarios
5. Support remote data sources for cloud deployment
6. Improve import performance via parallelism

## 2. Architecture Design

### 2.1 Module Structure

```
crates/graphdb-cli/src/
├── io/
│   ├── mod.rs              # Module re-exports (extend existing)
│   ├── import.rs           # ImportConfig, ImportTarget, ImportFormat (extend)
│   ├── export.rs           # ExportConfig, ExportFormat (extend)
│   ├── dump.rs             # NEW: DumpConfig, DumpFormat
│   ├── restore.rs          # NEW: RestoreConfig
│   ├── schema_io.rs        # NEW: SchemaImportConfig, SchemaExportConfig (CLI layer)
│   ├── remote.rs           # NEW: RemoteSource, S3/Azure/GCS support
│   ├── csv/
│   │   ├── importer.rs     # CsvImporter (existing, extend with parallelism)
│   │   └── exporter.rs     # CsvExporter (existing)
│   ├── json/
│   │   ├── importer.rs     # JsonImporter (existing, extend with parallelism)
│   │   └── exporter.rs     # JsonExporter (existing)
│   ├── parquet/            # NEW (optional, low priority)
│   │   ├── importer.rs
│   │   └── exporter.rs
│   ├── rdf/                # NEW (Phase 5)
│   │   └── exporter.rs
│   └── graphml/            # NEW (Phase 5)
│       └── exporter.rs
├── command/
│   ├── parser/
│   │   └── meta/
│   │       └── io.rs       # Extend: add dump/restore/schema commands
│   └── executor/
│       └── mod.rs          # Extend: add dump/restore/schema handlers
```

```
crates/graphdb-api/src/
├── api/
│   ├── server/
│   │   └── http/
│   │       ├── handlers/
│   │       │   ├── batch.rs    # Existing
│   │       │   ├── import.rs   # NEW: POST /v1/import
│   │       │   └── export.rs   # NEW: GET /v1/export
│   │       └── router.rs       # Extend: add import/export routes
│   └── core/
│       ├── batch.rs            # Existing, extend with parallel execution
│       └── dump_restore.rs     # NEW: Core dump/restore logic
```

```
crates/graphdb-core/src/
└── core/
    └── types/
        ├── import_export.rs    # Existing, extend with new formats
        └── dump_restore.rs     # NEW: DumpMetadata, RestoreOptions
```

### 2.2 Core Data Structures

#### 2.2.1 Dump Configuration

```rust
// crates/graphdb-core/src/core/types/dump_restore.rs

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DumpConfig {
    pub source_space: Option<String>,       // None = all spaces
    pub format: DumpFormat,
    pub include_schema: bool,
    pub include_data: bool,
    pub compression: CompressionType,
    pub output_path: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DumpFormat {
    Binary,       // Custom binary format (fast, compact)
    JsonLines,    // JSONL (human-readable, easy to parse)
    Parquet,      // Apache Parquet (columnar, efficient)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CompressionType {
    None,
    Zstd,         // Default, good balance of speed/ratio
    Lz4,          // Fast decompression
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DumpMetadata {
    pub version: String,                   // GraphDB version
    pub timestamp: i64,                    // Unix timestamp
    pub spaces: Vec<SpaceDumpInfo>,
    pub checksum: String,                  // SHA256 of dump content
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpaceDumpInfo {
    pub name: String,
    pub vertex_count: u64,
    pub edge_count: u64,
    pub tags: Vec<String>,
    pub edge_types: Vec<String>,
}
```

#### 2.2.2 Restore Configuration

```rust
#[derive(Debug, Clone)]
pub struct RestoreConfig {
    pub source_path: PathBuf,
    pub target_space: Option<String>,       // Rename space during restore
    pub overwrite_existing: bool,
    pub strict_mode: bool,                   // Fail on schema conflicts
    pub restore_schema: bool,
    pub restore_data: bool,
}

#[derive(Debug, Clone, Default)]
pub struct RestoreStats {
    pub spaces_restored: usize,
    pub vertices_restored: u64,
    pub edges_restored: u64,
    pub errors: Vec<String>,
    pub duration_ms: u64,
}
```

#### 2.2.3 Remote Source Configuration

```rust
// crates/graphdb-cli/src/io/remote.rs

#[derive(Debug, Clone)]
pub enum RemoteSource {
    Local(PathBuf),
    S3 { bucket: String, key: String, region: Option<String> },
    AzureBlob { account: String, container: String, blob: String },
    Gcs { bucket: String, object: String },
    Http(String),     // HTTP/HTTPS URL
}

impl RemoteSource {
    pub fn is_remote(&self) -> bool {
        !matches!(self, RemoteSource::Local(_))
    }

    pub async fn open(&self) -> Result<Box<dyn AsyncRead + Unpin>> {
        match self {
            RemoteSource::Local(path) => {
                let file = tokio::fs::File::open(path).await?;
                Ok(Box::new(tokio::io::BufReader::new(file)))
            }
            RemoteSource::S3 { bucket, key, region } => {
                // Use aws-sdk-s3 to get object stream
                todo!()
            }
            RemoteSource::AzureBlob { .. } => {
                todo!()
            }
            RemoteSource::Gcs { .. } => {
                todo!()
            }
            RemoteSource::Http(url) => {
                // Use reqwest to get response stream
                todo!()
            }
        }
    }
}
```

#### 2.2.4 Parallel Batch Executor

```rust
// crates/graphdb-api/src/api/core/batch.rs (extend)

#[derive(Debug, Clone)]
pub struct ParallelBatchConfig {
    pub worker_count: usize,         // Number of parallel workers
    pub batch_per_worker: usize,     // Items per worker batch
    pub retry_on_conflict: bool,     // Retry individual items on conflict
}

impl Default for ParallelBatchConfig {
    fn default() -> Self {
        Self {
            worker_count: num_cpus::get(),
            batch_per_worker: 1000,
            retry_on_conflict: false,
        }
    }
}
```

### 2.3 Data Flow

#### 2.3.1 Dump Flow

```
\dump mydb /backup/graphdb.dump
    │
    ▼
DumpExecutor::execute()
    │
    ├─ 1. Read schema metadata (spaces, tags, edge_types)
    │
    ├─ 2. For each space:
    │   ├─ Stream all vertices → serialize → write to dump
    │   └─ Stream all edges → serialize → write to dump
    │
    ├─ 3. Compute checksum
    │
    └─ 4. Write DumpMetadata header
```

#### 2.3.2 Restore Flow

```
restore /backup/graphdb.dump mydb
    │
    ▼
RestoreExecutor::execute()
    │
    ├─ 1. Read & validate DumpMetadata
    │
    ├─ 2. Verify checksum
    │
    ├─ 3. For each space in dump:
    │   ├─ Create space/tags/edge_types if not exists
    │   └─ Batch insert vertices/edges
    │
    └─ 4. Verify counts match metadata
```

#### 2.3.3 Full Export Flow

```
\export csv /data/mydb.csv --space mydb --all
    │
    ▼
FullExporter::execute()
    │
    ├─ 1. List all tags and edge_types in space
    │
    ├─ 2. For each tag:
    │   └─ Stream vertices → serialize as CSV rows
    │
    ├─ 3. For each edge_type:
    │   └─ Stream edges → serialize as CSV rows
    │
    └─ 4. Write to output file (single or multi-file)
```

#### 2.3.4 Parallel Import Flow

```
\import csv /data/nodes.csv tag Person --threads 8
    │
    ▼
ParallelCsvImporter::import()
    │
    ├─ 1. Split file into chunks (by line count)
    │
    ├─ 2. For each chunk, spawn worker:
    │   ├─ Parse CSV records
    │   ├─ Build INSERT queries
    │   └─ Batch insert via storage layer
    │
    ├─ 3. Merge results & stats
    │
    └─ 4. Report summary
```

## 3. Implementation Plan

### Phase 1: Dump/Restore (Backup & Recovery)

**Priority**: Highest
**Estimated Effort**: ~2 weeks

#### 1.1 Core Types (`graphdb-core`)

- [ ] Add `core/types/dump_restore.rs` with:
  - `DumpConfig`, `DumpFormat`, `CompressionType`, `DumpMetadata`
  - `RestoreConfig`, `RestoreStats`
  - Serialization/deserialization for metadata

#### 1.2 Storage Layer (`graphdb-storage`)

- [ ] Add `StorageClient` methods:
  - `stream_vertices(space: &str) -> impl Stream<Item = Vertex>`
  - `stream_edges(space: &str) -> impl Stream<Item = Edge>`
  - `stream_vertices_by_tag(space: &str, tag: &str) -> impl Stream<Item = Vertex>`
  - `stream_edges_by_type(space: &str, edge_type: &str) -> impl Stream<Item = Edge>`
  - `insert_vertices_stream(space: &str, stream: impl Stream<Item = Vertex>) -> Result<()>`
  - `insert_edges_stream(space: &str, stream: impl Stream<Item = Edge>) -> Result<()>`

#### 1.3 Core Dump/Restore (`graphdb-api/core`)

- [ ] Add `api/core/dump_restore.rs`:
  - `DumpExecutor` — reads storage, serializes to dump format
  - `RestoreExecutor` — reads dump, writes to storage
  - Support for Binary and JSONL dump formats
  - Checksum verification (SHA256)
  - Zstd compression for binary format

#### 1.4 CLI Commands (`graphdb-cli`)

- [ ] Add meta command parsers in `command/parser/meta/io.rs`:
  - `parse_dump(arg)` — `\dump <database> <output_path> [--format binary|jsonl] [--compress zstd|lz4|none]`
  - `parse_restore(arg)` — `restore <input_path> <database> [--overwrite] [--strict]`
- [ ] Add command executors in `command/executor/mod.rs`:
  - `execute_dump()` — creates DumpExecutor, runs it
  - `execute_restore()` — creates RestoreExecutor, runs it
- [ ] Add help text in `command/meta_commands.rs`
- [ ] Add unit tests in `command/parser/tests.rs`

#### 1.5 Testing

- [ ] Unit tests for serialization/deserialization
- [ ] Integration test: dump → restore → verify data integrity
- [ ] Integration test: dump → restore with space rename
- [ ] Integration test: dump → restore with overwrite existing
- [ ] Performance test: large dataset (1M+ vertices) dump/restore timing

### Phase 2: Full Space Export & Schema I/O

**Priority**: High
**Estimated Effort**: ~1 week

#### 2.1 Full Space Export

- [ ] Extend `ExportConfig` with `export_all: bool` and `space: Option<String>`
- [ ] Extend `\export` parser: support `--space <name>` and `--all` flags
- [ ] Implement `FullExporter` in `graphdb-cli/src/io/`:
  - Lists all tags/edge_types in a space
  - Streams all data via storage layer
  - Supports CSV/JSON/JSONL output
- [ ] Option: multi-export mode (one file per tag/edge_type)

#### 2.2 Schema Import/Export

- [ ] Implement CLI commands:
  - `\export schema <file> [--format json|yaml]`
  - `\import schema <file>`
- [ ] Implement `SchemaExporter` — exports space definitions (tags, edge_types, properties)
- [ ] Implement `SchemaImporter` — imports schema, creates tags/edge_types
- [ ] Wire existing `SchemaExportConfig`/`SchemaImportResult` types

#### 2.3 Testing

- [ ] Integration test: full export → re-import → verify counts
- [ ] Integration test: schema export → create new space → schema import → verify structure
- [ ] Edge case: export empty space

### Phase 3: API Layer Import/Export Endpoints

**Priority**: Medium
**Estimated Effort**: ~1 week

#### 3.1 HTTP Import Endpoint

- [ ] Add `POST /v1/import` handler:
  - Accepts `multipart/form-data` (file upload)
  - Parameters: `space`, `format`, `target` (tag/edge), `batch_size`
  - Returns: import job ID (async) or sync result
- [ ] Add `GET /v1/import/{id}` for async status

#### 3.2 HTTP Export Endpoint

- [ ] Add `GET /v1/export` handler:
  - Parameters: `space`, `format`, `query` (optional), `all` (bool)
  - Returns: file stream with appropriate Content-Type/Content-Disposition
  - Supports streaming for large exports

#### 3.3 gRPC Import/Export Services

- [ ] Add to `proto/graphdb.proto`:
  - `rpc Import(ImportRequest) returns (ImportResponse)`
  - `rpc Export(ExportRequest) returns (stream ExportChunk)`
  - `rpc GetImportStatus(ImportStatusRequest) returns (ImportStatusResponse)`
- [ ] Implement gRPC handlers

#### 3.4 Testing

- [ ] Integration test: upload CSV via HTTP → verify data
- [ ] Integration test: download export via HTTP → verify file content
- [ ] Load test: concurrent import requests

### Phase 4: Remote Data Sources & Parallel Import

**Priority**: Medium
**Estimated Effort**: ~1.5 weeks

#### 4.1 Remote Source Support

- [ ] Add `crates/graphdb-cli/src/io/remote.rs`:
  - `RemoteSource` enum with Local/S3/Azure/GCS/Http variants
  - `RemoteSource::open()` → returns `AsyncRead`
- [ ] Add dependencies to `graphdb-cli/Cargo.toml`:
  - `aws-sdk-s3` (optional, behind feature flag)
  - `azure_storage_blobs` (optional, behind feature flag)
  - `object_store` crate (unified API, preferred approach)
- [ ] Update `ImportConfig` to accept `RemoteSource` instead of `PathBuf`
- [ ] Update CLI parser to detect URL scheme (s3://, azb://, gs://, http://)

#### 4.2 Parallel Import

- [ ] Extend `BatchProcessor` with parallel execution:
  - Use `rayon` for CPU-bound parsing
  - Use `tokio` tasks for IO-bound insertion
  - Configurable worker count via `--threads` parameter
- [ ] Add `ParallelBatchConfig` to `graphdb-api/core/batch.rs`
- [ ] Ensure thread safety: each worker operates on independent data partition

#### 4.3 Testing

- [ ] Unit test: parallel import produces same results as sequential
- [ ] Integration test: import from S3 (mock)
- [ ] Performance test: parallel vs sequential speedup

### Phase 5: Graph Serialization Formats (RDF/GraphML)

**Priority**: Low
**Estimated Effort**: ~1 week

#### 5.1 RDF Export

- [ ] Add `ExportFormat::Rdf` variant
- [ ] Implement RDF writer:
  - Map vertices to RDF resources
  - Map edges to RDF properties (object properties)
  - Map vertex properties to RDF datatype properties
  - Support Turtle and N-Triples serialization
- [ ] Consider using `rio_turtle` crate or custom serializer

#### 5.2 GraphML Export

- [ ] Add `ExportFormat::GraphMl` variant
- [ ] Implement GraphML writer:
  - Standard GraphML XML format
  - Include node/edge attributes as GraphML data elements

#### 5.3 Testing

- [ ] Integration test: export to RDF → validate with external RDF parser
- [ ] Integration test: export to GraphML → import into Gephi/yEd

## 4. CLI Command Syntax

### 4.1 New Commands

```sql
-- Dump entire database
\dump <database> <output_path> [--format binary|jsonl|parquet] [--compress zstd|lz4|none]

-- Restore from dump
restore <input_path> <database> [--overwrite] [--strict] [--schema-only] [--data-only]

-- Full space export
\export csv <file> --space <name> [--all] [--stream]
\export json <file> --space <name> [--all] [--stream]

-- Schema export/import
\export schema <file> [--format json|yaml]
\import schema <file>

-- Import with remote source
\import csv s3://bucket/data/nodes.csv tag Person
\import json https://example.com/data.json tag Person

-- Import with parallelism
\import csv <file> tag <name> [--threads <n>]
```

### 4.2 Extended Existing Commands

```sql
-- Export with space scope
\export csv <file> <query> [--stream] [--chunk-size <n>] [--space <name>]

-- Copy with remote source
\copy <tag> from 's3://bucket/data.csv'
```

## 5. API Endpoints

### 5.1 New REST Endpoints

| Method | Path | Description |
|--------|------|-------------|
| POST | `/v1/import` | Upload file for import (multipart/form-data) |
| GET | `/v1/import/{id}` | Get import job status |
| GET | `/v1/export` | Export data (streaming response) |
| POST | `/v1/dump` | Create database dump |
| GET | `/v1/dump/{id}` | Get dump status / download |
| POST | `/v1/restore` | Restore from dump |
| GET | `/v1/schema/export` | Export schema as JSON/YAML |
| POST | `/v1/schema/import` | Import schema from JSON/YAML |

### 5.2 New gRPC Services

```protobuf
service GraphDBImportExport {
  rpc Import(ImportRequest) returns (ImportResponse);
  rpc GetImportStatus(ImportStatusRequest) returns (ImportStatusResponse);
  rpc Export(ExportRequest) returns (stream ExportChunk);
  rpc Dump(DumpRequest) returns (DumpResponse);
  rpc Restore(RestoreRequest) returns (stream RestoreProgress);
}
```

## 6. Dependencies

### 6.1 New Dependencies

| Crate | Dependency | Purpose | Phase |
|-------|-----------|---------|-------|
| `graphdb-core` | `serde`, `serde_json` | Metadata serialization | 1 |
| `graphdb-core` | `sha2` | Checksum computation | 1 |
| `graphdb-cli` | `zstd` / `lz4_flex` | Compression | 1 |
| `graphdb-cli` | `rayon` | Parallel processing | 4 |
| `graphdb-cli` | `object_store` | Remote data source access | 4 |
| `graphdb-cli` | `tokio` (already exists) | Async file I/O | 1 |
| `graphdb-storage` | `futures::Stream` | Streaming API | 1 |
| `graphdb-api` | `multipart` (axum) | File upload handling | 3 |

### 6.2 Feature Flags

```toml
# graphdb-cli/Cargo.toml
[features]
default = []
dump-restore = ["zstd", "lz4_flex", "sha2"]
remote-sources = ["object_store"]
parallel-import = ["rayon"]
rdf-export = []
graphml-export = []
```

## 7. Error Handling

### 7.1 Error Types

```rust
#[derive(Debug, thiserror::Error)]
pub enum DumpError {
    #[error("Storage error: {0}")]
    Storage(#[from] StorageError),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Checksum mismatch: expected {expected}, got {actual}")]
    ChecksumMismatch { expected: String, actual: String },
    #[error("Unsupported dump version: {0}")]
    UnsupportedVersion(String),
    #[error("Dump file corrupted: {0}")]
    Corrupted(String),
}

#[derive(Debug, thiserror::Error)]
pub enum RestoreError {
    #[error("Storage error: {0}")]
    Storage(#[from] StorageError),
    #[error("Schema conflict: {0}")]
    SchemaConflict(String),
    #[error("Dump corrupted: {0}")]
    Corrupted(String),
    #[error("Space already exists: {0}")]
    SpaceExists(String),
}
```

### 7.2 Error Recovery

- Dump: write to temp file, rename on completion (atomic)
- Restore: rollback on failure (use transaction if possible)
- Import: continue-on-error mode with error log
- Parallel import: per-chunk error isolation

## 8. Performance Considerations

### 8.1 Streaming Design

All dump/restore/export operations use streaming to handle datasets larger than available memory:
- Read from storage in batches (1000-10000 items)
- Serialize and write immediately
- Never hold entire dataset in memory

### 8.2 Compression

- Default: zstd level 3 (good balance)
- Optional: lz4 for faster decompression (restore scenarios)
- Compress in chunks to support streaming

### 8.3 Parallelism

- Parallel import: split input file into chunks, parse in parallel, batch insert sequentially (to avoid transaction conflicts)
- Configurable: `--threads N` or auto-detect CPU cores
- Memory bound: limit total buffered items across all threads

### 8.4 Benchmarks (Target)

| Operation | Dataset Size | Target Time |
|-----------|-------------|-------------|
| Dump | 1M vertices, 5M edges | < 30s |
| Restore | 1M vertices, 5M edges | < 60s |
| Full Export | 1M vertices | < 20s |
| Parallel Import (8 threads) | 1M rows | < 15s |

## 9. Security Considerations

- Validate file paths to prevent path traversal attacks
- Authenticate remote source credentials (use IAM roles, not hardcoded keys)
- Rate limit import/export endpoints to prevent DoS
- Sanitize schema names during restore to prevent injection
- Checksum verification on restore to detect corruption/tampering

## 10. Migration & Compatibility

- Dump format version field for future format evolution
- Graceful handling of unknown fields (forward compatibility)
- CLI: new commands are additive, existing commands unchanged
- API: new endpoints are additive, no breaking changes to existing endpoints
