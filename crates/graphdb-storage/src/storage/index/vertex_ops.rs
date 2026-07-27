use std::collections::HashMap;

use crate::core::types::{Index, IndexType, Timestamp, MAX_TIMESTAMP};
use crate::core::value::ordered_codec::OrderedCodec;
use crate::core::{StorageError, StorageResult, Value};
use crate::storage::index::helpers::{
    effective_index_values, merged_included_columns, vertex_entity_ref,
};
use crate::storage::index::key_codec::key_builder::normalize_int_value;
use crate::storage::index::key_codec::key_types::SecondaryIndexKey;
use crate::storage::index::key_codec::{KeyBuilder, KeyParser};
use crate::storage::index::traits::VertexIndexOps;
use crate::storage::index::types::{IndexIdentity, IndexRecord};
use crate::storage::index::{shard_runtime::IndexMaps, IndexDataManagerImpl};

fn add_entry(
    per_shard: &mut HashMap<u32, IndexMaps>,
    shard_id: u32,
    fwd_key: SecondaryIndexKey,
    rev_key: SecondaryIndexKey,
    entry: IndexRecord,
) {
    let (ref mut fwd_map, ref mut rev_map) = per_shard.entry(shard_id).or_default();
    fwd_map.insert(fwd_key, entry.clone());
    rev_map.insert(rev_key, entry);
}

impl VertexIndexOps for IndexDataManagerImpl {
    fn update_vertex_indexes_mvcc(
        &self,
        space_id: u64,
        vertex_id: &Value,
        index_name: &str,
        props: &[(String, Value)],
        write_ts: Timestamp,
    ) -> Result<(), StorageError> {
        let Some(index_id) = self.index_alias(space_id, index_name) else {
            return Ok(());
        };
        let identity = IndexIdentity { space_id, index_id };
        let runtime = self.runtime(space_id, index_id)?;

        let delta = {
            let _fence = runtime.read_fence();
            self.wait_for_active_barrier(&runtime);
            let catalog = self
                .manifest_catalog(space_id, index_id)
                .ok_or_else(|| StorageError::not_found("Index manifest catalog is unavailable"))?;
            let manifest = catalog.acquire();
            let chain = runtime.generation_chain_until(manifest.manifest().generation)?;
            let index_definition = self
                .index_definitions
                .read()
                .get(&identity)
                .cloned();
            let reverse_prefix =
                KeyBuilder::build_vertex_reverse_key_v2(space_id, vertex_id, index_name)?;
            let reverse_end = KeyBuilder::build_range_end(&reverse_prefix);
            let mut existing_values = Vec::new();
            let mut existing_columns = Vec::new();
            let mut existing_columns_ts = 0;
            for gen in &chain {
                for shard in gen.shards() {
                    for (key, record) in shard
                        .reverse_range(&reverse_prefix.0, &reverse_end.0)
                    {
                        if !record.is_visible_at(write_ts) {
                            continue;
                        }
                        if let Ok(encoded) = KeyParser::extract_value_from_reverse_key(&key) {
                            if let Ok(value) = OrderedCodec::new().decode(&encoded) {
                                let nv = normalize_int_value(&value);
                                if !existing_values.contains(&nv) {
                                    existing_values.push(nv);
                                }
                            }
                        }
                        if record.created_ts >= existing_columns_ts {
                            existing_columns_ts = record.created_ts;
                            existing_columns = record.included_columns.clone().unwrap_or_default();
                        }
                    }
                }
            }
            let new_values =
                effective_index_values(index_definition.as_ref(), props, existing_values.clone());
            let included_columns =
                merged_included_columns(index_definition.as_ref(), existing_columns, props);

            let values_to_remove: Vec<&Value> = existing_values
                .iter()
                .filter(|v| !new_values.contains(v))
                .collect();
            let values_to_add: Vec<&Value> = new_values
                .iter()
                .filter(|v| !existing_values.contains(v))
                .collect();
            let values_to_update: Vec<&Value> = new_values
                .iter()
                .filter(|v| existing_values.contains(v))
                .collect();

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

            for value in &values_to_update {
                let forward =
                    KeyBuilder::build_vertex_index_key(space_id, index_name, value, vertex_id)?;
                let reverse =
                    KeyBuilder::build_vertex_reverse_key_with_value(space_id, vertex_id, index_name, value)?;
                let shard_id = route(&forward.0)?;
                let mut entry = IndexRecord::new_with_columns(write_ts, included_columns.clone())
                    .with_entity_version(write_ts);
                if let Some(entity) = vertex_entity_ref(vertex_id) {
                    entry = entry.with_entity_ref(entity);
                }
                add_entry(&mut per_shard, shard_id, forward.0, reverse.0, entry);
            }

            for value in &values_to_remove {
                let forward =
                    KeyBuilder::build_vertex_index_key(space_id, index_name, value, vertex_id)?;
                let reverse =
                    KeyBuilder::build_vertex_reverse_key_with_value(space_id, vertex_id, index_name, value)?;
                let shard_id = route(&forward.0)?;
                let mut entry = IndexRecord::new_with_columns(write_ts, included_columns.clone())
                    .with_entity_version(write_ts);
                entry.mark_deleted(write_ts);
                if let Some(entity) = vertex_entity_ref(vertex_id) {
                    entry = entry.with_entity_ref(entity);
                }
                add_entry(&mut per_shard, shard_id, forward.0, reverse.0, entry);
            }

            for value in &values_to_add {
                let forward =
                    KeyBuilder::build_vertex_index_key(space_id, index_name, value, vertex_id)?;
                let reverse =
                    KeyBuilder::build_vertex_reverse_key_with_value(space_id, vertex_id, index_name, value)?;
                let shard_id = route(&forward.0)?;
                let mut entry = IndexRecord::new_with_columns(write_ts, included_columns.clone())
                    .with_entity_version(write_ts);
                if let Some(entity) = vertex_entity_ref(vertex_id) {
                    entry = entry.with_entity_ref(entity);
                }
                add_entry(&mut per_shard, shard_id, forward.0, reverse.0, entry);
            }

            per_shard
        };

        if !delta.is_empty() {
            self.publish_delta_generation(identity, delta, write_ts)?;
        }
        Ok(())
    }

    fn delete_vertex_indexes_mvcc(
        &self,
        space_id: u64,
        vertex_id: &Value,
        index_names: &[String],
        write_ts: Timestamp,
    ) -> Result<(), StorageError> {
        for index_name in index_names {
            self.clear_vertex_entity(space_id, vertex_id, index_name, write_ts)?;
        }
        Ok(())
    }

    fn lookup_tag_index_mvcc(
        &self,
        space_id: u64,
        index: &Index,
        value: &Value,
        read_ts: Timestamp,
    ) -> Result<Vec<Value>, StorageError> {
        let Some(index_id) = self.index_alias(space_id, &index.name) else {
            return Ok(Vec::new());
        };
        let runtime = self.runtime(space_id, index_id)?;
        let _fence = runtime.read_fence();
        let catalog = self
            .manifest_catalog(space_id, index_id)
            .ok_or_else(|| StorageError::not_found(format!("Index {index_id} has no manifest")))?;
        let handle = catalog.acquire();
        let manifest = handle.manifest();
        let chain = runtime.generation_chain_until(manifest.generation)?;
        let prefix = KeyBuilder::build_vertex_index_value_prefix(space_id, &index.name, value)?;
        let end = KeyBuilder::build_range_end(&prefix);
        let mut seen = std::collections::HashSet::new();
        let mut tombstoned = std::collections::HashSet::new();
        let mut results = Vec::new();
            for generation in &chain {
                for shard in manifest
                    .shards
                    .iter()
                    .filter_map(|s| generation.shard(s.shard_id))
                {
                    for (key, entry) in shard
                        .forward_range(&prefix.0, &end.0)
                {
                    if let Ok(vertex_id) = KeyParser::parse_vertex_id_from_key(&key) {
                        if tombstoned.contains(&vertex_id) {
                            continue;
                        }
                        if seen.contains(&vertex_id) {
                            continue;
                        }
                        if entry.created_ts > read_ts {
                            continue;
                        }
                        if entry.deleted_ts.is_some_and(|d| d <= read_ts) {
                            tombstoned.insert(vertex_id);
                            continue;
                        }
                        seen.insert(vertex_id.clone());
                        results.push(vertex_id);
                    }
                }
            }
        }
        Ok(results)
    }

    fn clear_tag_index(&self, space_id: u64, index_name: &str) -> Result<(), StorageError> {
        let Some(index_id) = self.index_alias(space_id, index_name) else {
            return Ok(());
        };
        self.clear_index(
            index_id,
            space_id,
            index_name,
            IndexType::TagIndex,
            MAX_TIMESTAMP,
        )
    }
}
