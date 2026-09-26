use super::ShardedVertexTable;
use crate::vertex::IdKey;
use graphdb_core::types::Timestamp;
use graphdb_core::StorageResult;

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

impl ShardedVertexTable {
    /// GC split into (reclaimed vertices, reclaimed version-chain entries).
    /// Stable row-id semantic: reclaimed vertices only lose their keys to
    /// the free stack, live rows never move, so no cache invalidation is
    /// required for this label. The cutoff must be the global watermark
    /// safe timestamp.
    pub fn gc_detailed(&self, min_ts: Timestamp) -> StorageResult<(usize, usize)> {
        let mut reclaimed_vertices = 0;
        let mut version_entries = 0;
        for shard in &self.shards {
            let (vertices, versions) = shard.write().gc_detailed(min_ts)?;
            reclaimed_vertices += vertices;
            version_entries += versions;
        }
        Ok((reclaimed_vertices, version_entries))
    }

    /// Fold version-chain before-images eligible at `cutoff` across shards.
    ///
    /// Fold-only (no ID remap): safe to run inside coordinated maintenance
    /// passes that already handle ID re-densification separately. The cutoff
    /// must be the watermark safe timestamp shared with the rest of the pass.
    /// Returns the total number of version entries folded.
    pub fn fold_version_chains(&self, cutoff: Timestamp) -> usize {
        self.shards
            .iter()
            .map(|shard| shard.write().fold_version_chains(cutoff))
            .sum()
    }

    /// Aggregate primary-key reuse across shards for observability.
    ///
    /// Returns `(cumulative_reuses, free_depth_total)`: how many inserts
    /// recycled a deleted slot versus growing the id space, and how many
    /// holes await reuse. A growing high-water mark beside a flat reuse
    /// count means deletes are not being absorbed and compaction pressure
    /// builds instead.
    pub fn pk_reuse_stats(&self) -> (u64, usize) {
        let mut reuses = 0u64;
        let mut free_depth = 0usize;
        for shard in &self.shards {
            let table = shard.read();
            reuses = reuses.saturating_add(table.id_indexer.reuse_count());
            free_depth += table.id_indexer.free_depth();
        }
        (reuses, free_depth)
    }

    /// Aggregate primary-key index heap across shards for budget checks.
    ///
    /// Returns `(total_bytes, max_shard_bytes)`: the whole-table resident
    /// cost and the hottest shard. Operators compare these against the
    /// deployment budget; the index is fully resident, so a breach calls
    /// for a persistent-index overflow path, not larger flush thresholds.
    pub fn pk_memory_stats(&self) -> (usize, usize) {
        let mut total = 0usize;
        let mut max_shard = 0usize;
        for shard in &self.shards {
            let bytes = shard.read().id_indexer.memory_breakdown().total_bytes;
            total += bytes;
            max_shard = max_shard.max(bytes);
        }
        (total, max_shard)
    }

    /// Aggregate per-component primary-key memory accounting across shards.
    ///
    /// Sums every breakdown field so patrol logs show which structure (key
    /// heap, map, live set, delta log, sampler) dominates before a shard
    /// crosses its memory budget.
    pub fn pk_memory_breakdown(&self) -> crate::vertex::id_indexer::IdIndexMemoryBreakdown {
        let mut total = crate::vertex::id_indexer::IdIndexMemoryBreakdown::default();
        for shard in &self.shards {
            let breakdown = shard.read().id_indexer.memory_breakdown();
            total.slot_count += breakdown.slot_count;
            total.live_count += breakdown.live_count;
            total.free_depth += breakdown.free_depth;
            total.delta_entries += breakdown.delta_entries;
            total.delta_heap_bytes += breakdown.delta_heap_bytes;
            total.keys_heap_bytes += breakdown.keys_heap_bytes;
            total.map_bytes += breakdown.map_bytes;
            total.set_bytes += breakdown.set_bytes;
            total.free_bytes += breakdown.free_bytes;
            total.sketch_bytes += breakdown.sketch_bytes;
            total.total_bytes += breakdown.total_bytes;
        }
        total
    }

    /// Degraded-branch budget gate for the write entry.
    ///
    /// Sums the resident primary-key heap and refuses the write when it is
    /// already over `budget`. Thresholds come from deployment configuration;
    /// `None` disables the gate (tiering branch or ungated tables).
    pub fn check_pk_budget(
        &self,
        budget: Option<crate::vertex::tiering::PkIndexBudget>,
    ) -> graphdb_core::StorageResult<()> {
        let Some(budget) = budget else {
            return Ok(());
        };
        let (total, _) = self.pk_memory_stats();
        budget.check(total)
    }

    /// Aggregate primary-key probe traffic across shards for the cold-hot
    /// tiering design.
    ///
    /// Returns `(total_probes, max_shard_probes)`: how many key probes the
    /// resident index has served and where they concentrate. A skewed
    /// distribution (small hot set, large total) is the precondition for
    /// spilling cold keys to a persistent index; uniform traffic means
    /// tiering would only add a lookup hop with no memory win.
    pub fn pk_probe_stats(&self) -> (u64, u64) {
        let mut total = 0u64;
        let mut max_shard = 0u64;
        for shard in &self.shards {
            let probes = shard.read().id_indexer.probe_total();
            total = total.saturating_add(probes);
            max_shard = max_shard.max(probes);
        }
        (total, max_shard)
    }

    /// Aggregate flush trigger signals across shards.
    ///
    /// Sums delta entries and live rows, ORs the compaction-moved-rows
    /// flag, and pairs them with the table-wide dirty-page ratio and
    /// baseline age. Feeds [`super::super::flush_trigger::decide`]; the
    /// decision is logged with its reason code at each flush so full
    /// rewrites stay explainable.
    pub fn flush_signals(&self) -> super::super::flush_trigger::FlushSignals {
        use std::sync::atomic::Ordering;
        let mut delta_entries = 0usize;
        let mut live_rows = 0usize;
        let mut baseline_invalidated = false;
        for shard in &self.shards {
            let table = shard.read();
            delta_entries += table.id_indexer.delta_len();
            live_rows += table.id_indexer.len();
            baseline_invalidated |= table.id_indexer.baseline_invalidated();
        }
        let total_pages = self.total_pages();
        let dirty_page_ratio = if total_pages == 0 {
            0.0
        } else {
            self.total_dirty_pages() as f64 / total_pages as f64
        };
        let last_baseline = self.last_full_flush_ms.load(Ordering::Acquire);
        let millis_since_baseline = if last_baseline == 0 {
            u64::MAX
        } else {
            super::persistence::now_ms().saturating_sub(last_baseline)
        };
        super::super::flush_trigger::FlushSignals {
            delta_entries,
            live_rows,
            baseline_invalidated,
            millis_since_baseline,
            dirty_page_ratio,
        }
    }

    /// Advisory flush verdict for the checkpoint coordinator.
    ///
    /// Same signals as the flush-time log, exposed before the coordinator
    /// commits to a global strategy so a table overdue for a baseline can
    /// escalate the whole checkpoint to full. Read-only; choosing the flush
    /// kind stays with the coordinator because the epoch chain is global.
    pub fn flush_plan(&self) -> super::super::flush_trigger::FlushPlan {
        super::super::flush_trigger::decide(self.flush_signals())
    }

    /// Aggregate version-chain pressure across shards for observability.
    ///
    /// Returns `(total_entries, max_chain_len, memory_bytes)` after any
    /// pending fold, so maintenance logs can show whether the shared cutoff
    /// actually drained the chains or a pinned snapshot holds them.
    pub fn version_chain_pressure(&self) -> (usize, usize, usize) {
        let mut total_entries = 0usize;
        let mut max_len = 0usize;
        let mut memory_bytes = 0usize;
        for shard in &self.shards {
            let stats = shard.read().columns.version_chain_stats();
            total_entries += stats.total_entries;
            max_len = max_len.max(stats.max_len);
            memory_bytes += stats.memory_bytes;
        }
        (total_entries, max_len, memory_bytes)
    }

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
    /// [`SHARD_FRAGMENTATION_THRESHOLD`] are skipped (segment-level
    /// compaction): lazy ID recycling already reclaims their holes without a
    /// global remap, avoiding cross-shard coordination. The threshold is the
    /// hole-rate watermark for stable row IDs: below it, free-stack reuse
    /// absorbs deletes and live rows never move; above it, a barriered
    /// compact reclaims the shard and the returned mapping must propagate
    /// to edge endpoints before any new write is admitted.
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
        super::super::compaction::CompactionJournal,
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
                        continue;
                    }
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
        let combined = super::super::compaction::CompactionJournal::combine(&journals);
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
        super::super::compaction::CompactionJournal,
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
        let combined = super::super::compaction::CompactionJournal::combine(&journals);
        Ok((all_removed, std::collections::HashMap::new(), combined))
    }

    /// Evict cold column chunks across shards oldest-first until `max_bytes`
    /// are released. Returns `(chunks_evicted, bytes_released)`.
    ///
    /// Each shard is handled under its shard write lock with the
    /// evictability rechecked inside: a chunk that gained an overlay write
    /// or a version chain since selection is skipped for this pass.
    pub fn evict_cold_chunks(&self, max_bytes: u64) -> (usize, u64) {
        let mut count = 0usize;
        let mut freed = 0u64;
        for shard in &self.shards {
            if freed >= max_bytes {
                break;
            }
            let table = shard.write();
            let (n, bytes) = table
                .columns
                .evict_cold_chunks(max_bytes.saturating_sub(freed));
            count += n;
            freed += bytes;
        }
        (count, freed)
    }

    /// Quota-segmented eviction across shards for background tasks.
    /// `task_quota` caps one segment; over-quota work proceeds in segments
    /// instead of one burst. Returns `(chunks_evicted, bytes_released,
    /// segments)`.
    pub fn evict_cold_chunks_with_quota(
        &self,
        max_bytes: u64,
        task_quota: u64,
    ) -> (usize, u64, usize) {
        if task_quota >= max_bytes {
            let (count, freed) = self.evict_cold_chunks(max_bytes);
            return (count, freed, usize::from(count > 0));
        }
        let mut count = 0usize;
        let mut freed = 0u64;
        let mut segments = 0usize;
        for shard in &self.shards {
            if freed >= max_bytes {
                break;
            }
            let table = shard.write();
            let (n, bytes, segs) = table
                .columns
                .evict_cold_chunks_with_quota(max_bytes.saturating_sub(freed), task_quota);
            count += n;
            freed += bytes;
            segments += segs;
        }
        (count, freed, segments)
    }

    /// Eviction observability: `(resident_chunks, evicted_chunks,
    /// evicted_bytes, resident_bytes)` across shards.
    pub fn eviction_stats(&self) -> (usize, usize, usize, usize) {
        let mut resident_chunks = 0usize;
        let mut evicted_chunks = 0usize;
        let mut evicted_bytes = 0usize;
        let mut resident_bytes = 0usize;
        for shard in &self.shards {
            let table = shard.read();
            resident_chunks += table.columns.resident_chunk_count();
            evicted_chunks += table.columns.evicted_chunk_count();
            evicted_bytes += table.columns.evicted_bytes();
            resident_bytes += table.columns.resident_memory_usage();
        }
        (
            resident_chunks,
            evicted_chunks,
            evicted_bytes,
            resident_bytes,
        )
    }

    pub fn version_history_ref(
        &self,
    ) -> std::sync::Arc<std::sync::Mutex<crate::schema::LabelVersionHistory>> {
        self.shards[0].read().version_history_ref()
    }

    pub fn version_chain_memory_bytes(&self) -> usize {
        self.shards
            .iter()
            .map(|s| s.read().columns.version_chain_stats().memory_bytes)
            .sum()
    }

    pub fn memory_size(&self) -> usize {
        let mut total = std::mem::size_of::<Self>();
        for shard in &self.shards {
            total += shard.read().memory_size();
        }
        total
    }

    pub fn used_memory_size(&self) -> usize {
        let mut total = std::mem::size_of::<Self>();
        for shard in &self.shards {
            total += shard.read().used_memory_size();
        }
        total
    }
}
