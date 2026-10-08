//! Edge partition scatter-gather access: parallel mutation, lazy creation
//! and per-partition lock helpers.

use std::sync::Arc;
use std::time::Instant;

use parking_lot::RwLock;

use crate::edge::EdgeStore;
use linkrs_core::types::LabelId;
use linkrs_core::{StorageError, StorageResult};

use super::GraphDataStore;

impl GraphDataStore {
    pub(crate) fn edge_partition_keys(
        &self,
        edge_label: LabelId,
    ) -> StorageResult<Vec<super::EdgeTableKey>> {
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
        operation: impl Fn(super::EdgeTableKey, &mut EdgeStore) -> StorageResult<R> + Sync,
    ) -> StorageResult<Vec<R>> {
        use rayon::prelude::*;

        let keys = self.edge_partition_keys(edge_label)?;
        let arcs: Vec<(super::EdgeTableKey, Arc<RwLock<EdgeStore>>)> = {
            let guard = self.edge_tables.read();
            keys.iter()
                .filter_map(|key| guard.get(key).map(|arc| (*key, arc.clone())))
                .collect()
        };
        arcs.par_iter()
            .map(|(key, arc)| {
                let started = Instant::now();
                let mut table = arc.write();
                self.table_lock_metrics.record(true, started);
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
        operation: impl Fn(super::EdgeTableKey, &mut EdgeStore) -> StorageResult<R> + Sync,
    ) -> StorageResult<Vec<R>> {
        use rayon::prelude::*;

        let keys: Vec<super::EdgeTableKey> = {
            let index = self.read_edge_label_index();
            index.values().flat_map(|v| v.iter()).copied().collect()
        };
        let arcs: Vec<(super::EdgeTableKey, Arc<RwLock<EdgeStore>>)> = keys
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
                let started = Instant::now();
                let mut table = arc.write();
                self.table_lock_metrics.record(true, started);
                operation(*key, &mut table)
            })
            .collect()
    }

    /// Collect the live partition handles for one edge label.
    ///
    /// Scatter-gather entry for read fan-out: both the label index and the
    /// table registry are held only for the handle collection, then released
    /// before any table lock is taken. Callers pin the returned `Arc`s across
    /// their table reads, so a concurrent DDL drop cannot free a table under
    /// them; such a read observes the pre-drop snapshot. Unknown labels
    /// return empty instead of an error, matching filter-style traversal.
    pub(crate) fn matching_edge_partition_arcs(
        &self,
        edge_label: LabelId,
    ) -> Vec<Arc<RwLock<EdgeStore>>> {
        let keys: Vec<super::EdgeTableKey> =
            self.with_edge_label_index(|index| index.get(&edge_label).cloned().unwrap_or_default());
        if keys.is_empty() {
            return Vec::new();
        }
        let guard = self.edge_tables.read();
        keys.iter()
            .filter_map(|key| guard.get(key).cloned())
            .collect()
    }

    /// Take one partition read lock with contention accounting.
    ///
    /// Used by fan-out loops that already hold collected handles outside the
    /// catalog lock, so each table acquisition is timed on its own.
    pub(crate) fn read_edge_table<'a>(
        &self,
        arc: &'a Arc<RwLock<EdgeStore>>,
    ) -> parking_lot::RwLockReadGuard<'a, EdgeStore> {
        let started = Instant::now();
        let guard = arc.read();
        self.table_lock_metrics.record(false, started);
        guard
    }

    /// Take one partition write lock with contention accounting.
    pub(crate) fn write_edge_table<'a>(
        &self,
        arc: &'a Arc<RwLock<EdgeStore>>,
    ) -> parking_lot::RwLockWriteGuard<'a, EdgeStore> {
        let started = Instant::now();
        let guard = arc.write();
        self.table_lock_metrics.record(true, started);
        guard
    }

    /// Read a single edge table by key, holding only the table-level lock (not the catalog lock)
    /// during the operation. The catalog lock is released after the table lookup.
    pub(crate) fn with_single_edge_table<R>(
        &self,
        key: &super::EdgeTableKey,
        operation: impl FnOnce(&EdgeStore) -> StorageResult<R>,
    ) -> StorageResult<R> {
        let arc = {
            let guard = self.edge_tables.read();
            guard
                .get(key)
                .ok_or_else(|| StorageError::label_not_found(format!("edge partition {:?}", key)))?
                .clone()
        };
        let guard = self.read_edge_table(&arc);
        operation(&guard)
    }

    /// Try to get a mutable reference to a single edge table by key.
    /// Returns `None` if the key does not exist (no catalog lock held during operation).
    pub(crate) fn try_get_edge_table_mut(
        &self,
        key: &super::EdgeTableKey,
    ) -> Option<std::sync::Arc<parking_lot::RwLock<EdgeStore>>> {
        let guard = self.edge_tables.read();
        guard.get(key).cloned()
    }

    /// Mutate a single edge table by key, holding only the table-level lock during the operation.
    pub(crate) fn with_single_edge_table_mut<R>(
        &self,
        key: &super::EdgeTableKey,
        operation: impl FnOnce(&mut EdgeStore) -> StorageResult<R>,
    ) -> StorageResult<R> {
        let arc = {
            let guard = self.edge_tables.read();
            guard
                .get(key)
                .ok_or_else(|| StorageError::label_not_found(format!("edge partition {:?}", key)))?
                .clone()
        };
        let mut guard = self.write_edge_table(&arc);
        operation(&mut guard)
    }

    pub(crate) fn with_edge_partition_mut<R>(
        &self,
        key: super::EdgeTableKey,
        template_key: super::EdgeTableKey,
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
        let mut guard = self.write_edge_table(&table_arc);
        operation(&mut guard)
    }
}
