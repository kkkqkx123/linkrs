//! Vertex Table Core
//!
//! Main vertex storage with columnar layout.
//! Combines ID indexing, column storage, and timestamp tracking.
//!
//! # Concurrency Note
//!
//! Point paths are `&self`: the identity step (duplicate check, id
//! allocation, timestamp publish) serializes on the timestamp latch while
//! the data step (column segment writes) uses the column segment latches,
//! so point writes to different rows of one table proceed concurrently.
//! `IdIndexer` carries its own mutex; `ColumnStore` and `Column` are
//! internally locked. Schema, pending schema changes and the property
//! index cache mutate only under table-exclusive operations (schema
//! evolution, load, offline rebuild) and are read lock-free on point
//! paths. Exclusive operations keep their table-wide semantics by holding
//! the shard write guard across the same identity-plus-segments sequence.
//!
//! Lock order inside one table: identity (timestamp latch) before column
//! segments before overflow. Point paths never hold a segment latch while
//! acquiring the identity latch.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use super::super::vertex_timestamp::IdentityLatch;
use super::super::{
    primary_key_mirror_value, ColumnStore, IdIndexer, IdKey, LabelId, PkLookup, Timestamp,
    VertexId, VertexRecord, VertexSchema, VertexTimestamp,
};
use crate::encoding::EncodingSelector;
use crate::schema::{LabelVersionHistory, SchemaObjectType};
use graphdb_core::{StorageError, StorageResult, Value};

#[derive(Debug, Clone)]
pub struct VertexTableConfig {
    pub initial_capacity: usize,
    /// Payload size above which strings spill to the per-column overflow
    /// file. `usize::MAX` disables overflow routing (inline storage).
    pub string_overflow_threshold: usize,
    /// Rows per chunk for chunk-local encodings and update overlays.
    pub chunk_capacity: usize,
}

impl Default for VertexTableConfig {
    fn default() -> Self {
        Self {
            initial_capacity: 4096,
            string_overflow_threshold: crate::vertex::column::overflow::DEFAULT_OVERFLOW_THRESHOLD,
            chunk_capacity: crate::vertex::column::chunk::DEFAULT_CHUNK_ROWS,
        }
    }
}

#[derive(Debug)]
pub struct VertexTable {
    pub(super) label: LabelId,
    pub(super) label_name: String,
    pub(super) schema: VertexSchema,
    pub(super) id_indexer: IdIndexer,
    pub(super) columns: ColumnStore,
    /// Identity latch: duplicate check, id allocation and timestamp publish
    /// hold this across the identity step so concurrent point inserts of one
    /// table allocate each key exactly once. The data step runs after the
    /// guard is released. `IdIndexer` serializes its own map internally;
    /// this latch orders the map-plus-timestamp pair.
    pub(super) timestamps: IdentityLatch,
    pub(super) is_open: AtomicBool,
    /// Cache for property name → index mapping to avoid O(n) schema lookups.
    /// Invalidated whenever schema changes.
    pub(super) property_index_cache: HashMap<String, usize>,
    /// Version history tracking for schema changes
    pub(super) version_history: Arc<Mutex<LabelVersionHistory>>,
    /// Persistent encoding selector with accumulated compression feedback.
    /// Feedback is gathered across flushes so that `should_reencode` can
    /// detect when a column's compression ratio degrades and recommend
    /// re-evaluating the encoding choice.
    pub(super) encoding_selector: EncodingSelector,
    /// Payload size above which new string/blob columns spill to the
    /// per-column overflow file (applied when columns are created).
    pub(super) string_overflow_threshold: usize,
    /// Rows per chunk for chunk-local encodings (applied when columns are
    /// created).
    pub(super) chunk_capacity: usize,
    /// One in-flight staged schema change (prepare/fill/publish/abort).
    /// Pending state lives only in memory; publishing is the visibility
    /// boundary. While set, `set_schema` is rejected so the staged change
    /// cannot be silently discarded.
    pub(super) pending_schema_change: Option<super::staged_schema::PendingVertexSchemaChange>,
}

impl VertexTable {
    /// Per-row version-chain length above which `gc` emits a pressure
    /// warning. Chains grow without bound while the GC watermark is pinned,
    /// so crossing this threshold points at a stuck snapshot, not a hot row.
    const VERSION_CHAIN_PRESSURE_WARN_LEN: usize = 1024;

    pub fn with_config(
        label: LabelId,
        label_name: String,
        schema: VertexSchema,
        config: VertexTableConfig,
    ) -> Self {
        let columns = ColumnStore::with_capacity(schema.properties.len());

        for prop in &schema.properties {
            columns.add_column(prop.name.clone(), prop.data_type.clone(), prop.nullable);
            if let Some(col) = columns.get_column(&prop.name) {
                col.set_chunk_capacity(config.chunk_capacity);
            }
            if matches!(
                prop.data_type,
                graphdb_core::DataType::String | graphdb_core::DataType::Blob
            ) {
                if let Some(col) = columns.get_column(&prop.name) {
                    col.set_overflow_threshold(config.string_overflow_threshold);
                }
            }
        }

        let mut property_index_cache = HashMap::new();
        for (idx, prop) in schema.properties.iter().enumerate() {
            property_index_cache.insert(prop.name.clone(), idx);
        }

        let version_history = Arc::new(Mutex::new(LabelVersionHistory::new(
            label,
            label_name.clone(),
            SchemaObjectType::Vertex,
        )));

        Self {
            label,
            label_name,
            schema,
            id_indexer: IdIndexer::with_capacity(config.initial_capacity),
            columns,
            timestamps: IdentityLatch::new(VertexTimestamp::with_capacity(config.initial_capacity)),
            is_open: AtomicBool::new(true),
            property_index_cache,
            version_history,
            encoding_selector: EncodingSelector::default(),
            string_overflow_threshold: config.string_overflow_threshold,
            chunk_capacity: config.chunk_capacity,
            pending_schema_change: None,
        }
    }

    pub fn insert(
        &self,
        external_id: &str,
        properties: &[(String, Value)],
        ts: Timestamp,
    ) -> StorageResult<u32> {
        self.insert_by_key(IdKey::Text(external_id.to_string()), properties, ts)
    }

    pub fn insert_by_i64(
        &self,
        external_id: i64,
        properties: &[(String, Value)],
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
        properties: &[(String, Value)],
    ) -> StorageResult<Vec<(String, Value)>> {
        if !self.is_open.load(Ordering::Acquire) {
            return Err(StorageError::storage_not_open());
        }

        Self::validate_key_shape(key)?;

        let mut converted: Vec<(String, Value)> = Vec::with_capacity(properties.len());
        for (name, value) in properties {
            // Use cached index lookup instead of O(n) schema search
            let prop_idx = self
                .property_index_cache
                .get(name)
                .ok_or_else(|| StorageError::column_not_found(name.clone()))?;
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
    /// allocation or reserved-id binding, timestamp publish) under the
    /// identity latch, then the data step on the column segments. A
    /// data-step failure rolls the identity step back so no allocated id
    /// escapes. `reserved` declares a local id obtained earlier from
    /// [`Self::reserve_identity`] for this key; the commit binds the key to
    /// that exact id, and a mismatch with an existing (tombstoned) binding
    /// is rejected so rows already referenced by other structures cannot
    /// silently change identity.
    pub fn apply_insert(
        &self,
        key: IdKey,
        converted: &[(String, Value)],
        ts: Timestamp,
        reserved: Option<u32>,
    ) -> StorageResult<u32> {
        if !self.is_open.load(Ordering::Acquire) {
            if let Some(id) = reserved {
                self.id_indexer.release_reserved(id);
            }
            return Err(StorageError::storage_not_open());
        }

        // Identity step: the timestamp write guard is the identity latch.
        let internal_id = {
            let mut stamps = self.timestamps.write();
            let identity: StorageResult<u32> =
                if let Some(existing) = self.id_indexer.get_index(&key) {
                    if stamps.is_valid(existing, ts) {
                        Err(StorageError::vertex_already_exists(format!("{:?}", key)))
                    } else if matches!(reserved, Some(id) if id != existing) {
                        // Re-insert after deletion reuses the allocated id,
                        // so a reservation naming a different slot cannot be
                        // honored: reject rather than silently bind
                        // elsewhere and mis-link rows already referencing it.
                        Err(StorageError::vertex_already_exists(format!("{:?}", key)))
                    } else {
                        // Re-open the closed lifetime window at `ts`: the
                        // recycled slot's stamps are overwritten by a plain
                        // insert (id reuse), not by any delete-revert entry.
                        stamps.insert(existing, ts);
                        Ok(existing)
                    }
                } else {
                    let bound = match reserved {
                        Some(id) => self.id_indexer.register_reserved(key, id).map(|()| id),
                        None => self.id_indexer.insert(key),
                    };
                    bound.inspect(|internal_id| {
                        // Fresh bindings publish their creation stamp on the
                        // same identity latch, exactly like the reuse arm.
                        stamps.insert(*internal_id, ts);
                    })
                };
            match identity {
                Ok(internal_id) => internal_id,
                Err(error) => {
                    // A declared reservation is unbound whenever the identity
                    // step fails: the divergence and duplicate arms never
                    // bind, and a failed register leaves the slot unbound.
                    if let Some(id) = reserved {
                        self.id_indexer.release_reserved(id);
                    }
                    return Err(error);
                }
            }
        };

        // Data step: segment-latched column writes, identity latch released.
        if let Err(error) = self
            .columns
            .set_versioned(internal_id as usize, converted, ts)
        {
            self.undo_apply_insert(internal_id);
            return Err(error);
        }
        Ok(internal_id)
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
        properties: &[(String, Value)],
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
        mut properties: Vec<(String, Value)>,
    ) -> StorageResult<Vec<(String, Value)>> {
        let Some(pk_def) = self.schema.properties.get(self.schema.primary_key_index) else {
            return Ok(properties);
        };
        let mirror = primary_key_mirror_value(&pk_def.data_type, key)?;
        let mirror = mirror.try_cast_to(&pk_def.data_type)?;
        match properties.iter().find(|(name, _)| name == &pk_def.name) {
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

    pub fn get_by_internal_id(&self, internal_id: u32, ts: Timestamp) -> Option<VertexRecord> {
        self.get_projected_by_internal_id(internal_id, ts, None)
    }

    /// Row survival stamps for pending-aware rechecks.
    ///
    /// Returns `(create_ts, delete_ts)` with `None` for a live row. `None`
    /// (unknown row) lets the caller fall back to the plain predicate result.
    pub fn row_timestamps(&self, internal_id: u32) -> Option<(Timestamp, Option<Timestamp>)> {
        let stamps = self.timestamps.read();
        let create_ts = stamps.get_start_ts(internal_id)?;
        Some((create_ts, stamps.get_end_ts(internal_id)))
    }

    /// Per-column covering version stamps for cache fences.
    ///
    /// Companion of the column values read by
    /// [`VertexTable::get_projected_by_internal_id`]: the caller seeds the
    /// record cache with these stamps and revalidates a hit by comparing
    /// them against live storage.
    pub fn row_picked_starts(&self, internal_id: u32, ts: Timestamp) -> Vec<Timestamp> {
        self.columns.picked_starts_at(internal_id as usize, ts)
    }

    /// Snapshot-visible live IDs at `ts` in allocation order.
    ///
    /// Enumeration and point reads share this one visibility predicate:
    /// rows invisible at `ts` (including timestamp-deleted rows awaiting
    /// watermark-gated GC) are excluded. There is no unfiltered variant;
    /// sizing callers use `total_count` or `id_hole_stats` instead.
    /// Used by lazy paginated scans.
    pub fn live_ids(&self, ts: Timestamp) -> Vec<u32> {
        let stamps = self.timestamps.read();
        self.id_indexer
            .live_ids()
            .into_iter()
            .filter(|&id| stamps.is_valid(id, ts))
            .collect()
    }

    /// Batch variant of [`get_projected_by_internal_id`].
    ///
    /// Validity is checked once per id, then all requested rows are decoded
    /// column-at-a-time in a single pass. The output is aligned with the
    /// input order; invalid or missing ids yield `None`.
    pub fn get_projected_batch(
        &self,
        internal_ids: &[u32],
        ts: Timestamp,
        projection: Option<&[String]>,
    ) -> Vec<Option<VertexRecord>> {
        if !self.is_open.load(Ordering::Acquire) {
            return internal_ids.iter().map(|_| None).collect();
        }

        let stamps = self.timestamps.read();
        let mut positions: Vec<(usize, u32)> = Vec::with_capacity(internal_ids.len());
        for (pos, &id) in internal_ids.iter().enumerate() {
            if stamps.is_valid(id, ts) {
                positions.push((pos, id));
            }
        }

        let mut out: Vec<Option<VertexRecord>> = internal_ids.iter().map(|_| None).collect();
        if positions.is_empty() {
            return out;
        }

        let row_indices: Vec<usize> = positions.iter().map(|&(_, id)| id as usize).collect();
        let props = match projection {
            Some(names) => self
                .columns
                .get_projected_batch_at_ts(&row_indices, names, ts),
            None => self.columns.get_batch_at_ts(&row_indices, ts),
        };

        for ((pos, id), prop_row) in positions.into_iter().zip(props) {
            let key = match self.id_indexer.get_key(id) {
                Some(key) => key,
                None => continue,
            };
            // Index keys are length-checked at insert, so a decode failure
            // surfaces as a missing row under the existing absence contract.
            let vid = match key {
                IdKey::Int(i) => match VertexId::try_from_int64(i).ok() {
                    Some(vid) => vid,
                    None => continue,
                },
                IdKey::Text(s) => match VertexId::try_from_string(&s).ok() {
                    Some(vid) => vid,
                    None => continue,
                },
            };
            let properties: Vec<(String, Value)> = prop_row
                .into_iter()
                .filter_map(|(name, opt_val)| opt_val.map(|v| (name, v)))
                .collect();
            out[pos] = Some(VertexRecord {
                vid,
                internal_id: id,
                properties,
            });
        }
        out
    }

    /// Column-major batch decode (A1).  Decodes the requested columns for
    /// `internal_ids` column-at-a-time into typed [`ColumnValues`] arrays.
    /// The ids must already be valid at `ts`; validity is not re-checked here.
    pub fn get_projected_columns(
        &self,
        internal_ids: &[u32],
        ts: Timestamp,
        names: &[String],
    ) -> Vec<(String, crate::cursor::ColumnValues)> {
        if !self.is_open.load(Ordering::Acquire) {
            return names
                .iter()
                .map(|n| {
                    (
                        n.clone(),
                        crate::cursor::ColumnValues::General(vec![None; internal_ids.len()]),
                    )
                })
                .collect();
        }
        let row_indices: Vec<usize> = internal_ids.iter().map(|&id| id as usize).collect();
        self.columns
            .get_projected_columns_at_ts(&row_indices, names, ts)
    }

    pub fn get_projected_by_internal_id(
        &self,
        internal_id: u32,
        ts: Timestamp,
        projection: Option<&[String]>,
    ) -> Option<VertexRecord> {
        if !self.is_open.load(Ordering::Acquire) {
            return None;
        }

        if !self.timestamps.read().is_valid(internal_id, ts) {
            return None;
        }

        let external_id = self.id_indexer.get_key(internal_id)?;
        let props = projection.map_or_else(
            || self.columns.get_at_ts(internal_id as usize, ts),
            |names| {
                self.columns
                    .get_projected_at_ts(internal_id as usize, names, ts)
            },
        );
        let properties: Vec<(String, Value)> = props
            .into_iter()
            .filter_map(|(name, opt_val)| opt_val.map(|v| (name, v)))
            .collect();

        let vid = match external_id {
            IdKey::Int(i) => VertexId::try_from_int64(i).ok()?,
            IdKey::Text(s) => VertexId::try_from_string(&s).ok()?,
        };

        Some(VertexRecord {
            vid,
            internal_id,
            properties,
        })
    }

    /// Read-only validation and normalization for one property update:
    /// primary-key mirror rejection, column existence and type checks.
    /// Staging paths call this before buffering so an invalid column fails
    /// with no global write.
    pub fn prepare_update(&self, col_name: &str, value: &Value) -> StorageResult<Value> {
        if self
            .schema
            .properties
            .get(self.schema.primary_key_index)
            .is_some_and(|pk| pk.name == col_name)
        {
            return Err(StorageError::invalid_operation(format!(
                "Primary key column '{}' mirrors the vertex id and cannot be updated; delete and re-insert the vertex instead",
                col_name
            )));
        }

        // Use cached index lookup
        let prop_idx = self
            .property_index_cache
            .get(col_name)
            .ok_or_else(|| StorageError::column_not_found(col_name.to_string()))?;
        let prop_def = &self.schema.properties[*prop_idx];

        if value.data_type() != prop_def.data_type {
            value.try_cast_to(&prop_def.data_type)
        } else {
            Ok(value.clone())
        }
    }

    pub fn update_property(
        &self,
        internal_id: u32,
        col_name: &str,
        value: &Value,
        ts: Timestamp,
    ) -> StorageResult<()> {
        if !self.is_open.load(Ordering::Acquire) {
            return Err(StorageError::storage_not_open());
        }

        if !self.timestamps.read().is_valid(internal_id, ts) {
            return Err(StorageError::vertex_not_found());
        }

        // The primary key column mirrors the row's own key: restating the
        // mirror changes nothing and is a no-op (a delete-then-insert folded
        // into a whole-row update always carries it), while a divergent
        // value would fork the key from its mirror and is rejected.
        if let Some(pk_def) = self.schema.properties.get(self.schema.primary_key_index) {
            if pk_def.name == col_name {
                let key = self
                    .id_indexer
                    .get_key(internal_id)
                    .ok_or_else(StorageError::vertex_not_found)?;
                let mirror = primary_key_mirror_value(&pk_def.data_type, &key)?
                    .try_cast_to(&pk_def.data_type)?;
                let provided = value.try_cast_to(&pk_def.data_type)?;
                if provided != mirror {
                    return Err(StorageError::invalid_operation(format!(
                        "Primary key column '{}' must mirror the vertex id: got {:?}, expected {:?}",
                        col_name, provided, mirror
                    )));
                }
                return Ok(());
            }
        }

        let converted_value = self.prepare_update(col_name, value)?;

        self.columns.set_property_versioned(
            internal_id as usize,
            col_name,
            Some(&converted_value),
            ts,
        )?;
        Ok(())
    }

    fn delete_by_key(&self, key: &IdKey, ts: Timestamp) -> StorageResult<()> {
        if !self.is_open.load(Ordering::Acquire) {
            return Err(StorageError::storage_not_open());
        }

        let internal_id = self
            .id_indexer
            .get_index(key)
            .ok_or(StorageError::vertex_not_found())?;

        self.apply_delete(internal_id, ts)
    }

    /// Apply one delete by internal id: timestamp tombstone under the
    /// identity latch, then the row dirty mark on the column segments.
    pub fn apply_delete(&self, internal_id: u32, ts: Timestamp) -> StorageResult<()> {
        if !self.is_open.load(Ordering::Acquire) {
            return Err(StorageError::storage_not_open());
        }

        {
            let mut stamps = self.timestamps.write();
            if !stamps.is_valid(internal_id, ts) {
                return Err(StorageError::vertex_not_found());
            }
            stamps.remove(internal_id, ts);
        }
        self.mark_row_dirty(internal_id as usize);
        Ok(())
    }

    pub fn delete_by_internal_id(&self, internal_id: u32, ts: Timestamp) -> StorageResult<()> {
        self.apply_delete(internal_id, ts)
    }

    /// Batch delete multiple vertices by external ID.
    /// Returns count of successfully deleted vertices.
    pub fn batch_delete(&self, external_ids: &[&str], ts: Timestamp) -> StorageResult<usize> {
        if !self.is_open.load(Ordering::Acquire) {
            return Err(StorageError::storage_not_open());
        }

        let mut deleted_count = 0;

        for external_id in external_ids {
            match self.delete_by_key(&IdKey::Text(external_id.to_string()), ts) {
                Ok(_) => {
                    deleted_count += 1;
                }
                Err(e) => {
                    // Skip this vertex and continue with others; the failure is
                    // logged through the standard log facade instead of stderr.
                    log::warn!("batch_delete skipped vertex {}: {}", external_id, e);
                }
            }
        }

        Ok(deleted_count)
    }

    /// Batch delete multiple vertices by i64 external ID.
    /// Returns count of successfully deleted vertices.
    pub fn batch_delete_i64(&self, external_ids: &[i64], ts: Timestamp) -> StorageResult<usize> {
        if !self.is_open.load(Ordering::Acquire) {
            return Err(StorageError::storage_not_open());
        }

        let mut deleted_count = 0;

        for external_id in external_ids {
            match self.delete_by_key(&IdKey::Int(*external_id), ts) {
                Ok(_) => {
                    deleted_count += 1;
                }
                Err(e) => {
                    // Skip this vertex and continue with others; the failure is
                    // logged through the standard log facade instead of stderr.
                    log::warn!("batch_delete_i64 skipped vertex {}: {}", external_id, e);
                }
            }
        }

        Ok(deleted_count)
    }

    pub fn get_internal_id_by_i64_raw(&self, external_id: i64) -> Option<u32> {
        if !self.is_open.load(Ordering::Acquire) {
            return None;
        }
        self.id_indexer.get_index(&IdKey::Int(external_id))
    }

    /// Lookup internal ID from external string without timestamp check.
    /// Returns Some(internal_id) even for deleted vertices.
    pub fn get_internal_id_raw(&self, external_id: &str) -> Option<u32> {
        if !self.is_open.load(Ordering::Acquire) {
            return None;
        }
        self.id_indexer
            .get_index(&IdKey::Text(external_id.to_string()))
    }

    pub fn get_external_id(&self, internal_id: u32, ts: Timestamp) -> Option<IdKey> {
        if !self.is_open.load(Ordering::Acquire)
            || !self.timestamps.read().is_valid(internal_id, ts)
        {
            return None;
        }
        self.id_indexer.get_key(internal_id)
    }

    /// Lookup external ID from internal ID without timestamp check.
    /// Returns the external ID even for deleted vertices.
    pub fn get_external_id_raw(&self, internal_id: u32) -> Option<IdKey> {
        if !self.is_open.load(Ordering::Acquire) {
            return None;
        }
        self.id_indexer.get_key(internal_id)
    }

    /// Declared data type of the column `name`, if it exists in the schema.
    pub fn data_type_of(&self, name: &str) -> Option<graphdb_core::types::DataType> {
        self.columns.data_type_of(name)
    }

    pub fn total_count(&self) -> usize {
        self.id_indexer.len()
    }

    /// Next free local id within this table: the high-water mark of the
    /// local id space. Deleted slots are recycled through the free stack
    /// on insert, so this only grows while holes remain unreused; row IDs
    /// stay stable for live rows between watermark-gated compactions.
    pub fn next_local_id(&self) -> u32 {
        self.id_indexer.next_index()
    }

    /// Live vertex count at `ts` (excludes vertices deleted at or before
    /// `ts`) and total allocated local IDs (the high-water mark, including
    /// free-stack holes awaiting reuse). The gap `allocated - live` is the
    /// number of slots reclaimable by a compaction at `ts`; below the
    /// hole-rate watermark the free stack absorbs deletes without moving
    /// live rows.
    pub fn id_hole_stats(&self, ts: Timestamp) -> (usize, usize) {
        let allocated = self.next_local_id() as usize;
        let deleted = self.timestamps.read().iter_deleted(ts).count();
        (allocated.saturating_sub(deleted), allocated)
    }

    /// Mark all columns' page containing `row_idx` as dirty.
    pub fn mark_row_dirty(&self, row_idx: usize) {
        self.columns.mark_row_dirty(row_idx);
    }

    pub fn dirty_pages(&self) -> Vec<crate::persistence::dirty_page::PageId> {
        self.columns.collect_dirty_pages()
    }

    pub fn clear_dirty(&self) {
        self.columns.clear_dirty();
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

    pub fn scan(&self, ts: Timestamp) -> VertexIterator<'_> {
        VertexIterator::new(self, ts)
    }

    pub fn schema(&self) -> &VertexSchema {
        &self.schema
    }

    /// Replace the published schema without staging.
    ///
    /// Recovery and undo compensation only. Live evolution must go through
    /// prepare/fill/publish/abort; while a staged change is pending this
    /// is rejected.
    pub fn set_schema(&mut self, schema: VertexSchema) -> StorageResult<()> {
        if self.pending_schema_change.is_some() {
            return Err(StorageError::invalid_operation(
                "set_schema rejected while a staged vertex schema change is pending".to_string(),
            ));
        }
        self.schema = schema;

        // Rebuild property index cache
        self.property_index_cache.clear();
        for (idx, prop) in self.schema.properties.iter().enumerate() {
            self.property_index_cache.insert(prop.name.clone(), idx);
        }
        Ok(())
    }

    /// Get reference to version history Arc for shared access
    pub fn version_history_ref(&self) -> Arc<Mutex<LabelVersionHistory>> {
        Arc::clone(&self.version_history)
    }

    pub fn memory_size(&self) -> usize {
        let mut total = 0;

        total += self.id_indexer.memory_size();
        total += self.columns.memory_size();
        total += self.timestamps.read().memory_size();

        // Account for label_name string (content only)
        total += self.label_name.len();

        // Account for property_index_cache HashMap (actual entries, not capacity)
        total += self.property_index_cache.len()
            * (std::mem::size_of::<String>() + std::mem::size_of::<usize>());

        total += std::mem::size_of::<Self>();

        total
    }

    pub fn used_memory_size(&self) -> usize {
        let mut total = 0;

        let active_count = self.id_indexer.len();
        total += active_count * std::mem::size_of::<(String, u32)>();

        total += self.columns.used_memory_size();

        total += self.timestamps.read().size() * std::mem::size_of::<Timestamp>();

        // Account for actual label_name usage
        total += self.label_name.len();

        // Account for property_index_cache actual entries
        total += self.property_index_cache.len() * (24 + std::mem::size_of::<usize>()); // String overhead + usize

        total
    }

    // ==================== MVCC Methods ====================
    // Snapshot truth lives in the transaction layer watermarks. The caller
    // passes the watermark safe timestamp; timestamp compaction is always
    // cutoff-gated.

    /// Fold version-chain before-images eligible at `cutoff`.
    ///
    /// Fold-only entry for watermark-coordinated maintenance: drops
    /// before-images no active snapshot can observe without touching the ID
    /// space (no re-densification, no timestamp compaction). Returns the
    /// number of version entries folded. The cutoff must come from the
    /// shared watermark capture of the maintenance pass.
    pub fn fold_version_chains(&self, cutoff: Timestamp) -> usize {
        let removed = self.columns.gc_versions(cutoff);
        self.columns.maybe_rebuild_zones_exact();
        removed
    }

    /// Perform garbage collection on version data older than min_ts
    ///
    /// Reclaims deleted vertices (from the id indexer / timestamps) and drops
    /// property version-chain entries that no active snapshot can observe.
    ///
    /// Returns `(reclaimed vertices, reclaimed version-chain entries)`.
    /// Stable row-id semantic: reclaimed vertices only lose their keys to
    /// the free stack, live rows never move, so caches keyed by internal ID
    /// stay valid across this call. ID re-densification lives exclusively
    /// in the barriered offline remap path, never in background GC.
    pub fn gc_detailed(&self, min_ts: Timestamp) -> StorageResult<(usize, usize)> {
        // Property version-chain GC runs every pass regardless of deleted
        // vertices so before-images of overwritten properties are reclaimed.
        let version_removed = self.columns.gc_versions(min_ts);
        self.columns.maybe_rebuild_zones_exact();
        let version_stats = self.columns.version_chain_stats();
        log::trace!(
            "vertex gc version stats: total_rows={} total_entries={} max_len={} avg_len={:.2} memory_bytes={} removed={}",
            version_stats.total_rows,
            version_stats.total_entries,
            version_stats.max_len,
            version_stats.avg_len,
            version_stats.memory_bytes,
            version_removed
        );
        // Version chains are watermark-collected only: a long-lived snapshot
        // pins every chain, so an abnormally long chain almost always means
        // a stuck snapshot rather than a hot row. Surface it loudly instead
        // of growing silently.
        if version_stats.max_len > Self::VERSION_CHAIN_PRESSURE_WARN_LEN {
            log::warn!(
                "vertex table '{}' has a version chain of length {} (min_active_snapshot_ts={}); \
                 a pinned snapshot may be blocking garbage collection",
                self.label_name,
                version_stats.max_len,
                min_ts,
            );
        }

        // Collect all vertices deleted before min_ts
        let deleted_ids: Vec<u32> = self.timestamps.read().iter_deleted(min_ts).collect();

        if deleted_ids.is_empty() {
            return Ok((0, version_removed));
        }

        let mut count = 0usize;

        // Stable absorption: drop keys to the free stack and invalidate
        // their timestamp slots without moving any live row. Freed ids are
        // recycled by later inserts; columns keep their holes until reuse.
        let mut stamps = self.timestamps.write();
        for id in &deleted_ids {
            if let Some(key) = self.id_indexer.get_key(*id) {
                self.id_indexer.remove(&key);
                stamps.invalidate_slot(*id);
                count += 1;
            }
        }

        Ok((count, version_removed))
    }
}

pub struct VertexIterator<'a> {
    table: &'a VertexTable,
    ts: Timestamp,
    live_ids: std::vec::IntoIter<u32>,
}

impl<'a> VertexIterator<'a> {
    pub fn new(table: &'a VertexTable, ts: Timestamp) -> Self {
        Self {
            table,
            ts,
            live_ids: table.live_ids(ts).into_iter(),
        }
    }
}

impl<'a> Iterator for VertexIterator<'a> {
    type Item = VertexRecord;

    fn next(&mut self) -> Option<Self::Item> {
        for id in self.live_ids.by_ref() {
            if let Some(record) = self.table.get_by_internal_id(id, self.ts) {
                return Some(record);
            }
        }
        None
    }
}

#[cfg(test)]
#[path = "core_tests.rs"]
mod tests;
