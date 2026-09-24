//! Columnar fast-path configuration.

use serde::{Deserialize, Serialize};

/// Configuration for the columnar fast paths (column-block scan, etc.).
///
/// These knobs only change *how* the row-based `DataChunk` is filled by scan
/// sources; the output rows are bit-for-bit identical to the row-based path.
/// They are on by default; set `column_block_enabled` to false or export
/// `GRAPHDB_COLUMN_BLOCK_ENABLED=0` to roll back to the row-based path.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ColumnarConfig {
    /// Enable the storage column-block scan path.
    ///
    /// When enabled, storage sources stream column-major batches through the
    /// `next_column_batch` cursor API and build chunk typed columns directly
    /// from those batches (`column_block_hits` becomes observable in
    /// PROFILE/EXPLAIN ANALYZE). Default on — the row-based scan path stays
    /// as the fallback.
    #[serde(default = "default_column_block_enabled")]
    pub column_block_enabled: bool,
}

/// Default for the column-block scan path: on.
fn default_column_block_enabled() -> bool {
    true
}

impl Default for ColumnarConfig {
    fn default() -> Self {
        Self {
            column_block_enabled: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_columnar_config_default() {
        let config = ColumnarConfig::default();
        assert!(config.column_block_enabled);
    }
}
