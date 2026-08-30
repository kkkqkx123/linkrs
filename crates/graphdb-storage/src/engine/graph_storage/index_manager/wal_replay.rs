use crate::engine::graph_storage::context::GraphStorageContext;
use crate::index::types::IndexRecord;
use graphdb_core::types::{CommitLsn, Timestamp};
use graphdb_core::{StorageError, StorageResult};

use graphdb_transaction::wal::{
    collect_committed_transactions, filter_intents_for_indexes, CommittedWalTransaction,
    LocalWalParser, WalParser,
};

use super::stable_hash;
use super::IndexDataMaps;

pub(crate) fn committed_wal_transactions(
    ctx: &GraphStorageContext,
) -> StorageResult<Vec<CommittedWalTransaction>> {
    let Some(paths) = ctx.storage_paths() else {
        return Ok(Vec::new());
    };
    if !paths.wal_dir().exists() {
        return Ok(Vec::new());
    }

    let mut parser = LocalWalParser::new();
    parser
        .open(&paths.wal_dir().to_string_lossy())
        .map_err(|error| {
            StorageError::wal_error(format!(
                "Failed to parse WAL for index generation catch-up: {}",
                error
            ))
        })?;
    collect_committed_transactions(&parser.parse_all_entries()).map_err(|error| {
        StorageError::wal_error(format!(
            "Failed to validate WAL for index generation catch-up: {}",
            error
        ))
    })
}

pub(crate) fn wal_intents_for_index(
    ctx: &GraphStorageContext,
    space_id: u64,
    index: &graphdb_core::types::Index,
    start_lsn: CommitLsn,
    barrier_lsn: CommitLsn,
) -> StorageResult<Vec<graphdb_core::wal::OutboxIntent>> {
    let transactions = committed_wal_transactions(ctx)?;
    let mut index_ids = vec![index.id];
    for logical_name in [&index.name, &index.schema_name] {
        index_ids.push(stable_hash(logical_name.as_bytes()));
        index_ids.push(stable_hash(
            format!("{}:{}", space_id, logical_name).as_bytes(),
        ));
        index_ids.extend(index.fields.iter().map(|field| {
            stable_hash(format!("{}:{}:{}", space_id, logical_name, field.name).as_bytes())
        }));
    }

    Ok(filter_intents_for_indexes(
        &transactions,
        &index_ids,
        start_lsn,
        barrier_lsn,
    ))
}

fn record_changed_after(record: &IndexRecord, snapshot_timestamp: Timestamp) -> bool {
    record.created_ts > snapshot_timestamp
        || record
            .deleted_ts
            .is_some_and(|deleted_ts| deleted_ts > snapshot_timestamp)
}

pub(crate) fn replay_wal_partition<F, R>(
    (mut active_forward, mut active_reverse): IndexDataMaps,
    (rebuilt_forward, rebuilt_reverse): IndexDataMaps,
    snapshot_timestamp: Timestamp,
    intents: &[graphdb_core::wal::OutboxIntent],
    matches_forward: F,
    matches_reverse: R,
) -> IndexDataMaps
where
    F: Fn(&[u8]) -> bool,
    R: Fn(&[u8]) -> bool,
{
    let changed_entities = intents
        .iter()
        .map(|intent| &intent.mutation.entity_ref)
        .collect::<Vec<_>>();
    let matches_changed_entity = |record: &IndexRecord| {
        changed_entities.is_empty()
            || record
                .entity_ref
                .as_ref()
                .is_some_and(|entity| changed_entities.contains(&entity))
    };
    let forward_changes: Vec<(Vec<u8>, IndexRecord)> = active_forward
        .iter()
        .filter(|(key, record)| {
            matches_forward(key)
                && record_changed_after(record, snapshot_timestamp)
                && matches_changed_entity(record)
        })
        .map(|(key, record)| (key.clone(), record.clone()))
        .collect();
    let reverse_changes: Vec<(Vec<u8>, IndexRecord)> = active_reverse
        .iter()
        .filter(|(key, record)| {
            matches_reverse(key)
                && record_changed_after(record, snapshot_timestamp)
                && matches_changed_entity(record)
        })
        .map(|(key, record)| (key.clone(), record.clone()))
        .collect();

    active_forward.retain(|key, _| !matches_forward(key));
    active_reverse.retain(|key, _| !matches_reverse(key));
    active_forward.extend(rebuilt_forward);
    active_reverse.extend(rebuilt_reverse);
    active_forward.extend(forward_changes);
    active_reverse.extend(reverse_changes);
    (active_forward, active_reverse)
}
