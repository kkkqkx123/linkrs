use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use linkrs_core::error::QueryError;

use super::config::{SpillConfig, COLLECTOR_SPILL_ROWS_DEFAULT};
use super::hash_partition::HashPartitionSpiller;
use super::quota::DiskQuota;
use super::run_format::{SpilledRun, RUN_HEADER_SIZE};
use super::run_io::RunWriter;

// ── SpillManager ─────────────────────────────────────────────────────────────

/// Manages spill-file creation, cleanup, and tracking for one query execution.
///
/// Creates a unique subdirectory under `temp_dir` on construction and removes
/// it on drop only when empty; recursive cleanup is explicit via
/// `register_cleanup` at query end.
#[derive(Debug)]
pub struct SpillManager {
    pub(crate) config: SpillConfig,
    pub(crate) base_dir: PathBuf,
    pub(crate) file_counter: AtomicU64,
    pub(crate) spill_bytes: Arc<AtomicU64>,
    pub(crate) disk_quota: DiskQuota,
}

impl SpillManager {
    pub fn new(config: SpillConfig, query_id: u64) -> Result<Self, QueryError> {
        Self::new_with_quota(config, query_id, DiskQuota::default_quota())
    }

    /// Create with explicit disk quota.
    pub fn new_with_quota(
        config: SpillConfig,
        query_id: u64,
        disk_quota: DiskQuota,
    ) -> Result<Self, QueryError> {
        let base = config
            .temp_dir
            .clone()
            .unwrap_or_else(std::env::temp_dir)
            .join(format!("linkrs_spill_{}", query_id));
        std::fs::create_dir_all(&base)
            .map_err(|e| QueryError::execution(format!("Failed to create spill dir: {}", e)))?;
        Ok(Self {
            config,
            base_dir: base,
            file_counter: AtomicU64::new(0),
            spill_bytes: Arc::new(AtomicU64::new(0)),
            disk_quota,
        })
    }

    /// Create a run writer for spill data (versioned format with
    /// header/checksum). This is the single spill file format.
    ///
    /// Enforces `max_spill_files`: once the file budget is exhausted no new
    /// run can be created and a structured error is returned.
    pub fn create_run_writer(&self, schema_fingerprint: u64) -> Result<RunWriter, QueryError> {
        let id = self.file_counter.fetch_add(1, Ordering::Relaxed);
        if id >= self.config.max_spill_files as u64 {
            return Err(QueryError::execution(format!(
                "spill run: too many spill files (limit {})",
                self.config.max_spill_files,
            )));
        }
        std::fs::create_dir_all(&self.base_dir)
            .map_err(|e| QueryError::execution(format!("recreate spill dir: {}", e)))?;
        let path = self.base_dir.join(format!("run_{:016x}.run", id));
        let file = std::fs::File::create(&path)
            .map_err(|e| QueryError::execution(format!("create run file: {}", e)))?;

        // Reserve header space by writing dummy bytes
        let mut writer = BufWriter::new(file);
        let header_placeholder = [0u8; RUN_HEADER_SIZE as usize];
        writer
            .write_all(&header_placeholder)
            .map_err(|e| QueryError::execution(format!("write run header placeholder: {}", e)))?;

        Ok(RunWriter::new(writer, path, schema_fingerprint))
    }

    pub fn base_dir(&self) -> &Path {
        &self.base_dir
    }

    /// Access the spill configuration.
    pub fn config(&self) -> &SpillConfig {
        &self.config
    }

    /// Total spilled bytes finalized through this manager.
    pub fn spilled_bytes(&self) -> u64 {
        self.spill_bytes.load(Ordering::Relaxed)
    }

    /// Effective terminal-collector spill threshold in logical rows.
    ///
    /// `SpillConfig::collect_spill_rows`: `None` selects the default,
    /// `Some(0)` disables collector spill.
    pub fn collector_spill_threshold(&self) -> u64 {
        match self.config.collect_spill_rows {
            None => COLLECTOR_SPILL_ROWS_DEFAULT,
            Some(0) => u64::MAX,
            Some(v) => v,
        }
    }

    /// Finalize a run while enforcing disk quota.
    ///
    /// Single choke point for all spill paths: the run is finalized first
    /// (its exact on-disk size is only known then), quota is reserved, and
    /// on quota failure the file is removed so no orphaned run is left
    /// behind. Callers must route new `RunWriter::finalize` uses through
    /// here; direct `finalize` remains for tests and legacy paths.
    pub fn finalize_run(&self, writer: RunWriter) -> Result<SpilledRun, QueryError> {
        let path = writer.path().to_path_buf();
        let run = writer.finalize()?;
        if let Err(e) = self.disk_quota.try_reserve(run.byte_size) {
            let _ = std::fs::remove_file(&path);
            return Err(e);
        }
        self.spill_bytes.fetch_add(run.byte_size, Ordering::Relaxed);
        Ok(run)
    }

    /// Access the disk quota.
    pub fn disk_quota(&self) -> &DiskQuota {
        &self.disk_quota
    }

    /// Register recursive cleanup with an execution runtime.
    pub fn register_cleanup(
        &self,
        runtime: &crate::executor::streaming::runtime::ExecutionRuntime,
    ) {
        let base = self.base_dir.clone();
        runtime.on_cleanup(move || {
            let _ = std::fs::remove_dir_all(&base);
        });
    }
}

/// Finalize a partition spiller through the runtime's spill manager.
///
/// Quota-enforcing choke point for partition spill paths (aggregate, window,
/// group-by, distinct/materialize, join): when a manager is present each run
/// goes through [`SpillManager::finalize_run`] and the run sizes are recorded
/// into the runtime [`ColumnarStats`](crate::executor::streaming::runtime::ColumnarStats).
/// Without a manager there is no quota to enforce, so runs are finalized
/// directly.
pub fn finalize_partitions_with_runtime(
    mut spiller: HashPartitionSpiller,
    runtime: Option<&Arc<crate::executor::streaming::runtime::ExecutionRuntime>>,
) -> Result<Vec<Option<SpilledRun>>, QueryError> {
    match runtime.and_then(|rt| rt.get_spill_manager()) {
        Some(sm) => {
            let runs = spiller.finalize_with_manager(&sm)?;
            if let Some(rt) = runtime {
                let stats = rt.columnar_stats();
                for run in runs.iter().flatten() {
                    stats.record_spill(run.row_count, run.byte_size);
                }
            }
            Ok(runs)
        }
        None => {
            let mut runs = Vec::with_capacity(spiller.writers.len());
            for writer in spiller.writers.drain(..) {
                match writer {
                    Some(w) => runs.push(Some(w.finalize()?)),
                    None => runs.push(None),
                }
            }
            Ok(runs)
        }
    }
}

impl Drop for SpillManager {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir(&self.base_dir);
    }
}
