use std::sync::atomic::{AtomicU64, Ordering};

pub struct MigrationMetrics {
    pub total_migrations: AtomicU64,
    pub successful_migrations: AtomicU64,
    pub failed_migrations: AtomicU64,
    pub total_rows_migrated: AtomicU64,
    pub total_duration_ms: AtomicU64,
}

impl MigrationMetrics {
    pub fn new() -> Self {
        Self {
            total_migrations: AtomicU64::new(0),
            successful_migrations: AtomicU64::new(0),
            failed_migrations: AtomicU64::new(0),
            total_rows_migrated: AtomicU64::new(0),
            total_duration_ms: AtomicU64::new(0),
        }
    }

    pub fn record_success(&self, rows: u64, duration_ms: u64) {
        self.total_migrations.fetch_add(1, Ordering::Relaxed);
        self.successful_migrations.fetch_add(1, Ordering::Relaxed);
        self.total_rows_migrated.fetch_add(rows, Ordering::Relaxed);
        self.total_duration_ms.fetch_add(duration_ms, Ordering::Relaxed);
    }

    pub fn record_failure(&self, duration_ms: u64) {
        self.total_migrations.fetch_add(1, Ordering::Relaxed);
        self.failed_migrations.fetch_add(1, Ordering::Relaxed);
        self.total_duration_ms.fetch_add(duration_ms, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> MigrationMetricsSnapshot {
        MigrationMetricsSnapshot {
            total_migrations: self.total_migrations.load(Ordering::Relaxed),
            successful_migrations: self.successful_migrations.load(Ordering::Relaxed),
            failed_migrations: self.failed_migrations.load(Ordering::Relaxed),
            total_rows_migrated: self.total_rows_migrated.load(Ordering::Relaxed),
            total_duration_ms: self.total_duration_ms.load(Ordering::Relaxed),
        }
    }

    pub fn reset(&self) {
        self.total_migrations.store(0, Ordering::Relaxed);
        self.successful_migrations.store(0, Ordering::Relaxed);
        self.failed_migrations.store(0, Ordering::Relaxed);
        self.total_rows_migrated.store(0, Ordering::Relaxed);
        self.total_duration_ms.store(0, Ordering::Relaxed);
    }
}

impl Default for MigrationMetrics {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationMetricsSnapshot {
    pub total_migrations: u64,
    pub successful_migrations: u64,
    pub failed_migrations: u64,
    pub total_rows_migrated: u64,
    pub total_duration_ms: u64,
}

static GLOBAL_MIGRATION_METRICS: std::sync::OnceLock<MigrationMetrics> = std::sync::OnceLock::new();

pub fn global_migration_metrics() -> &'static MigrationMetrics {
    GLOBAL_MIGRATION_METRICS.get_or_init(MigrationMetrics::new)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metrics_record_success() {
        let m = MigrationMetrics::new();
        m.record_success(10, 100);
        m.record_success(5, 50);
        let snap = m.snapshot();
        assert_eq!(snap.total_migrations, 2);
        assert_eq!(snap.successful_migrations, 2);
        assert_eq!(snap.total_rows_migrated, 15);
        assert_eq!(snap.total_duration_ms, 150);
    }

    #[test]
    fn test_metrics_record_failure() {
        let m = MigrationMetrics::new();
        m.record_success(10, 100);
        m.record_failure(20);
        let snap = m.snapshot();
        assert_eq!(snap.total_migrations, 2);
        assert_eq!(snap.failed_migrations, 1);
        assert_eq!(snap.successful_migrations, 1);
    }

    #[test]
    fn test_metrics_reset() {
        let m = MigrationMetrics::new();
        m.record_success(10, 100);
        m.reset();
        let snap = m.snapshot();
        assert_eq!(snap.total_migrations, 0);
    }
}
