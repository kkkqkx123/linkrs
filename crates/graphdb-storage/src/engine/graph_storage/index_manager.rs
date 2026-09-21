use crate::index::types::IndexRecord;
use std::collections::BTreeMap;

pub(crate) type IndexDataMaps = (
    BTreeMap<Vec<u8>, IndexRecord>,
    BTreeMap<Vec<u8>, IndexRecord>,
);

pub(crate) mod cascade;
pub mod checkpoint;
pub(crate) mod edge_index;
pub(crate) mod generation;
pub(crate) mod tag_index;
#[cfg(test)]
mod tests;
pub mod wal_replay;

pub(crate) use cascade::{
    drop_edge_indexes_by_type_cascade, drop_space_indexes_cascade, drop_tag_indexes_by_tag_cascade,
};
pub(crate) use checkpoint::{
    build_edge_index_data, build_vertex_index_data, generation_output_paths,
    remove_generation_build_state, resolve_crash_recovery, save_generation_build_state,
    write_generation_checkpoint,
};
pub(crate) use edge_index::{
    create_edge_index, drop_edge_index, get_edge_index, list_edge_indexes, rebuild_edge_index,
};
#[cfg(test)]
pub(crate) use generation::{clear_generation_faults, inject_generation_fault};
pub(crate) use generation::{
    current_wal_lsn, fail_if_generation_fault_is_injected, next_generation, stable_hash,
    GenerationFaultPoint,
};
pub(crate) use tag_index::{
    create_tag_index, drop_tag_index, get_tag_index, list_tag_indexes, lookup_index,
    rebuild_tag_index,
};
pub(crate) use wal_replay::{replay_wal_partition, wal_intents_for_index};
