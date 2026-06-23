//! Transaction Monitor
//!
//! Provides monitoring and metrics collection for transactions

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use dashmap::DashMap;

use crate::transaction::context::TransactionContext;
use crate::transaction::types::{
    TransactionId, TransactionInfo, TransactionMetrics, TransactionStats,
};

/// Transaction Monitor
///
/// Responsible for collecting and reporting transaction metrics and statistics.
pub struct TransactionMonitor {
    stats: Arc<TransactionStats>,
}

impl TransactionMonitor {
    pub fn new(stats: Arc<TransactionStats>) -> Self {
        Self { stats }
    }

    pub fn stats(&self) -> &TransactionStats {
        &self.stats
    }

    /// Get transaction metrics
    ///
    /// # Arguments
    /// * `active_transactions` - Reference to the active transactions map
    ///
    /// # Returns
    /// * `TransactionMetrics` - Transaction metrics
    pub fn get_metrics(
        &self,
        active_transactions: &DashMap<TransactionId, Arc<TransactionContext>>,
    ) -> TransactionMetrics {
        let mut metrics = TransactionMetrics::new();

        let durations: Vec<Duration> = active_transactions
            .iter()
            .map(|entry| entry.value().start_time.elapsed())
            .collect();

        if durations.is_empty() {
            return metrics;
        }

        let mut sorted_durations = durations.clone();
        sorted_durations.sort();

        metrics.p50_duration = sorted_durations[sorted_durations.len() * 50 / 100];
        metrics.p95_duration = sorted_durations[sorted_durations.len() * 95 / 100];
        metrics.p99_duration = sorted_durations[sorted_durations.len() * 99 / 100];

        let total: Duration = durations.iter().sum();
        metrics.avg_duration = total / durations.len() as u32;

        metrics.long_transactions = active_transactions
            .iter()
            .filter(|entry| entry.value().start_time.elapsed() > Duration::from_secs(10))
            .map(|entry| entry.value().info())
            .collect();

        metrics.total_count = self.stats.total_transactions.load(Ordering::Relaxed);

        metrics
    }

    /// Get all active transactions info
    ///
    /// # Arguments
    /// * `active_transactions` - Reference to the active transactions map
    ///
    /// # Returns
    /// * `Vec<TransactionInfo>` - Active transactions info
    pub fn get_active_transactions(
        &self,
        active_transactions: &DashMap<TransactionId, Arc<TransactionContext>>,
    ) -> Vec<TransactionInfo> {
        active_transactions
            .iter()
            .map(|entry| entry.value().info())
            .collect()
    }

    /// Get long transactions (duration > 10s)
    ///
    /// # Arguments
    /// * `active_transactions` - Reference to the active transactions map
    ///
    /// # Returns
    /// * `Vec<TransactionInfo>` - Long transactions info
    pub fn get_long_transactions(
        &self,
        active_transactions: &DashMap<TransactionId, Arc<TransactionContext>>,
    ) -> Vec<TransactionInfo> {
        active_transactions
            .iter()
            .filter(|entry| entry.value().start_time.elapsed() > Duration::from_secs(10))
            .map(|entry| entry.value().info())
            .collect()
    }

    /// Get transaction info
    ///
    /// # Arguments
    /// * `active_transactions` - Reference to the active transactions map
    /// * `txn_id` - Transaction ID
    ///
    /// # Returns
    /// * `Some(TransactionInfo)` - If transaction exists
    /// * `None` - If transaction does not exist
    pub fn get_transaction_info(
        &self,
        active_transactions: &DashMap<TransactionId, Arc<TransactionContext>>,
        txn_id: TransactionId,
    ) -> Option<TransactionInfo> {
        active_transactions
            .get(&txn_id)
            .map(|entry| entry.value().info())
    }

    /// List active transactions
    ///
    /// # Arguments
    /// * `active_transactions` - Reference to the active transactions map
    ///
    /// # Returns
    /// * `Vec<TransactionInfo>` - Active transactions info
    pub fn list_active_transactions(
        &self,
        active_transactions: &DashMap<TransactionId, Arc<TransactionContext>>,
    ) -> Vec<TransactionInfo> {
        active_transactions
            .iter()
            .map(|entry| entry.value().info())
            .collect()
    }
}

impl Default for TransactionMonitor {
    fn default() -> Self {
        Self::new(Arc::new(TransactionStats::new()))
    }
}
