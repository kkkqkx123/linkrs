//! Vertex table crash-recovery contract.
//!
//! Durability spans two layers with exactly one commit point:
//!
//! 1. Commit: the engine transaction WAL (`InsertVertexRedo` and peers)
//!    is the durable source of truth for committed but unflushed rows.
//!    Crash recovery replays it before tables serve reads, so a crash
//!    between commit and flush loses nothing as long as replay runs.
//! 2. Checkpoint: full or incremental flush writes shard files first and
//!    `commit_manifest.json` last. The manifest lists every file the open
//!    path may trust; files outside it are never read.
//! 3. Open: a manifest-listed table-critical file that is missing or corrupt
//!    refuses the whole table open instead of running sick. Per-column files
//!    (overflow sidecars and incremental column pages) degrade to
//!    column-unavailable instead. There is deliberately no degraded
//!    single-shard open: serving a subset of shards would hand out global
//!    IDs whose siblings silently vanished, which readers cannot distinguish
//!    from genuine absence.
//!
//! Fault-injection coverage for this contract lives in
//! `crates/graphdb-storage/tests/persistence_recovery.rs`: flush plus
//! reload, plus corrupt-manifest refusal (a manifest-listed file that is
//! missing or corrupt must fail the open).
//!
//! Split by responsibility; each submodule owns one persistence domain and
//! contributes either manifest types or an `impl ShardedVertexTable` block:
//! - `common`: manifest version plus wall-clock helper.
//! - `table_manifest`: shard layout pin that global IDs decode with.
//! - `commit_manifest`: one checkpoint pin written atomically last.
//! - `sidecar`: derived snapshot caches, verifiable but discardable.
//! - `health`: offline read-only inspection for half-damaged stores.
//! - `flush`: full plus incremental checkpoint writes.
//! - `load`: strict recovery reads plus delta apply.

pub(crate) mod commit_manifest;
mod common;
mod flush;
mod health;
mod load;
mod sidecar;
mod table_manifest;

#[cfg(test)]
mod tests;

pub(crate) use commit_manifest::CommitKind;
pub(crate) use common::now_ms;
