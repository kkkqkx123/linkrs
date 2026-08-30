use crate::executor::streaming::spill::SpillConfig;

/// Default worker count for serial execution.
pub const DEFAULT_MAX_WORKERS: usize = 1;

/// Default per-partition output channel capacity.
pub const DEFAULT_MAX_BUFFERED_CHUNKS: usize = 10;

/// Create the default spill configuration.
pub fn default_spill_config() -> SpillConfig {
    SpillConfig::default()
}
