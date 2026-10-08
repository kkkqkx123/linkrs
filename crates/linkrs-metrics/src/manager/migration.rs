//! Data migration observability.
use std::collections::HashMap;

use super::core::StatsManager;
use super::metric_type::MetricType;

impl StatsManager {
    pub fn record_migration_start(&self) {
        self.add_value(MetricType::MigrationTotalCount);
        self.add_value(MetricType::MigrationActiveCount);
    }

    pub fn record_migration_success(&self, rows: u64, duration_ms: u64) {
        self.dec_value(MetricType::MigrationActiveCount);
        self.add_value_with_amount(MetricType::MigrationRowsMigrated, rows);
        self.add_value_with_amount(MetricType::MigrationDurationMs, duration_ms);
    }

    pub fn record_migration_failure(&self, duration_ms: u64) {
        self.dec_value(MetricType::MigrationActiveCount);
        self.add_value(MetricType::MigrationErrorsTotal);
        self.add_value_with_amount(MetricType::MigrationDurationMs, duration_ms);
    }

    pub fn get_migration_metrics(&self) -> HashMap<MetricType, u64> {
        let mut out = HashMap::new();
        for mt in [
            MetricType::MigrationTotalCount,
            MetricType::MigrationRowsMigrated,
            MetricType::MigrationDurationMs,
            MetricType::MigrationErrorsTotal,
            MetricType::MigrationActiveCount,
        ] {
            if let Some(v) = self.get_value(mt) {
                out.insert(mt, v);
            }
        }
        out
    }
}
