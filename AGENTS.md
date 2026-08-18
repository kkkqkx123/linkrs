# GraphDB Project Context

**No-backward-compatible**
At present, the project is in the development stage and there is no need to specifically consider backward compatibility. It is important to maintain a reasonable architecture.

## Language

Always use English in code, comments, logging, error info. Use Chinese in docs
**Never use any Chinese in any code files.**

## Project Overview

A lightweight single-node graph database reimplemented in Rust, focusing on local deployment.

## Architecture

Workspace with 11 sub-crates under `crates/`:

- `graphdb-core` - core data structures, types, errors
- `graphdb-config` - configuration management
- `graphdb-search` - fulltext search (BM25)
- `graphdb-sync` - synchronization primitives
- `graphdb-transaction` - transaction management
- `graphdb-storage` - storage engine (CSR, memory-mapped containers)
- `graphdb-query` - query engine, parser, executor
- `graphdb-api` - transport-independent core API + embedded/C-API
- `graphdb-server` - network service layer (HTTP/gRPC/web management)
- `graphdb-wire` - wire DTOs shared between server and CLI
- `graphdb-migration` - schema/data migration

Root `src/` has `lib.rs`, `main.rs`, `c_api.rs` with `pub use dep_crate::api as api` re-exports.

Dependency DAG: core → config → search → sync → transaction → storage → query → api → server

Outside crates: `crates/bm25`, `crates/qdrant-client`, `crates/graphdb-cli`, `crates/tantivy`

## Key Directories

- `crates/*` - sub-crates + third-party (bm25, vector-client, tantivy)
- `src/` - root crate (server binary, re-exports, C API)
- `tests/` - integration tests
- `proto/` - gRPC protobuf definitions

## Building and Running

Prerequisites: rustc 1.88.0, cargo 1.88.0

SIMD note (Phase 0): `.cargo/config.toml` sets `-C target-cpu=x86-64-v3`
(AVX2, Haswell+ 2013) for auto-vectorization. **A v3-built binary requires
AVX2 hardware at runtime** (the whole binary may emit AVX2 instructions;
the runtime kernel checks in `vector-search/src/distance/` only guard
baseline builds). For older CPUs rebuild with the baseline target:
`RUSTFLAGS="-C target-cpu=x86_64"` (or delete the `[target]` section).
`aarch64-*` targets are declared but not yet verified (NEON is ARMv8
baseline, no flags needed).

## Development Conventions

- Rust standard formatting (`cargo fmt`)
- Modular design following Rust conventions

## Testing

```shell
cargo test --lib -- --nocapture               # lib tests
cargo test <test_name>                         # specific test(s)
```

Test organization: unit tests in same file (`#[cfg(test)]`), separate `test.rs` for large files, integration tests in `tests/`, benchmarks in `benches/`.

## Coding Standards

- **Security**: Never use unwrap (use expect in tests). No unsafe except low-level ops, documented in `docs/archive/unsafe.md`.
- **Types**: Minimize `dyn`, prefer concrete types. All dynamic dispatch documented in `docs/archive/dynamic.md`.
- **Dependencies**: sub-crates form a strict DAG (no circular deps).
