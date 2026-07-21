use crate::core::types::{Index, IndexType, Timestamp, MAX_TIMESTAMP};
use crate::core::{StorageError, Value};
use crate::storage::index::helpers::{
    edge_entity_ref, effective_index_values, merged_included_columns,
};
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
        let prefix = KeyBuilder::build_edge_index_prefix(space_id, index_name);
        let end = KeyBuilder::build_range_end(&prefix);
        let expected_entity = edge_entity_ref(edge_src, edge_dst, edge_type, ranking);
        let mut existing_values = Vec::new();
        let mut existing_columns = Vec::new();
        let mut existing_columns_ts = 0;
        for shard in generation.shards() {
            for (key, record) in shard
                .forward()
                .read()
                .range(prefix.0.clone()..end.0.clone())
            {
                if !record.is_visible_at(write_ts) {
                    continue;
                }
                let matches_entity = record
                    .entity_ref
                    .as_ref()
                    .zip(expected_entity.as_ref())
                    .is_some_and(|(actual, expected)| actual == expected)
                    || KeyParser::parse_edge_identity_from_key(key).is_ok_and(
                        |(candidate_src, candidate_dst, candidate_type, candidate_rank)| {
                            candidate_src == *edge_src
                                && candidate_dst == *edge_dst
                                && candidate_type == edge_type
                                && candidate_rank == ranking
                        },
                    );
                if !matches_entity {
                    continue;
                }
                if let Ok(value) = KeyParser::parse_prop_value_from_edge_key(key) {
                    if !existing_values.contains(&value) {
                        existing_values.push(value);
                    }
                }
                if record.created_ts >= existing_columns_ts {
                    existing_columns_ts = record.created_ts;
                    existing_columns = record.included_columns.clone();
                }
            }
        }
        let indexed_values =
            effective_index_values(index_definition.as_ref(), props, existing_values);
        let included_columns =
            merged_included_columns(index_definition.as_ref(), existing_columns, props);
        if !indexed_values.is_empty() {
            self.clear_edge_entity(edge, index_name, write_ts)?;
        }
        for value in indexed_values {
            let forward = KeyBuilder::build_edge_index_key(
                space_id, index_name, &value, edge_src, edge_dst, edge_type, ranking,
            )?;
            let reverse = KeyBuilder::build_edge_reverse_key(
                space_id, edge_src, edge_dst, edge_type, ranking, index_name,
            )?;
            let forward_end = KeyBuilder::build_range_end(&forward);
            let reverse_end = KeyBuilder::build_range_end(&reverse);
            for shard in generation.shards() {
                let keys = shard
                    .forward()
                    .read()
                    .range(forward.0.clone()..forward_end.0.clone())
                    .filter(|(_, entry)| entry.is_visible_at(write_ts))
                    .map(|(key, _)| key.clone())
                    .collect::<Vec<_>>();
                let mut data = shard.forward().write();
                for key in keys {
                    if let Some(entry) = data.get_mut(&key) {
                        entry.mark_deleted(write_ts);
                    }
                }
                let keys = shard
                    .reverse()
                    .read()
                    .range(reverse.0.clone()..reverse_end.0.clone())
                    .filter(|(_, entry)| entry.is_visible_at(write_ts))
                    .map(|(key, _)| key.clone())
                    .collect::<Vec<_>>();
                let mut data = shard.reverse().write();
                for key in keys {
                    if let Some(entry) = data.get_mut(&key) {
                        entry.mark_deleted(write_ts);
                    }
                }
            }
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
            target
                .forward()
                .write()
                .insert(target.physical_key(&forward.0), entry.clone());
            target
                .reverse()
                .write()
                .insert(target.physical_key(&reverse.0), entry);
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

    fn lookup_edge_index_mvcc(
        &self,
        space_id: u64,
        index: &Index,
        value: &Value,
        read_ts: Timestamp,
    ) -> Result<Vec<(Value, Value, String, i64)>, StorageError> {
        let Some(index_id) = self.index_alias(space_id, &index.name) else {
            return Ok(Vec::new());
        };
        let runtime = self.runtime(space_id, index_id)?;
        let _fence = runtime.read_fence();
        let (manifest, _runtime, generation) = self.active_generation(space_id, index_id)?;
        let prefix = KeyBuilder::build_edge_index_prefix(space_id, &index.name);
        let end = KeyBuilder::build_range_end(&prefix);
        let mut seen = std::collections::HashSet::new();
        let mut results = Vec::new();
        for shard in manifest
            .manifest()
            .shards
            .iter()
            .filter_map(|shard| generation.shard(shard.shard_id))
        {
            for (key, entry) in shard
                .forward()
                .read()
                .range(prefix.0.clone()..end.0.clone())
            {
                if !entry.is_visible_at(read_ts) {
                    continue;
                }
                if !KeyParser::parse_prop_value_from_edge_key(key)
                    .is_ok_and(|stored| stored == *value)
                {
                    continue;
                }
                if let Ok((src, dst, edge_type, ranking)) =
                    KeyParser::parse_edge_identity_from_key(key)
                {
                    if seen.insert((src.clone(), dst.clone(), edge_type.clone(), ranking)) {
                        results.push((src, dst, edge_type, ranking));
                    }
                }
            }
        }
        Ok(results)
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
