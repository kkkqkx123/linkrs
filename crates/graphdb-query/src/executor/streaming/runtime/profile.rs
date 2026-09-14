//! Per-operator execution profiling.
//!
//! [`OperatorProfile`] is the serializable snapshot, [`ProfileEntry`] holds
//! lock-free atomic counters for the hot path, [`ProfileBoard`] is the
//! per-query store, and [`ProfileCollector`] aggregates profiles for
//! EXPLAIN / PROFILE output.

use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Instant;

use parking_lot::Mutex;

use parking_lot::RwLock;

use crate::executor::streaming::plan::types::PhysicalOperatorId;

/// Per-operator profile snapshot
#[derive(Debug, Clone, Default)]
pub struct OperatorProfile {
    pub physical_operator_id: PhysicalOperatorId,
    pub node_id: i64,
    pub partition_id: Option<usize>,
    pub name: String,
    pub open_time_us: u64,
    pub next_time_us: u64,
    pub close_time_us: u64,
    pub output_rows: u64,
    /// Number of chunks produced (advances).  Used as the per-operator
    /// execution loop count in the feedback path.
    pub advance_count: u64,
    pub peak_memory: u64,
    pub peak_memory_bytes: u64,
    pub spilled_bytes: u64,
    pub spill_count: u64,
    /// Logical rows written to spill runs. Displayed next to the static
    /// `spill_threshold` config so PROFILE shows configured vs actual.
    pub spilled_rows: u64,
}

/// Identifies an operator instance in a partitioned executor tree.
///
/// A physical operator occurs once per local partition. Logical node IDs may
/// be shared by multiple physical operators, so they are display metadata and
/// cannot identify a profile entry. Global and gather operators use `None`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct OperatorProfileKey {
    pub physical_operator_id: PhysicalOperatorId,
    pub partition_id: Option<usize>,
}

impl OperatorProfileKey {
    pub const fn new(
        physical_operator_id: PhysicalOperatorId,
        partition_id: Option<usize>,
    ) -> Self {
        Self {
            physical_operator_id,
            partition_id,
        }
    }
}

/// Per-operator atomic profile counters for lock-free hot-path updates.
///
/// Each operator registers a [`ProfileEntry`] during `open()`.  Subsequent
/// timing and row-count updates use atomic operations with no lock held,
/// eliminating mutex contention on the per-advance hot path.
#[derive(Debug)]
pub struct ProfileEntry {
    pub physical_operator_id: PhysicalOperatorId,
    pub node_id: AtomicI64,
    pub partition_id: Option<usize>,
    pub name: parking_lot::Mutex<String>,
    pub open_time_us: AtomicU64,
    pub next_time_us: AtomicU64,
    pub close_time_us: AtomicU64,
    pub output_rows: AtomicU64,
    pub advance_count: AtomicU64,
    pub peak_memory_bytes: AtomicU64,
    pub spilled_bytes: AtomicU64,
    pub spill_count: AtomicU64,
    pub spilled_rows: AtomicU64,
}

impl ProfileEntry {
    pub fn new(profile: &OperatorProfile) -> Self {
        Self {
            physical_operator_id: profile.physical_operator_id,
            node_id: AtomicI64::new(profile.node_id),
            partition_id: profile.partition_id,
            name: parking_lot::Mutex::new(profile.name.clone()),
            open_time_us: AtomicU64::new(profile.open_time_us),
            next_time_us: AtomicU64::new(profile.next_time_us),
            close_time_us: AtomicU64::new(profile.close_time_us),
            output_rows: AtomicU64::new(profile.output_rows),
            advance_count: AtomicU64::new(profile.advance_count),
            peak_memory_bytes: AtomicU64::new(profile.peak_memory_bytes),
            spilled_bytes: AtomicU64::new(profile.spilled_bytes),
            spill_count: AtomicU64::new(profile.spill_count),
            spilled_rows: AtomicU64::new(profile.spilled_rows),
        }
    }

    /// Snapshot current atomics into an [`OperatorProfile`] for reporting.
    pub fn snapshot(&self) -> OperatorProfile {
        OperatorProfile {
            physical_operator_id: self.physical_operator_id,
            node_id: self.node_id.load(Ordering::Relaxed),
            partition_id: self.partition_id,
            name: self.name.lock().clone(),
            open_time_us: self.open_time_us.load(Ordering::Relaxed),
            next_time_us: self.next_time_us.load(Ordering::Relaxed),
            close_time_us: self.close_time_us.load(Ordering::Relaxed),
            output_rows: self.output_rows.load(Ordering::Relaxed),
            advance_count: self.advance_count.load(Ordering::Relaxed),
            peak_memory: self.peak_memory_bytes.load(Ordering::Relaxed),
            peak_memory_bytes: self.peak_memory_bytes.load(Ordering::Relaxed),
            spilled_bytes: self.spilled_bytes.load(Ordering::Relaxed),
            spill_count: self.spill_count.load(Ordering::Relaxed),
            spilled_rows: self.spilled_rows.load(Ordering::Relaxed),
        }
    }
}

/// Lock-free profile store for hot-path operator timing and row-count updates.
///
/// Uses `RwLock` for structural operations (first-access entry creation) and
/// `AtomicU64` counters so that per-advance increments never block.
///
/// Design:
/// - `register_operator()` — called during `open()`, pre-creates entries
/// - `record_timing()` / `record_rows()` — lock-free fast path
/// - `flush_to_collector()` — aggregate into a [`ProfileCollector`] at end
#[derive(Debug)]
pub struct ProfileBoard {
    entries: RwLock<HashMap<OperatorProfileKey, Arc<ProfileEntry>>>,
    pub total_rows: AtomicU64,
    pub total_time_us: AtomicU64,
    start_time: Mutex<Option<Instant>>,
    end_time: Mutex<Option<Instant>>,
    pub parallel_wall_time_us: AtomicU64,
    pub parallel_work_time_us: AtomicU64,
    pub parallel_workers: AtomicUsize,
    pub parallel_buffered_chunks_peak: AtomicUsize,
    pub parallel_buffered_bytes_peak: AtomicUsize,
}

impl Default for ProfileBoard {
    fn default() -> Self {
        Self::new()
    }
}

impl ProfileBoard {
    pub fn new() -> Self {
        Self {
            entries: RwLock::new(HashMap::new()),
            total_rows: AtomicU64::new(0),
            total_time_us: AtomicU64::new(0),
            start_time: Mutex::new(None),
            end_time: Mutex::new(None),
            parallel_wall_time_us: AtomicU64::new(0),
            parallel_work_time_us: AtomicU64::new(0),
            parallel_workers: AtomicUsize::new(0),
            parallel_buffered_chunks_peak: AtomicUsize::new(0),
            parallel_buffered_bytes_peak: AtomicUsize::new(0),
        }
    }

    pub fn record_start(&self) {
        *self.start_time.lock() = Some(Instant::now());
    }

    pub fn record_end(&self) {
        let elapsed = self
            .start_time
            .lock()
            .map(|t| t.elapsed().as_micros() as u64)
            .unwrap_or(0);
        self.total_time_us.store(elapsed, Ordering::Relaxed);
        *self.end_time.lock() = Some(Instant::now());
    }

    /// Register (or update) a per-operator profile entry from a snapshot.
    ///
    /// Called during `open()` to pre-populate entries so the hot-path
    /// access in `advance()` never needs to write-lock.
    pub fn register_operator(&self, profile: &OperatorProfile) -> Arc<ProfileEntry> {
        let key = OperatorProfileKey::new(profile.physical_operator_id, profile.partition_id);
        let entry = Arc::new(ProfileEntry::new(profile));
        self.entries.write().insert(key, entry.clone());
        entry
    }

    /// Find an entry by key (read-lock, hot-path friendly).
    pub fn get_entry(&self, key: &OperatorProfileKey) -> Option<Arc<ProfileEntry>> {
        self.entries.read().get(key).cloned()
    }

    /// Aggregate all entries into a [`ProfileCollector`] for EXPLAIN output.
    pub fn flush_to_collector(&self) -> ProfileCollector {
        let mut collector = ProfileCollector::new();
        let guard = self.entries.read();
        for entry in guard.values() {
            collector.operators.insert(
                OperatorProfileKey::new(entry.physical_operator_id, entry.partition_id),
                entry.snapshot(),
            );
        }
        drop(guard);
        collector.total_rows = self.total_rows.load(Ordering::Relaxed);
        collector.total_time_us = self.total_time_us.load(Ordering::Relaxed);
        collector.start_time = *self.start_time.lock();
        collector.end_time = *self.end_time.lock();
        collector.parallel_wall_time_us = self.parallel_wall_time_us.load(Ordering::Relaxed);
        collector.parallel_work_time_us = self.parallel_work_time_us.load(Ordering::Relaxed);
        collector.parallel_workers = self.parallel_workers.load(Ordering::Relaxed);
        collector.parallel_buffered_chunks_peak =
            self.parallel_buffered_chunks_peak.load(Ordering::Relaxed);
        collector.parallel_buffered_bytes_peak =
            self.parallel_buffered_bytes_peak.load(Ordering::Relaxed);
        collector
    }
}

/// Collects execution profile data across all operators (for EXPLAIN output).
#[derive(Debug, Default)]
pub struct ProfileCollector {
    pub operators: HashMap<OperatorProfileKey, OperatorProfile>,
    pub total_rows: u64,
    pub total_time_us: u64,
    pub start_time: Option<Instant>,
    pub end_time: Option<Instant>,
    /// Wall-clock time spent in parallel partition execution.
    pub parallel_wall_time_us: u64,
    /// Sum of per-worker execution time (may exceed wall time).
    pub parallel_work_time_us: u64,
    /// Maximum number of workers used by any coordinator in this query.
    pub parallel_workers: usize,
    /// Peak number of chunks retained in output queues.
    pub parallel_buffered_chunks_peak: usize,
    /// Peak accounted bytes retained in output queues.
    pub parallel_buffered_bytes_peak: usize,
}

impl ProfileCollector {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_start(&mut self) {
        self.start_time = Some(Instant::now());
    }

    pub fn record_end(&mut self) {
        self.end_time = Some(Instant::now());
        if let Some(start) = self.start_time {
            self.total_time_us = start.elapsed().as_micros() as u64;
        }
    }

    pub fn add_rows(&mut self, count: u64) {
        self.total_rows += count;
    }

    pub fn record_operator_profile(&mut self, profile: OperatorProfile) {
        let key = OperatorProfileKey::new(profile.physical_operator_id, profile.partition_id);
        self.operators.insert(key, profile);
    }

    /// Return a snapshot of the parallel profile fields for
    /// EXPLAIN / PROFILE output.
    pub fn parallel_profile(&self) -> (u64, u64, usize, usize, usize) {
        (
            self.parallel_wall_time_us,
            self.parallel_work_time_us,
            self.parallel_workers,
            self.parallel_buffered_chunks_peak,
            self.parallel_buffered_bytes_peak,
        )
    }

    /// Aggregate profiles from partition execution into this collector.
    ///
    /// For each operator node_id, sums timing/output_rows and takes the max
    /// of peak_memory_bytes across partitions.
    pub fn aggregate_partition_profiles(&mut self, partition_profiles: &[ProfileCollector]) {
        for pp in partition_profiles {
            for (key, op) in &pp.operators {
                let entry = self
                    .operators
                    .entry(*key)
                    .or_insert_with(|| OperatorProfile {
                        physical_operator_id: key.physical_operator_id,
                        node_id: op.node_id,
                        partition_id: key.partition_id,
                        name: op.name.clone(),
                        ..OperatorProfile::default()
                    });
                entry.open_time_us += op.open_time_us;
                entry.next_time_us += op.next_time_us;
                entry.close_time_us += op.close_time_us;
                entry.output_rows += op.output_rows;
                entry.advance_count += op.advance_count;
                entry.peak_memory_bytes = entry.peak_memory_bytes.max(op.peak_memory_bytes);
                entry.spill_count += op.spill_count;
                entry.spilled_bytes += op.spilled_bytes;
                entry.spilled_rows += op.spilled_rows;
            }
            self.total_rows += pp.total_rows;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_profile_collector() {
        let mut pc = ProfileCollector::new();
        pc.record_start();
        std::thread::sleep(std::time::Duration::from_micros(100));
        pc.record_end();
        assert!(pc.total_time_us > 0);
    }

    #[test]
    fn test_partition_profile_aggregation_preserves_partition_identity() {
        let mut first = ProfileCollector::new();
        first.record_operator_profile(OperatorProfile {
            physical_operator_id: PhysicalOperatorId(7),
            node_id: 7,
            partition_id: Some(0),
            name: "ScanVertices".to_string(),
            output_rows: 2,
            peak_memory_bytes: 10,
            ..OperatorProfile::default()
        });
        let mut second = ProfileCollector::new();
        second.record_operator_profile(OperatorProfile {
            physical_operator_id: PhysicalOperatorId(7),
            node_id: 7,
            partition_id: Some(1),
            name: "ScanVertices".to_string(),
            output_rows: 3,
            peak_memory_bytes: 20,
            ..OperatorProfile::default()
        });

        let mut aggregate = ProfileCollector::new();
        aggregate.aggregate_partition_profiles(&[first, second]);

        assert_eq!(aggregate.operators.len(), 2);
        assert_eq!(
            aggregate
                .operators
                .get(&OperatorProfileKey::new(PhysicalOperatorId(7), Some(0)))
                .expect("partition zero profile")
                .output_rows,
            2
        );
        assert_eq!(
            aggregate
                .operators
                .get(&OperatorProfileKey::new(PhysicalOperatorId(7), Some(1)))
                .expect("partition one profile")
                .peak_memory_bytes,
            20
        );
    }

    #[test]
    fn test_partition_profile_aggregation_sums_spill_rows() {
        let mut first = ProfileCollector::new();
        first.record_operator_profile(OperatorProfile {
            physical_operator_id: PhysicalOperatorId(9),
            node_id: 9,
            partition_id: Some(0),
            name: "Sort".to_string(),
            spilled_rows: 100,
            spilled_bytes: 2048,
            spill_count: 1,
            ..OperatorProfile::default()
        });
        let mut second = ProfileCollector::new();
        second.record_operator_profile(OperatorProfile {
            physical_operator_id: PhysicalOperatorId(9),
            node_id: 9,
            partition_id: Some(1),
            name: "Sort".to_string(),
            spilled_rows: 50,
            spilled_bytes: 1024,
            spill_count: 1,
            ..OperatorProfile::default()
        });

        let mut aggregate = ProfileCollector::new();
        aggregate.aggregate_partition_profiles(&[first, second]);
        let entry = aggregate
            .operators
            .get(&OperatorProfileKey::new(PhysicalOperatorId(9), Some(0)))
            .expect("partition zero profile");
        assert_eq!(entry.spilled_rows, 100);
        let entry = aggregate
            .operators
            .get(&OperatorProfileKey::new(PhysicalOperatorId(9), Some(1)))
            .expect("partition one profile");
        assert_eq!(entry.spilled_rows, 50);
    }
}
