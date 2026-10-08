//! Checkpoint advisory signals for the flush coordinator.

use super::super::ShardedVertexTable;

impl ShardedVertexTable {
    /// Aggregate flush trigger signals across shards.
    ///
    /// Sums delta entries and live rows, ORs the compaction-moved-rows
    /// flag, and pairs them with the table-wide dirty-page ratio and
    /// baseline age. Feeds [`crate::vertex::vertex_table::flush_trigger::decide`];
    /// the decision is logged with its reason code at each flush so full
    /// rewrites stay explainable.
    pub fn flush_signals(&self) -> crate::vertex::vertex_table::flush_trigger::FlushSignals {
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
            super::super::persistence::now_ms().saturating_sub(last_baseline)
        };
        crate::vertex::vertex_table::flush_trigger::FlushSignals {
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
    pub fn flush_plan(&self) -> crate::vertex::vertex_table::flush_trigger::FlushPlan {
        crate::vertex::vertex_table::flush_trigger::decide(self.flush_signals())
    }

    /// One-shot primary-key anchor decision for the whole table.
    ///
    /// Peeks every shard without consuming flags, then consumes the
    /// invalidation flags exactly once and broadcasts one verdict: any
    /// invalidated shard or a table-wide over-threshold delta anchors every
    /// shard, so shards never diverge on the baseline-vs-delta choice.
    pub fn decide_pk_anchor(&self) -> bool {
        use crate::vertex::id_indexer::IdManager;
        let mut live_rows = 0usize;
        let mut delta_entries = 0usize;
        let mut invalidated = false;
        for shard in &self.shards {
            let table = shard.read();
            live_rows += table.id_indexer.len();
            delta_entries += table.id_indexer.delta_len();
            invalidated |= table.id_indexer.baseline_invalidated();
        }
        if invalidated {
            for shard in &self.shards {
                let _ = shard.read().take_pk_baseline_invalidated();
            }
            return true;
        }
        delta_entries >= IdManager::anchor_threshold_for_live(live_rows)
    }
}
