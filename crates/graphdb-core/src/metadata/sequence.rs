use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicI64, Ordering};

use crate::StorageError;

/// Sequence definition
///
/// An auto-incrementing counter object with configurable bounds and cycle behavior.
/// The `current_value` field uses atomic operations for thread-safe access.
#[derive(Debug, Serialize, Deserialize)]
pub struct SequenceDef {
    /// Sequence name
    pub name: String,
    /// Current value (skipped in serialization, managed via atomic)
    #[serde(skip)]
    current_value: AtomicI64,
    /// Increment step
    pub increment: i64,
    /// Minimum value
    pub min_value: i64,
    /// Maximum value
    pub max_value: i64,
    /// Whether to cycle when hitting bounds
    pub cycle: bool,
    /// Initial value (used for persistence and reset)
    pub start_value: i64,
}

impl Clone for SequenceDef {
    fn clone(&self) -> Self {
        Self {
            name: self.name.clone(),
            current_value: AtomicI64::new(self.current_value.load(Ordering::SeqCst)),
            increment: self.increment,
            min_value: self.min_value,
            max_value: self.max_value,
            cycle: self.cycle,
            start_value: self.start_value,
        }
    }
}

impl SequenceDef {
    pub fn new(
        name: String,
        start: i64,
        increment: i64,
        min_value: i64,
        max_value: i64,
        cycle: bool,
    ) -> Self {
        Self {
            name,
            current_value: AtomicI64::new(start),
            increment,
            min_value,
            max_value,
            cycle,
            start_value: start,
        }
    }

    /// Get the current value without incrementing
    pub fn current_value(&self) -> i64 {
        self.current_value.load(Ordering::SeqCst)
    }

    /// Get the next value with atomic increment.
    ///
    /// Uses a compare-exchange retry loop so concurrent callers never lose
    /// increments. Returns `Err` if the value would exceed bounds and
    /// `cycle` is false.
    pub fn next_value(&self) -> Result<i64, StorageError> {
        loop {
            let current = self.current_value.load(Ordering::SeqCst);
            let next = current + self.increment;

            let new_value = if next > self.max_value {
                if self.cycle {
                    self.min_value
                } else {
                    return Err(StorageError::db_error(format!(
                        "Sequence '{}' overflow: value {} would exceed max_value {}",
                        self.name, next, self.max_value
                    )));
                }
            } else if next < self.min_value {
                if self.cycle {
                    self.max_value
                } else {
                    return Err(StorageError::db_error(format!(
                        "Sequence '{}' underflow: value {} would go below min_value {}",
                        self.name, next, self.min_value
                    )));
                }
            } else {
                next
            };

            match self.current_value.compare_exchange(
                current,
                new_value,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => return Ok(new_value),
                // Lost the race to another increment; retry with fresh state.
                Err(_) => continue,
            }
        }
    }

    /// Reset to the initial start value
    pub fn reset(&self) {
        self.current_value.store(self.start_value, Ordering::SeqCst);
    }

    /// Set the current value directly
    pub fn set_value(&self, value: i64) {
        self.current_value.store(value, Ordering::SeqCst);
    }
}

impl Default for SequenceDef {
    fn default() -> Self {
        Self::new(String::new(), 1, 1, i64::MIN, i64::MAX, false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sequence_basic() {
        let seq = SequenceDef::new("test".to_string(), 1, 1, 1, 100, false);
        assert_eq!(seq.current_value(), 1);
        assert_eq!(seq.next_value().unwrap(), 2);
        assert_eq!(seq.next_value().unwrap(), 3);
        assert_eq!(seq.current_value(), 3);
    }

    #[test]
    fn test_sequence_increment() {
        let seq = SequenceDef::new("test".to_string(), 10, 5, 1, 1000, false);
        assert_eq!(seq.next_value().unwrap(), 15);
        assert_eq!(seq.next_value().unwrap(), 20);
    }

    #[test]
    fn test_sequence_negative_increment() {
        let seq = SequenceDef::new("test".to_string(), 100, -10, 1, 1000, false);
        assert_eq!(seq.next_value().unwrap(), 90);
        assert_eq!(seq.next_value().unwrap(), 80);
    }

    #[test]
    fn test_sequence_cycle() {
        let seq = SequenceDef::new("test".to_string(), 1, 1, 1, 3, true);
        assert_eq!(seq.next_value().unwrap(), 2);
        assert_eq!(seq.next_value().unwrap(), 3);
        assert_eq!(seq.next_value().unwrap(), 1); // cycles back to min
    }

    #[test]
    fn test_sequence_overflow_no_cycle() {
        let seq = SequenceDef::new("test".to_string(), 1, 1, 1, 3, false);
        assert_eq!(seq.next_value().unwrap(), 2);
        assert_eq!(seq.next_value().unwrap(), 3);
        assert!(seq.next_value().is_err());
    }

    #[test]
    fn test_sequence_underflow_no_cycle() {
        let seq = SequenceDef::new("test".to_string(), 100, -50, 10, 200, false);
        assert_eq!(seq.next_value().unwrap(), 50);
        assert!(seq.next_value().is_err());
    }

    #[test]
    fn test_sequence_reset() {
        let seq = SequenceDef::new("test".to_string(), 1, 1, 1, 100, false);
        seq.next_value().unwrap();
        seq.next_value().unwrap();
        assert_eq!(seq.current_value(), 3);
        seq.reset();
        assert_eq!(seq.current_value(), 1);
    }

    #[test]
    fn test_sequence_set_value() {
        let seq = SequenceDef::new("test".to_string(), 1, 1, 1, 100, false);
        seq.set_value(50);
        assert_eq!(seq.current_value(), 50);
        assert_eq!(seq.next_value().unwrap(), 51);
    }

    #[test]
    fn test_sequence_concurrent_next_value() {
        use std::sync::Arc;
        use std::thread;

        let seq = Arc::new(SequenceDef::new("test".to_string(), 0, 1, 0, 10000, false));
        let mut handles = vec![];

        for _ in 0..10 {
            let seq = Arc::clone(&seq);
            handles.push(thread::spawn(move || {
                for _ in 0..1000 {
                    seq.next_value().unwrap();
                }
            }));
        }

        for h in handles {
            h.join().unwrap();
        }

        assert_eq!(seq.current_value(), 10000);
    }
}
