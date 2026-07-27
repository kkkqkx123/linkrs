use crate::core::types::{IndexType, Timestamp, MAX_TIMESTAMP};
use crate::core::value::ordered_codec::OrderedCodec;
use crate::core::{StorageError, Value};
use crate::storage::index::helpers::{
    effective_index_values, edge_entity_ref, merged_included_columns,
};
use crate::storage::index::key_codec::key_builder::normalize_int_value;
use crate::storage::index::key_codec::{KeyBuilder, KeyParser};
use crate::storage::index::traits::EdgeIndexOps;
use crate::storage::index::types::{EdgeIdentity, IndexIdentity, IndexRecord};
use crate::storage::index::IndexDataManagerImpl;

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
        let runtime = self.runtime(space_id, index_id)?;
        let _fence = runtime.read_fence();
        self.wait_for_active_barrier(&runtime);
        let catalog = self
            .manifest_catalog(space_id, index_id)
            .ok_or_else(|| StorageError::not_found("Index manifest catalog is unavailable"))?;
        let manifest = catalog.acquire();
        let generation = runtime
            .generation(manifest.manifest().generation)
            .ok_or_else(|| {
                StorageError::not_found("Active index runtime generation is unavailable")
            })?;
        let index_definition = self
            .index_definitions
            .read()
            .get(&IndexIdentity { space_id, index_id })
            .cloned();
        let reverse_prefix = KeyBuilder::build_edge_reverse_key(
            space_id, edge_src, edge_dst, edge_type, ranking, index_name,
        )?;
        let reverse_end = KeyBuilder::build_range_end(&reverse_prefix);
        let mut existing_values = Vec::new();
        let mut existing_columns = Vec::new();
        let mut existing_columns_ts = 0;
        for shard in generation.shards() {
            for (key, record) in shard
                .read_reverse()
                .range(reverse_prefix.0.clone()..reverse_end.0.clone())
            {
                if !record.is_visible_at(write_ts) {
                    continue;
                }
                if let Ok(encoded) = KeyParser::extract_value_from_edge_reverse_key(key) {
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

        for value in &values_to_update {
            let forward = KeyBuilder::build_edge_index_key(
                space_id, index_name, value, edge_src, edge_dst, edge_type, ranking,
            )?;
            let reverse = KeyBuilder::build_edge_reverse_key_with_value(
                space_id, edge_src, edge_dst, edge_type, ranking, index_name, value,
            )?;
            let target = manifest
                .manifest()
                .route_key(&forward.0)
                .and_then(|shard| generation.shard(shard.shard_id))
                .ok_or_else(|| {
                    StorageError::invalid_operation("Index manifest does not cover the ordered key")
                })?;
            let fwd_key = forward.0.clone();
            target.update_forward(|map| {
                if let Some(entry) = map.get_mut(&fwd_key) {
                    entry.mark_deleted(write_ts);
                }
            });
            let rev_key = reverse.0.clone();
            target.update_reverse(|map| {
                if let Some(entry) = map.get_mut(&rev_key) {
                    entry.mark_deleted(write_ts);
                }
            });
            let entity_ref = edge_entity_ref(edge_src, edge_dst, edge_type, ranking);
            let mut entry = IndexRecord::new_with_columns(write_ts, included_columns.clone())
                .with_entity_version(write_ts);
            if let Some(entity) = entity_ref {
                entry = entry.with_entity_ref(entity);
            }
            target.update_forward(|map| {
                map.insert(target.versioned_key(&forward.0), entry.clone());
            });
            target.update_reverse(|map| {
                map.insert(target.versioned_key(&reverse.0), entry);
            });
            target.mark_dirty();
        }

        for value in &values_to_remove {
            let forward = KeyBuilder::build_edge_index_key(
                space_id, index_name, value, edge_src, edge_dst, edge_type, ranking,
            )?;
            let reverse = KeyBuilder::build_edge_reverse_key(
                space_id, edge_src, edge_dst, edge_type, ranking, index_name,
            )?;
            let target = manifest
                .manifest()
                .route_key(&forward.0)
                .and_then(|shard| generation.shard(shard.shard_id))
                .ok_or_else(|| {
                    StorageError::invalid_operation("Index manifest does not cover the ordered key")
                })?;
            let forward_end = KeyBuilder::build_range_end(&forward);
            let fwd_keys: Vec<_> = target
                .read_forward()
                .range(forward.0.clone()..forward_end.0.clone())
                .filter(|(_, entry)| entry.is_visible_at(write_ts))
                .map(|(key, _)| key.clone())
                .collect();
            if !fwd_keys.is_empty() {
                target.update_forward(|map| {
                    for key in &fwd_keys {
                        if let Some(entry) = map.get_mut(key) {
                            entry.mark_deleted(write_ts);
                        }
                    }
                });
            }
            let reverse_end = KeyBuilder::build_range_end(&reverse);
            let rev_keys: Vec<_> = target
                .read_reverse()
                .range(reverse.0.clone()..reverse_end.0.clone())
                .filter(|(_, entry)| entry.is_visible_at(write_ts))
                .map(|(key, _)| key.clone())
                .collect();
            if !rev_keys.is_empty() {
                target.update_reverse(|map| {
                    for key in &rev_keys {
                        if let Some(entry) = map.get_mut(key) {
                            entry.mark_deleted(write_ts);
                        }
                    }
                });
            }
            if !fwd_keys.is_empty() || !rev_keys.is_empty() {
                target.mark_dirty();
            }
        }

        for value in &values_to_add {
            let forward = KeyBuilder::build_edge_index_key(
                space_id, index_name, value, edge_src, edge_dst, edge_type, ranking,
            )?;
            let reverse = KeyBuilder::build_edge_reverse_key_with_value(
                space_id, edge_src, edge_dst, edge_type, ranking, index_name, value,
            )?;
            let target = manifest
                .manifest()
                .route_key(&forward.0)
                .and_then(|shard| generation.shard(shard.shard_id))
                .ok_or_else(|| {
                    StorageError::invalid_operation("Index manifest does not cover the ordered key")
                })?;
            let entity_ref = edge_entity_ref(edge_src, edge_dst, edge_type, ranking);
            let mut entry = IndexRecord::new_with_columns(write_ts, included_columns.clone())
                .with_entity_version(write_ts);
            if let Some(entity) = entity_ref {
                entry = entry.with_entity_ref(entity);
            }
            target.update_forward(|map| {
                map.insert(forward.0, entry.clone());
            });
            target.update_reverse(|map| {
                map.insert(reverse.0, entry);
            });
            target.mark_dirty();
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
