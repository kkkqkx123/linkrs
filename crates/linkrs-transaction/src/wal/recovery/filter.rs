//! Dry-replay consistency filtering and committed-transaction boundary computation

use postcard::from_bytes;

use crate::wal::{RecoveryResult, WalOpType};
use linkrs_core::{StorageError, StorageResult};

use super::manager::RecoveryManager;
use super::RecoveryApplier;

impl RecoveryManager {
    /// Run a dry replay scan without applying mutations, returning the
    /// last consistent commit boundary. Mirrors Ladybug's
    /// `WALReplayer::dryReplay`.
    pub fn dry_replay(&self) -> StorageResult<crate::wal::dry_replay::DryReplayResult> {
        crate::wal::dry_replay::dry_replay(
            &self.config.wal_dir,
            self.config.verify_checksum,
            self.config.throw_on_wal_replay_failure,
        )
        .map_err(|e| StorageError::wal_error(format!("Dry replay failed: {}", e)))
    }

    pub(super) fn apply_dry_replay_filter(
        &self,
        wal_result: RecoveryResult,
    ) -> StorageResult<(
        RecoveryResult,
        Option<crate::wal::dry_replay::DryReplayResult>,
    )> {
        if self.config.throw_on_wal_replay_failure {
            return Ok((wal_result, None));
        }
        let dry = crate::wal::dry_replay::find_last_consistent_commit(
            wal_result,
            self.config.throw_on_wal_replay_failure,
        );
        let total = dry.total_scanned;
        let truncated = dry.truncated_tail;
        let corrupted = dry.corrupted_count;
        let last_lsn = dry.last_consistent_lsn;
        let last_ts = dry.last_consistent_timestamp;
        let consistent_entries = dry.consistent_entries.clone();
        let filtered = RecoveryResult {
            all_entries: consistent_entries,
            last_timestamp: last_ts,
            last_lsn,
            corrupted_count: corrupted,
            skipped_count: truncated,
        };
        let _ = total;
        Ok((filtered, Some(dry)))
    }

    pub(super) fn max_transaction_id(&self, wal_result: &RecoveryResult) -> u64 {
        wal_result
            .all_entries
            .iter()
            .filter_map(|entry| {
                let op_type = WalOpType::try_from(entry.header.op_type).ok()?;
                match op_type {
                    WalOpType::OutboxIntent => {
                        from_bytes::<linkrs_core::wal::OutboxIntent>(&entry.payload)
                            .ok()
                            .map(|intent| intent.transaction_id.as_u64())
                    }
                    WalOpType::TransactionCommit => {
                        from_bytes::<linkrs_core::wal::TransactionCommit>(&entry.payload)
                            .ok()
                            .map(|commit| commit.transaction_id.as_u64())
                    }
                    WalOpType::TransactionAbort => {
                        from_bytes::<linkrs_core::wal::TransactionAbort>(&entry.payload)
                            .ok()
                            .map(|abort| abort.transaction_id.as_u64())
                    }
                    _ => None,
                }
            })
            .max()
            .unwrap_or(0)
    }

    pub(super) fn replay_wal_entries(
        &mut self,
        wal_result: &RecoveryResult,
        applier: &dyn RecoveryApplier,
    ) -> StorageResult<()> {
        let has_sync_envelope = wal_result.all_entries.iter().any(|entry| {
            matches!(
                WalOpType::try_from(entry.header.op_type),
                Ok(WalOpType::OutboxIntent
                    | WalOpType::TransactionCommit
                    | WalOpType::TransactionAbort)
            )
        });
        if !has_sync_envelope {
            self.replay_parsed_entries(&wal_result.all_entries, applier)?;
            self.stats.last_lsn = wal_result.last_lsn;
            return Ok(());
        }
        let transactions = crate::wal::collect_committed_transactions(&wal_result.all_entries)
            .map_err(|error| {
                StorageError::wal_error(format!(
                    "Failed to validate committed WAL batches: {}",
                    error
                ))
            })?;
        for transaction in transactions {
            self.replay_parsed_entries(&transaction.redo_entries, applier)?;
            self.stats.last_lsn = crate::wal::Lsn::new(transaction.commit_lsn.get());
        }
        Ok(())
    }
}
