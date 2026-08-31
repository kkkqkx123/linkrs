use crate::executor::streaming::spill::SpillConfig;

/// Default worker count for serial execution.
#[allow(dead_code)]
pub const DEFAULT_MAX_WORKERS: usize = 1;

/// Default per-partition output channel capacity.
#[allow(dead_code)]
pub const DEFAULT_MAX_BUFFERED_CHUNKS: usize = 10;

/// Create the default spill configuration.
#[allow(dead_code)]
pub fn default_spill_config() -> SpillConfig {
    SpillConfig::default()
}
