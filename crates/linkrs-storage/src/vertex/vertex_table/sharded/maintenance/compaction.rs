//! ID-space compaction across shards: gated remap versus stable absorption.

use super::super::ShardedVertexTable;
use super::thresholds::{hole_rate, STABLE_ROW_ID_HOLE_WATERMARK};
use crate::vertex::IdKey;
use linkrs_core::types::Timestamp;
use linkrs_core::StorageResult;

impl ShardedVertexTable {
    /// Compact vertices deleted at or before `ts` across all shards.
    /// Offline remap tool: re-densifies above-watermark shards and returns
    /// the old-to-new *global* internal ID mapping for edge endpoint
    /// propagation under the maintenance commit barrier, followed by a
    /// checkpoint. Production compaction uses
    /// [`Self::compact_with_cutoff_stable_collect`] instead and never moves
    /// live rows. The cutoff must be the watermark safe timestamp, never a
    /// bare transaction stamp.
    ///
    /// Returns the removed external keys, the old-to-new *global* internal
    /// ID mapping (shard-local rows translated into encoded global IDs),
    /// and the combined journal of the per-shard executions, which callers
    /// must propagate to edge CSR rows before dependent queries.
    ///
    /// Shards whose fragmentation ratio is below
    /// [`super::thresholds::SHARD_FRAGMENTATION_THRESHOLD`] are skipped
    /// (segment-level compaction): lazy ID recycling already reclaims their
    /// holes without a global remap, avoiding cross-shard coordination. The
    /// threshold is the hole-rate watermark for stable row IDs: below it,
    /// free-stack reuse absorbs deletes and live rows never move; above it,
    /// a barriered compact reclaims the shard and the returned mapping must
    /// propagate to edge endpoints before any new write is admitted.
    ///
    /// Each compacted shard runs under its shard write lock, which is the
    /// per-shard commit barrier: concurrent writes to that shard block
    /// while reads continue on their snapshots.
    pub fn compact_with_cutoff_collect_mapping(
        &self,
        cutoff: Timestamp,
    ) -> StorageResult<(
        Vec<IdKey>,
        std::collections::HashMap<u32, u32>,
        super::super::super::compaction::CompactionJournal,
    )> {
        let mut all_removed = Vec::new();
        let mut all_mapping = std::collections::HashMap::new();
        let mut journals = Vec::new();
        for (idx, shard) in self.shards.iter().enumerate() {
            // Selective compaction: only compact shards with significant
            // fragmentation to avoid global remapping overhead. The
            // long-term hole-rate watermark shares this threshold.
            {
                let table = shard.read();
                let (live, allocated) = table.id_hole_stats(cutoff);
                if allocated > 0 {
                    let frag = hole_rate(live, allocated);
                    if frag < STABLE_ROW_ID_HOLE_WATERMARK {
                        // Shard is sufficiently dense; lazy recycling will
                        // reclaim holes without compaction.
                        log::debug!(
                            "compaction skips dense shard {}: live={} allocated={} hole_rate={:.3}",
                            idx,
                            live,
                            allocated,
                            frag,
                        );
                        continue;
                    }
                    log::debug!(
                        "compaction remaps shard {}: live={} allocated={} hole_rate={:.3}",
                        idx,
                        live,
                        allocated,
                        frag,
                    );
                }
            }
            let mut table = shard.write();
            let (removed, local_mapping, journal) =
                table.compact_with_cutoff_collect_mapping(cutoff)?;
            journals.push(journal);
            for (old_local, new_local) in local_mapping {
                all_mapping.insert(
                    self.encode_id(idx, old_local),
                    self.encode_id(idx, new_local),
                );
            }
            all_removed.extend(removed);
        }
        let combined = super::super::super::compaction::CompactionJournal::combine(&journals);
        Ok((all_removed, all_mapping, combined))
    }

    /// Stable row-id collection across shards: hole absorption without moving
    /// any live row.
    ///
    /// Unlike [`Self::compact_with_cutoff_collect_mapping`], this path has no
    /// fragmentation gate (every shard absorbs its watermark-eligible holes
    /// through the free stack) and always returns an empty mapping, so the
    /// maintenance layer must produce zero edge endpoint rewrites. Use it to
    /// validate the long-term policy before retiring the remap cascade.
    pub fn compact_with_cutoff_stable_collect(
        &self,
        cutoff: Timestamp,
    ) -> StorageResult<(
        Vec<IdKey>,
        std::collections::HashMap<u32, u32>,
        super::super::super::compaction::CompactionJournal,
    )> {
        let mut all_removed = Vec::new();
        let mut journals = Vec::new();
        for shard in &self.shards {
            let mut table = shard.write();
            let (removed, mapping, journal) = table.compact_with_cutoff_stable_collect(cutoff)?;
            debug_assert!(mapping.is_empty());
            journals.push(journal);
            all_removed.extend(removed);
        }
        let combined = super::super::super::compaction::CompactionJournal::combine(&journals);
        Ok((all_removed, std::collections::HashMap::new(), combined))
    }
}
