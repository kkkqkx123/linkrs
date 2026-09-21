//! Mutating paths: staging commit, insert, delete and undo.
//!
//! Per-entry commit order is fixed: version-authority record, property row,
//! out-direction topology, in-direction topology, secondary index, owner map.
//! Each step compensates on failure of a later step (physical rollback of the
//! applied topology leg, property row release, authority record removal), so
//! a failed entry leaves no single-direction topology and no ownerless
//! timestamp. Batch atomicity rides on top: prevalidation first, then
//! in-order apply with prefix rollback, then WAL-first crash semantics
//! documented on [`EdgeStore::commit_staging_batch`].

mod bundled;
mod consistency;
mod delete;
mod property;
mod revert;
mod staging;
mod topology;
mod validation;

pub use delete::IncidentDeletedEdge;

#[cfg(test)]
mod tests;
