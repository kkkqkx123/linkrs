use std::sync::Arc;

use crate::executor::base::MemoryTracker;
use crate::executor::streaming::runtime::ExecutionRuntime;
use crate::executor::streaming::spill::{
    hash_bytes_partition, schema_fingerprint, RunReader, RunWriter, SpillManager, SpilledRun,
    COLLECTOR_RUN_ROWS_MAX, HASH_JOIN_MAX_DEPTH, HASH_JOIN_PARTITIONS_DEFAULT,
};
use graphdb_core::error::QueryError;
use graphdb_core::types::expr::Expression;
use graphdb_core::Value;

use super::{evaluate_join_key, HashJoinBuildSide, JoinKeyValue};

/// Partition index for a join key.
///
/// NULL (or all-NULL composite) keys always map to partition 0 so both sides
/// agree on placement; every other key is partitioned by the same FNV-1a
/// primitive that backs [`hash_bytes_partition`], keeping build and probe
/// rows with equal keys co-located.
pub fn hash_join_key_partition(key: &JoinKeyValue, num_partitions: u64) -> usize {
    if num_partitions == 0 {
        return 0;
    }
    if key.is_nullish() {
        return 0;
    }
    let bytes = postcard::to_allocvec(key).unwrap_or_default();
    hash_bytes_partition(&bytes, num_partitions) as usize
}

/// Per-partition rotating run writer.
///
/// Each partition holds one open [`RunWriter`] plus finalized runs; writers
/// rotate every [`COLLECTOR_RUN_ROWS_MAX`] rows so neither the writer-side
/// body buffer nor the reader-side full-run load grows without bound.
#[derive(Debug)]
pub struct RotatingPartitionWriter {
    manager: Arc<SpillManager>,
    fingerprint: u64,
    num_partitions: u64,
    writers: Vec<RunWriter>,
    open_rows: Vec<u64>,
    finished: Vec<Vec<SpilledRun>>,
    total_rows: u64,
    total_bytes: u64,
}

impl RotatingPartitionWriter {
    pub fn new(
        manager: Arc<SpillManager>,
        col_names: &[String],
        num_partitions: u64,
    ) -> Result<Self, QueryError> {
        let fingerprint = schema_fingerprint(col_names);
        let n = num_partitions.max(1) as usize;
        let mut writers = Vec::with_capacity(n);
        for _ in 0..n {
            writers.push(manager.create_run_writer(fingerprint)?);
        }
        Ok(Self {
            manager,
            fingerprint,
            num_partitions: num_partitions.max(1),
            writers,
            open_rows: vec![0; n],
            finished: vec![Vec::new(); n],
            total_rows: 0,
            total_bytes: 0,
        })
    }

    pub fn num_partitions(&self) -> u64 {
        self.num_partitions
    }

    pub fn insert(&mut self, partition: usize, row: &[Value]) -> Result<(), QueryError> {
        if partition >= self.writers.len() {
            return Err(QueryError::execution(format!(
                "spill run: join partition {} out of range {}",
                partition,
                self.writers.len(),
            )));
        }
        if self.open_rows[partition] >= COLLECTOR_RUN_ROWS_MAX {
            self.rotate(partition)?;
        }
        self.writers[partition].write_row(row)?;
        self.open_rows[partition] += 1;
        self.total_rows += 1;
        Ok(())
    }

    fn rotate(&mut self, partition: usize) -> Result<(), QueryError> {
        let fingerprint = self.fingerprint;
        let path = std::mem::replace(
            &mut self.writers[partition],
            self.manager.create_run_writer(fingerprint)?,
        );
        let run = self.manager.finalize_run(path)?;
        self.total_bytes += run.byte_size;
        self.finished[partition].push(run);
        self.open_rows[partition] = 0;
        Ok(())
    }

    /// Finalize all open writers and return per-partition runs.
    ///
    /// Idle open writers (no rows buffered) are discarded without
    /// finalization so empty partitions cost no spill files.
    pub fn finish(mut self) -> Result<Vec<Vec<SpilledRun>>, QueryError> {
        let mut out = std::mem::take(&mut self.finished);
        for (partition, writer) in self.writers.drain(..).enumerate() {
            if self.open_rows[partition] > 0 {
                let run = self.manager.finalize_run(writer)?;
                self.total_bytes += run.byte_size;
                out[partition].push(run);
            } else {
                let _ = std::fs::remove_file(writer.path());
            }
        }
        Ok(out)
    }

    pub fn total_rows(&self) -> u64 {
        self.total_rows
    }

    pub fn total_bytes(&self) -> u64 {
        self.total_bytes
    }
}

/// Open build-side spill state held while the build input is partitioned.
#[derive(Debug)]
pub struct PendingBuildSpill {
    pub writer: RotatingPartitionWriter,
    pub build_col_names: Vec<String>,
}

/// Sub-partitions created when a single Grace partition exceeds the memory
/// budget. Bounded by [`HASH_JOIN_MAX_DEPTH`] repartition levels.
pub const GRACE_SUB_PARTITIONS: u64 = 4;

/// Partitioned join serving state: per-partition runs plus the currently
/// loaded in-memory partition.
#[derive(Debug)]
pub struct PartitionedJoinState {
    build_runs: Vec<Vec<SpilledRun>>,
    probe_runs: Vec<Vec<SpilledRun>>,
    build_col_names: Vec<String>,
    probe_col_names: Vec<String>,
    num_partitions: u64,
    current: usize,
    build_side: HashJoinBuildSide,
    probe_rows: Vec<Vec<Value>>,
    probe_pos: usize,
    exhausted: bool,
    /// Per-partition repartition level (each capped at [`HASH_JOIN_MAX_DEPTH`]).
    depths: Vec<u32>,
    manager: Option<Arc<SpillManager>>,
    hash_keys: Vec<Expression>,
    probe_keys: Vec<Expression>,
}

impl PartitionedJoinState {
    pub fn new(
        build_runs: Vec<Vec<SpilledRun>>,
        probe_runs: Vec<Vec<SpilledRun>>,
        build_col_names: Vec<String>,
        probe_col_names: Vec<String>,
        manager: Option<Arc<SpillManager>>,
        hash_keys: Vec<Expression>,
        probe_keys: Vec<Expression>,
    ) -> Self {
        let num_partitions = build_runs.len().max(probe_runs.len()).max(1) as u64;
        let depths = vec![0u32; num_partitions as usize];
        Self {
            build_runs,
            probe_runs,
            build_col_names,
            probe_col_names,
            num_partitions,
            current: 0,
            build_side: HashJoinBuildSide::new(),
            probe_rows: Vec::new(),
            probe_pos: 0,
            exhausted: false,
            depths,
            manager,
            hash_keys,
            probe_keys,
        }
    }

    pub fn build_col_names(&self) -> &[String] {
        &self.build_col_names
    }

    pub fn probe_col_names(&self) -> &[String] {
        &self.probe_col_names
    }

    pub fn build_side(&self) -> &HashJoinBuildSide {
        &self.build_side
    }

    pub fn probe_rows(&self) -> &[Vec<Value>] {
        &self.probe_rows
    }

    pub fn probe_pos(&self) -> usize {
        self.probe_pos
    }

    pub fn set_probe_pos(&mut self, pos: usize) {
        self.probe_pos = pos;
    }

    pub fn is_exhausted(&self) -> bool {
        self.exhausted
    }

    /// Load the first non-empty partition. Returns false when both sides are
    /// empty.
    pub fn load_first(
        &mut self,
        hash_keys: &[Expression],
        memory_tracker: &mut MemoryTracker,
        runtime: Option<&Arc<ExecutionRuntime>>,
    ) -> Result<bool, QueryError> {
        self.current = 0;
        self.exhausted = false;
        self.load_current(hash_keys, memory_tracker, runtime)
    }

    /// Advance past the current partition. Returns false when no partitions
    /// remain.
    pub fn advance(
        &mut self,
        hash_keys: &[Expression],
        memory_tracker: &mut MemoryTracker,
        runtime: Option<&Arc<ExecutionRuntime>>,
    ) -> Result<bool, QueryError> {
        if self.exhausted {
            return Ok(false);
        }
        self.current += 1;
        self.load_current(hash_keys, memory_tracker, runtime)
    }

    fn load_current(
        &mut self,
        hash_keys: &[Expression],
        memory_tracker: &mut MemoryTracker,
        runtime: Option<&Arc<ExecutionRuntime>>,
    ) -> Result<bool, QueryError> {
        while self.current < self.num_partitions as usize {
            let idx = self.current;
            let build_empty = self.build_runs.get(idx).is_none_or(Vec::is_empty);
            let probe_empty = self.probe_runs.get(idx).is_none_or(Vec::is_empty);
            if build_empty && probe_empty {
                self.current += 1;
                continue;
            }
            self.build_side.clear();
            memory_tracker.reset();
            self.probe_rows.clear();
            self.probe_pos = 0;
            let load_result = self.load_partition_rows(idx, hash_keys, memory_tracker);
            match load_result {
                Ok(()) => return Ok(true),
                Err(e) if e.to_string().contains("exceeds memory budget") => {
                    // Single partition too large: split it into sub-partitions
                    // and retry at the same index (depth-bounded).
                    self.split_partition(idx, runtime)?;
                    // Retry without advancing: index now points at the first
                    // sub-partition replacing the oversized one.
                    continue;
                }
                Err(e) => return Err(e),
            }
        }
        self.exhausted = true;
        Ok(false)
    }

    fn load_partition_rows(
        &mut self,
        idx: usize,
        hash_keys: &[Expression],
        memory_tracker: &mut MemoryTracker,
    ) -> Result<(), QueryError> {
        let build_empty = self.build_runs.get(idx).is_none_or(Vec::is_empty);
        let probe_empty = self.probe_runs.get(idx).is_none_or(Vec::is_empty);
        if !build_empty {
            for run in &self.build_runs[idx] {
                let mut reader = RunReader::open(run).map_err(|e| {
                    QueryError::execution(format!("spill run: build partition load: {}", e))
                })?;
                while let Some(row) = reader.read_row()? {
                    memory_tracker.try_reserve_row(&row).map_err(|_| {
                        QueryError::execution(format!(
                            "spill run: join partition {} exceeds memory budget \
                             (single partition too large for Grace join)",
                            idx,
                        ))
                    })?;
                    let key = evaluate_join_key(&row, &self.build_col_names, hash_keys)?;
                    self.build_side.insert_keyed_row(key, &row)?;
                }
            }
        }
        if !probe_empty {
            for run in &self.probe_runs[idx] {
                let mut reader = RunReader::open(run).map_err(|e| {
                    QueryError::execution(format!("spill run: probe partition load: {}", e))
                })?;
                while let Some(row) = reader.read_row()? {
                    memory_tracker.try_reserve_row(&row).map_err(|_| {
                        QueryError::execution(format!(
                            "spill run: join partition {} exceeds memory budget \
                             (single partition too large for Grace join)",
                            idx,
                        ))
                    })?;
                    self.probe_rows.push(row);
                }
            }
        }
        Ok(())
    }

    /// Split an oversized partition into [`GRACE_SUB_PARTITIONS`] sub-partitions
    /// by re-hashing its spilled rows. Depth-bounded by [`HASH_JOIN_MAX_DEPTH`];
    /// beyond the limit the original budget error is returned.
    fn split_partition(
        &mut self,
        idx: usize,
        runtime: Option<&Arc<ExecutionRuntime>>,
    ) -> Result<(), QueryError> {
        let depth = self.depths.get(idx).copied().unwrap_or(0);
        if depth >= HASH_JOIN_MAX_DEPTH {
            return Err(QueryError::execution(format!(
                "spill run: join partition {} exceeds memory budget \
                 (Grace repartition depth {} exhausted)",
                idx, HASH_JOIN_MAX_DEPTH,
            )));
        }
        let manager = self.manager.clone().ok_or_else(|| {
            QueryError::execution("spill run: spill manager not available".to_string())
        })?;
        let sub = GRACE_SUB_PARTITIONS;
        let build_names = self.build_col_names.clone();
        let probe_names = self.probe_col_names.clone();
        let hash_keys = self.hash_keys.clone();
        let probe_keys = self.probe_keys.clone();
        let mut build_writer = RotatingPartitionWriter::new(manager.clone(), &build_names, sub)?;
        let mut probe_writer = RotatingPartitionWriter::new(manager.clone(), &probe_names, sub)?;
        for run in self.build_runs.get(idx).cloned().unwrap_or_default() {
            let mut reader = RunReader::open(&run).map_err(|e| {
                QueryError::execution(format!("spill run: build partition load: {}", e))
            })?;
            while let Some(row) = reader.read_row()? {
                let key = evaluate_join_key(&row, &build_names, &hash_keys)?;
                let partition = hash_join_key_partition(&key, sub);
                build_writer.insert(partition, &row)?;
            }
        }
        for run in self.probe_runs.get(idx).cloned().unwrap_or_default() {
            let mut reader = RunReader::open(&run).map_err(|e| {
                QueryError::execution(format!("spill run: probe partition load: {}", e))
            })?;
            while let Some(row) = reader.read_row()? {
                let key = evaluate_join_key(&row, &probe_names, &probe_keys)?;
                let partition = hash_join_key_partition(&key, sub);
                probe_writer.insert(partition, &row)?;
            }
        }
        let new_build = build_writer.finish()?;
        let new_probe = probe_writer.finish()?;
        if let Some(rt) = runtime {
            // Cumulative counters: replaced-run bytes were already recorded
            // when first spilled and stay counted as total bytes ever spilled.
            let stats = rt.columnar_stats();
            for run in new_build.iter().flatten().chain(new_probe.iter().flatten()) {
                stats.record_spill(run.row_count, run.byte_size);
            }
        }
        // The data now lives in the new sub-partitions: unlink the replaced
        // files and release their quota reservations, mirroring
        // `HashPartitionSpiller::repartition`. New files were quota-checked
        // at finalization. Runtime `ColumnarStats` stay cumulative (total
        // bytes ever spilled); per-operator `spilled_bytes` is re-synced to
        // the live file set by the caller via `byte_count`.
        for run in self.build_runs.get(idx).cloned().unwrap_or_default() {
            let _ = std::fs::remove_file(&run.path);
            manager.disk_quota().release(run.byte_size);
        }
        for run in self.probe_runs.get(idx).cloned().unwrap_or_default() {
            let _ = std::fs::remove_file(&run.path);
            manager.disk_quota().release(run.byte_size);
        }
        self.build_runs.splice(idx..=idx, new_build);
        self.probe_runs.splice(idx..=idx, new_probe);
        self.num_partitions += sub - 1;
        let next_depth = depth + 1;
        self.depths
            .splice(idx..=idx, vec![next_depth; sub as usize]);
        // `current` still points at idx, now the first sub-partition.
        Ok(())
    }

    /// Best-effort removal of partition files.
    pub fn cleanup_files(&self) {
        for runs in self.build_runs.iter().chain(self.probe_runs.iter()) {
            for run in runs {
                let _ = std::fs::remove_file(&run.path);
            }
        }
    }

    pub fn run_count(&self) -> u64 {
        (self.build_runs.iter().map(Vec::len).sum::<usize>()
            + self.probe_runs.iter().map(Vec::len).sum::<usize>()) as u64
    }

    pub fn row_count(&self) -> u64 {
        self.build_runs
            .iter()
            .chain(self.probe_runs.iter())
            .flatten()
            .map(|r| r.row_count)
            .sum()
    }

    pub fn byte_count(&self) -> u64 {
        self.build_runs
            .iter()
            .chain(self.probe_runs.iter())
            .flatten()
            .map(|r| r.byte_size)
            .sum()
    }
}

/// Per-operator Grace Hash Join spill state stored inline in the hash join
/// variants. `Default` is the pure in-memory fast path.
#[derive(Debug, Default)]
pub struct GraceJoinState {
    pub pending_build: Option<PendingBuildSpill>,
    pub partitioned: Option<PartitionedJoinState>,
    pub spilled_bytes: u64,
    pub spill_runs: u64,
    pub spilled_rows: u64,
    pub probe_col_names: Vec<String>,
}

impl GraceJoinState {
    pub fn has_spilled(&self) -> bool {
        self.pending_build.is_some() || self.partitioned.is_some()
    }

    pub fn cleanup_files(&self) {
        if let Some(partitioned) = &self.partitioned {
            partitioned.cleanup_files();
        }
    }
}

/// Default Grace partition count for hash join spill.
pub fn grace_partitions() -> u64 {
    HASH_JOIN_PARTITIONS_DEFAULT
}

/// Spill the current in-memory build side into partitioned runs.
///
/// Creates (or reuses) the pending build spiller in `grace`, drains indexed
/// rows without re-evaluating keys, and releases the build memory budget.
/// The build loop routes all remaining build input straight into the pending
/// spiller; finalization happens once the build input is exhausted.
pub fn spill_build_side(
    manager: Arc<SpillManager>,
    build_side: &mut HashJoinBuildSide,
    memory_tracker: &mut MemoryTracker,
    build_col_names: &[String],
    grace: &mut GraceJoinState,
) -> Result<(), QueryError> {
    if grace.pending_build.is_none() {
        grace.pending_build = Some(PendingBuildSpill {
            writer: RotatingPartitionWriter::new(
                manager.clone(),
                build_col_names,
                grace_partitions(),
            )?,
            build_col_names: build_col_names.to_vec(),
        });
    }
    let pending = grace.pending_build.as_mut().expect("pending must exist");
    let num_partitions = pending.writer.num_partitions();
    for (key, row) in build_side.take_indexed_rows() {
        let partition = hash_join_key_partition(&key, num_partitions);
        pending.writer.insert(partition, &row)?;
    }
    memory_tracker.reset();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key_of(value: Value) -> JoinKeyValue {
        JoinKeyValue::from(value)
    }

    #[test]
    fn key_partition_is_stable_across_sides() {
        for key in [
            key_of(Value::Int(7)),
            key_of(Value::BigInt(-3)),
            key_of(Value::string("k")),
            JoinKeyValue::Multi(vec![Value::Int(1), Value::string("x")]),
        ] {
            let build = hash_join_key_partition(&key, 32);
            let probe = hash_join_key_partition(&key, 32);
            assert_eq!(build, probe);
            assert!(build < 32);
        }
    }

    #[test]
    fn null_keys_share_partition_zero() {
        let null_single = key_of(Value::Null(graphdb_core::value::NullType::Null));
        let null_multi = JoinKeyValue::Multi(vec![
            Value::Null(graphdb_core::value::NullType::Null),
            Value::Null(graphdb_core::value::NullType::Null),
        ]);
        assert_eq!(hash_join_key_partition(&null_single, 32), 0);
        assert_eq!(hash_join_key_partition(&null_multi, 32), 0);
    }

    #[test]
    fn rotating_writer_bounds_open_runs() {
        let manager = Arc::new(
            SpillManager::new(
                crate::executor::streaming::spill::SpillConfig::default(),
                4301,
            )
            .unwrap(),
        );
        let mut writer =
            RotatingPartitionWriter::new(manager.clone(), &["a".to_string()], 2).unwrap();
        for i in 0..(COLLECTOR_RUN_ROWS_MAX + 10) {
            writer.insert(0, &[Value::Int(i as i32)]).unwrap();
        }
        writer.insert(1, &[Value::Int(-1)]).unwrap();
        let runs = writer.finish().unwrap();
        assert_eq!(runs.len(), 2);
        assert!(runs[0].len() >= 2);
        let total: usize = runs[0].iter().map(|r| r.row_count as usize).sum();
        assert_eq!(total, (COLLECTOR_RUN_ROWS_MAX + 10) as usize);
        assert_eq!(
            runs.iter().flatten().map(|r| r.row_count).sum::<u64>(),
            COLLECTOR_RUN_ROWS_MAX + 11
        );
    }

    #[test]
    fn partitioned_load_reports_corrupt_build_run() {
        use crate::executor::base::{MemoryBudget, MemoryTracker};

        let manager = Arc::new(
            SpillManager::new(
                crate::executor::streaming::spill::SpillConfig::default(),
                4302,
            )
            .unwrap(),
        );
        let names = vec!["a".to_string()];
        let mut writer = RotatingPartitionWriter::new(manager.clone(), &names, 1).unwrap();
        for i in 0..10 {
            writer.insert(0, &[Value::Int(i)]).unwrap();
        }
        let build_runs = writer.finish().unwrap();

        // Corrupt the stored body checksum (header bytes 24..32) of every
        // build run so partition load must fail with a checksum error.
        let mut corrupted = 0;
        for entry in std::fs::read_dir(manager.base_dir()).unwrap().flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("run") {
                continue;
            }
            let mut bytes = std::fs::read(&path).unwrap();
            bytes[24] ^= 0xff;
            std::fs::write(&path, &bytes).unwrap();
            corrupted += 1;
        }
        assert!(corrupted > 0);

        let mut state = PartitionedJoinState::new(
            build_runs,
            vec![Vec::new()],
            names.clone(),
            names,
            Some(manager),
            Vec::new(),
            Vec::new(),
        );
        let mut tracker = MemoryTracker::new(MemoryBudget::default_budget());
        let err = state.load_first(&[], &mut tracker, None).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("build partition load") && msg.contains("checksum mismatch"),
            "unexpected error: {}",
            msg
        );
    }
}
