# GraphDB Project Context

**No-backward-compatible**
At present, the project is in the development stage and there is no need to specifically consider backward compatibility. It is important to maintain a reasonable architecture.

## Language

Always use English in code, comments, logging, error info. Use Chinese in docs
**Never use any Chinese in any code files.**

## Project Overview

A lightweight single-node graph database reimplemented in Rust, focusing on local deployment.

## Architecture

Workspace with 8 sub-crates under `crates/`:

- `graphdb-core` - core data structures, types, errors
- `graphdb-config` - configuration management
- `graphdb-search` - fulltext search (BM25)
- `graphdb-sync` - synchronization primitives
- `graphdb-transaction` - transaction management
- `graphdb-storage` - storage engine (CSR, memory-mapped containers)
- `graphdb-query` - query engine, parser, executor
- `graphdb-api` - API layer (HTTP, gRPC, embedded/C-API)

Root `src/` has `lib.rs`, `main.rs`, `c_api.rs` with `pub use dep_crate::api as api` re-exports.

Dependency DAG: core → config → search → sync → transaction → storage → query → api

Outside crates: `crates/bm25`, `crates/qdrant-client`, `./graphdb-cli`

## Key Directories

- `crates/*` - 8 sub-crates + third-party (bm25, vector-client)
- `src/` - root crate (server binary, re-exports, C API)
- `tests/` - integration tests
- `proto/` - gRPC protobuf definitions

## Building and Running

Prerequisites: rustc 1.88.0, cargo 1.88.0

```shell
cargo clippy --all-targets --all-features            # full compile check
cargo check --workspace --features server,fulltext-search,c-api,grpc,qdrant  # check with all features
```

## Development Conventions

- Rust standard formatting (`cargo fmt`)
- Modular design following Rust conventions

## Completed Storage Refactoring

The following cleanups are done (see `docs/storage/remaining_work.md` for remaining):

| Phase | Description |
|-------|-------------|
| 1 | Deleted `TransactionWriter`, `QueryOps`, `EdgeTraversalParams` |
| 2 | Deleted `IndexUpdater`, `IndexDataManager` trait, unused index types |
| 3 | Column encoding integrated into compact/freeze and flush/load |
| 4 | Physical zstd compression wired into table flush/load pipeline |
| 7 | Deleted `InsertEdgeUndoParams`, `LoadFromPartsParams`, index method cleanup |

## Testing

```shell
cargo test --lib -- --nocapture               # lib tests
cargo test --test '*' -- --nocapture           # integration tests
cargo test <test_name>                         # specific test(s)
```

Test organization: unit tests in same file (`#[cfg(test)]`), separate `test.rs` for large files, integration tests in `tests/`, benchmarks in `benches/`.

## Coding Standards

- **Security**: Never use unwrap (use expect in tests). No unsafe except low-level ops, documented in `docs/archive/unsafe.md`.
- **Types**: Minimize `dyn`, prefer concrete types. All dynamic dispatch documented in `docs/archive/dynamic.md`.
- **Dependencies**: 8 sub-crates form a strict DAG (no circular deps).
