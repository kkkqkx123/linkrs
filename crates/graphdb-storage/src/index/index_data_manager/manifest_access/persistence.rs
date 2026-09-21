use crate::index::helpers::{
    edge_entity_ref, vertex_entity_ref,
};
use crate::index::key_codec::key_types::SecondaryIndexKey;
use crate::index::key_codec::{KeyBuilder, KeyParser};
use crate::index::manifest::{
    IndexManifest, ManifestCatalog,
};
use crate::index::shard_runtime::IndexMaps;
use crate::index::types::{EdgeIdentity, IndexIdentity, IndexRecord};
use graphdb_core::types::{
    IndexType, Timestamp,
};
use graphdb_core::value::ordered_codec::OrderedCodec;
use graphdb_core::{StorageError, StorageResult, Value};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use super::super::IndexDataManagerImpl;

impl IndexDataManagerImpl {
    pub fn flush<P: AsRef<Path>>(&self, path: P) -> StorageResult<()> {
        if self.index_root.is_none() && !self.manifest_catalogs.read().is_empty() {
            return Err(StorageError::invalid_operation(
                "Flushing native indexes requires a persistent index root",
            ));
        }
        let path = path.as_ref();
        let manifest_dir = path.join("native_index_manifests");
        std::fs::create_dir_all(&manifest_dir)?;
        for (identity, catalog) in self.manifest_catalogs.read().iter() {
            let manifest = catalog.acquire();
            let runtime = self.runtime(identity.space_id, identity.index_id)?;
            if self.index_types.read().get(identity).is_some() {
                runtime.flush_generation(manifest.manifest())?;
            }
            manifest.manifest().store(
                &manifest_dir.join(format!("{}-{}.bin", identity.space_id, identity.index_id)),
            )?;
        }
        Ok(())
    }

    pub fn load<P: AsRef<Path>>(&mut self, path: P) -> StorageResult<()> {
        let path = path.as_ref();
        let manifest_dir = path.join("native_index_manifests");
        if manifest_dir.is_dir() {
            let mut catalogs = self.manifest_catalogs.write();
            for entry in std::fs::read_dir(manifest_dir)? {
                let manifest_path = entry?.path();
                if manifest_path.extension().and_then(|value| value.to_str()) != Some("bin") {
                    continue;
                }
                let manifest = IndexManifest::load(&manifest_path)?;
                catalogs.insert(
                    IndexIdentity {
                        space_id: manifest.space_id,
                        index_id: manifest.index_id,
                    },
                    Arc::new(ManifestCatalog::new(manifest)?),
                );
            }
        }
        for space_entry in std::fs::read_dir(path)? {
            let space_path = space_entry?.path();
            if !space_path.is_dir() {
                continue;
            }
            for index_entry in std::fs::read_dir(space_path)? {
                let index_path = index_entry?.path();
                self.resolve_split_crash_recovery(&index_path)?;
                let candidate = index_path.join("manifest.bin");
                if !candidate.is_file() {
                    continue;
                }
                let manifest = IndexManifest::load(&candidate)?;
                self.manifest_catalogs.write().insert(
                    IndexIdentity {
                        space_id: manifest.space_id,
                        index_id: manifest.index_id,
                    },
                    Arc::new(ManifestCatalog::new(manifest)?),
                );
            }
        }
        Ok(())
    }

    pub(crate) fn clear_vertex_entity(
        &self,
        space_id: u64,
        vertex_id: &Value,
        index_name: &str,
        write_ts: Timestamp,
    ) -> StorageResult<()> {
        let Some(index_id) = self.index_alias(space_id, index_name) else {
            return Ok(());
        };
        let identity = IndexIdentity { space_id, index_id };
        // fold pending writes into the chain so their entries can be
        // tombstoned; otherwise a delete would miss accumulated entries.
        self.publish_pending_delta(identity)?;
        let runtime = self.runtime(space_id, index_id)?;

        let delta = {
            self.wait_for_active_barrier(&runtime);
            let catalog = self.manifest_catalog(space_id, index_id).ok_or_else(|| {
                StorageError::not_found(format!("Index {index_id} has no manifest"))
            })?;
            let handle = catalog.acquire();
            let chain = runtime.generation_chain_until(handle.manifest().generation)?;
            let _chain_pins = self.pin_chain_manifests(&catalog, &chain);

            let reverse_prefix =
                KeyBuilder::build_vertex_reverse_key_v2(space_id, vertex_id, index_name)?;
            let reverse_end = KeyBuilder::build_range_end(&reverse_prefix);

            let mut reverse_meta: Vec<(SecondaryIndexKey, Vec<u8>)> = Vec::new();
            for gen in &chain {
                for shard in gen.shards() {
                    if !shard.reverse_may_have_range(&reverse_prefix.0, &reverse_end.0) {
                        continue;
                    }
                    for (key, record) in shard.reverse_range(&reverse_prefix.0, &reverse_end.0) {
                        if !record.is_visible_at(write_ts) {
                            continue;
                        }
                        if let Ok(encoded) = KeyParser::extract_value_from_reverse_key(&key) {
                            reverse_meta.push((key, encoded));
                        }
                    }
                }
            }

            if reverse_meta.is_empty() {
                return Ok(());
            }

            let route = |key: &[u8]| -> StorageResult<u32> {
                handle
                    .manifest()
                    .route_key(key)
                    .map(|s| s.shard_id)
                    .ok_or_else(|| {
                        StorageError::invalid_operation(
                            "Index manifest does not cover the ordered key",
                        )
                    })
            };

            let mut per_shard: HashMap<u32, IndexMaps> = HashMap::new();
            let entity_ref = vertex_entity_ref(vertex_id);

            let encoded_values: Vec<Vec<u8>> =
                reverse_meta.iter().map(|(_, e)| e.clone()).collect();
            for (rev_key, _) in reverse_meta {
                let shard_id = route(&rev_key)?;
                let (_, ref mut rev_map) = per_shard.entry(shard_id).or_default();
                let mut entry = IndexRecord::new(write_ts);
                entry.mark_deleted(write_ts);
                if let Some(ref e) = entity_ref {
                    entry = entry.with_entity_ref(e.clone());
                }
                rev_map.insert(rev_key, entry);
            }

            let mut seen_fwd: HashSet<Vec<u8>> = HashSet::new();
            for encoded in &encoded_values {
                let Ok(value) = OrderedCodec::new().decode(encoded) else {
                    continue;
                };
                let Ok(forward) =
                    KeyBuilder::build_vertex_index_key(space_id, index_name, &value, vertex_id)
                else {
                    continue;
                };
                if !seen_fwd.insert(forward.0.clone()) {
                    continue;
                }
                let fwd_end = KeyBuilder::build_range_end(&forward);

                let mut fwd_keys: Vec<SecondaryIndexKey> = Vec::new();
                for gen in &chain {
                    for shard in gen.shards() {
                        for (key, record) in shard.forward_range(&forward.0, &fwd_end.0) {
                            if record.is_visible_at(write_ts) {
                                fwd_keys.push(key);
                            }
                        }
                    }
                }

                for fwd_key in &fwd_keys {
                    let shard_id = route(fwd_key)?;
                    let (ref mut fwd_map, _) = per_shard.entry(shard_id).or_default();
                    let mut entry = IndexRecord::new(write_ts);
                    entry.mark_deleted(write_ts);
                    if let Some(ref e) = entity_ref {
                        entry = entry.with_entity_ref(e.clone());
                    }
                    fwd_map.insert(fwd_key.clone(), entry);
                }
            }

            per_shard
        };

        if !delta.is_empty() {
            self.accumulate_delta(identity, delta, write_ts)?;
        }
        Ok(())
    }

    pub(crate) fn clear_edge_entity(
        &self,
        edge: &EdgeIdentity<'_>,
        index_name: &str,
        write_ts: Timestamp,
    ) -> StorageResult<()> {
        let space_id = edge.space_id;
        let src = edge.src;
        let dst = edge.dst;
        let edge_type = edge.edge_type;
        let ranking = edge.ranking;
        let Some(index_id) = self.index_alias(space_id, index_name) else {
            return Ok(());
        };
        let identity = IndexIdentity { space_id, index_id };
        // fold pending writes into the chain so their entries can be
        // tombstoned; otherwise a delete would miss accumulated entries.
        self.publish_pending_delta(identity)?;
        let runtime = self.runtime(space_id, index_id)?;

        let delta = {
            self.wait_for_active_barrier(&runtime);
            let catalog = self.manifest_catalog(space_id, index_id).ok_or_else(|| {
                StorageError::not_found(format!("Index {index_id} has no manifest"))
            })?;
            let handle = catalog.acquire();
            let chain = runtime.generation_chain_until(handle.manifest().generation)?;
            let _chain_pins = self.pin_chain_manifests(&catalog, &chain);

            let reverse_prefix = KeyBuilder::build_edge_reverse_key(
                space_id, src, dst, edge_type, ranking, index_name,
            )?;
            let reverse_end = KeyBuilder::build_range_end(&reverse_prefix);

            let mut reverse_meta: Vec<(SecondaryIndexKey, Vec<u8>)> = Vec::new();
            for gen in &chain {
                for shard in gen.shards() {
                    if !shard.reverse_may_have_range(&reverse_prefix.0, &reverse_end.0) {
                        continue;
                    }
                    for (key, record) in shard.reverse_range(&reverse_prefix.0, &reverse_end.0) {
                        if !record.is_visible_at(write_ts) {
                            continue;
                        }
                        if let Ok(encoded) = KeyParser::extract_value_from_edge_reverse_key(&key) {
                            reverse_meta.push((key, encoded));
                        }
                    }
                }
            }

            if reverse_meta.is_empty() {
                return Ok(());
            }

            let route = |key: &[u8]| -> StorageResult<u32> {
                handle
                    .manifest()
                    .route_key(key)
                    .map(|s| s.shard_id)
                    .ok_or_else(|| {
                        StorageError::invalid_operation(
                            "Index manifest does not cover the ordered key",
                        )
                    })
            };

            let mut per_shard: HashMap<u32, IndexMaps> = HashMap::new();
            let entity_ref = edge_entity_ref(src, dst, edge_type, ranking);

            let encoded_values: Vec<Vec<u8>> =
                reverse_meta.iter().map(|(_, e)| e.clone()).collect();
            for (rev_key, _) in reverse_meta {
                let shard_id = route(&rev_key)?;
                let (_, ref mut rev_map) = per_shard.entry(shard_id).or_default();
                let mut entry = IndexRecord::new(write_ts);
                entry.mark_deleted(write_ts);
                if let Some(ref e) = entity_ref {
                    entry = entry.with_entity_ref(e.clone());
                }
                rev_map.insert(rev_key, entry);
            }

            let mut seen_fwd: HashSet<Vec<u8>> = HashSet::new();
            for encoded in &encoded_values {
                let Ok(value) = OrderedCodec::new().decode(encoded) else {
                    continue;
                };
                let Ok(forward) = KeyBuilder::build_edge_index_key(
                    space_id, index_name, &value, src, dst, edge_type, ranking,
                ) else {
                    continue;
                };
                if !seen_fwd.insert(forward.0.clone()) {
                    continue;
                }
                let fwd_end = KeyBuilder::build_range_end(&forward);

                let mut fwd_keys: Vec<SecondaryIndexKey> = Vec::new();
                for gen in &chain {
                    for shard in gen.shards() {
                        for (key, record) in shard.forward_range(&forward.0, &fwd_end.0) {
                            if record.is_visible_at(write_ts) {
                                fwd_keys.push(key);
                            }
                        }
                    }
                }

                for fwd_key in &fwd_keys {
                    let shard_id = route(fwd_key)?;
                    let (ref mut fwd_map, _) = per_shard.entry(shard_id).or_default();
                    let mut entry = IndexRecord::new(write_ts);
                    entry.mark_deleted(write_ts);
                    if let Some(ref e) = entity_ref {
                        entry = entry.with_entity_ref(e.clone());
                    }
                    fwd_map.insert(fwd_key.clone(), entry);
                }
            }

            per_shard
        };

        if !delta.is_empty() {
            self.accumulate_delta(identity, delta, write_ts)?;
        }
        Ok(())
    }

    /// Tombstone all visible entries of one index without removing files.
    ///
    /// This is MVCC-safe logical clear: every visible forward and reverse key
    /// gets a deletion tombstone at `write_ts`, so readers at newer timestamps
    /// observe an empty index while older snapshots still converge. Physical
    /// checkpoint files are retained for those readers and reclaimed later by
    /// `retire_generations` once `safe_ts` passes. Dropping an index is
    /// different: `unregister_native_index` plus `remove_checkpoint_dirs_by_id`
    /// removes runtime state and checkpoint directories because no reader can
    /// pin a dropped index anymore.
    ///
    /// Clearing never backfills from primary storage. It is intended for the
    /// drop path only and is not exposed as a standalone user operation; to
    /// truncate an index, drop it and create it again.
    pub(crate) fn clear_index(
        &self,
        index_id: u64,
        space_id: u64,
        index_name: &str,
        index_type: IndexType,
        write_ts: Timestamp,
    ) -> StorageResult<()> {
        let identity = IndexIdentity { space_id, index_id };
        // fold pending writes into the chain before clearing the index.
        self.publish_pending_delta(identity)?;
        let runtime = self.runtime(space_id, index_id)?;

        let delta = {
            self.wait_for_active_barrier(&runtime);
            let catalog = self.manifest_catalog(space_id, index_id).ok_or_else(|| {
                StorageError::not_found(format!("Index {index_id} has no manifest"))
            })?;
            let handle = catalog.acquire();
            let chain = runtime.generation_chain_until(handle.manifest().generation)?;
            let _chain_pins = self.pin_chain_manifests(&catalog, &chain);

            let (prefix, end) = match index_type {
                IndexType::TagIndex => {
                    let p = KeyBuilder::build_vertex_index_prefix(space_id, index_name);
                    let e = KeyBuilder::build_range_end(&p);
                    (p, e)
                }
                IndexType::EdgeIndex => {
                    let p = KeyBuilder::build_edge_index_prefix(space_id, index_name);
                    let e = KeyBuilder::build_range_end(&p);
                    (p, e)
                }
            };

            let route = |key: &[u8]| -> StorageResult<u32> {
                handle
                    .manifest()
                    .route_key(key)
                    .map(|s| s.shard_id)
                    .ok_or_else(|| {
                        StorageError::invalid_operation(
                            "Index manifest does not cover the ordered key",
                        )
                    })
            };

            let mut per_shard: HashMap<u32, IndexMaps> = HashMap::new();

            for shard_def in &handle.manifest().shards {
                let mut fwd_keys: Vec<SecondaryIndexKey> = Vec::new();
                for gen in &chain {
                    if let Some(shard) = gen.shard(shard_def.shard_id) {
                        for (key, record) in shard.forward_range(&prefix.0, &end.0) {
                            if record.is_visible_at(write_ts) {
                                fwd_keys.push(key);
                            }
                        }
                    }
                }

                for fwd_key in fwd_keys {
                    let shard_id = route(&fwd_key)?;
                    let (ref mut fwd_map, _) = per_shard.entry(shard_id).or_default();
                    let mut entry = IndexRecord::new(write_ts);
                    entry.mark_deleted(write_ts);
                    fwd_map.insert(fwd_key, entry);
                }

                let rev_match: fn(&[u8], &str) -> bool = match index_type {
                    IndexType::TagIndex => |key, name| {
                        KeyParser::parse_vertex_reverse_key_v2(key).is_ok_and(|(_, n)| n == name)
                    },
                    IndexType::EdgeIndex => |key, name| {
                        KeyParser::parse_edge_reverse_key(key)
                            .is_ok_and(|(_, _, _, _, n)| n == name)
                    },
                };

                let mut rev_keys: Vec<SecondaryIndexKey> = Vec::new();
                for gen in &chain {
                    if let Some(shard) = gen.shard(shard_def.shard_id) {
                        for (key, record) in shard.iter_reverse() {
                            if record.is_visible_at(write_ts) && rev_match(&key, index_name) {
                                rev_keys.push(key);
                            }
                        }
                    }
                }

                for rev_key in rev_keys {
                    let shard_id = route(&rev_key)?;
                    let (_, ref mut rev_map) = per_shard.entry(shard_id).or_default();
                    let mut entry = IndexRecord::new(write_ts);
                    entry.mark_deleted(write_ts);
                    rev_map.insert(rev_key, entry);
                }
            }

            per_shard
        };

        if !delta.is_empty() {
            self.accumulate_delta(identity, delta, write_ts)?;
        }
        Ok(())
    }
}
