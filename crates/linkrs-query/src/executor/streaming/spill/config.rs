use std::path::PathBuf;

/// Configuration for disk spill behavior.
#[derive(Debug, Clone)]
pub struct SpillConfig {
    /// Directory for spill files. `None` → system temp dir.
    pub temp_dir: Option<PathBuf>,
    /// Maximum number of spill files per operator instance.
    pub max_spill_files: usize,
    /// Terminal collector spill threshold in logical rows.
    /// `None` → default threshold; `Some(0)` → collector never spills.
    pub collect_spill_rows: Option<u64>,
}

impl Default for SpillConfig {
    fn default() -> Self {
        Self {
            temp_dir: None,
            max_spill_files: 64,
            collect_spill_rows: None,
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
