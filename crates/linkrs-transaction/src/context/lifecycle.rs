//! Transaction context behavior: transaction lifecycle, timeouts, ownership and state

use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use super::TransactionContext;
use crate::error::TransactionError;
use crate::types::*;
use crate::wal::Timestamp;
impl TransactionContext {
    pub fn get_type(&self) -> TransactionType {
        self.txn_type
    }

    pub fn get_concurrency_mode(&self) -> ConcurrencyMode {
        self.concurrency_mode
    }

    /// Get current state
    pub fn state(&self) -> TransactionState {
        self.state.load()
    }

    /// Get the MVCC timestamp
    pub fn timestamp(&self) -> Timestamp {
        self.start_timestamp
    }

    /// Get the commit timestamp allocated at commit time (0 = not committed).
    pub fn commit_timestamp(&self) -> Timestamp {
        self.commit_timestamp.load(Ordering::Relaxed)
    }

    /// Record the commit timestamp allocated by
    /// `VersionManager::allocate_commit_timestamp`.
    pub fn set_commit_timestamp(&self, commit_ts: Timestamp) {
        self.commit_timestamp.store(commit_ts, Ordering::Relaxed);
    }

    /// Check if transaction has expired
    pub fn is_expired(&self) -> bool {
        self.start_time.elapsed() > self.timeout
    }

    /// Check if query timeout has been exceeded
    pub fn is_query_timeout(&self) -> bool {
        if let Some(query_timeout) = self.query_timeout {
            self.statement_start.load().elapsed() > query_timeout
        } else {
            false
        }
    }

    /// Check if statement timeout has been exceeded
    pub fn is_statement_timeout(&self, statement_start: Instant) -> bool {
        if let Some(statement_timeout) = self.statement_timeout {
            statement_start.elapsed() > statement_timeout
        } else {
            false
        }
    }

    /// Check if idle timeout has been exceeded
    pub fn is_idle_timeout(&self) -> bool {
        if let Some(idle_timeout) = self.idle_timeout {
            self.last_activity.load().elapsed() > idle_timeout
        } else {
            false
        }
    }

    /// Check if any timeout has been exceeded
    pub fn check_timeouts(&self) -> Result<(), TransactionError> {
        if self.is_expired() {
            return Err(TransactionError::transaction_timeout());
        }

        if self.is_idle_timeout() {
            return Err(TransactionError::transaction_timeout());
        }

        Ok(())
    }

    /// Update last activity timestamp
    pub fn update_activity(&self) {
        self.last_activity.store(Instant::now());
    }

    /// Begin one statement and return its monotonic start time.
    pub fn begin_statement(&self) -> Result<Instant, TransactionError> {
        self.can_execute()?;
        self.check_timeouts()?;
        let start = Instant::now();
        self.statement_start.store(start);
        self.increment_query_count();
        Ok(start)
    }

    /// Finish one statement and enforce query and statement timeouts.
    pub fn finish_statement(&self, statement_start: Instant) -> Result<(), TransactionError> {
        let query_timed_out = self
            .query_timeout
            .is_some_and(|timeout| statement_start.elapsed() > timeout);
        let statement_timed_out = self.is_statement_timeout(statement_start);
        self.statement_start.store(Instant::now());
        self.update_activity();
        if query_timed_out || statement_timed_out {
            self.mark_rollback_only();
            return Err(TransactionError::transaction_timeout());
        }
        Ok(())
    }

    pub fn mark_rollback_only(&self) {
        self.rollback_only.store(true);
    }

    pub fn is_rollback_only(&self) -> bool {
        self.rollback_only.load()
    }

    pub fn set_owner(&self, owner: impl Into<String>) {
        *self.owner.write() = Some(owner.into());
    }

    pub fn owner(&self) -> Option<String> {
        self.owner.read().clone()
    }

    pub fn owner_matches(&self, owner: Option<&str>) -> bool {
        match (self.owner.read().as_deref(), owner) {
            (None, _) => true,
            (Some(expected), Some(actual)) => expected == actual,
            (Some(_), None) => false,
        }
    }

    /// One-way flag guarding the manager-owned write leases (checkpoint gate
    /// slot, single-writer exclusion owner).
    ///
    /// Claimed exactly once per transaction by
    /// `TransactionManager::release_write_lease`, which is shared by the
    /// commit path and every abort path. Timestamp retirement is NOT covered
    /// by this flag (retiring an already-terminal slot is a no-op by state
    /// check) and always runs on abort.
    pub fn mark_resources_released(&self) -> bool {
        self.resources_released
            .compare_exchange(false, true)
            .is_ok()
    }

    pub fn resources_released(&self) -> bool {
        self.resources_released.load()
    }

    /// Get remaining time
    pub fn remaining_time(&self) -> Duration {
        let elapsed = self.start_time.elapsed();
        if elapsed >= self.timeout {
            Duration::from_secs(0)
        } else {
            self.timeout - elapsed
        }
    }

    /// State transition
    ///
    /// Valid transitions form a DAG:
    ///   Active → Committing | Aborting
    ///   Committing → Committed | Aborting | Aborted
    ///   Aborting → Aborted
    ///
    /// `Committed` is terminal: a transaction that reached it can never be
    /// aborted. A durable commit whose storage finalization failed stays
    /// `Committing` so recovery can re-drive finalization.
    pub fn transition_to(&self, new_state: TransactionState) -> Result<(), TransactionError> {
        loop {
            let current = self.state.load();

            let valid_transition = matches!(
                (current, new_state),
                (
                    TransactionState::Active,
                    TransactionState::Committing | TransactionState::Aborting
                ) | (
                    TransactionState::Committing,
                    TransactionState::Committed
                        | TransactionState::Aborting
                        | TransactionState::Aborted
                ) | (TransactionState::Aborting, TransactionState::Aborted)
            );

            if !valid_transition {
                return Err(TransactionError::invalid_state_transition(
                    current, new_state,
                ));
            }

            if self.state.compare_exchange(current, new_state).is_ok() {
                return Ok(());
            }
        }
    }

    /// Reject write operations on read-only transactions up front.
    ///
    /// This is the `validateManualTransaction` equivalent: callers that know
    /// the statement kind fail fast here instead of wasting execution
    /// resources only to be rejected at commit certification.
    pub fn validate_write_allowed(&self) -> Result<(), TransactionError> {
        if self.read_only {
            return Err(TransactionError::read_only_transaction());
        }
        Ok(())
    }

    /// Check if operation can be executed
    pub fn can_execute(&self) -> Result<(), TransactionError> {
        let state = self.state.load();

        if !state.can_execute() {
            return Err(TransactionError::invalid_state_for_execution(state));
        }

        if self.is_rollback_only() {
            return Err(TransactionError::invalid_state_for_execution(state));
        }

        if self.is_expired() {
            return Err(TransactionError::transaction_expired());
        }

        Ok(())
    }

    /// Get transaction info
    pub fn info(&self) -> TransactionInfo {
        let modified_tables = self.get_modified_tables();
        let savepoint_count = self.get_all_savepoints().len();
        TransactionInfo {
            id: self.id,
            state: self.state.load(),
            txn_type: self.txn_type,
            start_time: self.start_time,
            elapsed: self.start_time.elapsed(),
            is_read_only: self.read_only,
            isolation_level: self.isolation_level,
            query_count: self.query_count.load(Ordering::Relaxed),
            mutation_count: self.mutation_count.load(Ordering::Relaxed),
            modified_tables,
            savepoint_count,
            read_timestamp: self.effective_read_timestamp(),
            write_timestamp: if self.read_only { 0 } else { self.timestamp() },
            owner: self.owner(),
            last_activity: self.last_activity.load().elapsed(),
            rollback_only: self.is_rollback_only(),
            blocking_reason: if self.is_rollback_only() {
                Some("transaction is marked rollback-only".to_string())
            } else {
                None
            },
            staged_bytes: self.staged_bytes(),
            undo_bytes: self.undo_log_len() as u64,
        }
    }

    /// Clear all state
    pub fn clear(&self) -> Result<(), TransactionError> {
        self.clear_undo_logs()?;
        {
            let mut write_set = self.write_set.lock();
            *write_set = WriteSet::new();
        }
        self.write_validated.store(false);
        self.rollback_only.store(false);
        self.resources_released.store(false);
        self.staged_bytes.store(0, Ordering::Relaxed);
        {
            let mut tables = self.modified_tables.lock();
            tables.clear();
        }
        {
            let mut manager = self.savepoint_manager.write();
            manager.clear();
        }
        self.mutation_journal.write().truncate(0);
        self.local_wal.lock().clear();
        self.restore_read_set(WriteSet::new());
        self.mutation_count.store(0, Ordering::Relaxed);
        self.undo_bytes.store(0, Ordering::Relaxed);
        Ok(())
    }
}
