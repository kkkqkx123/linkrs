//! Recovery orchestration skeleton and lifecycle interface

use crate::wal::{LocalWalParser, Lsn, ParallelWalParser, RecoveryResult, WalParser};
use linkrs_core::{StorageError, StorageResult};

use super::config::{RecoveryConfig, RecoveryStats};
use super::RecoveryApplier;

pub struct RecoveryManager {
    pub(super) config: RecoveryConfig,
    pub(super) stats: RecoveryStats,
}

impl RecoveryManager {
    pub fn new(config: RecoveryConfig) -> Self {
        Self {
            config,
            stats: RecoveryStats::default(),
        }
    }

    pub fn recover_with_applier(
        &mut self,
        applier: &dyn RecoveryApplier,
    ) -> StorageResult<RecoveryStats> {
        let start = std::time::Instant::now();

        self.stats = RecoveryStats::default();
        self.stats.last_lsn = self.config.start_lsn.unwrap_or(Lsn::ZERO);

        let wal_result = self.parse_wal_files()?;
        let (wal_result, dry_stats) = self.apply_dry_replay_filter(wal_result)?;
        if let Some(dry) = dry_stats {
            log::info!(
                "Dry replay: scanned={}, consistent={}, truncated_tail={}, corrupted={}, sequence_gaps={}, last_consistent_lsn={}",
                dry.total_scanned,
                dry.consistent_entries.len(),
                dry.truncated_tail,
                dry.corrupted_count,
                dry.sequence_gaps,
                dry.last_consistent_lsn
            );
            if dry.truncated_tail > 0 {
                log::warn!(
                    "WAL dry replay truncated {} torn-tail entries",
                    dry.truncated_tail
                );
            }
            if dry.sequence_gaps > 0 {
                log::warn!(
                    "WAL dry replay detected {} sequence gap(s)",
                    dry.sequence_gaps
                );
            }
        }
        self.stats.max_timestamp = wal_result.last_timestamp;
        self.stats.max_transaction_id = self.max_transaction_id(&wal_result);
        self.stats.errors_encountered = wal_result
            .corrupted_count
            .saturating_add(wal_result.skipped_count);
        if self.stats.errors_encountered > 0 {
            log::warn!(
                "WAL recovery found {} recoverable tail/corruption markers",
                self.stats.errors_encountered
            );
        }

        self.restore_from_checkpoint(&wal_result)?;
        self.replay_wal_entries(&wal_result, applier)?;
        self.stats.recovery_time_ms = start.elapsed().as_millis() as u64;

        Ok(self.stats.clone())
    }

    fn parse_wal_files(&self) -> StorageResult<RecoveryResult> {
        if self.config.parallel_recovery {
            let parser = ParallelWalParser::new()
                .with_recovery_mode(self.config.recovery_mode)
                .with_verify_checksum(self.config.verify_checksum);
            parser
                .parse_parallel(&self.config.wal_dir)
                .map_err(|e| StorageError::db_error(format!("WAL parse error: {}", e)))
        } else {
            let mut parser = LocalWalParser::new();
            parser
                .open(&self.config.wal_dir.to_string_lossy())
                .map_err(|e| StorageError::db_error(format!("WAL open error: {}", e)))?;
            Ok(RecoveryResult {
                all_entries: parser.parse_all_entries(),
                last_timestamp: parser.last_timestamp(),
                last_lsn: parser.last_lsn(),
                corrupted_count: parser.corrupted_count(),
                skipped_count: parser.skipped_count(),
            })
        }
    }

    fn restore_from_checkpoint(&mut self, _wal_result: &RecoveryResult) -> StorageResult<()> {
        if !self.config.data_dir.exists() {
            std::fs::create_dir_all(&self.config.data_dir)?;
            return Ok(());
        }
        self.stats.checkpoints_processed = 1;
        Ok(())
    }

    pub fn stats(&self) -> &RecoveryStats {
        &self.stats
    }

    pub fn needs_recovery(&self) -> bool {
        self.config.wal_dir.exists()
            && std::fs::read_dir(&self.config.wal_dir)
                .map(|mut entries| entries.next().is_some())
                .unwrap_or(false)
    }

    pub fn clear_wal_files(&self) -> StorageResult<()> {
        if !self.config.wal_dir.exists() {
            return Ok(());
        }
        for entry in std::fs::read_dir(&self.config.wal_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "wal") {
                std::fs::remove_file(&path)?;
            }
        }
        Ok(())
    }
}

impl Default for RecoveryManager {
    fn default() -> Self {
        Self::new(RecoveryConfig::default())
    }
}
