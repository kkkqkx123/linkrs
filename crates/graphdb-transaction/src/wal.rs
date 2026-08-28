//! Write-Ahead Log (WAL) Module
//!
//! Provides durability guarantees through write-ahead logging.
//!
//! ## Components
//!
//! - `WalWriter`: Write WAL entries to persistent storage
//! - `WalParser`: Parse WAL files for recovery
//! - `WalHeader`: WAL entry header format
//!
//! ## Usage
//!
//! ```rust
//! use graphdb_transaction::wal::{LocalWalWriter, WalWriter, WalOpType};
//!
//! let dir = tempfile::tempdir().unwrap();
//! let path = dir.path().to_string_lossy().to_string();
//!
//! let mut writer = LocalWalWriter::new(&path, 0);
//! writer.open().unwrap();
//! writer
//!     .append_entry(WalOpType::InsertVertex, 1, b"payload")
//!     .unwrap();
//! writer.sync().unwrap();
//! writer.close();
//! ```
//!
//! ## Recovery
//!
//! See the [`LocalWalParser`](parser/struct.LocalWalParser.html) docs for
//! recovery examples.

pub mod checkpoint;
pub mod commit;
pub mod filter;
pub mod parser;
pub mod recovery;
pub mod writer;

// Direct imports from core WAL layer
pub use graphdb_core::types::{TableId, TableTracker, TableTrackerConfig, TableType};
pub use graphdb_core::wal::redo::*;
pub use graphdb_core::wal::types::*;
pub use checkpoint::{Checkpoint, CheckpointManager, CheckpointMode, CheckpointResult};
pub use commit::{collect_committed_transactions, CommittedWalTransaction, TransactionWalEntry};
pub use filter::{filter_intents_for_index, filter_intents_for_indexes};
pub use parser::{
    LocalWalParser, ParallelWalParser, ParsedWalEntry, RecoveryResult, WalEntryIter, WalParser,
    WalParserFactory,
};
pub use recovery::{RecoveryApplier, RecoveryConfig, RecoveryManager, RecoveryStats};

// Re-export core types for callers working in wal context
pub use graphdb_core::types::{ColumnId, EdgeId, LabelId, Timestamp, VertexId};
pub use writer::{LocalWalWriter, WalWriter};

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_wal_roundtrip() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let wal_path = temp_dir.path().to_string_lossy().to_string();

        {
            let mut writer = LocalWalWriter::new(&wal_path, 0);
            writer.open().expect("Failed to open WAL");

            writer
                .append_entry(WalOpType::InsertVertex, 1, b"test_data")
                .expect("Failed to append");

            writer.sync().expect("Failed to sync");
        }

        let mut parser = LocalWalParser::new();
        parser.open(&wal_path).expect("Failed to parse WAL");

        let entries = parser.parse_all_entries();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].payload, b"test_data");
    }

    #[test]
    fn test_wal_fragmented_roundtrip() {
        use WAL_MAX_RECORD_SIZE;

        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let wal_path = temp_dir.path().to_string_lossy().to_string();

        let large_payload: Vec<u8> = (0..(WAL_MAX_RECORD_SIZE * 2 + 5000))
            .map(|i| (i % 256) as u8)
            .collect();

        {
            let config = WalConfig::new().with_checksum(true);
            let mut writer = LocalWalWriter::with_config(&wal_path, 0, config);
            writer.open().expect("Failed to open WAL");

            writer
                .append_entry(WalOpType::InsertVertex, 1, &large_payload)
                .expect("Failed to append");

            writer.sync().expect("Failed to sync");
        }

        let mut parser = LocalWalParser::new();
        parser.open(&wal_path).expect("Failed to parse WAL");

        let entries = parser.parse_all_entries();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].payload, large_payload);
    }
}
