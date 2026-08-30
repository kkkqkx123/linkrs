use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::engine::graph_storage::context::GraphStorageContext;
use crate::index::chunk::chunked_index::ChunkedIndex;
use crate::index::chunk::serialize::write_chunked_index_checkpoint;
use crate::index::key_codec::KeyBuilder;
use crate::index::manifest::{GenerationBuildState, GenerationState};
use crate::index::types::IndexRecord;
use graphdb_core::types::{IndexGeneration, Timestamp};
use graphdb_core::wal::EntityRef;
use graphdb_core::{StorageError, StorageResult, Value};

use super::stable_hash;
use super::IndexDataMaps;

pub(crate) fn write_generation_checkpoint(
    gen_dir: &Path,
    forward: &BTreeMap<Vec<u8>, IndexRecord>,
    reverse: &BTreeMap<Vec<u8>, IndexRecord>,
) -> StorageResult<()> {
    const POOL_CAPACITY: u64 = 64 * 1024 * 1024;
    let fwd_dir = gen_dir.join("forward_chunks");
    let rev_dir = gen_dir.join("reverse_chunks");
    let fwd = ChunkedIndex::from_btree(vec![], forward, POOL_CAPACITY);
    let rev = ChunkedIndex::from_btree(vec![], reverse, POOL_CAPACITY);
    write_chunked_index_checkpoint(&fwd_dir, &fwd)?;
    write_chunked_index_checkpoint(&rev_dir, &rev)?;
    Ok(())
}

pub(crate) fn save_generation_build_state(
    ctx: &GraphStorageContext,
    space_id: u64,
    index_name: &str,
    state: &GenerationBuildState,
) -> StorageResult<()> {
    if ctx.work_dir().is_none() {
        return Ok(());
    }
    let serialized =
        serde_json::to_vec(state).map_err(|e| StorageError::serialize_error(e.to_string()))?;
    let dir = build_state_dir(ctx, space_id)?;
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{index_name}_generation_build.json"));
    let temporary = path.with_extension("tmp");
    {
        use std::io::Write;
        let mut file = std::fs::File::create(&temporary)?;
        file.write_all(&serialized)?;
        file.sync_all()?;
    }
    std::fs::rename(&temporary, &path)?;
    if let Some(parent) = path.parent() {
        std::fs::File::open(parent)?.sync_all()?;
    }
    Ok(())
}

pub(crate) fn load_generation_build_state(
    ctx: &GraphStorageContext,
    space_id: u64,
    index_name: &str,
) -> StorageResult<Option<GenerationBuildState>> {
    if ctx.work_dir().is_none() {
        return Ok(None);
    }
    let dir = build_state_dir(ctx, space_id)?;
    let path = dir.join(format!("{index_name}_generation_build.json"));
    if !path.exists() {
        return Ok(None);
    }
    let bytes = std::fs::read(&path)?;
    let state: GenerationBuildState = serde_json::from_slice(&bytes)
        .map_err(|e| StorageError::deserialize_error(e.to_string()))?;
    Ok(Some(state))
}

pub(crate) fn remove_generation_build_state(
    ctx: &GraphStorageContext,
    space_id: u64,
    index_name: &str,
) -> StorageResult<()> {
    if ctx.work_dir().is_none() {
        return Ok(());
    }
    let dir = build_state_dir(ctx, space_id)?;
    let path = dir.join(format!("{index_name}_generation_build.json"));
    if path.exists() {
        std::fs::remove_file(&path)?;
    }
    Ok(())
}

pub(crate) fn build_state_dir(ctx: &GraphStorageContext, space_id: u64) -> StorageResult<PathBuf> {
    ctx.work_dir()
        .as_ref()
        .map(|dir| {
            dir.join("generation_build_state")
                .join(space_id.to_string())
        })
        .ok_or_else(|| StorageError::db_error("No work directory configured".to_string()))
}

pub(crate) fn resolve_crash_recovery(
    ctx: &GraphStorageContext,
    space_id: u64,
    index_name: &str,
) -> StorageResult<()> {
    let Some(build_state) = load_generation_build_state(ctx, space_id, index_name)? else {
        return Ok(());
    };
    if !build_state.is_active() && !matches!(build_state.state, GenerationState::Publishing) {
        log::warn!(
            "Discarding incomplete generation build for index {index_name} (state={:?})",
            build_state.state
        );
        remove_generation_build_state(ctx, space_id, index_name)?;
        return Ok(());
    }
    if matches!(build_state.state, GenerationState::Publishing) {
        log::info!("Completing generation build for index {index_name} from Publishing state");
    }
    Ok(())
}

pub(crate) fn generation_output_paths(
    ctx: &GraphStorageContext,
    space_id: u64,
    index_id: u64,
    generation: IndexGeneration,
) -> (PathBuf, Option<PathBuf>) {
    if let Some(index_dir) = ctx.storage_paths().map(|paths| paths.indexes_dir()) {
        let index_root = index_dir
            .join(space_id.to_string())
            .join(index_id.to_string());
        (
            index_root.join(format!("generation-{}", generation.get())),
            Some(index_root.join("manifest.bin")),
        )
    } else {
        (
            PathBuf::from("memory-index")
                .join(space_id.to_string())
                .join(index_id.to_string())
                .join(format!("generation-{}", generation.get())),
            None,
        )
    }
}

fn edge_entity_ref(edge: &graphdb_core::Edge) -> EntityRef {
    EntityRef::Edge {
        src: edge.src,
        dst: edge.dst,
        edge_type: stable_hash(edge.edge_type.as_bytes()) as u32,
        ranking: edge.ranking,
    }
}

pub(crate) fn build_vertex_index_data(
    space_id: u64,
    index: &graphdb_core::types::Index,
    vertices: &[graphdb_core::Vertex],
    snapshot_timestamp: Timestamp,
) -> StorageResult<IndexDataMaps> {
    let mut forward = BTreeMap::new();
    let mut reverse = BTreeMap::new();

    for vertex in vertices {
        let indexed_values: Vec<Value> = index
            .fields
            .iter()
            .filter_map(|field| vertex.properties.get(&field.name).cloned())
            .collect();
        let included_columns = index
            .properties
            .iter()
            .filter_map(|name| {
                vertex
                    .properties
                    .get(name)
                    .cloned()
                    .map(|value| (name.clone(), value))
            })
            .collect::<Vec<_>>();
        let vid_value = Value::from(vertex.vid);

        for prop_value in &indexed_values {
            let logical_forward_key =
                KeyBuilder::build_vertex_index_key(space_id, &index.name, prop_value, &vid_value)?;
            let logical_reverse_key =
                KeyBuilder::build_vertex_reverse_key_v2(space_id, &vid_value, &index.name)?;

            let entry = IndexRecord::new_with_columns(snapshot_timestamp, included_columns.clone())
                .with_entity_version(snapshot_timestamp)
                .with_entity_ref(EntityRef::Vertex(vertex.vid));

            forward.insert(logical_forward_key.0, entry.clone());
            reverse.insert(logical_reverse_key.0, entry);
        }
    }
    Ok((forward, reverse))
}

pub(crate) fn build_edge_index_data(
    space_id: u64,
    index: &graphdb_core::types::Index,
    edges: &[graphdb_core::Edge],
    snapshot_timestamp: Timestamp,
) -> StorageResult<IndexDataMaps> {
    let mut forward = BTreeMap::new();
    let mut reverse = BTreeMap::new();

    for edge in edges {
        let indexed_values: Vec<Value> = index
            .fields
            .iter()
            .filter_map(|field| edge.props.get(&field.name).cloned())
            .collect();
        let included_columns = index
            .properties
            .iter()
            .filter_map(|name| {
                edge.props
                    .get(name)
                    .cloned()
                    .map(|value| (name.clone(), value))
            })
            .collect::<Vec<_>>();
        let src_value = Value::from(edge.src);
        let dst_value = Value::from(edge.dst);

        for prop_value in &indexed_values {
            let logical_forward_key = KeyBuilder::build_edge_index_key(
                space_id,
                &index.name,
                prop_value,
                &src_value,
                &dst_value,
                &edge.edge_type,
                edge.ranking,
            )?;
            let logical_reverse_key = KeyBuilder::build_edge_reverse_key(
                space_id,
                &src_value,
                &dst_value,
                &edge.edge_type,
                edge.ranking,
                &index.name,
            )?;

            let entry = IndexRecord::new_with_columns(snapshot_timestamp, included_columns.clone())
                .with_entity_version(snapshot_timestamp)
                .with_entity_ref(edge_entity_ref(edge));

            forward.insert(logical_forward_key.0, entry.clone());
            reverse.insert(logical_reverse_key.0, entry);
        }
    }
    Ok((forward, reverse))
}
