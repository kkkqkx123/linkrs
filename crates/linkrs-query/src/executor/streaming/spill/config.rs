use std::path::PathBuf;

/// Configuration for disk spill behavior.
#[derive(Debug, Clone)]
pub struct SpillConfig {
    /// Directory for spill files. `None` uses the system temp directory.
    pub temp_dir: Option<PathBuf>,
    /// Maximum number of spill run files for one query execution.
    pub max_spill_files: usize,
    /// Terminal collector spill threshold in logical rows.
    /// `None` uses the default threshold; `Some(0)` disables collector spill.
    pub collect_spill_rows: Option<u64>,
    /// Maximum bytes of spill runs for one query. Zero means unlimited.
    pub disk_quota_bytes: u64,
}

impl Default for SpillConfig {
    fn default() -> Self {
        Self {
            temp_dir: None,
            max_spill_files: 256,
            collect_spill_rows: None,
            disk_quota_bytes: 2 * 1024 * 1024 * 1024,
        }
    }
}

/// Default terminal-collector spill threshold in logical rows.
pub const COLLECTOR_SPILL_ROWS_DEFAULT: u64 = 200_000;

/// Maximum rows per terminal-collector run file. Bounds both the writer-side
/// body buffer and the reader-side full-run load peak.
pub const COLLECTOR_RUN_ROWS_MAX: u64 = 65_536;

/// Default Grace Hash Join partition count.
pub const HASH_JOIN_PARTITIONS_DEFAULT: u64 = 32;

/// Maximum Grace Hash Join repartition depth.
pub const HASH_JOIN_MAX_DEPTH: u32 = 3;
