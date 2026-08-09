//! Columnar fast-path configuration.

use serde::{Deserialize, Serialize};

/// Configuration for the columnar fast paths (A1 column-block scan, etc.).
///
/// These knobs only change *how* the row-based `DataChunk` is filled by scan
/// sources; the output rows are bit-for-bit identical to the row-based path.
/// They are off by default so production behavior is unchanged until a
/// decision gate (see `docs/plan/fallback-and-typed-column-analysis.md`) is
/// crossed.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct ColumnarConfig {
    /// Enable the storage column-block scan path (A1).
    ///
    /// When enabled, storage sources stream column-major batches through the
    /// `next_column_batch` cursor API and build chunk typed columns directly
    /// from those batches (`column_block_hits` becomes observable in
    /// PROFILE/EXPLAIN ANALYZE). Default off — the row-based scan path is the
    /// production default.
    #[serde(default)]
    pub column_block_enabled: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_columnar_config_default() {
        let config = ColumnarConfig::default();
        assert!(!config.column_block_enabled);
    }
}
