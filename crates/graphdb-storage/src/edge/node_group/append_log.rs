//! Per-group write-through append log and its sidecar codec.
//!
//! Each group holds the committed delta (inserts plus tombstone markers)
//! since its last base rewrite. Insert-only groups checkpoint by persisting
//! the append log alone; the first delete dirt forces a base rewrite that
//! discards the log. Append row indexes are memory only: dropped after the
//! flush that persists them and rebuilt from later writes.

use graphdb_core::types::{EdgeId, Timestamp, VertexId};
use graphdb_core::{StorageError, StorageResult};

use super::super::{MutableCsrTrait, Nbr};
use super::{CsrShardSet, TableShardManifest};

/// Wire version of one append-log sidecar payload. Version 2 carries only the
/// address width so group-set growth never invalidates clean groups' sidecars;
/// version 1 payloads are rejected, never converted.
pub(crate) const APPEND_LOG_FORMAT_VERSION: u32 = 2;

/// One committed append-log insert: the row plus the stored neighbor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AppendInsert {
    pub local: u32,
    pub nbr: Nbr,
}

/// One committed append-log delete marker: the row plus the tombstone stamp.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AppendDelete {
    pub local: u32,
    pub edge_id: EdgeId,
    pub delete_ts: Timestamp,
}

/// Write-through delta of one group since its last base rewrite.
///
/// Every committed topology write lands in the base variant for reads and is
/// also recorded here. Insert-only groups checkpoint by persisting this log
/// alone; the first delete dirt in the group forces a base rewrite that
/// discards the log. The log is a row index only: it is dropped after the
/// flush that persists it and rebuilt from later writes.
#[derive(Debug, Clone, Default)]
pub(crate) struct ShardAppendLog {
    pub inserts: Vec<AppendInsert>,
    pub deletes: Vec<AppendDelete>,
}

impl ShardAppendLog {
    pub fn is_empty(&self) -> bool {
        self.inserts.is_empty() && self.deletes.is_empty()
    }

    pub fn clear(&mut self) {
        self.inserts.clear();
        self.deletes.clear();
    }

    pub fn op_count(&self) -> usize {
        self.inserts.len() + self.deletes.len()
    }
}

/// Encode one append-op sequence for a sidecar. Carries only the address
/// width so group-set growth never invalidates clean groups' sidecars; a
/// width mismatch still fails closed on load.
pub(crate) fn encode_append_ops(
    manifest: &TableShardManifest,
    inserts: &[AppendInsert],
    deletes: &[AppendDelete],
) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&APPEND_LOG_FORMAT_VERSION.to_le_bytes());
    out.extend_from_slice(&manifest.group_bits.to_le_bytes());
    out.extend_from_slice(&(inserts.len() as u64).to_le_bytes());
    for insert in inserts {
        out.extend_from_slice(&insert.local.to_le_bytes());
        out.extend_from_slice(&insert.nbr.endpoint.to_le_bytes());
        out.extend_from_slice(&insert.nbr.rank.to_le_bytes());
        out.extend_from_slice(&insert.nbr.edge_id.0.to_le_bytes());
        out.extend_from_slice(&insert.nbr.delete_ts.to_le_bytes());
    }
    out.extend_from_slice(&(deletes.len() as u64).to_le_bytes());
    for delete in deletes {
        out.extend_from_slice(&delete.local.to_le_bytes());
        out.extend_from_slice(&delete.edge_id.0.to_le_bytes());
        out.extend_from_slice(&delete.delete_ts.to_le_bytes());
    }
    out
}

/// Decode one append-op sequence. Fails closed on version, address-width,
/// section-size or trailing-byte mismatches.
pub(crate) fn decode_append_ops(
    data: &[u8],
    manifest: &TableShardManifest,
) -> StorageResult<(Vec<AppendInsert>, Vec<AppendDelete>)> {
    let mut cursor = 0usize;
    let take = |data: &[u8], cursor: &mut usize, len: usize| -> StorageResult<Vec<u8>> {
        if data.len() - *cursor < len {
            return Err(StorageError::deserialize_error(
                "append log payload too short",
            ));
        }
        let slice = data[*cursor..*cursor + len].to_vec();
        *cursor += len;
        Ok(slice)
    };
    let version = u32::from_le_bytes(
        take(data, &mut cursor, 4)?
            .try_into()
            .map_err(|_| StorageError::deserialize_error("append log version too short"))?,
    );
    if version != APPEND_LOG_FORMAT_VERSION {
        return Err(StorageError::deserialize_error(format!(
            "unsupported append log version: {}",
            version
        )));
    }
    let carried_bits = u32::from_le_bytes(
        take(data, &mut cursor, 4)?
            .try_into()
            .map_err(|_| StorageError::deserialize_error("append log width too short"))?,
    );
    if carried_bits != manifest.group_bits {
        return Err(StorageError::deserialize_error(format!(
            "append log width mismatch: log carries {}, table holds {}",
            carried_bits, manifest.group_bits
        )));
    }
    let insert_count = u64::from_le_bytes(
        take(data, &mut cursor, 8)?
            .try_into()
            .map_err(|_| StorageError::deserialize_error("append log insert count too short"))?,
    ) as usize;
    let mut inserts = Vec::with_capacity(insert_count);
    for _ in 0..insert_count {
        let local = u32::from_le_bytes(
            take(data, &mut cursor, 4)?
                .try_into()
                .map_err(|_| StorageError::deserialize_error("append log insert row too short"))?,
        );
        let endpoint = u32::from_le_bytes(
            take(data, &mut cursor, 4)?
                .try_into()
                .map_err(|_| StorageError::deserialize_error("append log endpoint too short"))?,
        );
        let rank = u64::from_le_bytes(
            take(data, &mut cursor, 8)?
                .try_into()
                .map_err(|_| StorageError::deserialize_error("append log rank too short"))?,
        ) as i64;
        let edge_id = EdgeId(u64::from_le_bytes(
            take(data, &mut cursor, 8)?
                .try_into()
                .map_err(|_| StorageError::deserialize_error("append log edge id too short"))?,
        ));
        let delete_ts =
            u64::from_le_bytes(take(data, &mut cursor, 8)?.try_into().map_err(|_| {
                StorageError::deserialize_error("append log delete stamp too short")
            })?);
        let nbr = Nbr::with_timestamps(endpoint, rank, edge_id, delete_ts);
        inserts.push(AppendInsert { local, nbr });
    }
    let delete_count = u64::from_le_bytes(
        take(data, &mut cursor, 8)?
            .try_into()
            .map_err(|_| StorageError::deserialize_error("append log delete count too short"))?,
    ) as usize;
    let mut deletes = Vec::with_capacity(delete_count);
    for _ in 0..delete_count {
        let local = u32::from_le_bytes(
            take(data, &mut cursor, 4)?
                .try_into()
                .map_err(|_| StorageError::deserialize_error("append log delete row too short"))?,
        );
        let edge_id = EdgeId(u64::from_le_bytes(
            take(data, &mut cursor, 8)?.try_into().map_err(|_| {
                StorageError::deserialize_error("append log delete edge id too short")
            })?,
        ));
        let delete_ts =
            u64::from_le_bytes(take(data, &mut cursor, 8)?.try_into().map_err(|_| {
                StorageError::deserialize_error("append log delete stamp too short")
            })?);
        deletes.push(AppendDelete {
            local,
            edge_id,
            delete_ts,
        });
    }
    if cursor != data.len() {
        return Err(StorageError::deserialize_error(
            "unexpected trailing data in append log".to_string(),
        ));
    }
    Ok((inserts, deletes))
}

impl CsrShardSet {
    /// Record a committed insert in the group append log.
    pub(crate) fn record_append_insert(&mut self, gid: usize, local: u32, nbr: Nbr) {
        if let Some(shard) = self.shards.get_mut(&gid) {
            shard.append.inserts.push(AppendInsert { local, nbr });
        }
    }

    /// Record a committed tombstone in the group append log.
    pub(crate) fn record_append_delete(
        &mut self,
        gid: usize,
        local: u32,
        edge_id: EdgeId,
        delete_ts: Timestamp,
    ) {
        if let Some(shard) = self.shards.get_mut(&gid) {
            shard.append.deletes.push(AppendDelete {
                local,
                edge_id,
                delete_ts,
            });
        }
    }

    /// Whether a group holds append-log deltas not yet merged into a base.
    pub fn group_has_append_log(&self, gid: usize) -> bool {
        self.shards
            .get(&gid)
            .is_some_and(|shard| !shard.append.is_empty())
    }

    /// Committed op count held in one group append log.
    pub fn group_append_op_count(&self, gid: usize) -> usize {
        self.shards
            .get(&gid)
            .map_or(0, |shard| shard.append.op_count())
    }

    /// Committed ops held in one group append log, oldest first.
    pub(crate) fn group_append_ops(&self, gid: usize) -> (Vec<AppendInsert>, Vec<AppendDelete>) {
        self.shards
            .get(&gid)
            .map(|shard| (shard.append.inserts.clone(), shard.append.deletes.clone()))
            .unwrap_or_default()
    }

    /// Drop one group append log after its states merged into a base.
    /// The row index is memory only; base files plus later logs rebuild it.
    pub fn clear_group_append_log(&mut self, gid: usize) {
        if let Some(shard) = self.shards.get_mut(&gid) {
            shard.append.clear();
        }
    }

    /// Drop every group append log, e.g. after a topology-wide rebuild
    /// whose base rewrite already carries all states.
    pub fn clear_all_append_logs(&mut self) {
        for shard in self.shards.values_mut() {
            shard.append.clear();
        }
    }

    /// Encode one group append log for an append-only checkpoint. Carries
    /// only the address width so group-set growth never invalidates clean
    /// groups' sidecars; a width mismatch is rejected on load instead of
    /// replayed against the wrong base. Borrows the in-memory log directly
    /// without cloning it into temporaries.
    pub fn encode_group_append_log(&self, gid: usize, manifest: &TableShardManifest) -> Vec<u8> {
        match self.shards.get(&gid) {
            Some(shard) => {
                encode_append_ops(manifest, &shard.append.inserts, &shard.append.deletes)
            }
            None => encode_append_ops(manifest, &[], &[]),
        }
    }

    /// Replay one append-log payload into a group base. Fails closed on
    /// version, address-width, section-size or trailing-byte mismatches.
    pub fn replay_group_append_log(
        &mut self,
        gid: usize,
        data: &[u8],
        manifest: &TableShardManifest,
    ) -> StorageResult<()> {
        let (inserts, deletes) = decode_append_ops(data, manifest)?;
        let shard = self.shards.get_mut(&gid).ok_or_else(|| {
            StorageError::deserialize_error(format!("group {} out of range on append replay", gid))
        })?;
        // Frozen groups carry no deltas by construction: freeze clears the
        // in-memory log and the base rewrite drops the sidecar. A sidecar for
        // a frozen base is damage, rejected here instead of replayed.
        if matches!(shard.variant, super::super::CsrVariant::Frozen(_)) {
            return Err(StorageError::deserialize_error(format!(
                "append sidecar for frozen group {}",
                gid
            )));
        }
        for insert in inserts {
            shard
                .variant
                .insert_edge(
                    insert.local,
                    VertexId::edge_endpoint_key(insert.nbr.endpoint, insert.nbr.rank),
                    insert.nbr.edge_id,
                    Timestamp::MAX,
                )
                .map_err(|e| {
                    StorageError::deserialize_error(format!(
                        "append log insert replay failed in group {}: {}",
                        gid, e
                    ))
                })?;
        }
        for delete in deletes {
            shard
                .variant
                .delete_edge(delete.local, delete.edge_id, delete.delete_ts)
                .map_err(|e| {
                    StorageError::deserialize_error(format!(
                        "append log delete replay failed in group {}: {}",
                        gid, e
                    ))
                })?;
        }
        Ok(())
    }
}
