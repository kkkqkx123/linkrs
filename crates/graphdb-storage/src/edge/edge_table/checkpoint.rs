//! Incremental checkpoint: per-group topology, timestamp and property shards
//! plus segment statistics and a manifest.
//!
//! Layout of one edge-table directory (edge-directory layout, distinct from
//! the per-group CSR payload markers 8/9 bound in `edge_table::persistence`):
//! - `meta.bin`: header section only (label ids, schema, next edge id), with
//!   the manifest commit tail appended so metadata and manifest share one
//!   atomic unit.
//! - `groups_manifest.bin`: address width plus existing out/in group id lists.
//!   Missing groups read as empty and never produce files.
//! - `out_g{gid}.bin` / `in_g{gid}.bin`: one page-compressed base payload per
//!   existing group, each self-validated by its row header on load. Topology
//!   columns persist through the integer column path with per-column
//!   bit-packing or run-length encoding and a plain fallback. Rewritten
//!   only for groups carrying delete dirt (base merges) or missing files.
//! - `out_g{gid}.append.bin` / `in_g{gid}.append.bin`: committed append-log
//!   sidecars holding the write-through delta since the group base rewrite.
//!   Insert-only groups checkpoint by persisting the sidecar alone, so
//!   flushed bytes stay proportional to the dirty regions instead of the
//!   group size. The sidecar carries only the address width and is rejected
//!   on mismatch; it is cumulative across append-only flushes and deleted by
//!   the base merge that absorbs it.
//! - `ts_g{gid}.bin`: authoritative timestamps for the owning group's edges,
//!   falling with the same dirt as the group. Small timestamp writes rewrite
//!   only dirty owners, never the whole table.
//! - `props_g{gid}.bin`: property rows for the owning group's edges, falling
//!   with the same dirt as the group. Small property writes rewrite only
//!   dirty owners, never the whole table.
//! - `segment_stats.bin`: per-group segment statistics collected at the
//!   checkpoint for scan pruning, persisted alongside the shards and
//!   restored on load.
//!
//! Commit protocol: group bases, sidecars, timestamp shards, property shards
//! and the segment-statistics snapshot are written first (all through atomic
//! shadow files), then the metadata file carrying the manifest tail, and the
//! manifest file is published last as the snapshot commit point. Loading trusts
//! the manifest tail embedded in `meta.bin` when it differs from
//! `groups_manifest.bin`: metadata is published before the manifest, so a torn
//! commit holds new metadata plus the previous manifest file, and the embedded
//! tail already describes the durable group files. A missing tail still means
//! file damage and the load is rejected. The manifest
//! epoch is the snapshot epoch: shadow (`.tmp`) files written before the
//! manifest commit are discardable uncommitted state reclaimed at startup by
//! the shadow cleanup. Only groups holding uncheckpointed writes are written;
//! clean groups are skipped, so flushed bytes stay proportional to dirty
//! groups rather than the table size.
//!
//! Organization (`checkpoint/`):
//! - `layout`: file names, paths and group-file parsing
//! - `snapshot_cache`: frozen snapshot_cache-file sidecar cache
//! - `topology`: group base payloads plus append-log sidecars
//! - `timestamps`: per-group timestamp shards
//! - `properties`: property dirty tracking plus per-group property shards
//! - `segment_stats`: per-group segment statistics snapshot
//! - `commit`: metadata file plus manifest publish
//! - `orphans`: orphan group-file collection
//! - `flush`: incremental flush orchestration and metrics
//! - `load`: incremental load orchestration with torn-commit recovery

pub mod commit;
pub mod flush;
pub mod layout;
pub mod load;
pub mod orphans;
pub mod properties;
pub mod segment_stats;
pub mod snapshot;
pub mod snapshot_cache;
pub mod timestamps;
pub mod topology;

#[cfg(test)]
mod tests;

pub use layout::{
    in_append_file, in_group_file, out_append_file, out_group_file, props_group_file,
    ts_group_file, GROUPS_MANIFEST_FILE, SEGMENT_STATS_FILE,
};
