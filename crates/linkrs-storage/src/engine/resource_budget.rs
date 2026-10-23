//! Resource budgets and memory accounting for the storage engine.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use linkrs_core::error::storage::StorageErrorKind;
use linkrs_core::types::Timestamp;
use linkrs_core::{StorageError, StorageResult};

/// Memory ownership categories used by the storage engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(usize)]
pub enum MemoryCategory {
    Data = 0,
    Index = 1,
    Mvcc = 2,
    Cache = 3,
    Background = 4,
}

impl MemoryCategory {
    const COUNT: usize = 5;

    /// Return all categories in a stable order for diagnostics.
    pub const fn all() -> [Self; Self::COUNT] {
        [
            Self::Data,
            Self::Index,
            Self::Mvcc,
            Self::Cache,
            Self::Background,
        ]
    }

    /// Return the metric-friendly category name.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Data => "data",
            Self::Index => "index",
            Self::Mvcc => "mvcc",
            Self::Cache => "cache",
            Self::Background => "background",
        }
    }
}

/// Validated memory limits for one storage instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryBudget {
    pub max_memory_bytes: u64,
    pub index_memory_bytes: u64,
    pub soft_limit_bytes: u64,
    pub hard_limit_bytes: u64,
}

impl MemoryBudget {
    /// Build a budget from a total limit and soft/hard ratios.
    pub fn new(
        max_memory_bytes: u64,
        index_memory_bytes: u64,
        soft_ratio: f64,
        hard_ratio: f64,
    ) -> StorageResult<Self> {
        if max_memory_bytes == 0 {
            return Err(StorageError::new(
                StorageErrorKind::InvalidInput,
                "max_memory_bytes must be greater than 0",
            ));
        }
        if index_memory_bytes == 0 || index_memory_bytes > max_memory_bytes {
            return Err(StorageError::new(
                StorageErrorKind::InvalidInput,
                "index_memory_bytes must be greater than 0 and no greater than max_memory_bytes",
            ));
        }
        if !soft_ratio.is_finite()
            || !hard_ratio.is_finite()
            || !(0.0..=1.0).contains(&soft_ratio)
            || !(0.0..=1.0).contains(&hard_ratio)
            || soft_ratio == 0.0
            || soft_ratio >= hard_ratio
        {
            return Err(StorageError::new(
                StorageErrorKind::InvalidInput,
                "memory ratios must satisfy 0 < soft_ratio < hard_ratio <= 1",
            ));
        }

        let soft_limit_bytes = (max_memory_bytes as f64 * soft_ratio).round() as u64;
        let hard_limit_bytes = (max_memory_bytes as f64 * hard_ratio).round() as u64;
        if soft_limit_bytes == 0 || hard_limit_bytes <= soft_limit_bytes {
            return Err(StorageError::new(
                StorageErrorKind::InvalidInput,
                "memory ratios produce invalid byte limits",
            ));
        }

        Ok(Self {
            max_memory_bytes,
            index_memory_bytes,
            soft_limit_bytes,
            hard_limit_bytes,
        })
    }

    pub(crate) fn from_validated(
        max_memory_bytes: u64,
        index_memory_bytes: u64,
        soft_ratio: f64,
        hard_ratio: f64,
    ) -> Self {
        Self {
            max_memory_bytes,
            index_memory_bytes,
            soft_limit_bytes: (max_memory_bytes as f64 * soft_ratio).round() as u64,
            hard_limit_bytes: (max_memory_bytes as f64 * hard_ratio).round() as u64,
        }
    }

    fn category_limit(self, category: MemoryCategory) -> u64 {
        match category {
            MemoryCategory::Index => self.index_memory_bytes,
            _ => self.max_memory_bytes,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MemoryUsage {
    pub current_bytes: u64,
    pub peak_bytes: u64,
}

/// Unified memory pressure level derived from one aggregate snapshot.
/// Reuses the instance soft/hard ratios; no additional thresholds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MemoryPressureLevel {
    #[default]
    Normal,
    Elevated,
    Critical,
}

impl MemoryPressureLevel {
    pub fn is_elevated(self) -> bool {
        matches!(self, Self::Elevated | Self::Critical)
    }
}

/// Point-in-time resource information suitable for logs and metrics exports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourceSnapshot {
    pub budget: MemoryBudget,
    pub categories: [MemoryUsage; MemoryCategory::COUNT],
    pub total_current_bytes: u64,
    pub total_peak_bytes: u64,
    pub soft_limit_events: u64,
    pub hard_limit_rejections: u64,
    pub active_snapshots: usize,
    pub oldest_snapshot_ts: Timestamp,
    pub tombstone_count: usize,
    pub tombstone_memory_bytes: u64,
    pub vertex_shard_count: usize,
    /// Unified pressure level for this snapshot.
    pub pressure_level: MemoryPressureLevel,
    /// Per-evictor freed bytes since process start, kept distinguishable so
    /// one pressured round attributes work without double counting.
    /// `spill` tracks cache-shrink work done through the spiller,
    /// `cache` tracks expired-trim work, `index` tracks cold-chunk eviction,
    /// `compaction` tracks generation retirement/compaction work.
    pub spill_relief_bytes: u64,
    pub spill_relief_triggers: u64,
    pub index_relief_bytes: u64,
    pub index_relief_triggers: u64,
    pub cache_relief_bytes: u64,
    pub cache_relief_triggers: u64,
    pub compaction_relief_bytes: u64,
    pub compaction_relief_triggers: u64,
}

impl ResourceSnapshot {
    pub fn usage(self, category: MemoryCategory) -> MemoryUsage {
        self.categories[category as usize]
    }

    pub fn soft_limit_exceeded(self) -> bool {
        self.total_current_bytes >= self.budget.soft_limit_bytes
    }

    pub fn hard_limit_exceeded(self) -> bool {
        self.total_current_bytes >= self.budget.hard_limit_bytes
    }

    /// Monotonic pressure mapping: totals only move the level upward through
    /// soft then hard, reusing the existing budget ratios.
    pub fn pressure_level(self) -> MemoryPressureLevel {
        self.pressure_level
    }
}

/// Relief step identity for the fixed priority chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReliefStep {
    ExpiredTrim,
    IndexEvict,
    CacheShrink,
    Compaction,
}

impl ReliefStep {
    pub const ALL: [Self; 4] = [
        Self::ExpiredTrim,
        Self::IndexEvict,
        Self::CacheShrink,
        Self::Compaction,
    ];
}

/// Thread-safe accounting shared by storage components.
pub struct MemoryAccounting {
    budget: MemoryBudget,
    current: [AtomicU64; MemoryCategory::COUNT],
    peaks: [AtomicU64; MemoryCategory::COUNT],
    total_current: AtomicU64,
    total_peak: AtomicU64,
    soft_limit_events: AtomicU64,
    hard_limit_rejections: AtomicU64,
    spill_relief_bytes: AtomicU64,
    spill_relief_triggers: AtomicU64,
    index_relief_bytes: AtomicU64,
    index_relief_triggers: AtomicU64,
    cache_relief_bytes: AtomicU64,
    cache_relief_triggers: AtomicU64,
    compaction_relief_bytes: AtomicU64,
    compaction_relief_triggers: AtomicU64,
}

impl std::fmt::Debug for MemoryAccounting {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemoryAccounting")
            .field("snapshot", &self.snapshot())
            .finish()
    }
}

impl MemoryAccounting {
    pub fn new(budget: MemoryBudget) -> Self {
        Self {
            budget,
            current: std::array::from_fn(|_| AtomicU64::new(0)),
            peaks: std::array::from_fn(|_| AtomicU64::new(0)),
            total_current: AtomicU64::new(0),
            total_peak: AtomicU64::new(0),
            soft_limit_events: AtomicU64::new(0),
            hard_limit_rejections: AtomicU64::new(0),
            spill_relief_bytes: AtomicU64::new(0),
            spill_relief_triggers: AtomicU64::new(0),
            index_relief_bytes: AtomicU64::new(0),
            index_relief_triggers: AtomicU64::new(0),
            cache_relief_bytes: AtomicU64::new(0),
            cache_relief_triggers: AtomicU64::new(0),
            compaction_relief_bytes: AtomicU64::new(0),
            compaction_relief_triggers: AtomicU64::new(0),
        }
    }

    pub fn budget(&self) -> MemoryBudget {
        self.budget
    }

    /// Replace an observed usage value for a category.
    pub fn report_usage(&self, category: MemoryCategory, bytes: u64) {
        let slot = &self.current[category as usize];
        let previous = slot.swap(bytes, Ordering::Relaxed);
        let total = if bytes >= previous {
            self.total_current
                .fetch_add(bytes - previous, Ordering::Relaxed)
                .saturating_add(bytes - previous)
        } else {
            self.total_current
                .fetch_sub(previous - bytes, Ordering::Relaxed)
                .saturating_sub(previous - bytes)
        };
        self.update_peak(&self.peaks[category as usize], bytes);
        self.update_peak(&self.total_peak, total);
        self.record_soft_crossing(total.saturating_sub(bytes), total);
    }

    /// Reserve temporary memory for a background operation.
    pub fn try_reserve(
        self: &Arc<Self>,
        category: MemoryCategory,
        bytes: u64,
    ) -> StorageResult<MemoryReservation> {
        let category_current = self.current[category as usize].load(Ordering::Relaxed);
        if category_current.saturating_add(bytes) > self.budget.category_limit(category) {
            self.hard_limit_rejections.fetch_add(1, Ordering::Relaxed);
            return Err(StorageError::new(
                StorageErrorKind::CapacityExceeded,
                format!(
                    "{} memory budget exceeded: requested {} bytes",
                    category.name(),
                    bytes
                ),
            ));
        }

        let mut current = self.total_current.load(Ordering::Relaxed);
        loop {
            let next = current.saturating_add(bytes);
            if next > self.budget.hard_limit_bytes {
                self.hard_limit_rejections.fetch_add(1, Ordering::Relaxed);
                return Err(StorageError::new(
                    StorageErrorKind::CapacityExceeded,
                    format!(
                        "total memory hard limit exceeded: requested {} bytes",
                        bytes
                    ),
                ));
            }
            match self.total_current.compare_exchange_weak(
                current,
                next,
                Ordering::AcqRel,
                Ordering::Relaxed,
            ) {
                Ok(_) => {
                    self.current[category as usize].fetch_add(bytes, Ordering::Relaxed);
                    self.update_peak(&self.peaks[category as usize], category_current + bytes);
                    self.update_peak(&self.total_peak, next);
                    self.record_soft_crossing(current, next);
                    return Ok(MemoryReservation {
                        accounting: Arc::clone(self),
                        category,
                        bytes,
                    });
                }
                Err(observed) => current = observed,
            }
        }
    }

    /// Reserve memory with automatic spilling on hard-limit failure.
    ///
    /// If `try_reserve` fails with `CapacityExceeded`, invokes `spill_fn` to
    /// free memory, then retries the reservation. If the spill fails or the
    /// retry also fails, the original error is returned.
    pub fn try_reserve_with_spill<F>(
        self: &Arc<Self>,
        category: MemoryCategory,
        bytes: u64,
        spill_fn: F,
    ) -> StorageResult<MemoryReservation>
    where
        F: Fn() -> Option<u64>,
    {
        match self.try_reserve(category, bytes) {
            Ok(reservation) => Ok(reservation),
            Err(original_err) => {
                if let Some(freed) = spill_fn() {
                    if freed > 0 {
                        return self.try_reserve(category, bytes);
                    }
                }
                Err(original_err)
            }
        }
    }

    /// Release memory from a category. Called by MemoryReservation::Drop
    /// and by eviction listeners. Saturating so a stale double report can
    /// never wrap the counters; totals track the minimum of both sides.
    pub(crate) fn release(&self, category: MemoryCategory, bytes: u64) {
        let cat_current = self.current[category as usize].load(Ordering::Relaxed);
        let total_current = self.total_current.load(Ordering::Relaxed);
        let decrement = bytes.min(cat_current).min(total_current);
        if decrement > 0 {
            self.current[category as usize].fetch_sub(decrement, Ordering::Relaxed);
            self.total_current.fetch_sub(decrement, Ordering::Relaxed);
        }
    }

    /// Release memory from a category (public wrapper for eviction callbacks).
    ///
    /// This is the public interface for components like the cache eviction
    /// listener to report memory that was freed outside the reservation system.
    pub fn release_category(&self, category: MemoryCategory, bytes: u64) {
        self.release(category, bytes);
    }

    /// Record freed bytes for one relief-chain step. Contributions stay
    /// distinguishable per evictor in the snapshot.
    pub fn record_relief(&self, step: ReliefStep, freed_bytes: u64) {
        if freed_bytes == 0 {
            return;
        }
        match step {
            ReliefStep::ExpiredTrim => {
                self.cache_relief_bytes
                    .fetch_add(freed_bytes, Ordering::Relaxed);
                self.cache_relief_triggers.fetch_add(1, Ordering::Relaxed);
            }
            ReliefStep::CacheShrink => {
                // Cache shrink runs through the spiller; the spiller already
                // records under the spill identity, so this path is only for
                // direct cache work outside the spiller.
                self.cache_relief_bytes
                    .fetch_add(freed_bytes, Ordering::Relaxed);
                self.cache_relief_triggers.fetch_add(1, Ordering::Relaxed);
            }
            ReliefStep::IndexEvict => {
                self.index_relief_bytes
                    .fetch_add(freed_bytes, Ordering::Relaxed);
                self.index_relief_triggers.fetch_add(1, Ordering::Relaxed);
            }
            ReliefStep::Compaction => {
                self.compaction_relief_bytes
                    .fetch_add(freed_bytes, Ordering::Relaxed);
                self.compaction_relief_triggers
                    .fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    /// Record spiller freed bytes under its own identity so spill, index,
    /// cache, and compaction contributions never alias in the snapshot.
    pub fn record_spill_relief(&self, freed_bytes: u64) {
        if freed_bytes == 0 {
            return;
        }
        self.spill_relief_bytes
            .fetch_add(freed_bytes, Ordering::Relaxed);
        self.spill_relief_triggers.fetch_add(1, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> ResourceSnapshot {
        let total_current_bytes = self.total_current.load(Ordering::Relaxed);
        let pressure_level = if total_current_bytes >= self.budget.hard_limit_bytes {
            MemoryPressureLevel::Critical
        } else if total_current_bytes >= self.budget.soft_limit_bytes {
            MemoryPressureLevel::Elevated
        } else {
            MemoryPressureLevel::Normal
        };
        ResourceSnapshot {
            budget: self.budget,
            categories: std::array::from_fn(|index| MemoryUsage {
                current_bytes: self.current[index].load(Ordering::Relaxed),
                peak_bytes: self.peaks[index].load(Ordering::Relaxed),
            }),
            total_current_bytes,
            total_peak_bytes: self.total_peak.load(Ordering::Relaxed),
            soft_limit_events: self.soft_limit_events.load(Ordering::Relaxed),
            hard_limit_rejections: self.hard_limit_rejections.load(Ordering::Relaxed),
            active_snapshots: 0,
            oldest_snapshot_ts: Timestamp::MAX,
            tombstone_count: 0,
            tombstone_memory_bytes: 0,
            vertex_shard_count: 0,
            pressure_level,
            spill_relief_bytes: self.spill_relief_bytes.load(Ordering::Relaxed),
            spill_relief_triggers: self.spill_relief_triggers.load(Ordering::Relaxed),
            index_relief_bytes: self.index_relief_bytes.load(Ordering::Relaxed),
            index_relief_triggers: self.index_relief_triggers.load(Ordering::Relaxed),
            cache_relief_bytes: self.cache_relief_bytes.load(Ordering::Relaxed),
            cache_relief_triggers: self.cache_relief_triggers.load(Ordering::Relaxed),
            compaction_relief_bytes: self.compaction_relief_bytes.load(Ordering::Relaxed),
            compaction_relief_triggers: self.compaction_relief_triggers.load(Ordering::Relaxed),
        }
    }

    fn update_peak(&self, peak: &AtomicU64, value: u64) {
        let mut previous = peak.load(Ordering::Relaxed);
        while value > previous {
            match peak.compare_exchange_weak(previous, value, Ordering::Relaxed, Ordering::Relaxed)
            {
                Ok(_) => break,
                Err(observed) => previous = observed,
            }
        }
    }

    fn record_soft_crossing(&self, previous: u64, current: u64) {
        if previous < self.budget.soft_limit_bytes && current >= self.budget.soft_limit_bytes {
            self.soft_limit_events.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// RAII handle for temporary background memory.
pub struct MemoryReservation {
    accounting: Arc<MemoryAccounting>,
    category: MemoryCategory,
    bytes: u64,
}

impl std::fmt::Debug for MemoryReservation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemoryReservation")
            .field("category", &self.category)
            .field("bytes", &self.bytes)
            .finish()
    }
}

impl Drop for MemoryReservation {
    fn drop(&mut self) {
        self.accounting.release(self.category, self.bytes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budget_rejects_invalid_limits() {
        assert!(MemoryBudget::new(0, 1, 0.8, 0.95).is_err());
        assert!(MemoryBudget::new(100, 101, 0.8, 0.95).is_err());
        assert!(MemoryBudget::new(100, 20, 0.95, 0.8).is_err());
    }

    #[test]
    fn accounting_tracks_pressure_and_releases_reservations() {
        let budget = MemoryBudget::new(100, 20, 0.8, 0.95).expect("valid budget");
        let accounting = Arc::new(MemoryAccounting::new(budget));

        let reservation = accounting
            .try_reserve(MemoryCategory::Background, 90)
            .expect("reservation below hard limit");
        let snapshot = accounting.snapshot();
        assert_eq!(snapshot.total_current_bytes, 90);
        assert!(snapshot.soft_limit_exceeded());
        assert_eq!(snapshot.soft_limit_events, 1);

        assert!(accounting
            .try_reserve(MemoryCategory::Background, 6)
            .is_err());
        assert_eq!(accounting.snapshot().hard_limit_rejections, 1);

        drop(reservation);
        assert_eq!(accounting.snapshot().total_current_bytes, 0);
    }

    #[test]
    fn index_budget_is_independent_from_total_budget() {
        let budget = MemoryBudget::new(100, 20, 0.8, 0.95).expect("valid budget");
        let accounting = Arc::new(MemoryAccounting::new(budget));
        assert!(accounting.try_reserve(MemoryCategory::Index, 21).is_err());
        assert!(accounting.try_reserve(MemoryCategory::Data, 20).is_ok());
    }

    #[test]
    fn pressure_level_moves_monotonically_with_usage() {
        let budget = MemoryBudget::new(100, 20, 0.8, 0.95).expect("valid budget");
        let accounting = Arc::new(MemoryAccounting::new(budget));
        assert_eq!(
            accounting.snapshot().pressure_level,
            MemoryPressureLevel::Normal
        );
        let _guard = accounting
            .try_reserve(MemoryCategory::Background, 85)
            .expect("below hard");
        assert_eq!(
            accounting.snapshot().pressure_level,
            MemoryPressureLevel::Elevated
        );
        let _guard2 = accounting
            .try_reserve(MemoryCategory::Background, 10)
            .expect("at hard");
        assert_eq!(
            accounting.snapshot().pressure_level,
            MemoryPressureLevel::Critical
        );
    }

    #[test]
    fn relief_stays_attributed_to_originating_step() {
        let budget = MemoryBudget::new(1000, 200, 0.8, 0.95).expect("valid budget");
        let accounting = Arc::new(MemoryAccounting::new(budget));
        accounting.record_relief(ReliefStep::CacheShrink, 40);
        accounting.record_relief(ReliefStep::IndexEvict, 25);
        accounting.record_spill_relief(15);
        accounting.record_relief(ReliefStep::Compaction, 7);
        let snapshot = accounting.snapshot();
        assert_eq!(snapshot.cache_relief_bytes, 40);
        assert_eq!(snapshot.cache_relief_triggers, 1);
        assert_eq!(snapshot.index_relief_bytes, 25);
        assert_eq!(snapshot.index_relief_triggers, 1);
        assert_eq!(snapshot.spill_relief_bytes, 15);
        assert_eq!(snapshot.spill_relief_triggers, 1);
        assert_eq!(snapshot.compaction_relief_bytes, 7);
        assert_eq!(snapshot.compaction_relief_triggers, 1);
    }

    #[test]
    fn release_never_wraps_below_zero() {
        let budget = MemoryBudget::new(100, 20, 0.8, 0.95).expect("valid budget");
        let accounting = Arc::new(MemoryAccounting::new(budget));
        accounting.release(MemoryCategory::Cache, 50);
        let snapshot = accounting.snapshot();
        assert_eq!(snapshot.categories[MemoryCategory::Cache as usize].current_bytes, 0);
        assert_eq!(snapshot.total_current_bytes, 0);
    }
}
