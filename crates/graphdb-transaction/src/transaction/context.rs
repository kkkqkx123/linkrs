//! Transaction Context
//!
//! Manages the state and resources of a single transaction.

use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use crossbeam_utils::atomic::AtomicCell;
use parking_lot::{Mutex, RwLock};

use super::error::TransactionError;
use super::rollback::CombinedRollback;
use super::types::*;
use super::undo_log::{UndoLogEntry, UndoLogManager, UndoTarget};
use super::wal::Timestamp;

/// Transaction Context
///
/// Manages the state and resources of a single transaction.
/// Uses MVCC timestamps for snapshot isolation.
pub struct TransactionContext {
    /// Transaction ID
    pub id: TransactionId,
    /// Current state
    state: AtomicCell<TransactionState>,
    /// Start timestamp (MVCC)
    pub start_timestamp: Timestamp,
    /// Snapshot timestamp for time-travel reads (None = use start_timestamp)
    pub snapshot_timestamp: Option<Timestamp>,
    /// Start time (for timeout tracking)
    pub start_time: Instant,
    /// Timeout duration
    timeout: Duration,
    /// Whether read-only
    pub read_only: bool,
    /// Isolation level
    pub isolation_level: IsolationLevel,
    /// Query timeout duration
    pub query_timeout: Option<Duration>,
    /// Statement timeout duration
    pub statement_timeout: Option<Duration>,
    /// Idle timeout duration
    pub idle_timeout: Option<Duration>,
    /// Last activity timestamp
    last_activity: AtomicCell<Instant>,
    /// Query count
    query_count: AtomicU64,
    /// Durability level
    pub durability: DurabilityLevel,
    /// Operation log (using RwLock to optimize read-heavy write-light scenarios)
    operation_logs: RwLock<Vec<OperationLog>>,
    /// Modified tables
    modified_tables: Mutex<Vec<String>>,
    /// Savepoint manager
    savepoint_manager: RwLock<SavepointManager>,
    /// Undo log manager for rollback
    undo_logs: RwLock<UndoLogManager>,
    /// Whether to enable two-phase commit
    two_phase_enabled: bool,
    /// Write set for conflict detection
    write_set: Mutex<WriteSet>,
}

impl fmt::Debug for TransactionContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TransactionContext")
            .field("id", &self.id)
            .field("state", &self.state.load())
            .field("start_timestamp", &self.start_timestamp)
            .field("snapshot_timestamp", &self.snapshot_timestamp)
            .field("read_only", &self.read_only)
            .field("isolation_level", &self.isolation_level)
            .field("durability", &self.durability)
            .finish()
    }
}

/// Savepoint Manager
pub(crate) struct SavepointManager {
    savepoints: HashMap<SavepointId, SavepointInfo>,
    next_id: SavepointId,
    next_sequence: u64,
}

impl SavepointManager {
    fn new() -> Self {
        Self {
            savepoints: HashMap::new(),
            next_id: 1,
            next_sequence: 1,
        }
    }

    fn create_savepoint(
        &mut self,
        name: Option<String>,
        operation_log_index: usize,
        undo_log_index: usize,
        sync_sequence: u64,
    ) -> SavepointId {
        let id = self.next_id;
        self.next_id += 1;
        let sequence = self.next_sequence;
        self.next_sequence += 1;
        let info = SavepointInfo {
            id,
            name,
            created_at: Instant::now(),
            sequence,
            operation_log_index,
            undo_log_index,
            sync_sequence,
        };
        self.savepoints.insert(id, info);
        id
    }

    fn get_savepoint(&self, id: SavepointId) -> Option<&SavepointInfo> {
        self.savepoints.get(&id)
    }

    fn remove_savepoint(&mut self, id: SavepointId) -> Option<SavepointInfo> {
        self.savepoints.remove(&id)
    }

    fn clear(&mut self) {
        self.savepoints.clear();
    }

    fn find_by_name(&self, name: &str) -> Option<SavepointInfo> {
        self.savepoints
            .values()
            .find(|sp| sp.name.as_deref() == Some(name))
            .cloned()
    }
}

impl TransactionContext {
    /// Create a new transaction context
    pub fn new(id: TransactionId, start_timestamp: Timestamp, config: TransactionConfig) -> Self {
        let now = Instant::now();
        Self {
            id,
            state: AtomicCell::new(TransactionState::Active),
            start_timestamp,
            snapshot_timestamp: None,
            start_time: now,
            timeout: config.timeout,
            read_only: false,
            isolation_level: config.isolation_level,
            query_timeout: config.query_timeout,
            statement_timeout: config.statement_timeout,
            idle_timeout: config.idle_timeout,
            last_activity: AtomicCell::new(now),
            query_count: AtomicU64::new(0),
            durability: config.durability,
            operation_logs: RwLock::new(Vec::new()),
            modified_tables: Mutex::new(Vec::new()),
            savepoint_manager: RwLock::new(SavepointManager::new()),
            undo_logs: RwLock::new(UndoLogManager::new()),
            two_phase_enabled: config.two_phase_commit,
            write_set: Mutex::new(WriteSet::new()),
        }
    }

    /// Create a new read-only transaction context
    pub fn new_readonly(
        id: TransactionId,
        start_timestamp: Timestamp,
        config: TransactionConfig,
    ) -> Self {
        let now = Instant::now();
        Self {
            id,
            state: AtomicCell::new(TransactionState::Active),
            start_timestamp,
            snapshot_timestamp: None,
            start_time: now,
            timeout: config.timeout,
            read_only: true,
            isolation_level: config.isolation_level,
            query_timeout: config.query_timeout,
            statement_timeout: config.statement_timeout,
            idle_timeout: config.idle_timeout,
            last_activity: AtomicCell::new(now),
            query_count: AtomicU64::new(0),
            durability: DurabilityLevel::Sync,
            operation_logs: RwLock::new(Vec::new()),
            modified_tables: Mutex::new(Vec::new()),
            savepoint_manager: RwLock::new(SavepointManager::new()),
            undo_logs: RwLock::new(UndoLogManager::new()),
            two_phase_enabled: config.two_phase_commit,
            write_set: Mutex::new(WriteSet::new()),
        }
    }

    /// Get current state
    pub fn state(&self) -> TransactionState {
        self.state.load()
    }

    /// Get the MVCC timestamp
    pub fn timestamp(&self) -> Timestamp {
        self.start_timestamp
    }

    /// Get the effective snapshot timestamp for reads
    pub fn effective_snapshot_timestamp(&self) -> Timestamp {
        self.snapshot_timestamp.unwrap_or(self.start_timestamp)
    }

    /// Set the snapshot timestamp for time-travel reads
    pub fn set_snapshot_timestamp(&mut self, ts: Timestamp) {
        self.snapshot_timestamp = Some(ts);
    }

    /// Check if transaction has expired
    pub fn is_expired(&self) -> bool {
        self.start_time.elapsed() > self.timeout
    }

    /// Check if query timeout has been exceeded
    pub fn is_query_timeout(&self) -> bool {
        if let Some(query_timeout) = self.query_timeout {
            self.start_time.elapsed() > query_timeout
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

        if self.is_query_timeout() {
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

    /// Increment query count
    pub fn increment_query_count(&self) {
        self.query_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Get query count
    pub fn query_count(&self) -> u64 {
        self.query_count.load(Ordering::Relaxed)
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
    pub fn transition_to(&self, new_state: TransactionState) -> Result<(), TransactionError> {
        loop {
            let current = self.state.load();

            let valid_transition = matches!(
                (current, new_state),
                (
                    TransactionState::Active,
                    TransactionState::Committing | TransactionState::Aborting
                ) | (TransactionState::Committing, TransactionState::Committed)
                    | (TransactionState::Committing, TransactionState::Aborted)
                    | (TransactionState::Aborting, TransactionState::Committed)
                    | (TransactionState::Aborting, TransactionState::Aborted)
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

    /// Whether to enable two-phase commit
    pub fn is_two_phase_enabled(&self) -> bool {
        self.two_phase_enabled
    }

    /// Get the write set for this transaction
    pub fn get_write_set(&self) -> WriteSet {
        self.write_set.lock().clone()
    }

    /// Check if write set is empty
    pub fn is_write_set_empty(&self) -> bool {
        self.write_set.lock().is_empty()
    }

    /// Get write set size (number of modified entities)
    pub fn write_set_size(&self) -> usize {
        self.write_set.lock().size()
    }

    /// Check if this transaction's write set conflicts with another
    pub fn has_write_conflict_with(&self, other: &TransactionContext) -> bool {
        let ws1 = self.write_set.lock();
        let ws2 = other.write_set.lock();
        ws1.has_conflict_with(&ws2)
    }

    /// Check if operation can be executed
    pub fn can_execute(&self) -> Result<(), TransactionError> {
        let state = self.state.load();

        if !state.can_execute() {
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
            start_time: self.start_time,
            elapsed: self.start_time.elapsed(),
            is_read_only: self.read_only,
            isolation_level: self.isolation_level,
            query_count: self.query_count.load(Ordering::Relaxed),
            modified_tables,
            savepoint_count,
        }
    }

    /// Add operation log
    pub fn add_operation_log(&self, operation: OperationLog) {
        let mut logs = self.operation_logs.write();
        logs.push(operation);
    }

    /// Batch add operation logs
    pub fn add_operation_logs(&self, operations: Vec<OperationLog>) {
        let mut logs = self.operation_logs.write();
        logs.extend(operations);
    }

    /// Get operation logs
    pub fn get_operation_logs(&self) -> Vec<OperationLog> {
        let logs = self.operation_logs.read();
        logs.clone()
    }

    /// Get operation log length
    pub fn operation_log_len(&self) -> usize {
        let logs = self.operation_logs.read();
        logs.len()
    }

    /// Get operation log at specified index
    pub fn get_operation_log(&self, index: usize) -> Option<OperationLog> {
        let logs = self.operation_logs.read();
        logs.get(index).cloned()
    }

    /// Get operation logs in specified range
    pub fn get_operation_logs_range(&self, start: usize, end: usize) -> Vec<OperationLog> {
        let logs = self.operation_logs.read();
        if start >= logs.len() {
            return Vec::new();
        }
        let end = end.min(logs.len());
        logs[start..end].to_vec()
    }

    /// Truncate operation logs to specified index
    pub fn truncate_operation_log(&self, index: usize) {
        let mut logs = self.operation_logs.write();
        if index < logs.len() {
            logs.truncate(index);
        }
    }

    /// Clear operation logs
    pub fn clear_operation_log(&self) {
        let mut logs = self.operation_logs.write();
        logs.clear();
    }

    /// Record table modification
    pub fn record_table_modification(&self, table_name: &str) {
        let mut tables = self.modified_tables.lock();
        if !tables.contains(&table_name.to_string()) {
            tables.push(table_name.to_string());
        }
    }

    /// Get modified tables
    pub fn get_modified_tables(&self) -> Vec<String> {
        let tables = self.modified_tables.lock();
        tables.clone()
    }

    /// Create savepoint
    pub fn create_savepoint(&self, name: Option<String>, sync_sequence: u64) -> SavepointId {
        let operation_log_index = self.operation_log_len();
        let undo_log_index = self.undo_log_len();
        let mut manager = self.savepoint_manager.write();
        manager.create_savepoint(name, operation_log_index, undo_log_index, sync_sequence)
    }

    /// Get savepoint info
    pub fn get_savepoint(&self, id: SavepointId) -> Option<SavepointInfo> {
        let manager = self.savepoint_manager.read();
        manager.get_savepoint(id).cloned()
    }

    /// Find savepoint by ID (alias for get_savepoint for API clarity)
    pub fn find_savepoint_by_id(&self, id: SavepointId) -> Option<SavepointInfo> {
        self.get_savepoint(id)
    }

    /// Get all savepoints
    pub fn get_all_savepoints(&self) -> Vec<SavepointInfo> {
        let manager = self.savepoint_manager.read();
        manager.savepoints.values().cloned().collect()
    }

    /// Find savepoint by name
    pub fn find_savepoint_by_name(&self, name: &str) -> Option<SavepointInfo> {
        let manager = self.savepoint_manager.read();
        manager.find_by_name(name)
    }

    /// Release savepoint
    pub fn release_savepoint(&self, id: SavepointId) -> Result<(), TransactionError> {
        let mut manager = self.savepoint_manager.write();
        manager
            .remove_savepoint(id)
            .map(|_| ())
            .ok_or(TransactionError::savepoint_not_found(id))
    }

    /// Rollback to savepoint
    pub fn rollback_to_savepoint<T: UndoTarget + ?Sized>(
        &self,
        id: SavepointId,
        target: &T,
    ) -> Result<(), TransactionError> {
        let state = self.state.load();
        if !state.can_execute() {
            return Err(TransactionError::invalid_state_for_abort(state));
        }

        if self.is_expired() {
            return Err(TransactionError::transaction_expired());
        }

        let savepoint_info = {
            let manager = self.savepoint_manager.read();
            manager
                .get_savepoint(id)
                .cloned()
                .ok_or(TransactionError::savepoint_not_found(id))?
        };

        // Use CombinedRollback for comprehensive savepoint rollback
        let rollback = CombinedRollback::new(self);
        rollback
            .rollback_operation_log_to_index(savepoint_info.operation_log_index)
            .map_err(|e| TransactionError::rollback_failed(e.to_string()))?;

        {
            let mut manager = self.savepoint_manager.write();
            // Delete savepoints created AFTER the target savepoint using
            // explicit sequence number (not ID). This ensures stable ordering
            // even if IDs are not assigned in strict creation order.
            let target_sequence = savepoint_info.sequence;
            let savepoints_to_remove: Vec<SavepointId> = manager
                .savepoints
                .iter()
                .filter(|(_, sp)| sp.sequence > target_sequence)
                .map(|(&id, _)| id)
                .collect();

            for sp_id in savepoints_to_remove {
                manager.remove_savepoint(sp_id);
            }
        }

        rollback
            .execute_undo_rollback_from_index(
                target,
                self.start_timestamp,
                savepoint_info.undo_log_index,
            )
            .map_err(|e| TransactionError::rollback_failed(e.to_string()))?;

        Ok(())
    }

    /// Add undo log
    pub fn add_undo_log(&self, log: UndoLogEntry) {
        let mut undo_logs = self.undo_logs.write();
        undo_logs.add(log);
    }

    /// Get undo log length
    pub fn undo_log_len(&self) -> usize {
        let undo_logs = self.undo_logs.read();
        undo_logs.len()
    }

    /// Clear undo logs
    pub fn clear_undo_logs(&self) {
        let mut undo_logs = self.undo_logs.write();
        undo_logs.clear();
    }

    /// Execute undo logs for rollback
    pub fn execute_undo_logs<T: UndoTarget + ?Sized>(
        &self,
        target: &T,
    ) -> Result<(), TransactionError> {
        let mut undo_logs = self.undo_logs.write();
        undo_logs
            .execute_undo(target, self.start_timestamp)
            .map_err(|e| TransactionError::rollback_failed(e.to_string()))
    }

    /// Execute undo logs starting from a specific index.
    pub fn execute_undo_logs_from_index<T: UndoTarget + ?Sized>(
        &self,
        target: &T,
        start_index: usize,
    ) -> Result<(), TransactionError> {
        let mut undo_logs = self.undo_logs.write();
        undo_logs
            .execute_undo_from_index(target, self.start_timestamp, start_index)
            .map_err(|e| TransactionError::rollback_failed(e.to_string()))
    }

    /// Clear all state
    pub fn clear(&self) {
        self.clear_operation_log();
        {
            let mut tables = self.modified_tables.lock();
            tables.clear();
        }
        {
            let mut manager = self.savepoint_manager.write();
            manager.clear();
        }
        {
            let mut undo_logs = self.undo_logs.write();
            undo_logs.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_transaction_context_basic() {
        let config = TransactionConfig::default();
        let ctx = TransactionContext::new(TransactionId(1), 1, config);

        assert_eq!(ctx.id, TransactionId(1));
        assert_eq!(ctx.timestamp(), 1);
        assert_eq!(ctx.state(), TransactionState::Active);
        assert!(!ctx.read_only);
    }

    #[test]
    fn test_transaction_context_readonly() {
        let config = TransactionConfig::default();
        let ctx = TransactionContext::new_readonly(TransactionId(1), 1, config);

        assert!(ctx.read_only);
    }

    #[test]
    fn test_transaction_context_state_transition() {
        let config = TransactionConfig::default();
        let ctx = TransactionContext::new(TransactionId(1), 1, config);

        assert!(ctx.transition_to(TransactionState::Committing).is_ok());
        assert_eq!(ctx.state(), TransactionState::Committing);
        assert!(ctx.transition_to(TransactionState::Committed).is_ok());
        assert_eq!(ctx.state(), TransactionState::Committed);
    }

    #[test]
    fn test_transaction_context_savepoint() {
        let config = TransactionConfig::default();
        let ctx = TransactionContext::new(TransactionId(1), 1, config);

        let sp_id = ctx.create_savepoint(Some("test".to_string()), 0);
        assert!(ctx.get_savepoint(sp_id).is_some());

        let sp = ctx.get_savepoint(sp_id).unwrap();
        assert_eq!(sp.name, Some("test".to_string()));
    }

    #[test]
    fn test_transaction_context_operation_log() {
        let config = TransactionConfig::default();
        let ctx = TransactionContext::new(TransactionId(1), 1, config);

        ctx.add_operation_log(OperationLog::InsertVertex {
            space: "test".to_string(),
            vertex_id: vec![1, 2, 3],
            previous_state: None,
        });

        assert_eq!(ctx.operation_log_len(), 1);
        assert!(ctx.get_operation_log(0).is_some());
    }
}
