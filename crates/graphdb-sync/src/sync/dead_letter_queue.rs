//! Dead Letter Queue for failed index operations
//!
//! Stores operations that failed after all retry attempts for later analysis and recovery.

use std::time::{Duration, SystemTime};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::sync::types::{IndexOperation};

/// Dead letter queue entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeadLetterEntry {
    /// The failed operation
    pub operation: IndexOperation,
    /// Error message
    pub error: String,
    /// Number of retry attempts
    pub retry_attempts: u32,
    /// First failure timestamp
    pub first_failure: SystemTime,
    /// Last failure timestamp
    pub last_failure: SystemTime,
    /// Whether this entry has been processed for recovery
    pub recovered: bool,
}

impl DeadLetterEntry {
    pub fn new(operation: IndexOperation, error: String, retry_attempts: u32) -> Self {
        let now = SystemTime::now();
        Self {
            operation,
            error,
            retry_attempts,
            first_failure: now,
            last_failure: now,
            recovered: false,
        }
    }

    pub fn update_failure(&mut self, error: String) {
        self.error = error;
        self.last_failure = SystemTime::now();
    }

    pub fn age(&self) -> Duration {
        self.first_failure
            .elapsed()
            .unwrap_or(Duration::from_secs(0))
    }

    pub fn mark_recovered(&mut self) {
        self.recovered = true;
    }
}

/// Dead letter queue configuration
#[derive(Debug, Clone)]
pub struct DeadLetterQueueConfig {
    /// Maximum number of entries in the queue
    pub max_size: usize,
    /// Maximum age of entries before automatic cleanup
    pub max_age: Duration,
    /// Whether to enable automatic cleanup
    pub auto_cleanup_enabled: bool,
}

impl DeadLetterQueueConfig {
    pub fn is_auto_cleanup_enabled(&self) -> bool {
        self.auto_cleanup_enabled
    }

    pub fn get_cleanup_interval(&self) -> Duration {
        self.max_age / 2
    }
}

impl Default for DeadLetterQueueConfig {
    fn default() -> Self {
        Self {
            max_size: 10_000,
            max_age: Duration::from_secs(3600), // 1 hour
            auto_cleanup_enabled: true,
        }
    }
}

/// Dead Letter Queue
#[derive(Debug)]
pub struct DeadLetterQueue {
    entries: Mutex<Vec<DeadLetterEntry>>,
    config: DeadLetterQueueConfig,
}

impl DeadLetterQueue {
    pub fn new(config: DeadLetterQueueConfig) -> Self {
        Self {
            entries: Mutex::new(Vec::with_capacity(config.max_size)),
            config,
        }
    }

    pub fn is_auto_cleanup_enabled(&self) -> bool {
        self.config.auto_cleanup_enabled
    }

    pub fn get_cleanup_interval(&self) -> Duration {
        self.config.max_age / 2
    }

    /// Add an entry to the dead letter queue
    pub fn add(&self, entry: DeadLetterEntry) {
        let mut entries = self.entries.lock();

        // Check if queue is full
        if entries.len() >= self.config.max_size {
            // Remove oldest entry if full
            if !entries.is_empty() {
                entries.remove(0);
                log::warn!("Dead letter queue is full, removed oldest entry");
            }
        }

        entries.push(entry);

        log::warn!("Added entry to dead letter queue (size: {})", entries.len());
    }

    /// Get all entries
    pub fn get_all(&self) -> Vec<DeadLetterEntry> {
        self.entries.lock().clone()
    }

    /// Get entries that haven't been recovered
    pub fn get_unrecovered(&self) -> Vec<DeadLetterEntry> {
        self.entries
            .lock()
            .iter()
            .filter(|e| !e.recovered)
            .cloned()
            .collect()
    }

    /// Get entries older than specified duration
    pub fn get_old_entries(&self, age: Duration) -> Vec<DeadLetterEntry> {
        self.entries
            .lock()
            .iter()
            .filter(|e| e.age() > age)
            .cloned()
            .collect()
    }

    /// Remove an entry by index
    pub fn remove(&self, index: usize) -> Option<DeadLetterEntry> {
        let mut entries = self.entries.lock();
        if index < entries.len() {
            let entry = entries.remove(index);
            Some(entry)
        } else {
            None
        }
    }

    /// Mark an entry as recovered
    pub fn mark_recovered(&self, index: usize) -> bool {
        let mut entries = self.entries.lock();
        if index < entries.len() {
            entries[index].mark_recovered();
            true
        } else {
            false
        }
    }

    /// Cleanup old entries
    pub fn cleanup(&self) -> usize {
        let mut entries = self.entries.lock();
        let initial_len = entries.len();

        entries.retain(|e| e.age() <= self.config.max_age);

        let removed = initial_len - entries.len();

        if removed > 0 {
            log::info!("Cleaned up {} old dead letter entries", removed);
        }

        removed
    }

    /// Get queue size
    pub fn len(&self) -> usize {
        self.entries.lock().len()
    }

    /// Check if queue is empty
    pub fn is_empty(&self) -> bool {
        self.entries.lock().is_empty()
    }

    /// Clear all entries
    pub fn clear(&self) {
        let mut entries = self.entries.lock();
        let count = entries.len();
        entries.clear();

        log::info!("Cleared {} entries from dead letter queue", count);
    }

    /// Get statistics about the dead letter queue
    pub fn get_stats(&self) -> DeadLetterQueueStats {
        let entries = self.entries.lock();
        let total = entries.len();
        let unrecovered = entries.iter().filter(|e| !e.recovered).count();
        let recovered = total - unrecovered;

        DeadLetterQueueStats {
            total_entries: total,
            unrecovered_entries: unrecovered,
            recovered_entries: recovered,
            oldest_entry_age: entries
                .iter()
                .map(|e| e.age())
                .max()
                .unwrap_or(Duration::from_secs(0)),
        }
    }
}

/// Statistics for dead letter queue
#[derive(Debug, Clone)]
pub struct DeadLetterQueueStats {
    pub total_entries: usize,
    pub unrecovered_entries: usize,
    pub recovered_entries: usize,
    pub oldest_entry_age: Duration,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::types::{ChangeType, IndexOpKey, IndexOperation};

    fn create_test_index_key() -> IndexOpKey {
        IndexOpKey {
            space_id: 1,
            tag_name: "Article".to_string(),
            field_name: "content".to_string(),
        }
    }

    fn create_test_operation() -> IndexOperation {
        IndexOperation::new_fulltext(
            IndexOpKey::new(1, "test_tag", "test_field"),
            ChangeType::Insert,
            "test_id",
            Some("test".to_string()),
        )
    }

    fn create_test_insert_operation(id: &str, text: &str) -> IndexOperation {
        IndexOperation::new_fulltext(
            create_test_index_key(),
            ChangeType::Insert,
            id,
            Some(text.to_string()),
        )
    }

    fn create_test_delete_operation(id: &str) -> IndexOperation {
        IndexOperation::new_fulltext(create_test_index_key(), ChangeType::Delete, id, None)
    }

    fn create_test_entry(error: &str, retry_attempts: u32) -> DeadLetterEntry {
        DeadLetterEntry::new(
            create_test_insert_operation("test_doc", "test content"),
            error.to_string(),
            retry_attempts,
        )
    }

    #[test]
    fn test_dead_letter_queue_creation() {
        let config = DeadLetterQueueConfig::default();
        let dlq = DeadLetterQueue::new(config);

        assert!(dlq.is_empty(), "New DLQ should be empty");
        assert_eq!(dlq.len(), 0, "New DLQ should have 0 entries");
    }

    #[test]
    fn test_dead_letter_queue_add() {
        let config = DeadLetterQueueConfig::default();
        let dlq = DeadLetterQueue::new(config);

        let entry = DeadLetterEntry::new(create_test_operation(), "test error".to_string(), 3);

        dlq.add(entry);
        assert_eq!(dlq.len(), 1);
    }

    #[test]
    fn test_add_entry_to_dlq() {
        let config = DeadLetterQueueConfig::default();
        let dlq = DeadLetterQueue::new(config);

        let entry = create_test_entry("Connection failed", 3);
        dlq.add(entry);

        assert!(
            !dlq.is_empty(),
            "DLQ should not be empty after adding entry"
        );
        assert_eq!(dlq.len(), 1, "DLQ should have 1 entry");
    }

    #[test]
    fn test_multiple_entries_in_dlq() {
        let config = DeadLetterQueueConfig::default();
        let dlq = DeadLetterQueue::new(config);

        for i in 0..5 {
            let entry = DeadLetterEntry::new(
                create_test_insert_operation(&format!("doc_{}", i), &format!("content {}", i)),
                format!("Error {}", i),
                3,
            );
            dlq.add(entry);
        }

        assert_eq!(dlq.len(), 5, "DLQ should have 5 entries");

        let all_entries = dlq.get_all();
        assert_eq!(all_entries.len(), 5, "Should retrieve all 5 entries");
    }

    #[test]
    fn test_dead_letter_queue_max_size() {
        let config = DeadLetterQueueConfig {
            max_size: 3,
            ..DeadLetterQueueConfig::default()
        };
        let dlq = DeadLetterQueue::new(config);

        for i in 0..5 {
            let entry = DeadLetterEntry::new(create_test_operation(), format!("error {}", i), 3);
            dlq.add(entry);
        }

        assert_eq!(dlq.len(), 3);
    }

    #[test]
    fn test_dlq_max_size_limit() {
        let config = DeadLetterQueueConfig {
            max_size: 3,
            ..DeadLetterQueueConfig::default()
        };
        let dlq = DeadLetterQueue::new(config);

        for i in 0..10 {
            let entry = create_test_entry(&format!("Error {}", i), 3);
            dlq.add(entry);
        }

        assert_eq!(dlq.len(), 3, "DLQ should respect max_size limit");
    }

    #[test]
    fn test_get_unrecovered_entries() {
        let config = DeadLetterQueueConfig::default();
        let dlq = DeadLetterQueue::new(config);

        for i in 0..3 {
            let entry = create_test_entry(&format!("Error {}", i), 3);
            dlq.add(entry);
        }

        let unrecovered = dlq.get_unrecovered();
        assert_eq!(
            unrecovered.len(),
            3,
            "All entries should be unrecovered initially"
        );

        dlq.mark_recovered(1);

        let unrecovered_after = dlq.get_unrecovered();
        assert_eq!(
            unrecovered_after.len(),
            2,
            "Should have 2 unrecovered after marking one as recovered"
        );
    }

    #[test]
    fn test_mark_entry_recovered() {
        let config = DeadLetterQueueConfig::default();
        let dlq = DeadLetterQueue::new(config);

        let entry = create_test_entry("Test error", 3);
        dlq.add(entry);

        let result = dlq.mark_recovered(0);
        assert!(result, "mark_recovered should return true for valid index");

        let all_entries = dlq.get_all();
        assert!(
            all_entries[0].recovered,
            "Entry should be marked as recovered"
        );
    }

    #[test]
    fn test_dead_letter_queue_recovery() {
        let config = DeadLetterQueueConfig::default();
        let dlq = DeadLetterQueue::new(config);

        let entry = DeadLetterEntry::new(create_test_operation(), "test error".to_string(), 3);
        dlq.add(entry);

        let unrecovered = dlq.get_unrecovered();
        assert_eq!(unrecovered.len(), 1);
        assert!(!unrecovered[0].recovered);

        dlq.mark_recovered(0);

        let unrecovered = dlq.get_unrecovered();
        assert_eq!(unrecovered.len(), 0);
    }

    #[test]
    fn test_remove_entry_from_dlq() {
        let config = DeadLetterQueueConfig::default();
        let dlq = DeadLetterQueue::new(config);

        for i in 0..3 {
            let entry = create_test_entry(&format!("Error {}", i), 3);
            dlq.add(entry);
        }

        let removed = dlq.remove(1);
        assert!(removed.is_some(), "remove should return the removed entry");
        assert_eq!(dlq.len(), 2, "DLQ should have 2 entries after removal");

        let invalid_remove = dlq.remove(10);
        assert!(
            invalid_remove.is_none(),
            "remove should return None for invalid index"
        );
    }

    #[test]
    fn test_dead_letter_queue_cleanup() {
        let config = DeadLetterQueueConfig {
            max_age: Duration::from_millis(100),
            ..DeadLetterQueueConfig::default()
        };
        let dlq = DeadLetterQueue::new(config);

        let entry = DeadLetterEntry::new(create_test_operation(), "test error".to_string(), 3);
        dlq.add(entry);

        // Wait for entry to become old
        std::thread::sleep(Duration::from_millis(150));

        let removed = dlq.cleanup();
        assert_eq!(removed, 1);
        assert_eq!(dlq.len(), 0);
    }

    #[test]
    fn test_dlq_cleanup() {
        let config = DeadLetterQueueConfig {
            max_age: Duration::from_millis(50),
            ..DeadLetterQueueConfig::default()
        };
        let dlq = DeadLetterQueue::new(config);

        let entry = create_test_entry("Test error", 3);
        dlq.add(entry);

        assert_eq!(dlq.len(), 1, "Should have 1 entry before cleanup");

        std::thread::sleep(Duration::from_millis(100));

        let removed = dlq.cleanup();
        assert_eq!(removed, 1, "Should remove 1 old entry");
        assert_eq!(dlq.len(), 0, "Should have 0 entries after cleanup");
    }

    #[test]
    fn test_dlq_statistics() {
        let config = DeadLetterQueueConfig::default();
        let dlq = DeadLetterQueue::new(config);

        for i in 0..5 {
            let entry = create_test_entry(&format!("Error {}", i), 3);
            dlq.add(entry);
        }

        dlq.mark_recovered(0);
        dlq.mark_recovered(2);

        let stats: DeadLetterQueueStats = dlq.get_stats();

        assert_eq!(stats.total_entries, 5, "Should have 5 total entries");
        assert_eq!(
            stats.recovered_entries, 2,
            "Should have 2 recovered entries"
        );
        assert_eq!(
            stats.unrecovered_entries, 3,
            "Should have 3 unrecovered entries"
        );
    }

    #[test]
    fn test_clear_dlq() {
        let config = DeadLetterQueueConfig::default();
        let dlq = DeadLetterQueue::new(config);

        for i in 0..5 {
            let entry = create_test_entry(&format!("Error {}", i), 3);
            dlq.add(entry);
        }

        assert_eq!(dlq.len(), 5, "Should have 5 entries before clear");

        dlq.clear();

        assert!(dlq.is_empty(), "DLQ should be empty after clear");
        assert_eq!(dlq.len(), 0, "DLQ should have 0 entries after clear");
    }

    #[test]
    fn test_entry_age_calculation() {
        let entry = create_test_entry("Test error", 3);

        std::thread::sleep(Duration::from_millis(100));

        let age = entry.age();
        assert!(
            age >= Duration::from_millis(100),
            "Entry age should be at least 100ms"
        );
    }

    #[test]
    fn test_entry_update_failure() {
        let mut entry = create_test_entry("Original error", 3);

        entry.update_failure("Updated error message".to_string());

        assert_eq!(
            entry.error, "Updated error message",
            "Error message should be updated"
        );
        assert!(
            entry.last_failure > entry.first_failure,
            "Last failure time should be updated"
        );
    }

    #[test]
    fn test_get_old_entries() {
        let config = DeadLetterQueueConfig::default();
        let dlq = DeadLetterQueue::new(config);

        for i in 0..3 {
            let entry = create_test_entry(&format!("Error {}", i), 3);
            dlq.add(entry);
        }

        std::thread::sleep(Duration::from_millis(100));

        let old_entries = dlq.get_old_entries(Duration::from_millis(50));
        assert_eq!(old_entries.len(), 3, "All entries should be old enough");
    }

    #[test]
    fn test_different_operation_types() {
        let config = DeadLetterQueueConfig::default();
        let dlq = DeadLetterQueue::new(config);

        let insert_entry = DeadLetterEntry::new(
            create_test_insert_operation("doc_1", "insert content"),
            "Insert failed".to_string(),
            3,
        );
        dlq.add(insert_entry);

        let delete_entry = DeadLetterEntry::new(
            create_test_delete_operation("doc_2"),
            "Delete failed".to_string(),
            2,
        );
        dlq.add(delete_entry);

        assert_eq!(
            dlq.len(),
            2,
            "Should have 2 entries with different operation types"
        );

        let all_entries = dlq.get_all();
        assert!(
            matches!(all_entries[0].operation.change_type, ChangeType::Insert),
            "First entry should be Insert operation"
        );
        assert!(
            matches!(all_entries[1].operation.change_type, ChangeType::Delete),
            "Second entry should be Delete operation"
        );
    }
}
