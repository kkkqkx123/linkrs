//! Liveness checks, projected reads, and the offline snapshot iterator.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use super::VertexTable;
use crate::vertex::{IdKey, Timestamp, VertexId, VertexRecord};
use linkrs_core::{StorageError, StorageResult, Value};

impl VertexTable {
    pub fn get_by_internal_id_offline(
        &self,
        internal_id: u32,
        ts: Timestamp,
    ) -> Option<VertexRecord> {
        self.get_projected_by_internal_id(internal_id, ts, None)
    }

    /// Single row-liveness entry for plain timestamp reads.
    ///
    /// Row life and death resolve here only: every content-serving read
    /// checks liveness exactly once through this entry and column version
    /// chains never recheck it. Pending-aware paths use the guard form
    /// instead; the two share the unified interval semantics. Online reads
    /// go through the transaction guard while this bare timestamp probe
    /// remains for offline barrier tools only.
    pub fn is_row_live_at(&self, internal_id: u32, ts: Timestamp) -> bool {
        self.timestamps.read().is_valid(internal_id, ts)
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
    /// Enumeration and point reads share the single liveness entry
    /// ([`Self::is_row_live_at`]): rows invisible at `ts` (including
    /// timestamp-deleted rows awaiting watermark-gated GC) are excluded.
    /// There is no unfiltered variant; sizing callers use `total_count` or
    /// `id_hole_stats` instead. Used by lazy paginated scans. Online
    /// enumeration goes through the guarded sharded cursor while this bare
    /// timestamp form remains for offline barrier tools only.
    pub fn live_ids(&self, ts: Timestamp) -> Vec<u32> {
        self.id_indexer
            .live_ids()
            .into_iter()
            .filter(|&id| self.is_row_live_at(id, ts))
            .collect()
    }

    /// Checkpoint-epoch floor below which attribute history is not retained.
    #[allow(dead_code)]
    pub fn history_floor(&self) -> Timestamp {
        self.history_floor
    }

    fn check_history_floor(&self, internal_id: u32, ts: Timestamp) -> StorageResult<()> {
        let floor = self.history_floor;
        if ts < floor {
            let created = self
                .timestamps
                .read()
                .get_start_ts(internal_id)
                .unwrap_or(0);
            if created <= ts {
                return Err(StorageError::history_before_floor(internal_id, ts, floor));
            }
        }
        Ok(())
    }

    /// Strict fenced batch read with explicit decode errors.
    ///
    /// Corrupt payloads fail instead of reading as missing. Attribute time travel ends at
    /// the last load: version chains do not survive checkpoints, so a
    /// query below the history floor for a row created at or below
    /// the query timestamp may need dropped before-images and fails
    /// instead of returning the current value. Rows created after the
    /// query timestamp still read as missing.
    pub fn try_get_projected_batch(
        &self,
        internal_ids: &[u32],
        ts: Timestamp,
        projection: Option<&[Arc<str>]>,
    ) -> StorageResult<Vec<Option<VertexRecord>>> {
        if !self.is_open.load(Ordering::Acquire) {
            return Ok(internal_ids.iter().map(|_| None).collect());
        }
        let mut positions: Vec<(usize, u32)> = Vec::with_capacity(internal_ids.len());
        for (pos, &id) in internal_ids.iter().enumerate() {
            if self.is_row_live_at(id, ts) {
                positions.push((pos, id));
            }
        }
        for &(_, id) in &positions {
            self.check_history_floor(id, ts)?;
        }

        let mut out: Vec<Option<VertexRecord>> = internal_ids.iter().map(|_| None).collect();
        if positions.is_empty() {
            return Ok(out);
        }
        let row_indices: Vec<usize> = positions.iter().map(|&(_, id)| id as usize).collect();
        let props = match projection {
            Some(names) => self
                .columns
                .try_get_projected_batch_at_ts(&row_indices, names, ts)?,
            None => self.columns.try_get_batch_at_ts(&row_indices, ts)?,
        };
        for ((pos, id), prop_row) in positions.into_iter().zip(props) {
            let key = match self.id_indexer.get_key(id) {
                Some(key) => key,
                None => continue,
            };
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
            let properties: Vec<(Arc<str>, Value)> = prop_row
                .into_iter()
                .filter_map(|(name, opt_val)| opt_val.map(|v| (name, v)))
                .collect();
            out[pos] = Some(VertexRecord {
                vid,
                internal_id: id,
                properties,
            });
        }
        Ok(out)
    }

    /// Column-major batch decode (A1).  Decodes the requested columns for
    /// `internal_ids` column-at-a-time into typed [`ColumnValues`] arrays.
    /// The ids must already be valid at `ts`; validity is not re-checked here.
    pub fn get_projected_columns_offline(
        &self,
        internal_ids: &[u32],
        ts: Timestamp,
        names: &[Arc<str>],
    ) -> Vec<(Arc<str>, crate::cursor::ColumnValues)> {
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
        projection: Option<&[Arc<str>]>,
    ) -> Option<VertexRecord> {
        if !self.is_open.load(Ordering::Acquire) {
            return None;
        }

        // Single liveness gate: column decodes below assume a live row and
        // never recheck the timestamp interval.
        if !self.is_row_live_at(internal_id, ts) {
            return None;
        }
        debug_assert!(self.is_row_live_at(internal_id, ts));

        let external_id = self.id_indexer.get_key(internal_id)?;
        let props = projection.map_or_else(
            || self.columns.get_at_ts(internal_id as usize, ts),
            |names| {
                self.columns
                    .get_projected_at_ts(internal_id as usize, names, ts)
            },
        );
        let properties: Vec<(Arc<str>, Value)> = props
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

    /// Strict single-row projection with explicit history and decode errors.
    ///
    /// Non-strict reads return missing below the history floor; this entry
    /// fails instead so callers can distinguish dropped history from a row
    /// created after the query timestamp. Row liveness stays with the caller
    /// through the single gate; never-written windows yield `Ok(None)` and
    /// corrupt payloads yield `Err` with the column and row.
    #[allow(dead_code)]
    pub fn try_get_projected_by_internal_id(
        &self,
        internal_id: u32,
        ts: Timestamp,
        projection: Option<&[Arc<str>]>,
    ) -> StorageResult<Option<VertexRecord>> {
        if !self.is_open.load(Ordering::Acquire) {
            return Ok(None);
        }
        if !self.is_row_live_at(internal_id, ts) {
            return Ok(None);
        }
        self.check_history_floor(internal_id, ts)?;
        let names: Vec<Arc<str>> = match projection {
            Some(names) => names.to_vec(),
            None => self
                .schema
                .properties
                .iter()
                .map(|prop| prop.name.clone())
                .collect(),
        };
        let row_idx = internal_id as usize;
        let props = self
            .columns
            .try_get_projected_batch_at_ts(&[row_idx], &names, ts)
            .map_err(|e| {
                StorageError::deserialize_error(format!(
                    "vertex row {} decode failed: {}",
                    internal_id, e
                ))
            })?;
        let Some(prop_row) = props.into_iter().next() else {
            return Ok(None);
        };
        let key = match self.id_indexer.get_key(internal_id) {
            Some(key) => key,
            None => return Ok(None),
        };
        let vid = match key {
            IdKey::Int(i) => match VertexId::try_from_int64(i).ok() {
                Some(vid) => vid,
                None => return Ok(None),
            },
            IdKey::Text(s) => match VertexId::try_from_string(&s).ok() {
                Some(vid) => vid,
                None => return Ok(None),
            },
        };
        let properties: Vec<(Arc<str>, Value)> = prop_row
            .into_iter()
            .filter_map(|(name, opt_val)| opt_val.map(|v| (name, v)))
            .collect();
        Ok(Some(VertexRecord {
            vid,
            internal_id,
            properties,
        }))
    }

    /// Fenced point read merging the projected decode with the per-column
    /// covering stamps in one pass per column. Single liveness gate like
    /// [`Self::get_projected_by_internal_id`]; the stamps travel with the
    /// record as its cache fence.
    pub fn get_projected_with_stamps(
        &self,
        internal_id: u32,
        ts: Timestamp,
        projection: Option<&[Arc<str>]>,
    ) -> Option<(VertexRecord, Vec<Timestamp>)> {
        if !self.is_open.load(Ordering::Acquire) {
            return None;
        }
        if !self.is_row_live_at(internal_id, ts) {
            return None;
        }
        let external_id = self.id_indexer.get_key(internal_id)?;
        let names: Vec<Arc<str>> = match projection {
            Some(names) => names.to_vec(),
            None => self
                .schema
                .properties
                .iter()
                .map(|prop| prop.name.clone())
                .collect(),
        };
        let (props, stamps) =
            self.columns
                .get_projected_with_stamps_at_ts(internal_id as usize, &names, ts);
        let properties: Vec<(Arc<str>, Value)> = props
            .into_iter()
            .filter_map(|(name, opt_val)| opt_val.map(|v| (name, v)))
            .collect();
        let vid = match external_id {
            IdKey::Int(i) => VertexId::try_from_int64(i).ok()?,
            IdKey::Text(s) => VertexId::try_from_string(&s).ok()?,
        };
        Some((
            VertexRecord {
                vid,
                internal_id,
                properties,
            },
            stamps,
        ))
    }

    /// Whether `col_name` is the primary-key mirror column.
    pub fn is_pk_column(&self, col_name: &str) -> bool {
        self.schema
            .properties
            .get(self.schema.primary_key_index)
            .is_some_and(|pk| &*pk.name == col_name)
    }

    /// Offline snapshot scan at `ts` for barrier-held maintenance tools.
    ///
    /// Online reads enumerate through the guarded sharded cursor instead.
    pub fn scan(&self, ts: Timestamp) -> VertexIterator<'_> {
        VertexIterator::new(self, ts)
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
            if let Some(record) = self.table.get_by_internal_id_offline(id, self.ts) {
                return Some(record);
            }
        }
        None
    }
}
