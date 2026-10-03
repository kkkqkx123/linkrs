//! Transaction manager behavior: transaction begin paths with admission control

use std::sync::atomic::Ordering;
use std::sync::Arc;

use super::TransactionManager;
use crate::context::TransactionContext;
use crate::error::TransactionError;
use crate::types::*;
impl TransactionManager {
    /// Start a new read transaction
    pub fn begin_read_transaction(
        &self,
        options: TransactionOptions,
    ) -> Result<TransactionId, TransactionError> {
        if self.shutdown_flag.load(Ordering::SeqCst) != 0 {
            return Err(TransactionError::internal(
                "Transaction manager is shutdown".to_string(),
            ));
        }

        self.maybe_cleanup_expired_transactions();

        let active_count = self.active_transactions.len();
        if active_count >= self.config.max_concurrent_transactions {
            return Err(TransactionError::too_many_transactions());
        }

        let txn_id = TransactionId(self.id_generator.fetch_add(1, Ordering::SeqCst));
        let timestamp = self
            .version_manager
            .acquire_read_timestamp()
            .map_err(|e| TransactionError::internal(e.to_string()))?;
        let timeout = options.timeout.unwrap_or(self.config.default_timeout);

        let config = TransactionConfig {
            timeout,
            durability: options.durability,
            isolation_level: options.isolation_level,
            query_timeout: options.query_timeout,
            statement_timeout: options.statement_timeout,
            idle_timeout: options.idle_timeout,
            ..self.config.txn_config.clone()
        };

        let context = Arc::new(TransactionContext::new_readonly(txn_id, timestamp, config));

        self.active_transactions.insert(txn_id, context);
        self.stats.record_txn_begin();
        if let Some(observable) = &self.observable {
            observable.record_txn_begin();
        }

        Ok(txn_id)
    }

    /// Start a new insert transaction
    ///
    /// Multiple insert transactions can be active concurrently.
    /// Conflict detection is performed by `check_write_set_conflict()`
    /// based on actual write set overlaps, not at transaction start time.
    ///
    /// Returns `TransactionError::CheckpointInProgress` if a checkpoint
    /// operation has paused new writes.
    pub fn begin_insert_transaction(
        &self,
        options: TransactionOptions,
    ) -> Result<TransactionId, TransactionError> {
        if self.shutdown_flag.load(Ordering::SeqCst) != 0 {
            return Err(TransactionError::internal(
                "Transaction manager is shutdown".to_string(),
            ));
        }

        // Checkpoint gate: refuse new writes during checkpoint drain.
        // In-memory mode skips the gate entirely (no WAL/checkpoint).
        if !self.config.in_memory {
            self.checkpoint_gate.acquire_write()?;
        }

        self.maybe_cleanup_expired_transactions();

        let active_count = self.active_transactions.len();
        if active_count >= self.config.max_concurrent_transactions {
            if !self.config.in_memory {
                self.checkpoint_gate.release_write();
            }
            return Err(TransactionError::too_many_transactions());
        }

        let txn_id = TransactionId(self.id_generator.fetch_add(1, Ordering::SeqCst));
        let timestamp = self
            .version_manager
            .acquire_insert_timestamp()
            .map_err(|e| {
                if !self.config.in_memory {
                    self.checkpoint_gate.release_write();
                }
                TransactionError::internal(e.to_string())
            })?;
        let timeout = options.timeout.unwrap_or(self.config.default_timeout);

        let config = TransactionConfig {
            timeout,
            durability: options.durability,
            isolation_level: options.isolation_level,
            query_timeout: options.query_timeout,
            statement_timeout: options.statement_timeout,
            idle_timeout: options.idle_timeout,
            ..self.config.txn_config.clone()
        };

        let context = Arc::new(TransactionContext::new(txn_id, timestamp, config));

        if context.get_concurrency_mode() == ConcurrencyMode::SingleWriter {
            // Use compare_exchange so a failed contender does not overwrite the
            // legitimate owner's id (swap would corrupt ownership tracking).
            // The timestamp acquired above must be released on contention,
            // otherwise the write frontier stays pinned by an orphaned Pending slot.
            if self
                .write_exclusion_owner
                .compare_exchange(0, txn_id.0, Ordering::SeqCst, Ordering::SeqCst)
                .is_err()
            {
                self.version_manager.abort_write_timestamp(timestamp);
                if !self.config.in_memory {
                    self.checkpoint_gate.release_write();
                }
                return Err(TransactionError::write_transaction_conflict());
            }
            context.set_pessimistic_lock();
        }

        self.active_transactions.insert(txn_id, context);
        self.stats.record_txn_begin();
        if let Some(observable) = &self.observable {
            observable.record_txn_begin();
        }

        log::info!(
            "write transaction began: txn={:?} write_ts={} max_concurrent={}",
            txn_id,
            timestamp,
            self.config.max_concurrent_transactions
        );

        Ok(txn_id)
    }

    /// Start a new transaction
    pub fn begin_transaction(
        &self,
        options: TransactionOptions,
    ) -> Result<TransactionId, TransactionError> {
        if options.read_only {
            self.begin_read_transaction(options)
        } else {
            self.begin_insert_transaction(options)
        }
    }

    /// Begin a transaction and bind it to an API/session owner.
    pub fn begin_transaction_with_owner(
        &self,
        options: TransactionOptions,
        owner: impl Into<String>,
    ) -> Result<TransactionId, TransactionError> {
        let txn_id = self.begin_transaction(options)?;
        self.set_transaction_owner(txn_id, owner)?;
        Ok(txn_id)
    }
}
