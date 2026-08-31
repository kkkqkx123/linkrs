//! Generation-scoped native-index storage.
//!
//! Each manifest generation owns independent shard maps. This makes the
//! manifest handle a real data-generation pin instead of metadata only.

use crate::index::key_codec::key_types::SecondaryIndexKey;
use crate::index::types::IndexRecord;
use graphdb_core::types::CommitLsn;
use parking_lot::RwLock;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

/// Shared publish barriers used by native indexes and WAL truncation.
pub(crate) type IndexBarrierRegistry = Arc<RwLock<HashMap<(u64, u64), CommitLsn>>>;

pub(crate) type IndexMaps = (
    BTreeMap<SecondaryIndexKey, IndexRecord>,
    BTreeMap<SecondaryIndexKey, IndexRecord>,
);

pub(crate) mod bloom;
pub(crate) mod generation;
pub(crate) mod runtime;
pub(crate) mod shard;

pub(crate) use generation::{generation_from_maps_with_pool_capacity, GenerationRuntime};
pub(crate) use runtime::IndexRuntime;
#[allow(unused_imports)]
pub(crate) use shard::ShardRuntime;
