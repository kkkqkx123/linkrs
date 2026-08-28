use std::collections::HashMap;
use std::ops::{Deref, DerefMut};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use parking_lot::{RwLock, RwLockReadGuard, RwLockWriteGuard};

use graphdb_core::types::LabelId;
use graphdb_core::{StorageError, StorageResult};
use crate::edge::EdgeStore;
use crate::vertex::ShardedVertexTable;

#[derive(Hash, Eq, PartialEq, Clone, Copy, Debug)]
pub struct EdgeTableKey {
    pub src_label: LabelId,
    pub dst_label: LabelId,
    pub edge_label: LabelId,
}

impl EdgeTableKey {
    pub fn new(src_label: LabelId, dst_label: LabelId, edge_label: LabelId) -> Self {
        Self {
            src_label,
            dst_label,
            edge_label,
        }
    }
}

impl From<(LabelId, LabelId, LabelId)> for EdgeTableKey {
    fn from((src_label, dst_label, edge_label): (LabelId, LabelId, LabelId)) -> Self {
        Self {
            src_label,
            dst_label,
            edge_label,
        }
    }
}

pub struct GraphDataStore {
    vertex_tables: RwLock<HashMap<LabelId, Arc<ShardedVertexTable>>>,
    edge_tables: RwLock<HashMap<EdgeTableKey, Arc<RwLock<EdgeStore>>>>,
    vertex_label_names: RwLock<HashMap<String, LabelId>>,
    edge_label_names: RwLock<HashMap<String, LabelId>>,
    vertex_label_counter: RwLock<LabelId>,
    edge_label_counter: RwLock<LabelId>,
    /// Reverse index: edge_label -> list of EdgeTableKeys
    /// Enables O(1) lookup of all tables for a given edge label
    /// Significantly improves performance of edge property operations
    edge_label_index: RwLock<HashMap<LabelId, Vec<EdgeTableKey>>>,
    lock_metrics: CatalogLockMetrics,
}

/// A short-lived read view of catalog metadata. The view exposes closures,
/// rather than the underlying lock guards, so a caller cannot retain a table
/// guard while acquiring a lock from another catalog domain.
pub(crate) struct CatalogReadSnapshot<'a> {
    store: &'a GraphDataStore,
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
struct CatalogLockMetrics {
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
    guard: RwLockReadGuard<'a, T>,
    metrics: &'a CatalogLockMetrics,
    acquired_at: Instant,
    operation: CatalogLockOperation,
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
    guard: RwLockWriteGuard<'a, T>,
    metrics: &'a CatalogLockMetrics,
    acquired_at: Instant,
    operation: CatalogLockOperation,
}

pub(crate) struct CatalogWriteSet<'a> {
    pub vertex_label_names: CatalogWriteGuard<'a, HashMap<String, LabelId>>,
    pub edge_label_names: CatalogWriteGuard<'a, HashMap<String, LabelId>>,
    _vertex_label_counter: CatalogWriteGuard<'a, LabelId>,
    _edge_label_counter: CatalogWriteGuard<'a, LabelId>,
    pub vertex_tables: CatalogWriteGuard<'a, HashMap<LabelId, Arc<ShardedVertexTable>>>,
    pub edge_tables: CatalogWriteGuard<'a, HashMap<EdgeTableKey, Arc<RwLock<EdgeStore>>>>,
    _edge_label_index: CatalogWriteGuard<'a, HashMap<LabelId, Vec<EdgeTableKey>>>,
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
    fn record(&self, operation: CatalogLockOperation, started: Instant) {
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

    fn snapshot(&self) -> CatalogLockMetricsSnapshot {
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

impl GraphDataStore {
    pub fn new() -> Self {
        Self {
            vertex_tables: RwLock::new(HashMap::new()),
            edge_tables: RwLock::new(HashMap::new()),
            vertex_label_names: RwLock::new(HashMap::new()),
            edge_label_names: RwLock::new(HashMap::new()),
            vertex_label_counter: RwLock::new(0),
            edge_label_counter: RwLock::new(0),
            edge_label_index: RwLock::new(HashMap::new()),
            lock_metrics: CatalogLockMetrics::default(),
        }
    }

    // Catalog lock order for operations that touch multiple registries:
    // label names -> label counters -> vertex tables -> edge tables -> edge label index.
    // A caller must never retain one of these guards while requesting an earlier guard.
    pub(crate) fn read_vertex_tables(
        &self,
    ) -> CatalogReadGuard<'_, HashMap<LabelId, Arc<ShardedVertexTable>>> {
        let started = Instant::now();
        let guard = self.vertex_tables.read();
        self.lock_metrics
            .record(CatalogLockOperation::VertexTables, started);
        CatalogReadGuard {
            guard,
            metrics: &self.lock_metrics,
            acquired_at: Instant::now(),
            operation: CatalogLockOperation::VertexTables,
        }
    }

    #[cfg(test)]
    pub(crate) fn test_read_vertex_tables(
        &self,
    ) -> CatalogReadGuard<'_, HashMap<LabelId, Arc<ShardedVertexTable>>> {
        self.read_vertex_tables()
    }

    #[cfg(test)]
    pub(crate) fn read_vertex_label_names(&self) -> CatalogReadGuard<'_, HashMap<String, LabelId>> {
        let started = Instant::now();
        let guard = self.vertex_label_names.read();
        self.lock_metrics
            .record(CatalogLockOperation::VertexLabels, started);
        CatalogReadGuard {
            guard,
            metrics: &self.lock_metrics,
            acquired_at: Instant::now(),
            operation: CatalogLockOperation::VertexLabels,
        }
    }

    #[cfg(test)]
    pub(crate) fn read_edge_label_names(&self) -> CatalogReadGuard<'_, HashMap<String, LabelId>> {
        let started = Instant::now();
        let guard = self.edge_label_names.read();
        self.lock_metrics
            .record(CatalogLockOperation::EdgeLabels, started);
        CatalogReadGuard {
            guard,
            metrics: &self.lock_metrics,
            acquired_at: Instant::now(),
            operation: CatalogLockOperation::EdgeLabels,
        }
    }

    #[cfg(test)]
    pub(crate) fn read_vertex_counter(&self) -> CatalogReadGuard<'_, LabelId> {
        let started = Instant::now();
        let guard = self.vertex_label_counter.read();
        self.lock_metrics
            .record(CatalogLockOperation::VertexCounter, started);
        CatalogReadGuard {
            guard,
            metrics: &self.lock_metrics,
            acquired_at: Instant::now(),
            operation: CatalogLockOperation::VertexCounter,
        }
    }

    #[cfg(test)]
    pub(crate) fn read_edge_counter(&self) -> CatalogReadGuard<'_, LabelId> {
        let started = Instant::now();
        let guard = self.edge_label_counter.read();
        self.lock_metrics
            .record(CatalogLockOperation::EdgeCounter, started);
        CatalogReadGuard {
            guard,
            metrics: &self.lock_metrics,
            acquired_at: Instant::now(),
            operation: CatalogLockOperation::EdgeCounter,
        }
    }

    fn write_vertex_counter(&self) -> CatalogWriteGuard<'_, LabelId> {
        let started = Instant::now();
        let guard = self.vertex_label_counter.write();
        self.lock_metrics
            .record(CatalogLockOperation::VertexCounter, started);
        CatalogWriteGuard {
            guard,
            metrics: &self.lock_metrics,
            acquired_at: Instant::now(),
            operation: CatalogLockOperation::VertexCounter,
        }
    }

    fn write_edge_counter(&self) -> CatalogWriteGuard<'_, LabelId> {
        let started = Instant::now();
        let guard = self.edge_label_counter.write();
        self.lock_metrics
            .record(CatalogLockOperation::EdgeCounter, started);
        CatalogWriteGuard {
            guard,
            metrics: &self.lock_metrics,
            acquired_at: Instant::now(),
            operation: CatalogLockOperation::EdgeCounter,
        }
    }

    fn write_edge_label_index(&self) -> CatalogWriteGuard<'_, HashMap<LabelId, Vec<EdgeTableKey>>> {
        let started = Instant::now();
        let guard = self.edge_label_index.write();
        self.lock_metrics
            .record(CatalogLockOperation::EdgeLabelIndex, started);
        CatalogWriteGuard {
            guard,
            metrics: &self.lock_metrics,
            acquired_at: Instant::now(),
            operation: CatalogLockOperation::EdgeLabelIndex,
        }
    }

    fn write_vertex_tables(
        &self,
    ) -> CatalogWriteGuard<'_, HashMap<LabelId, Arc<ShardedVertexTable>>> {
        let started = Instant::now();
        let guard = self.vertex_tables.write();
        self.lock_metrics
            .record(CatalogLockOperation::VertexTables, started);
        CatalogWriteGuard {
            guard,
            metrics: &self.lock_metrics,
            acquired_at: Instant::now(),
            operation: CatalogLockOperation::VertexTables,
        }
    }

    pub(crate) fn read_edge_tables(
        &self,
    ) -> CatalogReadGuard<'_, HashMap<EdgeTableKey, Arc<RwLock<EdgeStore>>>> {
        let started = Instant::now();
        let guard = self.edge_tables.read();
        self.lock_metrics
            .record(CatalogLockOperation::EdgeTables, started);
        CatalogReadGuard {
            guard,
            metrics: &self.lock_metrics,
            acquired_at: Instant::now(),
            operation: CatalogLockOperation::EdgeTables,
        }
    }

    #[cfg(test)]
    pub(crate) fn test_read_edge_tables(
        &self,
    ) -> CatalogReadGuard<'_, HashMap<EdgeTableKey, Arc<RwLock<EdgeStore>>>> {
        self.read_edge_tables()
    }

    fn write_edge_tables(
        &self,
    ) -> CatalogWriteGuard<'_, HashMap<EdgeTableKey, Arc<RwLock<EdgeStore>>>> {
        let started = Instant::now();
        let guard = self.edge_tables.write();
        self.lock_metrics
            .record(CatalogLockOperation::EdgeTables, started);
        CatalogWriteGuard {
            guard,
            metrics: &self.lock_metrics,
            acquired_at: Instant::now(),
            operation: CatalogLockOperation::EdgeTables,
        }
    }

    #[cfg(test)]
    pub(crate) fn vertex_label_id(&self, name: &str) -> Option<LabelId> {
        self.vertex_label_id_for_name(name)
    }

    #[cfg(test)]
    pub(crate) fn vertex_label_id_for_name(&self, name: &str) -> Option<LabelId> {
        self.read_vertex_label_names().get(name).copied()
    }

    fn write_vertex_label_names(&self) -> CatalogWriteGuard<'_, HashMap<String, LabelId>> {
        let started = Instant::now();
        let guard = self.vertex_label_names.write();
        self.lock_metrics
            .record(CatalogLockOperation::VertexLabels, started);
        CatalogWriteGuard {
            guard,
            metrics: &self.lock_metrics,
            acquired_at: Instant::now(),
            operation: CatalogLockOperation::VertexLabels,
        }
    }

    fn write_edge_label_names(&self) -> CatalogWriteGuard<'_, HashMap<String, LabelId>> {
        let started = Instant::now();
        let guard = self.edge_label_names.write();
        self.lock_metrics
            .record(CatalogLockOperation::EdgeLabels, started);
        CatalogWriteGuard {
            guard,
            metrics: &self.lock_metrics,
            acquired_at: Instant::now(),
            operation: CatalogLockOperation::EdgeLabels,
        }
    }

    #[cfg(test)]
    pub(crate) fn edge_label_id(&self, name: &str) -> Option<LabelId> {
        self.read_edge_label_names().get(name).copied()
    }

    fn read_edge_label_index(&self) -> CatalogReadGuard<'_, HashMap<LabelId, Vec<EdgeTableKey>>> {
        let started = Instant::now();
        let guard = self.edge_label_index.read();
        self.lock_metrics
            .record(CatalogLockOperation::EdgeLabelIndex, started);
        CatalogReadGuard {
            guard,
            metrics: &self.lock_metrics,
            acquired_at: Instant::now(),
            operation: CatalogLockOperation::EdgeLabelIndex,
        }
    }

    #[cfg(test)]
    pub(crate) fn test_read_edge_label_index(
        &self,
    ) -> CatalogReadGuard<'_, HashMap<LabelId, Vec<EdgeTableKey>>> {
        self.read_edge_label_index()
    }

    pub(crate) fn lock_metrics(&self) -> CatalogLockMetricsSnapshot {
        self.lock_metrics.snapshot()
    }

    pub(crate) fn catalog_read_snapshot(&self) -> CatalogReadSnapshot<'_> {
        CatalogReadSnapshot { store: self }
    }

    /// Execute a read operation while the catalog guard remains internal to
    /// the catalog. Callers cannot retain a raw table guard across domains.
    pub(crate) fn with_vertex_tables<R>(
        &self,
        operation: impl FnOnce(&HashMap<LabelId, Arc<ShardedVertexTable>>) -> R,
    ) -> R {
        let tables = self.read_vertex_tables();
        operation(&tables)
    }

    pub(crate) fn with_edge_tables<R>(
        &self,
        operation: impl FnOnce(&HashMap<EdgeTableKey, Arc<RwLock<EdgeStore>>>) -> R,
    ) -> R {
        let tables = self.read_edge_tables();
        operation(&tables)
    }

    pub(crate) fn with_vertex_tables_mut<R>(
        &self,
        operation: impl FnOnce(&mut HashMap<LabelId, Arc<ShardedVertexTable>>) -> StorageResult<R>,
    ) -> StorageResult<R> {
        let mut tables = self.write_vertex_tables();
        operation(&mut tables)
    }

    pub(crate) fn with_vertex_tables_mut_result<R, E>(
        &self,
        operation: impl FnOnce(&mut HashMap<LabelId, Arc<ShardedVertexTable>>) -> Result<R, E>,
    ) -> Result<R, E> {
        let mut tables = self.write_vertex_tables();
        operation(&mut tables)
    }

    pub(crate) fn with_edge_label_index<R>(
        &self,
        operation: impl FnOnce(&HashMap<LabelId, Vec<EdgeTableKey>>) -> R,
    ) -> R {
        let index = self.read_edge_label_index();
        operation(&index)
    }

    /// Acquire every catalog registry in the documented global order. This
    /// is reserved for atomic schema/undo/recovery operations that update
    /// multiple registries together.
    pub(crate) fn catalog_write_set(&self) -> CatalogWriteSet<'_> {
        CatalogWriteSet {
            vertex_label_names: self.write_vertex_label_names(),
            edge_label_names: self.write_edge_label_names(),
            _vertex_label_counter: self.write_vertex_counter(),
            _edge_label_counter: self.write_edge_counter(),
            vertex_tables: self.write_vertex_tables(),
            edge_tables: self.write_edge_tables(),
            _edge_label_index: self.write_edge_label_index(),
        }
    }

    pub(crate) fn with_vertex_table_mut<R>(
        &self,
        label: LabelId,
        operation: impl FnOnce(&Arc<ShardedVertexTable>) -> StorageResult<R>,
    ) -> StorageResult<R> {
        let tables = self.write_vertex_tables();
        let table = tables
            .get(&label)
            .ok_or_else(|| StorageError::label_not_found(format!("vertex label {}", label)))?;
        operation(table)
    }

    pub(crate) fn edge_partition_keys(
        &self,
        edge_label: LabelId,
    ) -> StorageResult<Vec<EdgeTableKey>> {
        self.read_edge_label_index()
            .get(&edge_label)
            .cloned()
            .ok_or_else(|| StorageError::label_not_found(format!("edge label {}", edge_label)))
    }

    /// Iterate over all partitions of a given edge label, calling `operation` on each
    /// with only the per-table write lock held (no catalog write lock).
    ///
    /// Uses scatter-gather: all Arcs are collected under a brief catalog read lock,
    /// then each table is locked individually so different edge labels' partitions
    /// can be mutated concurrently. Partitions run in parallel (rayon); the closure
    /// must be `Sync` and its result `Send`, and results preserve partition order.
    pub(crate) fn for_each_edge_partition_mut<R: Send>(
        &self,
        edge_label: LabelId,
        operation: impl Fn(EdgeTableKey, &mut EdgeStore) -> StorageResult<R> + Sync,
    ) -> StorageResult<Vec<R>> {
        use rayon::prelude::*;

        let keys = self.edge_partition_keys(edge_label)?;
        let arcs: Vec<(EdgeTableKey, Arc<RwLock<EdgeStore>>)> = {
            let guard = self.edge_tables.read();
            keys.iter()
                .filter_map(|key| guard.get(key).map(|arc| (*key, arc.clone())))
                .collect()
        };
        arcs.par_iter()
            .map(|(key, arc)| {
                let mut table = arc.write();
                operation(*key, &mut table)
            })
            .collect()
    }

    /// Iterate over all partitions of all edge labels, calling `operation` on each
    /// with only the per-table write lock held (no catalog write lock).
    ///
    /// Uses scatter-gather: all EdgeTableKeys are collected from the edge_label_index
    /// under a brief catalog read lock, then each table is locked individually so
    /// partitions from different edge labels can be mutated concurrently.
    ///
    /// The operation runs in parallel over the partitions (rayon) because each
    /// partition is an independent lock domain: different edge labels' tables
    /// are never touched by the same call. The closure must be `Sync` and its
    /// result `Send`; results preserve partition order.
    pub(crate) fn for_all_edge_partitions_mut<R: Send>(
        &self,
        operation: impl Fn(EdgeTableKey, &mut EdgeStore) -> StorageResult<R> + Sync,
    ) -> StorageResult<Vec<R>> {
        use rayon::prelude::*;

        let keys: Vec<EdgeTableKey> = {
            let index = self.read_edge_label_index();
            index.values().flat_map(|v| v.iter()).copied().collect()
        };
        let arcs: Vec<(EdgeTableKey, Arc<RwLock<EdgeStore>>)> = keys
            .into_iter()
            .filter_map(|key| {
                let arc = {
                    let guard = self.edge_tables.read();
                    guard.get(&key).cloned()
                };
                arc.map(|arc| (key, arc))
            })
            .collect();
        arcs.par_iter()
            .map(|(key, arc)| {
                let mut table = arc.write();
                operation(*key, &mut table)
            })
            .collect()
    }

    /// Read a single edge table by key, holding only the table-level lock (not the catalog lock)
    /// during the operation. The catalog lock is released after the table lookup.
    pub(crate) fn with_single_edge_table<R>(
        &self,
        key: &EdgeTableKey,
        operation: impl FnOnce(&EdgeStore) -> StorageResult<R>,
    ) -> StorageResult<R> {
        let arc = {
            let guard = self.edge_tables.read();
            guard
                .get(key)
                .ok_or_else(|| StorageError::label_not_found(format!("edge partition {:?}", key)))?
                .clone()
        };
        let guard = arc.read();
        operation(&guard)
    }

    /// Try to get a mutable reference to a single edge table by key.
    /// Returns `None` if the key does not exist (no catalog lock held during operation).
    pub(crate) fn try_get_edge_table_mut(
        &self,
        key: &EdgeTableKey,
    ) -> Option<std::sync::Arc<parking_lot::RwLock<EdgeStore>>> {
        let guard = self.edge_tables.read();
        guard.get(key).cloned()
    }

    /// Mutate a single edge table by key, holding only the table-level lock during the operation.
    pub(crate) fn with_single_edge_table_mut<R>(
        &self,
        key: &EdgeTableKey,
        operation: impl FnOnce(&mut EdgeStore) -> StorageResult<R>,
    ) -> StorageResult<R> {
        let arc = {
            let guard = self.edge_tables.read();
            guard
                .get(key)
                .ok_or_else(|| StorageError::label_not_found(format!("edge partition {:?}", key)))?
                .clone()
        };
        let mut guard = arc.write();
        operation(&mut guard)
    }

    #[cfg(test)]
    pub(crate) fn catalog_counts(&self) -> (usize, usize) {
        (
            self.vertex_tables.read().len(),
            self.edge_tables.read().len(),
        )
    }

    pub(crate) fn register_vertex_type(
        &self,
        storage_name: String,
        requested_label: Option<LabelId>,
        table: impl FnOnce(LabelId) -> StorageResult<ShardedVertexTable>,
    ) -> StorageResult<LabelId> {
        let mut names = self.write_vertex_label_names();
        if names.contains_key(&storage_name) {
            return Err(StorageError::label_already_exists(storage_name));
        }
        let mut counter = self.write_vertex_counter();
        let mut tables = self.write_vertex_tables();
        let label = requested_label.unwrap_or(*counter);
        if tables.contains_key(&label) {
            return Err(StorageError::label_already_exists(format!(
                "label_id {}",
                label
            )));
        }
        *counter = (*counter).max(label.saturating_add(1));
        tables.insert(label, Arc::new(table(label)?));
        names.insert(storage_name, label);
        Ok(label)
    }

    pub(crate) fn register_edge_type(
        &self,
        storage_name: String,
        requested_label: Option<LabelId>,
        src_label: LabelId,
        dst_label: LabelId,
        table: impl FnOnce(LabelId) -> StorageResult<EdgeStore>,
    ) -> StorageResult<LabelId> {
        let mut names = self.write_edge_label_names();
        if names.contains_key(&storage_name) {
            return Err(StorageError::label_already_exists(storage_name));
        }
        let mut counter = self.write_edge_counter();
        let vertex_tables = self.read_vertex_tables();
        if src_label != 0 && !vertex_tables.contains_key(&src_label) {
            return Err(StorageError::label_not_found(format!(
                "source label {}",
                src_label
            )));
        }
        if dst_label != 0 && !vertex_tables.contains_key(&dst_label) {
            return Err(StorageError::label_not_found(format!(
                "destination label {}",
                dst_label
            )));
        }
        let mut edge_tables = self.write_edge_tables();
        let mut index = self.write_edge_label_index();
        let label = requested_label.unwrap_or(*counter);
        let key = EdgeTableKey::new(src_label, dst_label, label);
        if edge_tables.contains_key(&key) {
            return Err(StorageError::label_already_exists(format!(
                "label_id {}",
                label
            )));
        }
        *counter = (*counter).max(label.saturating_add(1));
        edge_tables.insert(key, Arc::new(RwLock::new(table(label)?)));
        index.entry(label).or_default().push(key);
        names.insert(storage_name, label);
        Ok(label)
    }

    pub(crate) fn drop_vertex_type(&self, name: &str) -> StorageResult<LabelId> {
        let mut vertex_names = self.write_vertex_label_names();
        let mut edge_names = self.write_edge_label_names();
        let mut vertex_tables = self.write_vertex_tables();
        let mut edge_tables = self.write_edge_tables();
        let mut edge_index = self.write_edge_label_index();
        let label = *vertex_names
            .get(name)
            .ok_or_else(|| StorageError::label_not_found(name.to_string()))?;
        vertex_names.remove(name);
        vertex_tables.remove(&label);
        let keys: Vec<_> = edge_tables
            .keys()
            .filter(|key| key.src_label == label || key.dst_label == label)
            .copied()
            .collect();
        for key in keys {
            edge_tables.remove(&key);
            if let Some(indexed_keys) = edge_index.get_mut(&key.edge_label) {
                indexed_keys.retain(|candidate| *candidate != key);
                if indexed_keys.is_empty() {
                    edge_index.remove(&key.edge_label);
                    edge_names.retain(|_, candidate| *candidate != key.edge_label);
                }
            }
        }
        Ok(label)
    }

    pub(crate) fn drop_edge_type(&self, name: &str) -> StorageResult<LabelId> {
        let mut names = self.write_edge_label_names();
        let mut tables = self.write_edge_tables();
        let mut index = self.write_edge_label_index();
        let label = *names
            .get(name)
            .ok_or_else(|| StorageError::label_not_found(name.to_string()))?;
        names.remove(name);
        for key in index.remove(&label).unwrap_or_default() {
            tables.remove(&key);
        }
        Ok(label)
    }

    pub(crate) fn drop_vertex_type_by_label(&self, label: LabelId) -> StorageResult<()> {
        let mut vertex_names = self.write_vertex_label_names();
        let mut edge_names = self.write_edge_label_names();
        let mut vertex_tables = self.write_vertex_tables();
        let mut edge_tables = self.write_edge_tables();
        let mut edge_index = self.write_edge_label_index();
        if vertex_tables.remove(&label).is_none() {
            return Err(StorageError::label_not_found(format!(
                "vertex label {}",
                label
            )));
        }
        vertex_names.retain(|_, candidate| *candidate != label);
        let keys: Vec<_> = edge_tables
            .keys()
            .filter(|key| key.src_label == label || key.dst_label == label)
            .copied()
            .collect();
        for key in keys {
            edge_tables.remove(&key);
            if let Some(indexed_keys) = edge_index.get_mut(&key.edge_label) {
                indexed_keys.retain(|candidate| *candidate != key);
                if indexed_keys.is_empty() {
                    edge_index.remove(&key.edge_label);
                    edge_names.retain(|_, candidate| *candidate != key.edge_label);
                }
            }
        }
        Ok(())
    }

    pub(crate) fn drop_edge_partition(&self, key: EdgeTableKey) -> StorageResult<()> {
        let mut names = self.write_edge_label_names();
        let mut tables = self.write_edge_tables();
        let mut index = self.write_edge_label_index();
        if tables.remove(&key).is_none() {
            return Ok(());
        }
        if let Some(keys) = index.get_mut(&key.edge_label) {
            keys.retain(|candidate| *candidate != key);
            if keys.is_empty() {
                index.remove(&key.edge_label);
                names.retain(|_, label| *label != key.edge_label);
            }
        }
        Ok(())
    }

    pub(crate) fn with_edge_partition_mut<R>(
        &self,
        key: EdgeTableKey,
        template_key: EdgeTableKey,
        create: impl FnOnce(&EdgeStore) -> StorageResult<EdgeStore>,
        operation: impl FnOnce(&mut EdgeStore) -> StorageResult<R>,
    ) -> StorageResult<R> {
        let table_arc = {
            // Phase 1: read lock (fast path — partition already exists)
            let guard = self.edge_tables.read();
            if let Some(arc) = guard.get(&key) {
                arc.clone()
            } else {
                drop(guard);
                // Phase 2: write lock (slow path — create partition)
                let mut tables = self.write_edge_tables();
                let mut index = self.write_edge_label_index();
                if !tables.contains_key(&key) {
                    let table = {
                        let template = tables.get(&template_key).ok_or_else(|| {
                            StorageError::label_not_found(format!("edge label {}", key.edge_label))
                        })?;
                        let guard = template.read();
                        create(&guard)?
                    };
                    tables.insert(key, Arc::new(RwLock::new(table)));
                    let indexed_keys = index.entry(key.edge_label).or_default();
                    if !indexed_keys.contains(&key) {
                        indexed_keys.push(key);
                    }
                }
                tables
                    .get(&key)
                    .ok_or_else(|| {
                        StorageError::label_not_found(format!("edge partition {:?}", key))
                    })?
                    .clone()
            }
        };
        let mut guard = table_arc.write();
        operation(&mut guard)
    }
}

impl Default for GraphDataStore {
    fn default() -> Self {
        Self::new()
    }
}
