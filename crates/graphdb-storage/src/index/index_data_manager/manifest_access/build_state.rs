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
    pub(crate) fn build_state_path(&self, index_root: &Path) -> PathBuf {
        index_root.join("generation_build.bin")
    }

    pub(crate) fn save_build_state(
        &self,
        index_root: &Path,
        state: &GenerationBuildState,
    ) -> StorageResult<()> {
        std::fs::create_dir_all(index_root)?;
        let path = self.build_state_path(index_root);
        let serialized = postcard::to_allocvec(state)
            .map_err(|e| StorageError::serialize_error(e.to_string()))?;
        let mut wrapped = Vec::new();
        write_versioned_payload(&mut wrapped, &serialized);
        crate::persistence::write_file_atomic(&path, &wrapped)
    }

    pub(crate) fn load_build_state(
        &self,
        index_root: &Path,
    ) -> StorageResult<Option<GenerationBuildState>> {
        let path = self.build_state_path(index_root);
        if !path.exists() {
            return Ok(None);
        }
        let mut file = std::fs::File::open(&path)?;
        let payload = read_versioned_payload(&mut file, "generation_build.bin")?;
        let state: GenerationBuildState = postcard::from_bytes(&payload)
            .map_err(|e| StorageError::deserialize_error(e.to_string()))?;
        Ok(Some(state))
    }

    pub(crate) fn remove_build_state(&self, index_root: &Path) -> StorageResult<()> {
        let path = self.build_state_path(index_root);
        if path.exists() {
            std::fs::remove_file(&path)?;
        }
        Ok(())
    }
}
