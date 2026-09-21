use crate::index::shard_runtime::{
    IndexBarrierRegistry,
    IndexRuntime,
};
use crate::index::types::IndexIdentity;
use graphdb_core::types::CommitLsn;
use parking_lot::RwLock;
use std::sync::Arc;

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
