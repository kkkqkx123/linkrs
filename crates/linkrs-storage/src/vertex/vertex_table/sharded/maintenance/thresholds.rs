//! Fragmentation watermark policy shared by compaction paths.

/// Fragmentation ratio threshold above which a shard is considered for
/// selective compaction. Segments with low fragmentation are skipped to
/// avoid global remapping. Crate-visible so the benchmark coverage gate
/// can pin it: any retune must keep its scan-bench coverage.
pub(crate) const SHARD_FRAGMENTATION_THRESHOLD: f64 = 0.25;

/// Long-term hole-rate watermark for stable row ids. Aliases the selective
/// compaction threshold so the watermark policy has one named anchor: below
/// it shards fold version chains only and live rows never move; above it the
/// offline remap path re-densifies while the stable path still moves nothing
/// and absorbs holes through the free stack. Shard-count manifests stay the
/// identifier-decoding anchor.
pub const STABLE_ROW_ID_HOLE_WATERMARK: f64 = SHARD_FRAGMENTATION_THRESHOLD;

/// Hole rate `1 - live / allocated` for one shard snapshot.
pub fn hole_rate(live: usize, allocated: usize) -> f64 {
    if allocated == 0 || live >= allocated {
        0.0
    } else {
        1.0 - (live as f64 / allocated as f64)
    }
}
