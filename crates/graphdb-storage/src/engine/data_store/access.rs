//! Metered catalog read/write accessors and the closure-style with_* facade.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use parking_lot::RwLock;

use crate::edge::EdgeStore;
use crate::vertex::ShardedVertexTable;
use graphdb_core::types::LabelId;
use graphdb_core::{StorageError, StorageResult};

use super::lock_metrics::{
    CatalogLockMetricsSnapshot, CatalogLockOperation, CatalogReadGuard, CatalogReadSnapshot,
    CatalogWriteGuard, CatalogWriteSet, TableLockMetricsSnapshot,
};
use super::GraphDataStore;

impl GraphDataStore {
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

    pub(super) fn write_vertex_counter(&self) -> CatalogWriteGuard<'_, LabelId> {
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

    pub(super) fn write_edge_counter(&self) -> CatalogWriteGuard<'_, LabelId> {
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

    pub(super) fn write_edge_label_index(
        &self,
    ) -> CatalogWriteGuard<'_, HashMap<LabelId, Vec<super::EdgeTableKey>>> {
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

    pub(super) fn write_vertex_tables(
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
    ) -> CatalogReadGuard<'_, HashMap<super::EdgeTableKey, Arc<RwLock<EdgeStore>>>> {
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
    ) -> CatalogReadGuard<'_, HashMap<super::EdgeTableKey, Arc<RwLock<EdgeStore>>>> {
        self.read_edge_tables()
    }

    pub(super) fn write_edge_tables(
        &self,
    ) -> CatalogWriteGuard<'_, HashMap<super::EdgeTableKey, Arc<RwLock<EdgeStore>>>> {
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

    pub(super) fn write_vertex_label_names(
        &self,
    ) -> CatalogWriteGuard<'_, HashMap<String, LabelId>> {
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

    pub(super) fn write_edge_label_names(&self) -> CatalogWriteGuard<'_, HashMap<String, LabelId>> {
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

    pub(super) fn read_edge_label_index(
        &self,
    ) -> CatalogReadGuard<'_, HashMap<LabelId, Vec<super::EdgeTableKey>>> {
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
    ) -> CatalogReadGuard<'_, HashMap<LabelId, Vec<super::EdgeTableKey>>> {
        self.read_edge_label_index()
    }

    pub(crate) fn lock_metrics(&self) -> CatalogLockMetricsSnapshot {
        self.lock_metrics.snapshot()
    }

    pub(crate) fn table_lock_metrics(&self) -> TableLockMetricsSnapshot {
        self.table_lock_metrics.snapshot()
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
        operation: impl FnOnce(&HashMap<super::EdgeTableKey, Arc<RwLock<EdgeStore>>>) -> R,
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
        operation: impl FnOnce(&HashMap<LabelId, Vec<super::EdgeTableKey>>) -> R,
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

    #[cfg(test)]
    pub(crate) fn catalog_counts(&self) -> (usize, usize) {
        (
            self.vertex_tables.read().len(),
            self.edge_tables.read().len(),
        )
    }
}
