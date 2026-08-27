#[cfg(test)]
use crate::core::types::IndexType;
use crate::core::types::{Index, Timestamp};
use crate::core::wal::{EntityRef, OutboxIntent};
use crate::core::{StorageError, StorageResult, Value};
use crate::index::chunk::chunked_index::ChunkedIndex;
use crate::index::chunk::serialize::write_chunked_index_checkpoint;
use crate::index::key_codec::key_types::SecondaryIndexKey;
use crate::index::manifest::IndexManifest;
use crate::index::shard_runtime::{GenerationRuntime, IndexMaps};
use crate::index::types::IndexRecord;
use std::collections::{BTreeMap, HashMap};

pub(crate) fn merge_split_wal_changes<F, R>(
    maps: &mut HashMap<u32, IndexMaps>,
    manifest: &IndexManifest,
    active_forward: &BTreeMap<SecondaryIndexKey, IndexRecord>,
    active_reverse: &BTreeMap<SecondaryIndexKey, IndexRecord>,
    intents: &[OutboxIntent],
    matches_forward: F,
    matches_reverse: R,
) -> StorageResult<()>
where
    F: Fn(&[u8]) -> bool,
    R: Fn(&[u8]) -> bool,
{
    let mut changed_entities = Vec::new();
    for entity in intents.iter().filter_map(entity_ref_from_intent) {
        if !changed_entities.contains(&entity) {
            changed_entities.push(entity);
        }
    }
    if changed_entities.is_empty() {
        return Ok(());
    }

    for (forward, reverse) in maps.values_mut() {
        forward.retain(|key, record| {
            !(matches_forward(key)
                && record
                    .entity_ref
                    .as_ref()
                    .is_some_and(|entity| changed_entities.contains(entity)))
        });
        reverse.retain(|key, record| {
            !(matches_reverse(key)
                && record
                    .entity_ref
                    .as_ref()
                    .is_some_and(|entity| changed_entities.contains(entity)))
        });
    }

    let mut entity_shards: Vec<(EntityRef, (Timestamp, u32))> = Vec::new();
    for (key, record) in active_forward {
        if !matches_forward(key) {
            continue;
        }
        let Some(entity) = record.entity_ref.clone() else {
            continue;
        };
        let shard_id = manifest
            .route_key(key)
            .ok_or_else(|| {
                StorageError::invalid_operation(
                    "WAL catch-up produced an index key outside the split manifest",
                )
            })?
            .shard_id;
        if let Some((_, (ts, current_shard))) = entity_shards
            .iter_mut()
            .find(|(candidate, _)| *candidate == entity)
        {
            if record.created_ts >= *ts {
                *ts = record.created_ts;
                *current_shard = shard_id;
            }
        } else {
            entity_shards.push((entity, (record.created_ts, shard_id)));
        }
    }

    for (key, record) in active_forward {
        if !matches_forward(key) {
            continue;
        }
        let shard = manifest.route_key(key).ok_or_else(|| {
            StorageError::invalid_operation(
                "WAL catch-up produced an index key outside the split manifest",
            )
        })?;
        maps.entry(shard.shard_id)
            .or_insert_with(|| (BTreeMap::new(), BTreeMap::new()))
            .0
            .insert(key.clone(), record.clone());
    }

    for (key, record) in active_reverse {
        if !matches_reverse(key) {
            continue;
        }
        let entity = record.entity_ref.as_ref().ok_or_else(|| {
            StorageError::invalid_operation("WAL catch-up reverse record has no owning entity")
        })?;
        let shard_id = entity_shards
            .iter()
            .find(|(candidate, _)| candidate == entity)
            .map(|(_, (_, shard_id))| *shard_id)
            .ok_or_else(|| {
                StorageError::invalid_operation(
                    "WAL catch-up reverse record has no routable forward record",
                )
            })?;
        maps.entry(shard_id)
            .or_insert_with(|| (BTreeMap::new(), BTreeMap::new()))
            .1
            .insert(key.clone(), record.clone());
    }

    Ok(())
}

fn entity_ref_from_intent(intent: &OutboxIntent) -> Option<EntityRef> {
    Some(intent.mutation.entity_ref.clone())
}

pub(crate) fn flush_split_generation(
    manifest: &IndexManifest,
    runtime: &GenerationRuntime,
) -> StorageResult<()> {
    for entry in &manifest.shards {
        let data = runtime
            .shard(entry.shard_id)
            .ok_or_else(|| StorageError::not_found("Split generation shard is unavailable"))?;
        let (forward, reverse) = data.snapshot();
        let fwd_dir = entry.checkpoint_file.join("forward_chunks");
        let rev_dir = entry.checkpoint_file.join("reverse_chunks");
        let fwd = ChunkedIndex::from_btree(vec![], &forward, data.pool_capacity);
        let rev = ChunkedIndex::from_btree(vec![], &reverse, data.pool_capacity);
        write_chunked_index_checkpoint(&fwd_dir, &fwd)?;
        write_chunked_index_checkpoint(&rev_dir, &rev)?;
    }
    Ok(())
}

pub(crate) fn vertex_entity_ref(value: &Value) -> Option<EntityRef> {
    match value {
        Value::BigInt(id) => Some(EntityRef::Vertex(
            crate::core::types::storage_ids::VertexId::from_int64(*id),
        )),
        Value::Int(id) => Some(EntityRef::Vertex(
            crate::core::types::storage_ids::VertexId::from_int64(*id as i64),
        )),
        Value::String(id) => Some(EntityRef::Vertex(id.parse::<i64>().map_or_else(
            |_| crate::core::types::storage_ids::VertexId::from_string(id.clone()),
            crate::core::types::storage_ids::VertexId::from_int64,
        ))),
        Value::Vertex(vertex) => Some(EntityRef::Vertex(vertex.vid)),
        _ => None,
    }
}

pub(crate) fn edge_entity_ref(
    src: &Value,
    dst: &Value,
    edge_type: &str,
    ranking: i64,
) -> Option<EntityRef> {
    let EntityRef::Vertex(src) = vertex_entity_ref(src)? else {
        return None;
    };
    let EntityRef::Vertex(dst) = vertex_entity_ref(dst)? else {
        return None;
    };
    Some(EntityRef::Edge {
        src,
        dst,
        edge_type: stable_hash(edge_type.as_bytes()) as u32,
        ranking,
    })
}

pub(crate) fn stable_hash(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

pub(crate) fn effective_index_values(
    index_definition: Option<&Index>,
    props: &[(String, Value)],
    existing_values: Vec<Value>,
) -> Vec<Value> {
    let Some(index) = index_definition else {
        return props.iter().map(|(_, value)| value.clone()).collect();
    };
    let values = index
        .fields
        .iter()
        .filter_map(|field| {
            props
                .iter()
                .find(|(name, _)| name == &field.name)
                .map(|(_, value)| value.clone())
        })
        .collect::<Vec<_>>();
    if values.is_empty() {
        existing_values
    } else {
        values
    }
}

pub(crate) fn merged_included_columns(
    index_definition: Option<&Index>,
    mut existing: Vec<(String, Value)>,
    props: &[(String, Value)],
) -> Vec<(String, Value)> {
    let Some(index) = index_definition else {
        return props.to_vec();
    };
    for name in &index.properties {
        let Some((_, value)) = props.iter().find(|(candidate, _)| candidate == name) else {
            continue;
        };
        if let Some((_, existing_value)) =
            existing.iter_mut().find(|(candidate, _)| candidate == name)
        {
            *existing_value = value.clone();
        } else {
            existing.push((name.clone(), value.clone()));
        }
    }
    existing
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::storage_ids::VertexId;
    use crate::core::types::{IndexConfig, IndexField};

    #[test]
    fn stable_hash_is_deterministic() {
        let a = stable_hash(b"hello");
        let b = stable_hash(b"hello");
        assert_eq!(a, b);
    }

    #[test]
    fn stable_hash_different_inputs_differ() {
        let a = stable_hash(b"hello");
        let b = stable_hash(b"world");
        assert_ne!(a, b);
    }

    #[test]
    fn stable_hash_empty_input() {
        let hash = stable_hash(b"");
        assert_ne!(hash, 0);
    }

    #[test]
    fn vertex_entity_ref_int() {
        let v = Value::Int(42);
        let entity = vertex_entity_ref(&v).expect("should resolve");
        assert_eq!(entity, EntityRef::Vertex(VertexId::from_int64(42)));
    }

    #[test]
    fn vertex_entity_ref_bigint() {
        let v = Value::BigInt(100);
        let entity = vertex_entity_ref(&v).expect("should resolve");
        assert_eq!(entity, EntityRef::Vertex(VertexId::from_int64(100)));
    }

    #[test]
    fn vertex_entity_ref_string_numeric() {
        let v = Value::string("42");
        let entity = vertex_entity_ref(&v).expect("should resolve");
        assert_eq!(entity, EntityRef::Vertex(VertexId::from_int64(42)));
    }

    #[test]
    fn vertex_entity_ref_string_non_numeric() {
        let v = Value::string("uuid-abc");
        let entity = vertex_entity_ref(&v).expect("should resolve");
        assert_eq!(
            entity,
            EntityRef::Vertex(VertexId::from_string("uuid-abc".to_string()))
        );
    }

    #[test]
    fn vertex_entity_ref_float_returns_none() {
        let v = Value::Float(1.5);
        assert!(vertex_entity_ref(&v).is_none());
    }

    #[test]
    fn edge_entity_ref_happy_path() {
        let src = Value::Int(1);
        let dst = Value::Int(2);
        let entity = edge_entity_ref(&src, &dst, "KNOWS", 0).expect("should resolve");
        let EntityRef::Edge {
            src: s,
            dst: d,
            edge_type,
            ranking,
        } = entity
        else {
            panic!("expected Edge entity ref");
        };
        assert_eq!(s, VertexId::from_int64(1));
        assert_eq!(d, VertexId::from_int64(2));
        assert_eq!(edge_type, stable_hash(b"KNOWS") as u32);
        assert_eq!(ranking, 0);
    }

    #[test]
    fn edge_entity_ref_float_src_returns_none() {
        let src = Value::Float(1.5);
        let dst = Value::Int(2);
        assert!(edge_entity_ref(&src, &dst, "KNOWS", 0).is_none());
    }

    #[test]
    fn effective_index_values_without_definition_uses_all_props() {
        let props = vec![
            ("name".to_string(), Value::string("Alice")),
            ("age".to_string(), Value::Int(30)),
        ];
        let values = effective_index_values(None, &props, vec![]);
        assert_eq!(values.len(), 2);
    }

    #[test]
    fn effective_index_values_with_definition_filters_by_fields() {
        let index = Index::new(IndexConfig {
            id: 1,
            name: "idx_name".to_string(),
            space_id: 1,
            schema_name: "person".to_string(),
            fields: vec![IndexField::new(
                "name".to_string(),
                Value::string(""),
                false,
            )],
            properties: vec![],
            index_type: IndexType::TagIndex,
            is_unique: false,
            covering: false,
            partial_condition: None,
        });
        let props = vec![
            ("name".to_string(), Value::string("Alice")),
            ("age".to_string(), Value::Int(30)),
        ];
        let values = effective_index_values(Some(&index), &props, vec![]);
        assert_eq!(values, vec![Value::string("Alice")]);
    }

    #[test]
    fn effective_index_values_falls_back_to_existing() {
        let index = Index::new(IndexConfig {
            id: 1,
            name: "idx_name".to_string(),
            space_id: 1,
            schema_name: "person".to_string(),
            fields: vec![IndexField::new(
                "name".to_string(),
                Value::string(""),
                false,
            )],
            properties: vec![],
            index_type: IndexType::TagIndex,
            is_unique: false,
            covering: false,
            partial_condition: None,
        });
        let props = vec![("age".to_string(), Value::Int(30))];
        let existing = vec![Value::string("Bob")];
        let values = effective_index_values(Some(&index), &props, existing.clone());
        assert_eq!(values, existing);
    }

    #[test]
    fn merged_included_columns_without_definition_returns_props() {
        let props = vec![("since".to_string(), Value::Int(2020))];
        let merged = merged_included_columns(None, vec![], &props);
        assert_eq!(merged, props);
    }

    #[test]
    fn merged_included_columns_updates_existing() {
        let index = Index::new(IndexConfig {
            id: 1,
            name: "idx_weight".to_string(),
            space_id: 1,
            schema_name: "knows".to_string(),
            fields: vec![IndexField::new("weight".to_string(), Value::Int(0), false)],
            properties: vec!["since".to_string()],
            index_type: IndexType::EdgeIndex,
            is_unique: false,
            covering: false,
            partial_condition: None,
        });
        let existing = vec![("since".to_string(), Value::Int(2020))];
        let props = vec![("since".to_string(), Value::Int(2024))];
        let merged = merged_included_columns(Some(&index), existing, &props);
        assert_eq!(merged, vec![("since".to_string(), Value::Int(2024))]);
    }

    #[test]
    fn merged_included_columns_appends_new() {
        let index = Index::new(IndexConfig {
            id: 1,
            name: "idx_weight".to_string(),
            space_id: 1,
            schema_name: "knows".to_string(),
            fields: vec![IndexField::new("weight".to_string(), Value::Int(0), false)],
            properties: vec!["since".to_string()],
            index_type: IndexType::EdgeIndex,
            is_unique: false,
            covering: false,
            partial_condition: None,
        });
        let existing = vec![];
        let props = vec![("since".to_string(), Value::Int(2024))];
        let merged = merged_included_columns(Some(&index), existing, &props);
        assert_eq!(merged, vec![("since".to_string(), Value::Int(2024))]);
    }
}
