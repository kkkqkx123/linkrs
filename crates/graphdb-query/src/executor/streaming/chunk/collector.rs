//! Terminal result collector: thread-local chunk accumulation.
//!
//! Ladybug `ResultCollector` analogue at the row level: each pipeline owns a
//! `LocalChunkCollector`, moves visible rows out of chunks exactly once
//! (`DataChunk::expand_visible_rows`, selection + multiplicity aware), and
//! merges collectors by moving whole blocks instead of per-Value clones.
//!
//! Storage is block-partitioned ([`COLLECTOR_BLOCK_ROWS`] rows per block) so
//! large results avoid the single-`Vec` reallocation peak and can be consumed
//! through the [`LocalChunkCollector::iter_rows`] cursor without flattening.
//!
//! Spill-through collect: when a [`SpillManager`](crate::executor::streaming::spill::SpillManager)
//! is attached via [`LocalChunkCollector::attach_spill_manager`] and the
//! logical-row threshold is exceeded, rows are written to versioned run files
//! (rotated every [`COLLECTOR_RUN_ROWS_MAX`] rows so neither the writer-side
//! body buffer nor the reader-side full-run load grows without bound) and the
//! memory blocks are released. Without an attached manager the collector is
//! pure in-memory with zero spill overhead.

use std::sync::Arc;

use super::core::DataChunk;
use crate::executor::base::MemoryBudget;
use crate::executor::streaming::spill::{
    schema_fingerprint, SpillManager, SpilledRun, COLLECTOR_RUN_ROWS_MAX,
};
use graphdb_core::error::QueryError;
use graphdb_core::Value;

use super::super::spill::{RunReader, RunWriter};

/// Rows per collector block. Bounds the reallocation peak of a single push.
pub const COLLECTOR_BLOCK_ROWS: usize = 4096;

/// Open spill run held by the collector.
#[derive(Debug)]
struct ActiveRun {
    writer: RunWriter,
    rows: u64,
    /// Disk quota pre-reserved for buffered rows (row-memory estimate).
    /// Released and replaced by the exact file size on finalization.
    reserved_bytes: u64,
}

/// Thread-local terminal accumulator for query results.
#[derive(Debug, Default)]
pub struct LocalChunkCollector {
    blocks: Vec<Vec<Vec<Value>>>,
    col_names: Vec<String>,
    /// Remaining (not yet drained) rows, memory + spilled.
    len: usize,
    total_logical: u128,
    spill_threshold_rows: Option<u64>,
    spill_manager: Option<Arc<SpillManager>>,
    spilled_runs: Vec<SpilledRun>,
    active_writer: Option<ActiveRun>,
    spill_fingerprint: Option<u64>,
    spilled_rows: u64,
    spilled_bytes: u64,
    drain_cursor: usize,
}

impl LocalChunkCollector {
    pub fn new(col_names: Vec<String>) -> Self {
        Self {
            blocks: Vec::new(),
            col_names,
            len: 0,
            total_logical: 0,
            spill_threshold_rows: None,
            spill_manager: None,
            spilled_runs: Vec::new(),
            active_writer: None,
            spill_fingerprint: None,
            spilled_rows: 0,
            spilled_bytes: 0,
            drain_cursor: 0,
        }
    }

    /// Attach a spill manager for spill-through collect.
    ///
    /// When no explicit threshold was configured via
    /// [`Self::set_spill_threshold_rows`], the manager's effective threshold
    /// (config value or default) is adopted. A threshold of `u64::MAX`
    /// (from `Some(0)` config) disables collector spill.
    pub fn attach_spill_manager(&mut self, manager: Arc<SpillManager>) {
        if self.spill_threshold_rows.is_none() {
            self.spill_threshold_rows = Some(manager.collector_spill_threshold());
        }
        self.spill_manager = Some(manager);
    }

    /// Move all visible (expanded) rows of `chunk` into the collector,
    /// splitting them across fixed-size blocks or spilling them to disk once
    /// the threshold is exceeded.
    pub fn push_chunk(&mut self, chunk: &mut DataChunk) -> Result<(), QueryError> {
        if self.col_names.is_empty() {
            self.col_names = chunk.col_names();
        }
        self.total_logical += u128::from(chunk.logical_len());
        let rows = chunk.expand_visible_rows();
        if rows.is_empty() {
            return Ok(());
        }
        if self.spill_manager.is_none() {
            self.push_rows_memory(rows);
            return Ok(());
        }
        if self.is_spilled() {
            self.write_rows_spilled(rows)?;
            return Ok(());
        }
        if self.spill_threshold_exceeded() {
            self.spill_memory_blocks()?;
            self.write_rows_spilled(rows)?;
        } else {
            self.push_rows_memory(rows);
        }
        Ok(())
    }

    /// Merge another collector by moving whole blocks (no per-Value clone).
    ///
    /// Spilled runs are concatenated in order; both open writers are
    /// finalized first so ordering is preserved. Merging two spilled
    /// collectors requires identical column names and the same spill manager
    /// (runs live in the manager's directory); otherwise an error is
    /// returned and the caller should drain via `into_rows` first. Pure
    /// in-memory merges keep the previous unchecked behavior.
    pub fn merge(&mut self, mut other: LocalChunkCollector) -> Result<(), QueryError> {
        let self_spilled = self.is_spilled();
        let other_spilled = other.is_spilled();
        if self_spilled || other_spilled {
            if !self.col_names.is_empty()
                && !other.col_names.is_empty()
                && self.col_names != other.col_names
            {
                return Err(QueryError::execution(
                    "spill run: cannot merge collectors with different column names".to_string(),
                ));
            }
            match (self.spill_manager.clone(), other.spill_manager.clone()) {
                (Some(a), Some(b)) => {
                    if !Arc::ptr_eq(&a, &b) {
                        return Err(QueryError::execution(
                            "spill run: cannot merge collectors from different spill managers"
                                .to_string(),
                        ));
                    }
                }
                (None, Some(manager)) => {
                    if self.len == 0 && self.total_logical == 0 {
                        self.spill_manager = Some(manager.clone());
                        if self.spill_threshold_rows.is_none() {
                            self.spill_threshold_rows = other.spill_threshold_rows;
                        }
                    } else {
                        return Err(QueryError::execution(
                            "spill run: cannot merge spilled collector without a spill manager"
                                .to_string(),
                        ));
                    }
                }
                (Some(_), None) => {
                    debug_assert!(
                        !other_spilled,
                        "spilled runs without a spill manager must not exist"
                    );
                }
                (None, None) => {
                    debug_assert!(
                        !self_spilled && !other_spilled,
                        "spilled state requires a spill manager"
                    );
                }
            }
            if self.col_names.is_empty() {
                self.col_names = std::mem::take(&mut other.col_names);
            }
            self.finalize_active_writer()?;
            other.finalize_active_writer()?;
            let other_remaining: Vec<SpilledRun> =
                other.spilled_runs[other.drain_cursor..].to_vec();
            self.spilled_runs.extend(other_remaining);
            self.spilled_rows += other.spilled_rows;
            self.spilled_bytes += other.spilled_bytes;
        } else if self.col_names.is_empty() {
            self.col_names = std::mem::take(&mut other.col_names);
        }
        self.total_logical += other.total_logical;
        self.len += other.len;
        if self.blocks.is_empty() {
            self.blocks = other.blocks;
            return Ok(());
        }
        for mut block in other.blocks {
            if block.is_empty() {
                continue;
            }
            let tail_has_room = self
                .blocks
                .last()
                .is_some_and(|b| b.len() < COLLECTOR_BLOCK_ROWS);
            if !tail_has_room {
                self.blocks.push(std::mem::take(&mut block));
                continue;
            }
            // Fill the tail first, then move the remainder as a new block.
            let tail = self.blocks.last_mut().expect("tail block must exist");
            while tail.len() < COLLECTOR_BLOCK_ROWS && !block.is_empty() {
                // `block` was filled front-to-back; pop from the back into a
                // staging vec would reverse order, so drain from the front in
                // bulk instead.
                let room = COLLECTOR_BLOCK_ROWS - tail.len();
                let take = room.min(block.len());
                tail.extend(block.drain(..take));
            }
            if !block.is_empty() {
                self.blocks.push(block);
            }
        }
        Ok(())
    }

    /// Expanded row count observed (selection + multiplicity applied).
    pub fn total_logical_rows(&self) -> u128 {
        self.total_logical
    }

    /// Remaining rows (memory + spilled, not yet drained).
    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn col_names(&self) -> &[String] {
        &self.col_names
    }

    /// Finalize the open run, if any, so all spilled rows are readable and
    /// counters are exact. Idempotent; `into_rows` and `drain_rows` call it
    /// automatically, while streaming `collect` paths call it explicitly
    /// before snapshotting observability counters.
    pub fn finish_spill(&mut self) -> Result<(), QueryError> {
        self.finalize_active_writer()
    }

    /// Full ordered results: spilled runs replayed in write order, then
    /// memory blocks. Peak memory is bounded by the largest single run.
    pub fn into_rows(mut self) -> Result<(Vec<Vec<Value>>, Vec<String>), QueryError> {
        self.finalize_active_writer()?;
        let mut out = Vec::with_capacity(self.len);
        for run in &self.spilled_runs[self.drain_cursor..] {
            let mut reader = RunReader::open(run)?;
            out.extend(reader.read_all()?);
        }
        for block in self.blocks {
            out.extend(block);
        }
        Ok((out, self.col_names))
    }

    /// Segmented results: each call returns at most one run's rows, so
    /// streaming consumers never hold more than a single run in memory.
    /// After the runs are exhausted the memory blocks are returned whole,
    /// then empty vectors signal completion.
    pub fn drain_rows(&mut self) -> Result<Vec<Vec<Value>>, QueryError> {
        if self.drain_cursor < self.spilled_runs.len() {
            let run = self.spilled_runs[self.drain_cursor].clone();
            let mut reader = RunReader::open(&run)?;
            let rows = reader.read_all()?;
            self.drain_cursor += 1;
            self.len = self.len.saturating_sub(rows.len());
            return Ok(rows);
        }
        if self.active_writer.is_some() {
            self.finalize_active_writer()?;
            return self.drain_rows();
        }
        if !self.blocks.is_empty() {
            let blocks = std::mem::take(&mut self.blocks);
            let mut out = Vec::new();
            for block in blocks {
                out.extend(block);
            }
            self.len = self.len.saturating_sub(out.len());
            return Ok(out);
        }
        Ok(Vec::new())
    }

    /// Block-partitioned memory rows.
    ///
    /// Only valid for in-memory collectors; spilled state must be consumed
    /// via [`Self::into_rows`] or [`Self::drain_rows`].
    pub fn into_blocks(self) -> (Vec<Vec<Vec<Value>>>, Vec<String>) {
        debug_assert!(
            !self.is_spilled(),
            "into_blocks drops spilled runs; use into_rows or drain_rows"
        );
        (self.blocks, self.col_names)
    }

    /// Ordered cursor over memory blocks only.
    ///
    /// When spilled, the disk portion is NOT included; paged consumers must
    /// use [`Self::drain_rows`], full consumers [`Self::into_rows`].
    pub fn iter_rows(&self) -> impl Iterator<Item = &Vec<Value>> {
        self.blocks.iter().flatten()
    }

    pub fn num_blocks(&self) -> usize {
        self.blocks.len()
    }

    /// Whether any row has been spilled to disk.
    pub fn is_spilled(&self) -> bool {
        !self.spilled_runs.is_empty() || self.active_writer.is_some()
    }

    /// Rows written to disk so far.
    pub fn spilled_rows(&self) -> u64 {
        self.spilled_rows
    }

    /// Bytes written to disk so far (on-disk file sizes).
    pub fn spilled_bytes(&self) -> u64 {
        self.spilled_bytes
    }

    /// Number of finalized spill runs.
    pub fn spilled_run_count(&self) -> usize {
        self.spilled_runs.len()
    }

    /// Configure the logical-row threshold that will trigger spill-through
    /// collect once a spill manager is attached.
    pub fn set_spill_threshold_rows(&mut self, threshold: u64) {
        self.spill_threshold_rows = Some(threshold);
    }

    /// Whether the configured spill threshold has been exceeded.
    pub fn spill_threshold_exceeded(&self) -> bool {
        match self.spill_threshold_rows {
            Some(t) => u128::from(t) < self.total_logical,
            None => false,
        }
    }

    fn push_rows_memory(&mut self, mut rows: Vec<Vec<Value>>) {
        if rows.is_empty() {
            return;
        }
        self.len += rows.len();
        // Drain from the front without shifting: reverse once, then pop.
        rows.reverse();
        while !rows.is_empty() {
            let tail_full = self
                .blocks
                .last()
                .is_none_or(|b| b.len() >= COLLECTOR_BLOCK_ROWS);
            if tail_full {
                self.blocks
                    .push(Vec::with_capacity(rows.len().min(COLLECTOR_BLOCK_ROWS)));
            }
            let tail = self.blocks.last_mut().expect("tail block must exist");
            while tail.len() < COLLECTOR_BLOCK_ROWS {
                if let Some(row) = rows.pop() {
                    tail.push(row);
                } else {
                    break;
                }
            }
        }
    }

    fn spill_fingerprint(&mut self) -> u64 {
        if let Some(fp) = self.spill_fingerprint {
            return fp;
        }
        let fp = schema_fingerprint(&self.col_names);
        self.spill_fingerprint = Some(fp);
        fp
    }

    fn ensure_active_writer(&mut self) -> Result<(), QueryError> {
        if self.active_writer.is_some() {
            return Ok(());
        }
        let manager = self.spill_manager.clone().ok_or_else(|| {
            QueryError::execution("spill run: spill manager not available".to_string())
        })?;
        let fp = self.spill_fingerprint();
        let writer = manager.create_run_writer(fp)?;
        self.active_writer = Some(ActiveRun {
            writer,
            rows: 0,
            reserved_bytes: 0,
        });
        Ok(())
    }

    fn rotate_active_writer_if_full(&mut self) -> Result<(), QueryError> {
        let full = self
            .active_writer
            .as_ref()
            .is_some_and(|a| a.rows >= COLLECTOR_RUN_ROWS_MAX);
        if full {
            self.finalize_active_writer()?;
        }
        Ok(())
    }

    fn finalize_active_writer(&mut self) -> Result<(), QueryError> {
        let Some(active) = self.active_writer.take() else {
            return Ok(());
        };
        let manager = self.spill_manager.clone().ok_or_else(|| {
            QueryError::execution("spill run: spill manager not available".to_string())
        })?;
        let rows = active.rows;
        let reserved = active.reserved_bytes;
        // Release the pre-reserved estimate first so `finalize_run` accounts
        // exactly once for the on-disk size. Releasing up front also avoids
        // leaking the estimate when finalization itself fails.
        manager.disk_quota().release(reserved);
        // Single quota choke point: `finalize_run` reserves the exact file
        // size, removes the file on quota failure, and tracks manager bytes.
        let run = manager.finalize_run(active.writer).map_err(|e| {
            let msg = e.to_string();
            if msg.contains("Disk quota") {
                e
            } else {
                QueryError::execution(format!("spill run: {}", msg))
            }
        })?;
        self.spilled_rows += rows;
        self.spilled_bytes += run.byte_size;
        self.spilled_runs.push(run);
        Ok(())
    }

    fn write_one_row(&mut self, row: Vec<Value>) -> Result<(), QueryError> {
        self.rotate_active_writer_if_full()?;
        let manager = self.spill_manager.clone().ok_or_else(|| {
            QueryError::execution("spill run: spill manager not available".to_string())
        })?;
        // Pre-reserve the disk quota by row-memory estimate so over-quota
        // writes fail fast at `push_chunk` time instead of only at run
        // finalization. The estimate is reconciled against the exact file
        // size in `finalize_active_writer`.
        let estimate = MemoryBudget::estimate_row_memory(&row) as u64;
        manager.disk_quota().try_reserve(estimate)?;
        let active = self.active_writer.as_mut().expect("writer must exist");
        active.writer.write_row(&row)?;
        active.rows += 1;
        active.reserved_bytes += estimate;
        Ok(())
    }

    fn write_rows_spilled(&mut self, rows: Vec<Vec<Value>>) -> Result<(), QueryError> {
        self.ensure_active_writer()?;
        self.len += rows.len();
        for row in rows {
            self.write_one_row(row)?;
        }
        Ok(())
    }

    fn spill_memory_blocks(&mut self) -> Result<(), QueryError> {
        if self.blocks.is_empty() {
            return Ok(());
        }
        self.ensure_active_writer()?;
        let blocks = std::mem::take(&mut self.blocks);
        for block in blocks {
            for row in block {
                self.write_one_row(row)?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::streaming::slot::SlotLayout;
    use crate::executor::streaming::spill::{DiskQuota, SpillConfig, SpillManager};
    use std::sync::Arc;

    fn chunk_with(rows: Vec<Vec<Value>>, selection: Option<Vec<usize>>, mult: u64) -> DataChunk {
        let layout = Arc::new(SlotLayout::from_names(&["a".to_string()]));
        let mut c = DataChunk::new_with_layout(rows, layout);
        if let Some(sel) = selection {
            c = c.with_selection(sel);
        }
        c.with_multiplicity(mult)
    }

    fn spill_manager(query_id: u64) -> Arc<SpillManager> {
        Arc::new(SpillManager::new(SpillConfig::default(), query_id).unwrap())
    }

    #[test]
    fn expands_selection_and_multiplicity_once() {
        let mut c = chunk_with(
            vec![
                vec![Value::Int(1)],
                vec![Value::Int(2)],
                vec![Value::Int(3)],
            ],
            Some(vec![0, 2]),
            2,
        );
        let mut collector = LocalChunkCollector::new(vec!["a".to_string()]);
        collector.push_chunk(&mut c).unwrap();
        assert_eq!(collector.total_logical_rows(), 4);
        assert_eq!(collector.len(), 4);
        assert!(c.rows.is_empty());
    }

    #[test]
    fn merge_sums_counts() {
        let mut a = LocalChunkCollector::new(vec!["a".to_string()]);
        let mut b = LocalChunkCollector::new(vec!["a".to_string()]);
        let mut c1 = chunk_with(vec![vec![Value::Int(1)]], None, 1);
        let mut c2 = chunk_with(vec![vec![Value::Int(2)]], None, 3);
        a.push_chunk(&mut c1).unwrap();
        b.push_chunk(&mut c2).unwrap();
        a.merge(b).unwrap();
        assert_eq!(a.total_logical_rows(), 4);
        assert_eq!(a.len(), 4);
    }

    #[test]
    fn splits_large_push_into_blocks() {
        let rows: Vec<Vec<Value>> = (0..5000).map(|i| vec![Value::Int(i)]).collect();
        let mut c = chunk_with(rows, None, 1);
        let mut collector = LocalChunkCollector::new(vec!["a".to_string()]);
        collector.push_chunk(&mut c).unwrap();
        assert_eq!(collector.len(), 5000);
        assert_eq!(collector.num_blocks(), 2);
        let (flat, _) = collector.into_rows().unwrap();
        assert_eq!(flat.len(), 5000);
        assert_eq!(flat[0], vec![Value::Int(0)]);
        assert_eq!(flat[4999], vec![Value::Int(4999)]);
    }

    #[test]
    fn merge_moves_blocks_and_iter_rows_preserves_order() {
        let mut a = LocalChunkCollector::new(vec!["a".to_string()]);
        let mut b = LocalChunkCollector::new(vec!["a".to_string()]);
        let mut c1 = chunk_with(vec![vec![Value::Int(1)], vec![Value::Int(2)]], None, 1);
        let mut c2 = chunk_with(vec![vec![Value::Int(3)]], None, 1);
        a.push_chunk(&mut c1).unwrap();
        b.push_chunk(&mut c2).unwrap();
        a.merge(b).unwrap();
        let ordered: Vec<i32> = a
            .iter_rows()
            .map(|r| match &r[0] {
                Value::Int(v) => *v,
                _ => -1,
            })
            .collect();
        assert_eq!(ordered, vec![1, 2, 3]);
        let (flat, _) = a.into_rows().unwrap();
        assert_eq!(flat.len(), 3);
    }

    #[test]
    fn spill_threshold_hook_defaults_off() {
        let mut collector = LocalChunkCollector::new(vec!["a".to_string()]);
        assert!(!collector.is_spilled());
        assert!(!collector.spill_threshold_exceeded());
        collector.set_spill_threshold_rows(2);
        let mut c = chunk_with(
            vec![
                vec![Value::Int(1)],
                vec![Value::Int(2)],
                vec![Value::Int(3)],
            ],
            None,
            1,
        );
        collector.push_chunk(&mut c).unwrap();
        assert!(collector.spill_threshold_exceeded());
        assert!(!collector.is_spilled());
    }

    #[test]
    fn spill_through_preserves_order() {
        let mut collector = LocalChunkCollector::new(vec!["a".to_string()]);
        collector.attach_spill_manager(spill_manager(4201));
        collector.set_spill_threshold_rows(10);
        for batch in 0..5 {
            let rows: Vec<Vec<Value>> = (0..10).map(|i| vec![Value::Int(batch * 10 + i)]).collect();
            let mut c = chunk_with(rows, None, 1);
            collector.push_chunk(&mut c).unwrap();
        }
        assert!(collector.is_spilled());
        collector.finish_spill().unwrap();
        assert_eq!(collector.spilled_rows(), 50);
        assert!(collector.spilled_bytes() > 0);
        let (flat, names) = collector.into_rows().unwrap();
        assert_eq!(flat.len(), 50);
        assert_eq!(names, vec!["a".to_string()]);
        for (i, row) in flat.iter().enumerate() {
            assert_eq!(row, &vec![Value::Int(i as i32)]);
        }
    }

    #[test]
    fn spill_drain_matches_into_rows() {
        let mut full = LocalChunkCollector::new(vec!["a".to_string()]);
        full.attach_spill_manager(spill_manager(4202));
        full.set_spill_threshold_rows(5);
        let mut drained = LocalChunkCollector::new(vec!["a".to_string()]);
        drained.attach_spill_manager(spill_manager(4203));
        drained.set_spill_threshold_rows(5);
        for batch in 0..3 {
            let rows: Vec<Vec<Value>> = (0..10).map(|i| vec![Value::Int(batch * 10 + i)]).collect();
            let mut c1 = chunk_with(rows.clone(), None, 1);
            let mut c2 = chunk_with(rows, None, 1);
            full.push_chunk(&mut c1).unwrap();
            drained.push_chunk(&mut c2).unwrap();
        }
        let (expected, _) = full.into_rows().unwrap();
        let mut got = Vec::new();
        loop {
            let segment = drained.drain_rows().unwrap();
            if segment.is_empty() {
                break;
            }
            got.extend(segment);
        }
        assert_eq!(got, expected);
    }

    #[test]
    fn spill_merge_memory_and_spilled() {
        let mut a = LocalChunkCollector::new(vec!["a".to_string()]);
        a.attach_spill_manager(spill_manager(4204));
        a.set_spill_threshold_rows(2);
        let mut c1 = chunk_with(
            vec![
                vec![Value::Int(1)],
                vec![Value::Int(2)],
                vec![Value::Int(3)],
            ],
            None,
            1,
        );
        a.push_chunk(&mut c1).unwrap();
        assert!(a.is_spilled());

        let mut b = LocalChunkCollector::new(vec!["a".to_string()]);
        let mut c2 = chunk_with(vec![vec![Value::Int(4)]], None, 1);
        b.push_chunk(&mut c2).unwrap();

        // Different managers: spilled merge must be rejected.
        b.attach_spill_manager(spill_manager(4205));
        assert!(a.merge(b).is_err());
    }

    #[test]
    fn spill_merge_same_manager_preserves_order() {
        let manager = spill_manager(4206);
        let mut a = LocalChunkCollector::new(vec!["a".to_string()]);
        a.attach_spill_manager(manager.clone());
        a.set_spill_threshold_rows(2);
        let mut c1 = chunk_with(
            vec![
                vec![Value::Int(1)],
                vec![Value::Int(2)],
                vec![Value::Int(3)],
            ],
            None,
            1,
        );
        a.push_chunk(&mut c1).unwrap();

        let mut b = LocalChunkCollector::new(vec!["a".to_string()]);
        b.attach_spill_manager(manager);
        b.set_spill_threshold_rows(100);
        let mut c2 = chunk_with(vec![vec![Value::Int(4)]], None, 1);
        b.push_chunk(&mut c2).unwrap();

        a.merge(b).unwrap();
        let (flat, _) = a.into_rows().unwrap();
        assert_eq!(
            flat,
            vec![
                vec![Value::Int(1)],
                vec![Value::Int(2)],
                vec![Value::Int(3)],
                vec![Value::Int(4)],
            ]
        );
    }

    #[test]
    fn spill_merge_column_mismatch_rejected() {
        let manager = spill_manager(4207);
        let mut a = LocalChunkCollector::new(vec!["a".to_string()]);
        a.attach_spill_manager(manager.clone());
        a.set_spill_threshold_rows(1);
        let mut c1 = chunk_with(vec![vec![Value::Int(1)], vec![Value::Int(2)]], None, 1);
        a.push_chunk(&mut c1).unwrap();
        assert!(a.is_spilled());

        let mut b = LocalChunkCollector::new(vec!["b".to_string()]);
        b.attach_spill_manager(manager);
        let mut c2 = chunk_with(vec![vec![Value::Int(3)]], None, 1);
        b.push_chunk(&mut c2).unwrap();

        assert!(a.merge(b).is_err());
    }

    #[test]
    fn spill_quota_error_surfaces() {
        let quota = DiskQuota::new(1);
        let config = SpillConfig {
            temp_dir: None,
            max_spill_files: 64,
            collect_spill_rows: Some(1),
        };
        let manager = Arc::new(SpillManager::new_with_quota(config, 4208, quota).unwrap());
        let mut collector = LocalChunkCollector::new(vec!["a".to_string()]);
        collector.attach_spill_manager(manager);
        // Threshold comes from the manager config (1 row).
        let rows: Vec<Vec<Value>> = (0..50).map(|i| vec![Value::Int(i)]).collect();
        let mut c = chunk_with(rows, None, 1);
        assert!(collector.push_chunk(&mut c).is_err());
    }

    #[test]
    fn no_manager_keeps_memory_behavior() {
        let mut collector = LocalChunkCollector::new(vec!["a".to_string()]);
        collector.set_spill_threshold_rows(1);
        let mut c = chunk_with(vec![vec![Value::Int(1)], vec![Value::Int(2)]], None, 1);
        collector.push_chunk(&mut c).unwrap();
        assert!(!collector.is_spilled());
        assert_eq!(collector.spilled_rows(), 0);
        let (flat, _) = collector.into_rows().unwrap();
        assert_eq!(flat.len(), 2);
    }

    fn spill_with_rows(query_id: u64, n: i32) -> (LocalChunkCollector, Arc<SpillManager>) {
        let manager = spill_manager(query_id);
        let mut collector = LocalChunkCollector::new(vec!["a".to_string()]);
        collector.attach_spill_manager(manager.clone());
        collector.set_spill_threshold_rows(1);
        let rows: Vec<Vec<Value>> = (0..n).map(|i| vec![Value::Int(i)]).collect();
        let mut c = chunk_with(rows, None, 1);
        collector.push_chunk(&mut c).unwrap();
        collector.finish_spill().unwrap();
        assert!(collector.is_spilled());
        (collector, manager)
    }

    fn corrupt_run_headers(manager: &SpillManager, offset: u64) {
        let entries = std::fs::read_dir(manager.base_dir()).unwrap();
        let mut corrupted = 0;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("run") {
                continue;
            }
            let mut bytes = std::fs::read(&path).unwrap();
            assert!(bytes.len() > offset as usize);
            bytes[offset as usize] ^= 0xff;
            std::fs::write(&path, &bytes).unwrap();
            corrupted += 1;
        }
        assert!(corrupted > 0, "expected at least one run file to corrupt");
    }

    fn corrupt_run_bodies(manager: &SpillManager, offset: u64) {
        let entries = std::fs::read_dir(manager.base_dir()).unwrap();
        let mut corrupted = 0;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("run") {
                continue;
            }
            let mut bytes = std::fs::read(&path).unwrap();
            if bytes.len() <= offset as usize {
                continue;
            }
            bytes[offset as usize] ^= 0xff;
            std::fs::write(&path, &bytes).unwrap();
            corrupted += 1;
        }
        assert!(corrupted > 0, "expected at least one run file to corrupt");
    }

    fn repair_run_header_checksums(manager: &SpillManager) {
        fn fnv1a_64_local(data: &[u8]) -> u64 {
            let mut hash = 0xcbf29ce484222325u64;
            for &b in data {
                hash ^= b as u64;
                hash = hash.wrapping_mul(0x100000001b3);
            }
            hash
        }
        let entries = std::fs::read_dir(manager.base_dir()).unwrap();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("run") {
                continue;
            }
            let mut bytes = std::fs::read(&path).unwrap();
            assert!(bytes.len() >= 48);
            let checksum = fnv1a_64_local(&bytes[0..40]);
            bytes[40..48].copy_from_slice(&checksum.to_le_bytes());
            std::fs::write(&path, &bytes).unwrap();
        }
    }

    #[test]
    fn spilled_checksum_failure_surfaces_at_replay() {
        let (collector, manager) = spill_with_rows(4211, 20);
        // Flip a byte inside the first section payload (past the 48-byte
        // header and 9-byte frame prefix): the frame checksum must fail.
        corrupt_run_bodies(&manager, 57);
        let err = collector.into_rows().unwrap_err();
        assert!(
            err.to_string().contains("checksum mismatch"),
            "unexpected error: {}",
            err
        );
    }

    #[test]
    fn spilled_fingerprint_failure_surfaces_at_replay() {
        let (collector, manager) = spill_with_rows(4212, 20);
        // Flip a byte in the stored schema fingerprint (header bytes 8..16),
        // then repair the header checksum so replay reaches fingerprint
        // validation instead of failing on header integrity.
        corrupt_run_headers(&manager, 8);
        repair_run_header_checksums(&manager);
        let err = collector.into_rows().unwrap_err();
        assert!(
            err.to_string().contains("fingerprint mismatch"),
            "unexpected error: {}",
            err
        );
    }
}
