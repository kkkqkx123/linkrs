# GraphDB

[中文版](README_zh.md)

A **lightweight single-node graph database** implemented in Rust, focusing on local deployment. Inspired by NebulaGraph's data model (spaces, tags, edge types, properties) but built as a self-contained Rust workspace.

> **Status**: Pre-v1.0, active development. No backward compatibility guaranteed.

---

## Features

- **Graph Data Model** — Spaces, vertex tags, edge types with typed properties
- **Cypher-compatible Query Language** — MATCH, RETURN, CREATE, streaming execution
- **CSR Storage Engine** — Compressed Sparse Row adjacency storage with 5 variants + immutable Csr, MVCC, compaction
- **Full-text Search** — BM25 via tantivy with jieba Chinese tokenization support
- **Vector Search** — HNSW indexing via Qdrant external service (cosine, euclidean, dot)
- **Multiple API Surfaces**:
  - **HTTP** (axum) — REST API for general use
  - **gRPC** (tonic/prost) — 77+ RPCs for high-performance access
  - **Embedded API** — Direct Rust library integration
  - **C API** — SQLite-like bindings for foreign languages (cbindgen)
- **Web Management UI** — React + TypeScript dashboard (graphdb-studio)
- **CLI Client** — Interactive REPL via graphdb-cli
- **Comprehensive Benchmark** — Criterion.rs-based performance measurement

---

## Architecture

```
crates/
├── graphdb-core          # Core types, errors, data structures
├── graphdb-config        # Configuration management
├── graphdb-fulltext        # Full-text search (tantivy/BM25)
├── graphdb-sync          # Synchronization primitives
├── graphdb-transaction   # Transaction management (MVCC)
├── graphdb-migration     # Schema/data migration
├── graphdb-storage       # CSR storage engine
├── graphdb-query         # Query parser, optimizer, streaming executor
├── graphdb-api           # HTTP, gRPC, embedded, C API
└── vector-client         # Qdrant vector search client
```

Dependency flow: `core → config → search → sync → transaction → storage → query → api`

---

## Quick Start

### Prerequisites

- Rust 1.88.0+
- Cargo 1.88.0+

### Build & Run

> **SIMD build flags (Phase 0)**: `.cargo/config.toml` compiles with
> `-C target-cpu=x86-64-v3` (AVX2, Haswell+, 2013) for automatic
> vectorization (verified ~3.46x on the autovectorization benchmark).
> **A v3-built binary requires AVX2 hardware at runtime** (the whole binary
> may emit AVX2 instructions; the runtime kernel checks in the vector
> engine only guard baseline builds). To run on older CPUs, rebuild with the
> baseline target:
> `RUSTFLAGS="-C target-cpu=x86_64" cargo build --release`
> (or delete the `[target.x86_64-unknown-linux-gnu]` section).
> `aarch64-*` targets are declared in `.cargo/config.toml` but not yet
> verified (NEON is ARMv8 baseline, no flags needed).

```shell
# Build the server
cargo build --release

# Start the server (default port 9758)
cargo run --release -- serve

# Run a single query
cargo run --release -- query "CREATE TAG person(name string, age int)"

# Run with custom config
cargo run --release -- serve -c /path/to/config.toml
```

### Feature Flags

| Feature | Description |
|---------|-------------|
| `server` (default) | HTTP/management server |
| `fulltext-search` | Full-text search engine |
| `jieba` | Chinese text segmentation |
| `qdrant` | Vector search via Qdrant |
| `grpc` | gRPC server |
| `c_api` | C language API bindings |
| `embedded` | Embedded database mode |

```shell
cargo build --release --features "server,fulltext-search,grpc,c_api"
```

### Quick Checks

```shell
cargo check --workspace --all-features
cargo clippy --all-targets --all-features
cargo test --lib
```

---

## Configuration

Configuration is managed via `config.toml`:

- **`[database]`** — Host, port (default 9758), storage path, max connections
- **`[transaction]`** — Default timeout (30s), max concurrent, 2PC toggle
- **`[log]`** — Log level, directory, file, rotation
- **`[auth]`** — Authorization toggle, default credentials, session timeout
- **`[grpc]`** — gRPC port (9669), keepalive, timeouts
- **`[vector]`** — Vector search engine connection, timeouts, retry
- **`[optimizer]`** — Query optimizer settings
- **`[monitoring]`** — Metrics, cache, slow query threshold

---

## Project Structure

| Path | Description |
|------|-------------|
| `crates/` | 11 sub-crates (8 core + migration + vector-client + cli) |
| `src/` | Root crate: server binary, C API, library re-exports |
| `frontend/` | graphdb-studio: React + TypeScript web UI |
| `crates/graphdb-cli/` | Interactive CLI client |
| `proto/` | gRPC protobuf definitions |
| `tests/` | Integration + C API + E2E tests |
| `benches/` | Criterion.rs benchmarks |
| `docs/` | Architecture, storage, query, API documentation |
| `include/` | C header (cbindgen-generated) |

---

## API Surfaces

### HTTP API (axum)
Default port `9758`. RESTful endpoints for all graph operations.

### gRPC API (tonic)
Port `9669`. 77+ RPCs covering health, auth, session, query, schema, batch, vector index, and configuration.

### Embedded API
Use directly as a Rust library:

```rust
use graphdb::api::Database;
let db = Database::open("path/to/data")?;
db.execute("CREATE TAG person(name string)")?;
```

### C API
SQLite-style interface. Include `include/graphdb.h` and link against `libgraphdb`.

```c
graphdb *db;
graphdb_open("path/to/data", &db);
graphdb_execute(db, "CREATE TAG person(name string)", NULL, NULL);
graphdb_close(db);
```

---

## Query Language

GraphDB supports a Cypher-compatible query language:

```cypher
CREATE TAG person(name string, age int);
CREATE EDGE knows(since date);

CREATE (:person {name: "Alice", age: 30});
CREATE (:person {name: "Bob", age: 25});
MATCH (a:person)-[:knows]->(b:person) RETURN a.name, b.name;
```

---

## CLI Client

```shell
cd crates/graphdb-cli
cargo run -- --host localhost --port 9758
```

Interactive REPL with syntax highlighting, history, CSV export, and pagination.

---

## Web UI (graphdb-studio)

```shell
cd frontend
npm install
npm run dev
```

React + TypeScript dashboard with graph visualization (Cytoscape), Ant Design components, and i18n support.

---

## Benchmarks

```shell
cargo bench
```

Criterion.rs benchmarks covering: storage throughput, transaction performance, query parsing/traversal, full-text/vector search, API latency, and end-to-end workflows.

---

## Contributing

- Code: English only, Rust standard formatting (`cargo fmt`)
- Documentation: English for code, Chinese for docs
- No `unwrap` in production code (use `expect` in tests)
- Minimize `dyn`, prefer concrete types
- See `AGENTS.md` for detailed conventions

---

## License

Apache-2.0
