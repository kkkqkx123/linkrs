use std::collections::HashMap;
use std::ops::{Deref, DerefMut};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use parking_lot::{RwLock, RwLockReadGuard, RwLockWriteGuard};

use crate::core::types::LabelId;
use crate::core::{StorageError, StorageResult};
use crate::storage::edge::EdgeStore;
use crate::storage::vertex::VertexTable;

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
    vertex_tables: RwLock<HashMap<LabelId, VertexTable>>,
    edge_tables: RwLock<HashMap<EdgeTableKey, EdgeStore>>,
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

#[derive(Debug, Default)]
struct CatalogLockMetrics {
    acquisitions: AtomicU64,
    wait_nanos: AtomicU64,
    hold_nanos: AtomicU64,
    contended: AtomicU64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct CatalogLockMetricsSnapshot {
    pub acquisitions: u64,
    pub wait_nanos: u64,
    pub hold_nanos: u64,
    pub contended: u64,
}

pub(crate) struct CatalogReadGuard<'a, T> {
    guard: RwLockReadGuard<'a, T>,
    metrics: &'a CatalogLockMetrics,
    acquired_at: Instant,
}

impl<T> Deref for CatalogReadGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.guard
    }
}

impl<T> Drop for CatalogReadGuard<'_, T> {
    fn drop(&mut self) {
        self.metrics.hold_nanos.fetch_add(
            self.acquired_at.elapsed().as_nanos().min(u64::MAX as u128) as u64,
            Ordering::Relaxed,
        );
    }
}

pub(crate) struct CatalogWriteGuard<'a, T> {
    guard: RwLockWriteGuard<'a, T>,
    metrics: &'a CatalogLockMetrics,
    acquired_at: Instant,
}

#[allow(dead_code)]
pub(crate) struct CatalogWriteSet<'a> {
    pub vertex_label_names: CatalogWriteGuard<'a, HashMap<String, LabelId>>,
    pub edge_label_names: CatalogWriteGuard<'a, HashMap<String, LabelId>>,
    pub vertex_label_counter: CatalogWriteGuard<'a, LabelId>,
    pub edge_label_counter: CatalogWriteGuard<'a, LabelId>,
    pub vertex_tables: CatalogWriteGuard<'a, HashMap<LabelId, VertexTable>>,
    pub edge_tables: CatalogWriteGuard<'a, HashMap<EdgeTableKey, EdgeStore>>,
    pub edge_label_index: CatalogWriteGuard<'a, HashMap<LabelId, Vec<EdgeTableKey>>>,
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
        self.metrics.hold_nanos.fetch_add(
            self.acquired_at.elapsed().as_nanos().min(u64::MAX as u128) as u64,
            Ordering::Relaxed,
        );
    }
}

impl CatalogLockMetrics {
    fn record(&self, started: Instant) {
        let waited = started.elapsed();
        self.acquisitions.fetch_add(1, Ordering::Relaxed);
        self.wait_nanos.fetch_add(
            waited.as_nanos().min(u64::MAX as u128) as u64,
            Ordering::Relaxed,
        );
        if waited >= std::time::Duration::from_micros(1) {
            self.contended.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn snapshot(&self) -> CatalogLockMetricsSnapshot {
        CatalogLockMetricsSnapshot {
            acquisitions: self.acquisitions.load(Ordering::Relaxed),
            wait_nanos: self.wait_nanos.load(Ordering::Relaxed),
            hold_nanos: self.hold_nanos.load(Ordering::Relaxed),
            contended: self.contended.load(Ordering::Relaxed),
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
    pub(crate) fn read_vertex_tables(&self) -> CatalogReadGuard<'_, HashMap<LabelId, VertexTable>> {
        let started = Instant::now();
        let guard = self.vertex_tables.read();
        self.lock_metrics.record(started);
        CatalogReadGuard {
            guard,
            metrics: &self.lock_metrics,
            acquired_at: Instant::now(),
        }
    }

    fn read_vertex_label_names(&self) -> CatalogReadGuard<'_, HashMap<String, LabelId>> {
        let started = Instant::now();
        let guard = self.vertex_label_names.read();
        self.lock_metrics.record(started);
        CatalogReadGuard {
            guard,
            metrics: &self.lock_metrics,
            acquired_at: Instant::now(),
        }
    }

    fn read_edge_label_names(&self) -> CatalogReadGuard<'_, HashMap<String, LabelId>> {
        let started = Instant::now();
        let guard = self.edge_label_names.read();
        self.lock_metrics.record(started);
        CatalogReadGuard {
            guard,
            metrics: &self.lock_metrics,
            acquired_at: Instant::now(),
        }
    }

    fn read_vertex_counter(&self) -> CatalogReadGuard<'_, LabelId> {
        let started = Instant::now();
        let guard = self.vertex_label_counter.read();
        self.lock_metrics.record(started);
        CatalogReadGuard {
            guard,
            metrics: &self.lock_metrics,
            acquired_at: Instant::now(),
        }
    }

    fn read_edge_counter(&self) -> CatalogReadGuard<'_, LabelId> {
        let started = Instant::now();
        let guard = self.edge_label_counter.read();
        self.lock_metrics.record(started);
        CatalogReadGuard {
            guard,
            metrics: &self.lock_metrics,
            acquired_at: Instant::now(),
        }
    }

    fn write_vertex_counter(&self) -> CatalogWriteGuard<'_, LabelId> {
        let started = Instant::now();
        let guard = self.vertex_label_counter.write();
        self.lock_metrics.record(started);
        CatalogWriteGuard {
            guard,
            metrics: &self.lock_metrics,
            acquired_at: Instant::now(),
        }
    }

    fn write_edge_counter(&self) -> CatalogWriteGuard<'_, LabelId> {
        let started = Instant::now();
        let guard = self.edge_label_counter.write();
        self.lock_metrics.record(started);
        CatalogWriteGuard {
            guard,
            metrics: &self.lock_metrics,
            acquired_at: Instant::now(),
        }
    }

    fn write_edge_label_index(&self) -> CatalogWriteGuard<'_, HashMap<LabelId, Vec<EdgeTableKey>>> {
        let started = Instant::now();
        let guard = self.edge_label_index.write();
        self.lock_metrics.record(started);
        CatalogWriteGuard {
            guard,
            metrics: &self.lock_metrics,
            acquired_at: Instant::now(),
        }
    }

    pub(crate) fn write_vertex_tables(
        &self,
    ) -> CatalogWriteGuard<'_, HashMap<LabelId, VertexTable>> {
        let started = Instant::now();
        let guard = self.vertex_tables.write();
        self.lock_metrics.record(started);
        CatalogWriteGuard {
            guard,
            metrics: &self.lock_metrics,
            acquired_at: Instant::now(),
        }
    }

    pub(crate) fn read_edge_tables(
        &self,
    ) -> CatalogReadGuard<'_, HashMap<EdgeTableKey, EdgeStore>> {
        let started = Instant::now();
        let guard = self.edge_tables.read();
        self.lock_metrics.record(started);
        CatalogReadGuard {
            guard,
            metrics: &self.lock_metrics,
            acquired_at: Instant::now(),
        }
    }

    pub(crate) fn write_edge_tables(
        &self,
    ) -> CatalogWriteGuard<'_, HashMap<EdgeTableKey, EdgeStore>> {
        let started = Instant::now();
        let guard = self.edge_tables.write();
        self.lock_metrics.record(started);
        CatalogWriteGuard {
            guard,
            metrics: &self.lock_metrics,
            acquired_at: Instant::now(),
        }
    }

    #[cfg(test)]
    pub(crate) fn vertex_label_id(&self, name: &str) -> Option<LabelId> {
        self.read_vertex_label_names().get(name).copied()
    }

    pub(crate) fn write_vertex_label_names(
        &self,
    ) -> CatalogWriteGuard<'_, HashMap<String, LabelId>> {
        let started = Instant::now();
        let guard = self.vertex_label_names.write();
        self.lock_metrics.record(started);
        CatalogWriteGuard {
            guard,
            metrics: &self.lock_metrics,
            acquired_at: Instant::now(),
        }
    }

    pub(crate) fn write_edge_label_names(&self) -> CatalogWriteGuard<'_, HashMap<String, LabelId>> {
        let started = Instant::now();
        let guard = self.edge_label_names.write();
        self.lock_metrics.record(started);
        CatalogWriteGuard {
            guard,
            metrics: &self.lock_metrics,
            acquired_at: Instant::now(),
        }
    }

    #[cfg(test)]
    pub(crate) fn edge_label_id(&self, name: &str) -> Option<LabelId> {
        self.read_edge_label_names().get(name).copied()
    }

    pub(crate) fn read_edge_label_index(
        &self,
    ) -> CatalogReadGuard<'_, HashMap<LabelId, Vec<EdgeTableKey>>> {
        let started = Instant::now();
        let guard = self.edge_label_index.read();
        self.lock_metrics.record(started);
        CatalogReadGuard {
            guard,
            metrics: &self.lock_metrics,
            acquired_at: Instant::now(),
        }
    }

    pub(crate) fn lock_metrics(&self) -> CatalogLockMetricsSnapshot {
        self.lock_metrics.snapshot()
    }

    /// Acquire every catalog registry in the documented global order. This
    /// is reserved for atomic schema/undo/recovery operations that update
    /// multiple registries together.
    pub(crate) fn catalog_write_set(&self) -> CatalogWriteSet<'_> {
        CatalogWriteSet {
            vertex_label_names: self.write_vertex_label_names(),
            edge_label_names: self.write_edge_label_names(),
            vertex_label_counter: self.write_vertex_counter(),
            edge_label_counter: self.write_edge_counter(),
            vertex_tables: self.write_vertex_tables(),
            edge_tables: self.write_edge_tables(),
            edge_label_index: self.write_edge_label_index(),
        }
    }

    pub(crate) fn with_vertex_table_mut<R>(
        &self,
        label: LabelId,
        operation: impl FnOnce(&mut VertexTable) -> StorageResult<R>,
    ) -> StorageResult<R> {
        let mut tables = self.write_vertex_tables();
        let table = tables
            .get_mut(&label)
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

    pub(crate) fn with_edge_partitions_mut<R>(
        &self,
        edge_label: LabelId,
        operation: impl FnOnce(
            &mut HashMap<EdgeTableKey, EdgeStore>,
            &[EdgeTableKey],
        ) -> StorageResult<R>,
    ) -> StorageResult<R> {
        let keys = self.edge_partition_keys(edge_label)?;
        let mut tables = self.write_edge_tables();
        operation(&mut tables, &keys)
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
        table: impl FnOnce(LabelId) -> StorageResult<VertexTable>,
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
        tables.insert(label, table(label)?);
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
        edge_tables.insert(key, table(label)?);
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
        let mut tables = self.write_edge_tables();
        let mut index = self.write_edge_label_index();
        if !tables.contains_key(&key) {
            let table = {
                let template = tables.get(&template_key).ok_or_else(|| {
                    StorageError::label_not_found(format!("edge label {}", key.edge_label))
                })?;
                create(template)?
            };
            tables.insert(key, table);
            let indexed_keys = index.entry(key.edge_label).or_default();
            if !indexed_keys.contains(&key) {
                indexed_keys.push(key);
            }
        }
        let table = tables
            .get_mut(&key)
            .ok_or_else(|| StorageError::label_not_found(format!("edge partition {:?}", key)))?;
        operation(table)
    }

    pub(crate) fn verify_invariants(&self) -> StorageResult<()> {
        let vertex_names = self.read_vertex_label_names();
        let edge_names = self.read_edge_label_names();
        let vertex_counter = self.read_vertex_counter();
        let edge_counter = self.read_edge_counter();
        let vertex_tables = self.read_vertex_tables();
        let edge_tables = self.read_edge_tables();
        let edge_index = self.read_edge_label_index();

        for (name, label) in &*vertex_names {
            if !vertex_tables.contains_key(label) {
                return Err(StorageError::invalid_operation(format!(
                    "vertex label '{}' points to missing table {}",
                    name, label
                )));
            }
        }
        for (name, label) in &*edge_names {
            if !edge_index.contains_key(label) {
                return Err(StorageError::invalid_operation(format!(
                    "edge label '{}' has no indexed partitions",
                    name
                )));
            }
        }
        for (key, table) in &*edge_tables {
            if !edge_index
                .get(&key.edge_label)
                .is_some_and(|keys| keys.contains(key))
            {
                return Err(StorageError::invalid_operation(format!(
                    "edge table {:?} is missing from reverse index",
                    key
                )));
            }
            if table.schema().label_id != key.edge_label {
                return Err(StorageError::invalid_operation(format!(
                    "edge table {:?} has mismatched schema label",
                    key
                )));
            }
        }
        if vertex_tables.keys().any(|label| *label >= *vertex_counter)
            || edge_index.keys().any(|label| *label >= *edge_counter)
        {
            return Err(StorageError::invalid_operation(
                "label counter does not cover registered labels".to_string(),
            ));
        }
        Ok(())
    }
}

impl Default for GraphDataStore {
    fn default() -> Self {
        Self::new()
    }
}
