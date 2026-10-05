//! Insert pipeline, identity allocation, and primary-key mirror writes.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use super::VertexTable;
use crate::vertex::{primary_key_mirror_value, IdKey, PkLookup, Timestamp};
use graphdb_core::{StorageError, StorageResult, Value};

impl VertexTable {
    pub fn insert(
        &self,
        external_id: &str,
        properties: &[(Arc<str>, Value)],
        ts: Timestamp,
    ) -> StorageResult<u32> {
        self.insert_by_key(IdKey::Text(external_id.to_string()), properties, ts)
    }

    pub fn insert_by_i64(
        &self,
        external_id: i64,
        properties: &[(Arc<str>, Value)],
        ts: Timestamp,
    ) -> StorageResult<u32> {
        self.insert_by_key(IdKey::Int(external_id), properties, ts)
    }

    /// Read-only validation and normalization for one insert: key shape,
    /// per-property type checks and the primary-key mirror fill. No global
    /// state is touched, so staging paths call this before buffering the
    /// row and offline paths call it before applying.
    pub fn prepare_insert(
        &self,
        key: &IdKey,
        properties: &[(Arc<str>, Value)],
    ) -> StorageResult<Vec<(std::sync::Arc<str>, Value)>> {
        if !self.is_open.load(Ordering::Acquire) {
            return Err(StorageError::storage_not_open());
        }

        Self::validate_key_shape(key)?;

        let mut converted: Vec<(std::sync::Arc<str>, Value)> = Vec::with_capacity(properties.len());
        for (name, value) in properties {
            let prop_idx = self
                .property_index_cache
                .get(name)
                .ok_or_else(|| StorageError::column_not_found(name.to_string()))?;
            let prop_def = &self.schema.properties[*prop_idx];

            if value.data_type() != prop_def.data_type {
                let converted_val = value.try_cast_to(&prop_def.data_type)?;
                converted.push((name.clone(), converted_val));
            } else {
                converted.push((name.clone(), value.clone()));
            }
        }
        self.apply_primary_key_mirror(key, converted)
    }

    /// Apply one prepared insert: identity step (duplicate check, id
    /// allocation or reserved-id binding, pending mark) under the
    /// identity latch, then the data step on the column segments, then the
    /// publish step flipping the pending mark into a creation stamp. Readers
    /// only observe the creation stamp, so a row becomes visible exactly
    /// when its column data is present. `reserved` declares a local id
    /// obtained earlier from [`Self::reserve_identity`] for this key; the
    /// commit binds the key to that exact id, and a mismatch with an
    /// existing (tombstoned) binding is rejected so rows already referenced
    /// by other structures cannot silently change identity.
    pub fn apply_insert(
        &self,
        key: IdKey,
        converted: &[(Arc<str>, Value)],
        ts: Timestamp,
        reserved: Option<u32>,
    ) -> StorageResult<u32> {
        if !self.is_open.load(Ordering::Acquire) {
            if let Some(id) = reserved {
                self.id_indexer.release_reserved(id);
            }
            return Err(StorageError::storage_not_open());
        }

        // Identity step: allocate or reuse the slot and mark it pending.
        // Pending slots are invisible to every liveness probe, so the data
        // step below runs without exposing a half-written row.
        let (internal_id, fresh) = {
            let mut stamps = self.timestamps.write();
            if let Some(existing) = self.id_indexer.get_index(&key) {
                if stamps.is_pending(existing) {
                    if let Some(id) = reserved {
                        self.id_indexer.release_reserved(id);
                    }
                    return Err(StorageError::vertex_already_exists(format!("{:?}", key)));
                }
                if stamps.is_valid(existing, ts) {
                    if let Some(id) = reserved {
                        self.id_indexer.release_reserved(id);
                    }
                    return Err(StorageError::vertex_already_exists(format!("{:?}", key)));
                }
                if matches!(reserved, Some(id) if id != existing) {
                    if let Some(id) = reserved {
                        self.id_indexer.release_reserved(id);
                    }
                    return Err(StorageError::vertex_already_exists(format!("{:?}", key)));
                }
                stamps.mark_pending(existing);
                (existing, false)
            } else {
                let bound = match reserved {
                    Some(id) => self.id_indexer.register_reserved(key, id).map(|()| id),
                    None => self.id_indexer.insert(key),
                };
                match bound {
                    Ok(internal_id) => {
                        stamps.mark_pending(internal_id);
                        (internal_id, true)
                    }
                    Err(error) => {
                        if let Some(id) = reserved {
                            self.id_indexer.release_reserved(id);
                        }
                        return Err(error);
                    }
                }
            }
        };

        // Data step: segment-latched column writes, identity latch released.
        if let Err(error) = self
            .columns
            .set_versioned(internal_id as usize, converted, ts)
        {
            self.abort_pending_insert(internal_id, fresh);
            return Err(error);
        }
        // Publish step: turn the pending mark into a creation stamp.
        {
            let mut stamps = self.timestamps.write();
            if stamps.is_pending(internal_id) {
                stamps.insert(internal_id, ts);
            } else {
                return Err(StorageError::invalid_operation(
                    "concurrent insert conflict on the same vertex id".to_string(),
                ));
            }
        }
        Ok(internal_id)
    }

    fn abort_pending_insert(&self, internal_id: u32, fresh: bool) {
        if fresh {
            if let Some(key) = self.id_indexer.get_key(internal_id) {
                self.id_indexer.remove(&key);
            }
            self.timestamps.write().invalidate_slot(internal_id);
        } else {
            self.timestamps.write().clear_pending(internal_id);
        }
    }

    /// Roll back one applied insert: drop the key back and invalidate the
    /// timestamp slot. The column version entries of a never-visible row
    /// stay unreachable until the recycled id reuses the slot.
    pub(crate) fn undo_apply_insert(&self, internal_id: u32) {
        if let Some(key) = self.id_indexer.get_key(internal_id) {
            self.id_indexer.remove(&key);
        }
        self.timestamps.write().invalidate_slot(internal_id);
    }

    fn insert_by_key(
        &self,
        key: IdKey,
        properties: &[(Arc<str>, Value)],
        ts: Timestamp,
    ) -> StorageResult<u32> {
        let converted = self.prepare_insert(&key, properties)?;
        self.apply_insert(key, &converted, ts, None)
    }

    /// Reserve a local id for a not-yet-committed row of this key. A key
    /// that is already bound (live or tombstoned awaiting GC) reports its
    /// existing id, matching the reuse semantics of
    /// [`Self::apply_insert`]; otherwise an unbound slot is taken from the
    /// id manager. Nothing is visible to lookups, scans, or the delta log
    /// until the id is registered by the commit apply, and
    /// [`Self::release_reserved_identity`] returns an unused reservation.
    pub fn reserve_identity(&self, key: &IdKey) -> StorageResult<u32> {
        if !self.is_open.load(Ordering::Acquire) {
            return Err(StorageError::storage_not_open());
        }
        Self::validate_key_shape(key)?;
        if let Some(existing) = self.id_indexer.get_index(key) {
            return Ok(existing);
        }
        self.id_indexer.reserve_next()
    }

    /// Return a reserved local id to the free stack. A no-op when the id
    /// was already bound (or is a live row id), so release only has to
    /// guard against leaking unbound reservations.
    pub fn release_reserved_identity(&self, internal_id: u32) {
        self.id_indexer.release_reserved(internal_id);
    }

    /// Cancel a previous release of a reserved local id: pulls the id back
    /// out of the id manager's free stack while its slot is still an
    /// unbound pending release. False means the slot is no longer a
    /// reclaimable release and the caller must reserve a fresh id.
    pub fn try_reclaim_reserved_identity(&self, internal_id: u32) -> bool {
        self.id_indexer.try_reclaim(internal_id)
    }

    fn validate_key_shape(key: &IdKey) -> StorageResult<()> {
        match key {
            IdKey::Int(id) if *id < 0 => Err(StorageError::invalid_input(format!(
                "Vertex id cannot be negative: {}",
                id
            ))),
            IdKey::Text(id) if id.len() > graphdb_core::types::VERTEX_ID_MAX_SIZE => {
                Err(StorageError::invalid_input(format!(
                    "Vertex id exceeds max length of {} bytes: got {} bytes",
                    graphdb_core::types::VERTEX_ID_MAX_SIZE,
                    id.len()
                )))
            }
            _ => Ok(()),
        }
    }

    /// Visibility-aware primary-key lookup. Collapses the old `get_index`
    /// plus timestamp-recheck pair into one call so cursor layers need no
    /// secondary filtering.
    pub fn lookup_internal_id(&self, key: &IdKey, ts: Timestamp) -> PkLookup {
        if !self.is_open.load(Ordering::Acquire) {
            return PkLookup::Missing;
        }
        let stamps = self.timestamps.read();
        self.id_indexer.lookup(key, |id| stamps.is_valid(id, ts))
    }

    /// Enforce the primary key mirror invariant on one write.
    ///
    /// The primary key column materializes the external id in the column's
    /// own type. A missing key property is filled in; a provided one must
    /// equal the derived mirror or the write is rejected.
    fn apply_primary_key_mirror(
        &self,
        key: &IdKey,
        mut properties: Vec<(std::sync::Arc<str>, Value)>,
    ) -> StorageResult<Vec<(std::sync::Arc<str>, Value)>> {
        let Some(pk_def) = self.schema.properties.get(self.schema.primary_key_index) else {
            return Ok(properties);
        };
        let mirror = primary_key_mirror_value(&pk_def.data_type, key)?;
        let mirror = mirror.try_cast_to(&pk_def.data_type)?;
        match properties.iter().find(|(name, _)| **name == *pk_def.name) {
            Some((_, provided)) => {
                let provided = provided.try_cast_to(&pk_def.data_type)?;
                if provided != mirror {
                    return Err(StorageError::invalid_input(format!(
                        "Primary key column '{}' must mirror the vertex id: got {:?}, expected {:?}",
                        pk_def.name, provided, mirror
                    )));
                }
            }
            None => properties.push((pk_def.name.clone(), mirror)),
        }
        Ok(properties)
    }

    /// Next free local id within this table: the high-water mark of the
    /// local id space. Deleted slots are recycled through the free stack
    /// on insert, so this only grows while holes remain unreused; row IDs
    /// stay stable for live rows between watermark-gated compactions.
    pub fn next_local_id(&self) -> u32 {
        self.id_indexer.next_index()
    }

    /// Pre-allocate capacity for `additional` more vertices in the ID indexer,
    /// the column buffers, and the timestamp vectors.
    ///
    /// Without column/timestamp reservation, every appended row in a large
    /// batch resizes the backing `Vec`s one element at a time, making bulk
    /// inserts quadratic in the table size.
    pub fn reserve_id_capacity(&self, additional: usize) {
        self.id_indexer.reserve(additional);
        self.columns.reserve(additional);
        self.timestamps.write().reserve(additional);
    }
}
