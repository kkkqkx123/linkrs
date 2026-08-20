use std::collections::{HashMap, HashSet};

use crate::core::types::{IndexType, Timestamp, MAX_TIMESTAMP};
use crate::core::value::ordered_codec::OrderedCodec;
use crate::core::{StorageError, StorageResult, Value};
use crate::storage::index::helpers::{
    edge_entity_ref, effective_index_values, merged_included_columns,
};
use crate::storage::index::key_codec::key_builder::normalize_int_value;
use crate::storage::index::key_codec::{KeyBuilder, KeyParser};
use crate::storage::index::traits::EdgeIndexOps;
use crate::storage::index::types::{EdgeIdentity, IndexIdentity, IndexRecord};
use crate::storage::index::{shard_runtime::IndexMaps, IndexDataManagerImpl};

fn add_entry(
    per_shard: &mut HashMap<u32, IndexMaps>,
    shard_id: u32,
    fwd_key: Vec<u8>,
    rev_key: Vec<u8>,
    entry: IndexRecord,
) {
    let (ref mut fwd_map, ref mut rev_map) = per_shard.entry(shard_id).or_default();
    fwd_map.insert(fwd_key, entry.clone());
    rev_map.insert(rev_key, entry);
}

impl EdgeIndexOps for IndexDataManagerImpl {
    fn update_edge_indexes_mvcc(
        &self,
        edge: &EdgeIdentity<'_>,
        index_name: &str,
        props: &[(String, Value)],
        write_ts: Timestamp,
    ) -> Result<(), StorageError> {
        let EdgeIdentity {
            space_id,
            src: edge_src,
            dst: edge_dst,
            edge_type,
            ranking,
        } = *edge;
        let Some(index_id) = self.index_alias(space_id, index_name) else {
            return Ok(());
        };
        let identity = IndexIdentity { space_id, index_id };
        let runtime = self.runtime(space_id, index_id)?;

        let delta = {
            self.wait_for_active_barrier(&runtime);
            let catalog = self
                .manifest_catalog(space_id, index_id)
                .ok_or_else(|| StorageError::not_found("Index manifest catalog is unavailable"))?;
            let manifest = catalog.acquire();
            let index_definition = self.index_definitions.read().get(&identity).cloned();

            let covering = index_definition.as_ref().is_some_and(|idx| idx.covering);
            let new_values = effective_index_values(index_definition.as_ref(), props, Vec::new());

            let values: Vec<Value>;
            let included_columns: Vec<(String, Value)>;
            if !new_values.is_empty() {
                values = new_values;
                included_columns = if covering {
                    merged_included_columns(index_definition.as_ref(), Vec::new(), props)
                } else {
                    Vec::new()
                };
            } else {
                let has_included = index_definition
                    .as_ref()
                    .is_some_and(|idx| !idx.properties.is_empty());
                if !has_included {
                    return Ok(());
                }
                let latest_gen = runtime.generation(manifest.manifest().generation);
                let reverse_prefix = KeyBuilder::build_edge_reverse_key(
                    space_id, edge_src, edge_dst, edge_type, ranking, index_name,
                )?;
                let reverse_end = KeyBuilder::build_range_end(&reverse_prefix);
                let mut existing_values = Vec::new();
                let mut existing_encoded: HashSet<Vec<u8>> = HashSet::new();
                let mut existing_columns = Vec::new();
                let mut covering_populated = false;
                if let Some(latest_gen) = latest_gen {
                    for shard in latest_gen.shards() {
                        if !shard.reverse_may_have_range(&reverse_prefix.0, &reverse_end.0) {
                            continue;
                        }
                        for (_suffix, record) in shard.reverse_range_suffix_visible(
                            &reverse_prefix.0,
                            &reverse_end.0,
                            write_ts,
                        ) {
                            if let Ok(encoded) =
                                KeyParser::extract_value_from_edge_reverse_suffix(&_suffix)
                            {
                                if existing_encoded.insert(encoded.clone()) {
                                    if let Ok(value) = OrderedCodec::new().decode(&encoded) {
                                        existing_values.push(normalize_int_value(&value));
                                    }
                                }
                            }
                            if covering && !covering_populated {
                                if let Some(cols) = &record.included_columns {
                                    existing_columns.clone_from(cols);
                                    covering_populated = true;
                                }
                            }
                        }
                    }
                }
                // also account for entries still awaiting generation publication.
                let pending_guard = self.pending_deltas.lock();
                if let Some(pending) = pending_guard.get(&identity) {
                    let mut scan = crate::storage::index::index_data_manager::PendingExistingScan {
                        existing_values: &mut existing_values,
                        existing_encoded: &mut existing_encoded,
                        existing_columns: &mut existing_columns,
                        covering_populated: &mut covering_populated,
                    };
                    crate::storage::index::index_data_manager::merge_pending_existing_values(
                        pending,
                        &reverse_prefix.0,
                        &reverse_end.0,
                        write_ts,
                        true,
                        &mut scan,
                    );
                }
                drop(pending_guard);
                if existing_values.is_empty() {
                    return Ok(());
                }
                values = existing_values;
                included_columns = if covering {
                    merged_included_columns(index_definition.as_ref(), existing_columns, props)
                } else {
                    Vec::new()
                };
            }

            let mut per_shard: HashMap<u32, IndexMaps> = HashMap::new();

            let route = |forward: &[u8]| -> StorageResult<u32> {
                manifest
                    .manifest()
                    .route_key(forward)
                    .map(|s| s.shard_id)
                    .ok_or_else(|| {
                        StorageError::invalid_operation(
                            "Index manifest does not cover the ordered key",
                        )
                    })
            };

            for value in &values {
                let forward = KeyBuilder::build_edge_index_key(
                    space_id, index_name, value, edge_src, edge_dst, edge_type, ranking,
                )?;
                let reverse = KeyBuilder::build_edge_reverse_key_with_value(
                    space_id, edge_src, edge_dst, edge_type, ranking, index_name, value,
                )?;
                let shard_id = route(&forward.0)?;
                let mut entry = if covering {
                    IndexRecord::new_with_columns(write_ts, included_columns.clone())
                } else {
                    IndexRecord::new(write_ts)
                }
                .with_entity_version(write_ts);
                if let Some(entity) = edge_entity_ref(edge_src, edge_dst, edge_type, ranking) {
                    entry = entry.with_entity_ref(entity);
                }
                add_entry(&mut per_shard, shard_id, forward.0, reverse.0, entry);
            }

            per_shard
        };

        if !delta.is_empty() {
            self.accumulate_delta(identity, delta, write_ts)?;
        }
        Ok(())
    }

    fn delete_edge_indexes_mvcc(
        &self,
        edge: &EdgeIdentity<'_>,
        index_names: &[String],
        write_ts: Timestamp,
    ) -> Result<(), StorageError> {
        for index_name in index_names {
            self.clear_edge_entity(edge, index_name, write_ts)?;
        }
        Ok(())
    }

    fn clear_edge_index(&self, space_id: u64, index_name: &str) -> Result<(), StorageError> {
        let Some(index_id) = self.index_alias(space_id, index_name) else {
            return Ok(());
        };
        self.clear_index(
            index_id,
            space_id,
            index_name,
            IndexType::EdgeIndex,
            MAX_TIMESTAMP,
        )
    }
}
