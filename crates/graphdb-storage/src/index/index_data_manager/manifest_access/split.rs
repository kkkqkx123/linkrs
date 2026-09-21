use crate::index::helpers::{
    edge_entity_ref, flush_split_generation, merge_split_wal_changes, vertex_entity_ref,
};
use crate::index::key_codec::key_types::SecondaryIndexKey;
use crate::index::key_codec::{KeyBuilder, KeyParser};
use crate::index::manifest::{
    GenerationBuildState, GenerationState, IndexManifest, IndexShard, ManifestCatalog,
    ManifestHandle,
};
use crate::index::shard_runtime::{
    generation_from_maps_with_pool_capacity, GenerationRuntime, IndexBarrierRegistry, IndexMaps,
    IndexRuntime,
};
use crate::index::types::{EdgeIdentity, IndexIdentity, IndexRecord};
use crate::persistence::{read_versioned_payload, write_versioned_payload};
use graphdb_core::types::{
    CommitLsn, Index, IndexGeneration, IndexType, SnapshotTimestamp, Timestamp,
};
use graphdb_core::value::ordered_codec::OrderedCodec;
use graphdb_core::wal::{EntityRef, OutboxIntent};
use graphdb_core::{StorageError, StorageResult, Value};
use graphdb_metrics::StatsManager;
use parking_lot::RwLock;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;

use super::super::remove_dir_if_empty;
use super::super::IndexDataManagerImpl;

impl IndexDataManagerImpl {
    pub fn resolve_split_crash_recovery(&self, index_root: &Path) -> StorageResult<()> {
        let Some(build_state) = self.load_build_state(index_root)? else {
            return Ok(());
        };
        let partial_generation =
            index_root.join(format!("generation-{}", build_state.generation.get()));
        if matches!(
            build_state.state,
            GenerationState::Building | GenerationState::CatchingUp
        ) {
            log::warn!(
                "Discarding incomplete split build state for gen {} (state={:?})",
                build_state.generation,
                build_state.state
            );
            if partial_generation.exists() {
                std::fs::remove_dir_all(&partial_generation)?;
            }
            self.remove_build_state(index_root)?;
        }
        if matches!(build_state.state, GenerationState::Publishing) {
            let manifest_path = index_root.join("manifest.bin");
            let published_generation = manifest_path
                .is_file()
                .then(|| IndexManifest::load(&manifest_path))
                .transpose()?
                .map(|manifest| manifest.generation);
            if published_generation == Some(build_state.generation) {
                log::info!("Completing split build from Publishing state");
                self.remove_build_state(index_root)?;
            } else {
                log::warn!("Discarding split in Publishing state without its published generation");
                if partial_generation.exists() {
                    std::fs::remove_dir_all(&partial_generation)?;
                }
                self.remove_build_state(index_root)?;
            }
        }
        if matches!(build_state.state, GenerationState::Active) {
            self.remove_build_state(index_root)?;
        }
        Ok(())
    }

    pub fn split_native_index<F, G>(
        &self,
        identity: IndexIdentity,
        boundary: Vec<u8>,
        snapshot_timestamp: SnapshotTimestamp,
        start_lsn: CommitLsn,
        barrier_lsn: F,
        wal_intents: G,
    ) -> StorageResult<()>
    where
        F: FnOnce() -> StorageResult<CommitLsn>,
        G: FnOnce(CommitLsn, CommitLsn) -> StorageResult<Vec<OutboxIntent>>,
    {
        // fold pending writes into the generation chain before splitting.
        self.publish_pending_delta(identity)?;
        let IndexIdentity { space_id, index_id } = identity;
        if let Some(stats) = &self.stats_manager {
            stats.record_generation_build();
        }
        if self.index_root.is_none() {
            return Err(StorageError::invalid_operation(
                "Native index splitting requires a persistent index root",
            ));
        }
        if snapshot_timestamp.get() == 0 || start_lsn == CommitLsn::ZERO {
            return Err(StorageError::invalid_operation(
                "Native index splitting requires non-zero snapshot timestamp and start LSN",
            ));
        }
        if boundary.is_empty() {
            return Err(StorageError::invalid_operation(
                "Index split boundary cannot be empty",
            ));
        }
        let catalog = self
            .manifest_catalog(space_id, index_id)
            .ok_or_else(|| StorageError::not_found(format!("Index {index_id} has no manifest")))?;
        let runtime = self.runtime(space_id, index_id)?;
        let current = catalog.acquire().manifest().clone();
        let split_position = current
            .shards
            .iter()
            .position(|shard| shard.contains(&boundary))
            .ok_or_else(|| {
                StorageError::invalid_operation("Split boundary is outside key space")
            })?;
        let shard = current.shards[split_position].clone();
        if shard.lower.as_ref() == Some(&boundary) || shard.upper.as_ref() == Some(&boundary) {
            return Err(StorageError::invalid_operation(
                "Split boundary must be inside an existing shard",
            ));
        }

        let generation = IndexGeneration::new(current.generation.get().saturating_add(1));
        let next_shard_id = current
            .shards
            .iter()
            .map(|entry| entry.shard_id)
            .max()
            .unwrap_or(0)
            .saturating_add(1);
        let checkpoint_parent = shard.checkpoint_file.parent().ok_or_else(|| {
            StorageError::invalid_operation("Index shard checkpoint path has no parent")
        })?;
        let index_root = shard
            .checkpoint_file
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with("shard-"))
            .then(|| {
                checkpoint_parent.parent().ok_or_else(|| {
                    StorageError::invalid_operation("Index shard generation path has no parent")
                })
            })
            .transpose()?
            .unwrap_or(checkpoint_parent);

        self.resolve_split_crash_recovery(index_root)?;
        let mut build_state = GenerationBuildState::new(generation, snapshot_timestamp, start_lsn);
        self.save_build_state(index_root, &build_state)?;

        let next_generation_dir = index_root.join(format!("generation-{}", generation.get()));
        let shard_a_path = next_generation_dir.join(format!("shard-{}", shard.shard_id));
        let shard_b_path = next_generation_dir.join(format!("shard-{next_shard_id}"));

        let index_type = self
            .index_types
            .read()
            .get(&IndexIdentity { space_id, index_id })
            .cloned()
            .ok_or_else(|| StorageError::not_found(format!("Index {index_id} has no type")))?;
        let index_definition = self
            .index_definitions
            .read()
            .get(&IndexIdentity { space_id, index_id })
            .cloned()
            .ok_or_else(|| {
                StorageError::not_found(format!("Index {index_id} has no definition"))
            })?;

        let mut shards = current.shards.clone();
        shards.splice(
            split_position..=split_position,
            [
                IndexShard {
                    shard_id: shard.shard_id,
                    lower: shard.lower.clone(),
                    upper: Some(boundary.clone()),
                    checkpoint_file: shard_a_path,
                    checksum: None,
                },
                IndexShard {
                    shard_id: next_shard_id,
                    lower: Some(boundary.clone()),
                    upper: shard.upper.clone(),
                    checkpoint_file: shard_b_path,
                    checksum: None,
                },
            ],
        );
        let next = IndexManifest::new(space_id, index_id, generation, shards)?;
        let mut maps = {
            let chain = runtime.generation_chain_until(current.generation)?;
            // Pin the generation chain so a concurrent reclamation cannot
            // delete the checkpoint files this snapshot may lazily reload.
            let _chain_pins = self.pin_chain_manifests(&catalog, &chain);
            let mut maps: HashMap<u32, IndexMaps> = HashMap::new();
            for current_shard in &current.shards {
                let mut forward = BTreeMap::new();
                let mut reverse = BTreeMap::new();
                for gen in &chain {
                    if let Some(shard) = gen.shard(current_shard.shard_id) {
                        let (f, r) = shard.snapshot();
                        for (key, entry) in f {
                            if entry.is_visible_at(snapshot_timestamp.get()) {
                                forward.entry(key).or_insert(entry);
                            }
                        }
                        for (key, entry) in r {
                            if entry.is_visible_at(snapshot_timestamp.get()) {
                                reverse.entry(key).or_insert(entry);
                            }
                        }
                    }
                }
                if current_shard.shard_id != shard.shard_id {
                    maps.insert(current_shard.shard_id, (forward, reverse));
                    continue;
                }
                let (forward_a, forward_b): (BTreeMap<_, _>, BTreeMap<_, _>) = forward
                    .into_iter()
                    .partition(|(key, _)| key.as_slice() < boundary.as_slice());
                let mut entity_shards: Vec<(EntityRef, (Timestamp, u32))> = Vec::new();
                for entry in forward_a.values() {
                    if let Some(entity) = entry.entity_ref.clone() {
                        if let Some((_, (ts, shard_id))) = entity_shards
                            .iter_mut()
                            .find(|(candidate, _)| *candidate == entity)
                        {
                            if entry.created_ts >= *ts {
                                *ts = entry.created_ts;
                                *shard_id = shard.shard_id;
                            }
                        } else {
                            entity_shards.push((entity, (entry.created_ts, shard.shard_id)));
                        }
                    }
                }
                for entry in forward_b.values() {
                    if let Some(entity) = entry.entity_ref.clone() {
                        if let Some((_, (ts, shard_id))) = entity_shards
                            .iter_mut()
                            .find(|(candidate, _)| *candidate == entity)
                        {
                            if entry.created_ts >= *ts {
                                *ts = entry.created_ts;
                                *shard_id = next_shard_id;
                            }
                        } else {
                            entity_shards.push((entity, (entry.created_ts, next_shard_id)));
                        }
                    }
                }
                let mut reverse_a = BTreeMap::new();
                let mut reverse_b = BTreeMap::new();
                for (key, entry) in reverse {
                    let target_shard = entry
                        .entity_ref
                        .as_ref()
                        .and_then(|entity| {
                            entity_shards
                                .iter()
                                .find(|(candidate, _)| candidate == entity)
                        })
                        .map(|(_, (_, shard_id))| *shard_id);
                    if target_shard == Some(next_shard_id) || target_shard.is_none() {
                        reverse_b.insert(key, entry);
                    } else {
                        reverse_a.insert(key, entry);
                    }
                }
                maps.insert(shard.shard_id, (forward_a, reverse_a));
                maps.insert(next_shard_id, (forward_b, reverse_b));
            }
            maps
        };

        build_state.transition_to_catching_up()?;
        self.save_build_state(index_root, &build_state)?;

        let active_manifest = catalog.acquire().manifest().clone();
        if active_manifest.generation != current.generation {
            return Err(StorageError::invalid_operation(
                "Index generation changed while splitting; retry the split",
            ));
        }
        let barrier_lsn = barrier_lsn()?;
        if barrier_lsn < start_lsn {
            return Err(StorageError::invalid_operation(
                "Split barrier LSN cannot precede its start LSN",
            ));
        }
        let intents = wal_intents(start_lsn, barrier_lsn)?;
        let active = runtime
            .generation(current.generation)
            .ok_or_else(|| StorageError::not_found("Active runtime generation is missing"))?;
        let mut active_forward = BTreeMap::new();
        let mut active_reverse = BTreeMap::new();
        for current_shard in &current.shards {
            let (forward, reverse) = active
                .shard(current_shard.shard_id)
                .ok_or_else(|| StorageError::not_found("Active runtime shard is missing"))?
                .snapshot();
            active_forward.extend(forward);
            active_reverse.extend(reverse);
        }
        match index_type {
            IndexType::TagIndex => {
                let forward_prefix =
                    KeyBuilder::build_vertex_index_prefix(space_id, &index_definition.name).0;
                merge_split_wal_changes(
                    &mut maps,
                    &next,
                    &active_forward,
                    &active_reverse,
                    &intents,
                    |key| key.starts_with(&forward_prefix),
                    |key| {
                        KeyParser::parse_vertex_reverse_key_v2(key)
                            .is_ok_and(|(_, name)| name == index_definition.name)
                    },
                )?;
            }
            IndexType::EdgeIndex => {
                let forward_prefix =
                    KeyBuilder::build_edge_index_prefix(space_id, &index_definition.name).0;
                merge_split_wal_changes(
                    &mut maps,
                    &next,
                    &active_forward,
                    &active_reverse,
                    &intents,
                    |key| key.starts_with(&forward_prefix),
                    |key| {
                        KeyParser::parse_edge_reverse_key(key)
                            .is_ok_and(|(_, _, _, _, name)| name == index_definition.name)
                    },
                )?;
            }
        }

        build_state.transition_to_publishing(barrier_lsn)?;
        self.save_build_state(index_root, &build_state)?;

        let current_gen = runtime.generation(current.generation);
        let (fwd_prefix, rev_prefix) = self.compute_prefixes(identity);
        let pool_cap = self.pool_capacity.load(Ordering::Relaxed);
        let next_runtime = generation_from_maps_with_pool_capacity(
            &next,
            maps,
            current_gen.as_ref(),
            0,
            fwd_prefix,
            rev_prefix,
            pool_cap,
        );
        flush_split_generation(&next, &next_runtime)?;

        next.store(&index_root.join("manifest.bin"))?;
        runtime.install_generation(next_runtime);
        catalog.publish(next)?;
        runtime.establish_barrier_lsn(barrier_lsn);
        self.record_barrier_lsn(IndexIdentity { space_id, index_id }, barrier_lsn);
        runtime.wait_for_barrier_lsn(barrier_lsn);
        if let Some(stats) = &self.stats_manager {
            stats.record_generation_publish();
        }
        self.record_manifest_state(&catalog);
        self.reclaim_retired_generations(IndexIdentity { space_id, index_id })?;

        build_state.transition_to_active()?;
        self.remove_build_state(index_root)?;
        Ok(())
    }
}
