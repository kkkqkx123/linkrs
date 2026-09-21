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
    pub(crate) fn barrier_registry(&self) -> IndexBarrierRegistry {
        Arc::clone(&self.barrier_registry)
    }

    pub(crate) fn rebuild_gate(&self) -> Arc<RwLock<()>> {
        Arc::clone(&self.rebuild_gate)
    }

    pub(crate) fn record_barrier_lsn(&self, identity: IndexIdentity, barrier_lsn: CommitLsn) {
        if barrier_lsn == CommitLsn::ZERO {
            return;
        }
        let mut barriers = self.barrier_registry.write();
        let entry = barriers
            .entry((identity.space_id, identity.index_id))
            .or_default();
        if barrier_lsn > *entry {
            *entry = barrier_lsn;
        }
    }

    pub(crate) fn advance_barriers(&self, commit_lsn: CommitLsn) {
        if commit_lsn == CommitLsn::ZERO {
            return;
        }
        for (identity, runtime) in self.runtimes.read().iter() {
            runtime.establish_barrier_lsn(commit_lsn);
            self.record_barrier_lsn(*identity, commit_lsn);
        }
    }

    pub(crate) fn wait_for_active_barrier(&self, runtime: &IndexRuntime) {
        let barrier_lsn = runtime.barrier_lsn();
        runtime.wait_for_barrier_lsn(barrier_lsn);
    }
}
