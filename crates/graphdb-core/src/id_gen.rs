//! ID Generator Module - Provides unique ID generation functionality
//!
//! Provides two ID generation strategies:
//! - IdGenerator: Sequential ID generation based on atomic counter, for session-level ID generation
//! - generate_id: Unique ID generation based on timestamp

use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// ID Generator based on atomic counter
///
/// Thread-safe sequential ID generator, suitable for scenarios requiring incrementing IDs
/// Used for session-level ID generation (e.g., execution plan ID), does not require global uniqueness
#[derive(Debug)]
pub struct IdGenerator {
    counter: AtomicI64,
}

impl IdGenerator {
    /// Create a new ID generator with specified initial value
    pub fn new(init: i64) -> Self {
        Self {
            counter: AtomicI64::new(init),
        }
    }

    /// Generate next ID
    pub fn id(&self) -> i64 {
        self.counter.fetch_add(1, Ordering::SeqCst)
    }

    /// Reset counter to specified value
    pub fn reset(&self, value: i64) {
        self.counter.store(value, Ordering::SeqCst);
    }

    /// Get current counter value
    pub fn current_value(&self) -> i64 {
        self.counter.load(Ordering::SeqCst)
    }
}

impl Clone for IdGenerator {
    fn clone(&self) -> Self {
        Self {
            counter: AtomicI64::new(self.current_value()),
        }
    }
}

impl Default for IdGenerator {
    fn default() -> Self {
        Self::new(0)
    }
}

/// Unique ID generation based on timestamp
///
/// Generates unique IDs using nanosecond timestamp, suitable for distributed scenarios or requiring global uniqueness
pub fn generate_id() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("Time went backwards")
        .as_nanos() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_id_generator() {
        let gen = IdGenerator::new(0);

        assert_eq!(gen.id(), 0);
        assert_eq!(gen.id(), 1);
        assert_eq!(gen.id(), 2);

        gen.reset(100);
        assert_eq!(gen.current_value(), 100);
        assert_eq!(gen.id(), 100);
    }

    #[test]
    fn test_generate_id() {
        let id1 = generate_id();
        let id2 = generate_id();

        assert_ne!(id1, id2);
    }
}
