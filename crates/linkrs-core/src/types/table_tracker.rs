//! Table Modification Tracker
//!
//! Tracks modified tables for efficient checkpointing and incremental flushing.
//! This is a simplified replacement for the page-based dirty tracking system.

use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use parking_lot::RwLock;

/// Table type identifier
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TableType {
    Vertex = 1,
    Edge = 2,
    Property = 3,
    Schema = 4,
}

impl TableType {
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            1 => Some(TableType::Vertex),
            2 => Some(TableType::Edge),
            3 => Some(TableType::Property),
            4 => Some(TableType::Schema),
            _ => None,
        }
    }

    pub fn as_u8(&self) -> u8 {
        *self as u8
    }
}

/// Table identifier
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TableId {
    /// Table type (Vertex, Edge, Schema)
    pub table_type: TableType,
    /// Label ID for the table
    pub label_id: u32,
}

impl TableId {
    pub fn new(table_type: TableType, label_id: u32) -> Self {
        Self {
            table_type,
            label_id,
        }
    }

    pub fn vertex(label_id: u32) -> Self {
        Self::new(TableType::Vertex, label_id)
    }

    pub fn edge(label_id: u32) -> Self {
        Self::new(TableType::Edge, label_id)
    }

    pub fn property(label_id: u32) -> Self {
        Self::new(TableType::Property, label_id)
    }

    pub fn schema() -> Self {
        Self::new(TableType::Schema, 0)
    }
}

/// Configuration for table tracker
#[derive(Debug, Clone)]
pub struct TableTrackerConfig {
    /// Number of modifications to trigger flush
    pub flush_threshold: usize,
    /// Time interval to trigger flush
    pub flush_interval: Duration,
}

impl Default for TableTrackerConfig {
    fn default() -> Self {
        Self {
            flush_threshold: 1000,
            flush_interval: Duration::from_secs(60),
        }
    }
}

/// Thread-safe table modification tracker
pub struct TableTracker {
    /// Set of modified table IDs
    modified_tables: RwLock<HashSet<TableId>>,
    /// Last flush timestamp
    last_flush: RwLock<Instant>,
    /// Flush threshold (number of modifications)
    flush_threshold: usize,
    /// Flush interval (time-based)
    flush_interval: Duration,
    /// Tables modified since last checkpoint
    tables_since_checkpoint: RwLock<HashSet<TableId>>,
    /// Modification counter
    modification_count: AtomicU64,
}

impl TableTracker {
    pub fn new(flush_threshold: usize, flush_interval: Duration) -> Self {
        Self {
            modified_tables: RwLock::new(HashSet::new()),
            last_flush: RwLock::new(Instant::now()),
            flush_threshold,
            flush_interval,
            tables_since_checkpoint: RwLock::new(HashSet::new()),
            modification_count: AtomicU64::new(0),
        }
    }

    pub fn with_config(config: TableTrackerConfig) -> Self {
        Self::new(config.flush_threshold, config.flush_interval)
    }

    /// Mark a table as modified
    pub fn mark_modified(&self, table_id: TableId) {
        self.modified_tables.write().insert(table_id);
        self.modification_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Mark multiple tables as modified
    pub fn mark_modified_batch(&self, table_ids: &[TableId]) {
        let mut modified = self.modified_tables.write();
        for table_id in table_ids {
            modified.insert(*table_id);
        }
        self.modification_count
            .fetch_add(table_ids.len() as u64, Ordering::Relaxed);
    }

    /// Mark a table as modified since checkpoint
    pub fn mark_modified_since_checkpoint(&self, table_id: TableId) {
        self.tables_since_checkpoint.write().insert(table_id);
    }

    /// Check if a table was modified since last checkpoint
    pub fn is_modified_since_checkpoint(&self, table_id: &TableId) -> bool {
        self.tables_since_checkpoint.read().contains(table_id)
    }

    /// Clear checkpoint tracking (call after checkpoint completes)
    pub fn clear_checkpoint_tracking(&self) {
        self.tables_since_checkpoint.write().clear();
    }

    /// Unmark a table as modified
    pub fn unmark_modified(&self, table_id: &TableId) {
        self.modified_tables.write().remove(table_id);
    }

    /// Check if a table is modified
    pub fn is_modified(&self, table_id: &TableId) -> bool {
        self.modified_tables.read().contains(table_id)
    }

    /// Check if flush should be triggered
    pub fn should_flush(&self) -> bool {
        let modified = self.modified_tables.read();
        let threshold_reached = modified.len() >= self.flush_threshold;
        drop(modified);

        let time_reached = self.last_flush.read().elapsed() >= self.flush_interval;

        threshold_reached || time_reached
    }

    /// Get all modified tables and reset the tracker
    pub fn flush_and_reset(&self) -> Vec<TableId> {
        let tables: Vec<TableId> = self.modified_tables.write().drain().collect();
        *self.last_flush.write() = Instant::now();
        tables
    }

    /// Get modified tables without resetting
    pub fn get_modified_tables(&self) -> Vec<TableId> {
        self.modified_tables.read().iter().copied().collect()
    }

    /// Get the number of modified tables
    pub fn get_modified_count(&self) -> usize {
        self.modified_tables.read().len()
    }

    /// Clear all modified tables
    pub fn clear(&self) {
        self.modified_tables.write().clear();
    }

    /// Update the last flush time
    pub fn update_flush_time(&self) {
        *self.last_flush.write() = Instant::now();
    }

    /// Get time since last flush
    pub fn time_since_last_flush(&self) -> Duration {
        self.last_flush.read().elapsed()
    }

    /// Get tables that need full writes (modified since checkpoint)
    pub fn get_tables_for_full_write(&self) -> Vec<TableId> {
        self.tables_since_checkpoint
            .read()
            .iter()
            .copied()
            .collect()
    }

    /// Get total modification count
    pub fn modification_count(&self) -> u64 {
        self.modification_count.load(Ordering::Relaxed)
    }
}

impl Default for TableTracker {
    fn default() -> Self {
        Self::with_config(TableTrackerConfig::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mark_modified() {
        let tracker = TableTracker::new(10, Duration::from_secs(60));
        let table_id = TableId::vertex(1);

        assert!(!tracker.is_modified(&table_id));

        tracker.mark_modified(table_id);
        assert!(tracker.is_modified(&table_id));
        assert_eq!(tracker.get_modified_count(), 1);
    }

    #[test]
    fn test_mark_modified_batch() {
        let tracker = TableTracker::new(10, Duration::from_secs(60));
        let tables = vec![TableId::vertex(1), TableId::vertex(2), TableId::edge(3)];

        tracker.mark_modified_batch(&tables);
        assert_eq!(tracker.get_modified_count(), 3);
    }

    #[test]
    fn test_unmark_modified() {
        let tracker = TableTracker::new(10, Duration::from_secs(60));
        let table_id = TableId::vertex(1);

        tracker.mark_modified(table_id);
        assert!(tracker.is_modified(&table_id));

        tracker.unmark_modified(&table_id);
        assert!(!tracker.is_modified(&table_id));
    }

    #[test]
    fn test_should_flush_threshold() {
        let tracker = TableTracker::new(3, Duration::from_secs(60));

        assert!(!tracker.should_flush());

        tracker.mark_modified(TableId::vertex(1));
        tracker.mark_modified(TableId::vertex(2));
        assert!(!tracker.should_flush());

        tracker.mark_modified(TableId::edge(3));
        assert!(tracker.should_flush());
    }

    #[test]
    fn test_flush_and_reset() {
        let tracker = TableTracker::new(10, Duration::from_secs(60));
        let tables = vec![TableId::vertex(1), TableId::edge(2)];

        tracker.mark_modified_batch(&tables);
        assert_eq!(tracker.get_modified_count(), 2);

        let flushed = tracker.flush_and_reset();
        assert_eq!(flushed.len(), 2);
        assert_eq!(tracker.get_modified_count(), 0);
    }

    #[test]
    fn test_checkpoint_tracking() {
        let tracker = TableTracker::new(10, Duration::from_secs(60));
        let table_id = TableId::vertex(1);

        tracker.mark_modified_since_checkpoint(table_id);
        assert!(tracker.is_modified_since_checkpoint(&table_id));

        tracker.clear_checkpoint_tracking();
        assert!(!tracker.is_modified_since_checkpoint(&table_id));
    }
}
