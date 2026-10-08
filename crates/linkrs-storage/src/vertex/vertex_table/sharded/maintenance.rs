//! Sharded table maintenance entry point.
//!
//! Split by responsibility; each submodule owns one maintenance domain and
//! contributes an `impl ShardedVertexTable` block:
//! - `thresholds`: fragmentation watermark policy plus hole-rate helper.
//! - `gc`: version-chain garbage collection and pressure observability.
//! - `pk_stats`: primary-key index reuse and memory observability.
//! - `flush`: checkpoint advisory signals for the flush coordinator.
//! - `compaction`: ID-space compaction, gated remap versus stable absorption.
//! - `eviction`: cold-chunk eviction and unified buffer accounting.
//! - `memory`: table-level memory accounting and version-history handle.

mod compaction;
mod eviction;
mod flush;
mod gc;
mod memory;
mod pk_stats;
pub(crate) mod thresholds;
