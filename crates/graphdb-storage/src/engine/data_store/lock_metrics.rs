//! Catalog and per-table lock metrics, plus the guard/write-set wrappers
//! that meter acquisitions, wait and hold times.

use std::collections::HashMap;
use std::ops::{Deref, DerefMut};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use parking_lot::{RwLock, RwLockReadGuard, RwLockWriteGuard};

use crate::edge::EdgeStore;
use crate::vertex::ShardedVertexTable;
use graphdb_core::types::LabelId;

use super::EdgeTableKey;
use super::GraphDataStore;

/// Contention counters for per-partition edge-table locks.
///
/// Catalog metrics cover the registry guards; these cover the table
/// `RwLock<EdgeStore>` acquisitions instead, split by read and write.
/// A rising write-wait share on one table while catalog waits stay flat
/// points at a hot partition (single hot vertex or single hot edge type),
/// which is the signal for narrower sharding rather than catalog work.
#[derive(Debug, Default)]
pub(crate) struct TableLockMetrics {
    read_acquisitions: AtomicU64,
    read_wait_nanos: AtomicU64,
    read_contended: AtomicU64,
    write_acquisitions: AtomicU64,
    write_wait_nanos: AtomicU64,
    write_contended: AtomicU64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct TableLockMetricsSnapshot {
    pub read_acquisitions: u64,
    pub read_wait_nanos: u64,
    pub read_contended: u64,
    pub write_acquisitions: u64,
    pub write_wait_nanos: u64,
    pub write_contended: u64,
}

impl TableLockMetrics {
    pub(super) fn record(&self, write: bool, started: Instant) {
        let waited = started.elapsed();
        let waited_nanos = waited.as_nanos().min(u64::MAX as u128) as u64;
        let contended = waited >= std::time::Duration::from_micros(1);
        if write {
            self.write_acquisitions.fetch_add(1, Ordering::Relaxed);
            self.write_wait_nanos
                .fetch_add(waited_nanos, Ordering::Relaxed);
            if contended {
                self.write_contended.fetch_add(1, Ordering::Relaxed);
            }
        } else {
            self.read_acquisitions.fetch_add(1, Ordering::Relaxed);
            self.read_wait_nanos
                .fetch_add(waited_nanos, Ordering::Relaxed);
            if contended {
                self.read_contended.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    pub(super) fn snapshot(&self) -> TableLockMetricsSnapshot {
        TableLockMetricsSnapshot {
            read_acquisitions: self.read_acquisitions.load(Ordering::Relaxed),
            read_wait_nanos: self.read_wait_nanos.load(Ordering::Relaxed),
            read_contended: self.read_contended.load(Ordering::Relaxed),
            write_acquisitions: self.write_acquisitions.load(Ordering::Relaxed),
            write_wait_nanos: self.write_wait_nanos.load(Ordering::Relaxed),
            write_contended: self.write_contended.load(Ordering::Relaxed),
        }
    }
}

/// A short-lived read view of catalog metadata. The view exposes closures,
/// rather than the underlying lock guards, so a caller cannot retain a table
/// guard while acquiring a lock from another catalog domain.
pub(crate) struct CatalogReadSnapshot<'a> {
    pub(super) store: &'a GraphDataStore,
}

impl CatalogReadSnapshot<'_> {
    pub(crate) fn with_vertex_tables<R>(
        &self,
        operation: impl FnOnce(&HashMap<LabelId, Arc<ShardedVertexTable>>) -> R,
    ) -> R {
        self.store.with_vertex_tables(operation)
    }

    pub(crate) fn with_edge_tables<R>(
        &self,
        operation: impl FnOnce(&HashMap<EdgeTableKey, Arc<RwLock<EdgeStore>>>) -> R,
    ) -> R {
        self.store.with_edge_tables(operation)
    }

    pub(crate) fn with_edge_label_index<R>(
        &self,
        operation: impl FnOnce(&HashMap<LabelId, Vec<EdgeTableKey>>) -> R,
    ) -> R {
        self.store.with_edge_label_index(operation)
    }
}

#[derive(Debug, Default)]
pub(crate) struct CatalogLockMetrics {
    acquisitions: AtomicU64,
    wait_nanos: AtomicU64,
    hold_nanos: AtomicU64,
    contended: AtomicU64,
    by_operation: [CatalogOperationMetrics; CatalogLockOperation::COUNT],
}

#[derive(Debug)]
struct CatalogOperationMetrics {
    acquisitions: AtomicU64,
    wait_nanos: AtomicU64,
    hold_nanos: AtomicU64,
    contended: AtomicU64,
}

impl Default for CatalogOperationMetrics {
    fn default() -> Self {
        Self::new()
    }
}

impl CatalogOperationMetrics {
    const fn new() -> Self {
        Self {
            acquisitions: AtomicU64::new(0),
            wait_nanos: AtomicU64::new(0),
            hold_nanos: AtomicU64::new(0),
            contended: AtomicU64::new(0),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct CatalogOperationMetricsSnapshot {
    pub acquisitions: u64,
    pub wait_nanos: u64,
    pub hold_nanos: u64,
    pub contended: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(usize)]
pub(crate) enum CatalogLockOperation {
    VertexLabels = 0,
    EdgeLabels = 1,
    VertexCounter = 2,
    EdgeCounter = 3,
    VertexTables = 4,
    EdgeTables = 5,
    EdgeLabelIndex = 6,
}

impl CatalogLockOperation {
    const COUNT: usize = 7;

    pub(crate) const fn all() -> [Self; Self::COUNT] {
        [
            Self::VertexLabels,
            Self::EdgeLabels,
            Self::VertexCounter,
            Self::EdgeCounter,
            Self::VertexTables,
            Self::EdgeTables,
            Self::EdgeLabelIndex,
        ]
    }

    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::VertexLabels => "vertex_labels",
            Self::EdgeLabels => "edge_labels",
            Self::VertexCounter => "vertex_counter",
            Self::EdgeCounter => "edge_counter",
            Self::VertexTables => "vertex_tables",
            Self::EdgeTables => "edge_tables",
            Self::EdgeLabelIndex => "edge_label_index",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct CatalogLockMetricsSnapshot {
    pub acquisitions: u64,
    pub wait_nanos: u64,
    pub hold_nanos: u64,
    pub contended: u64,
    pub by_operation: [CatalogOperationMetricsSnapshot; CatalogLockOperation::COUNT],
}

pub(crate) struct CatalogReadGuard<'a, T> {
    pub(super) guard: RwLockReadGuard<'a, T>,
    pub(super) metrics: &'a CatalogLockMetrics,
    pub(super) acquired_at: Instant,
    pub(super) operation: CatalogLockOperation,
}

impl<T> Deref for CatalogReadGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.guard
    }
}

impl<T> Drop for CatalogReadGuard<'_, T> {
    fn drop(&mut self) {
        let elapsed = self.acquired_at.elapsed().as_nanos().min(u64::MAX as u128) as u64;
        self.metrics
            .hold_nanos
            .fetch_add(elapsed, Ordering::Relaxed);
        let operation = &self.metrics.by_operation[self.operation as usize];
        operation.hold_nanos.fetch_add(elapsed, Ordering::Relaxed);
    }
}

pub(crate) struct CatalogWriteGuard<'a, T> {
    pub(super) guard: RwLockWriteGuard<'a, T>,
    pub(super) metrics: &'a CatalogLockMetrics,
    pub(super) acquired_at: Instant,
    pub(super) operation: CatalogLockOperation,
}

pub(crate) struct CatalogWriteSet<'a> {
    pub vertex_label_names: CatalogWriteGuard<'a, HashMap<String, LabelId>>,
    pub edge_label_names: CatalogWriteGuard<'a, HashMap<String, LabelId>>,
    pub _vertex_label_counter: CatalogWriteGuard<'a, LabelId>,
    pub _edge_label_counter: CatalogWriteGuard<'a, LabelId>,
    pub vertex_tables: CatalogWriteGuard<'a, HashMap<LabelId, Arc<ShardedVertexTable>>>,
    pub edge_tables: CatalogWriteGuard<'a, HashMap<EdgeTableKey, Arc<RwLock<EdgeStore>>>>,
    pub _edge_label_index: CatalogWriteGuard<'a, HashMap<LabelId, Vec<EdgeTableKey>>>,
}

impl<T> Deref for CatalogWriteGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.guard
    }
}

impl<T> DerefMut for CatalogWriteGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.guard
    }
}

impl<T> Drop for CatalogWriteGuard<'_, T> {
    fn drop(&mut self) {
        let elapsed = self.acquired_at.elapsed().as_nanos().min(u64::MAX as u128) as u64;
        self.metrics
            .hold_nanos
            .fetch_add(elapsed, Ordering::Relaxed);
        let operation = &self.metrics.by_operation[self.operation as usize];
        operation.hold_nanos.fetch_add(elapsed, Ordering::Relaxed);
    }
}

impl CatalogLockMetrics {
    pub(super) fn record(&self, operation: CatalogLockOperation, started: Instant) {
        let waited = started.elapsed();
        self.acquisitions.fetch_add(1, Ordering::Relaxed);
        self.wait_nanos.fetch_add(
            waited.as_nanos().min(u64::MAX as u128) as u64,
            Ordering::Relaxed,
        );
        if waited >= std::time::Duration::from_micros(1) {
            self.contended.fetch_add(1, Ordering::Relaxed);
        }
        let metrics = &self.by_operation[operation as usize];
        metrics.acquisitions.fetch_add(1, Ordering::Relaxed);
        metrics.wait_nanos.fetch_add(
            waited.as_nanos().min(u64::MAX as u128) as u64,
            Ordering::Relaxed,
        );
        if waited >= std::time::Duration::from_micros(1) {
            metrics.contended.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub(super) fn snapshot(&self) -> CatalogLockMetricsSnapshot {
        CatalogLockMetricsSnapshot {
            acquisitions: self.acquisitions.load(Ordering::Relaxed),
            wait_nanos: self.wait_nanos.load(Ordering::Relaxed),
            hold_nanos: self.hold_nanos.load(Ordering::Relaxed),
            contended: self.contended.load(Ordering::Relaxed),
            by_operation: std::array::from_fn(|index| {
                let metrics = &self.by_operation[index];
                CatalogOperationMetricsSnapshot {
                    acquisitions: metrics.acquisitions.load(Ordering::Relaxed),
                    wait_nanos: metrics.wait_nanos.load(Ordering::Relaxed),
                    hold_nanos: metrics.hold_nanos.load(Ordering::Relaxed),
                    contended: metrics.contended.load(Ordering::Relaxed),
                }
            }),
        }
    }
}
