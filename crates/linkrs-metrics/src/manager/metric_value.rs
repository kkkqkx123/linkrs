//! Atomic counter primitive with last-update timestamp.

use dashmap::DashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use super::metric_type::MetricType;

/// Space metrics type alias
pub(crate) type SpaceMetrics = Arc<DashMap<MetricType, Arc<MetricValue>>>;

/// metric
#[derive(Debug)]
pub struct MetricValue {
    pub value: AtomicU64,
    pub timestamp: AtomicU64,
}

impl MetricValue {
    pub fn new(value: u64) -> Self {
        let timestamp_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        Self {
            value: AtomicU64::new(value),
            timestamp: AtomicU64::new(timestamp_secs),
        }
    }

    pub fn increment(&self) {
        self.value.fetch_add(1, Ordering::Relaxed);
        self.update_timestamp();
    }

    pub fn add(&self, amount: u64) {
        self.value.fetch_add(amount, Ordering::Relaxed);
        self.update_timestamp();
    }

    pub fn decrement(&self) {
        let _ = self
            .value
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |v| {
                if v > 0 {
                    Some(v - 1)
                } else {
                    Some(0)
                }
            });
        self.update_timestamp();
    }

    pub fn set(&self, value: u64) {
        self.value.store(value, Ordering::Relaxed);
        self.update_timestamp();
    }

    pub fn get(&self) -> u64 {
        self.value.load(Ordering::Relaxed)
    }

    pub fn get_timestamp(&self) -> u64 {
        self.timestamp.load(Ordering::Relaxed)
    }

    fn update_timestamp(&self) {
        let timestamp_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        self.timestamp.store(timestamp_secs, Ordering::Relaxed);
    }
}
