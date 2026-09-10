//! Transaction lifecycle events and callbacks

use std::sync::Arc;

use super::writeset::WriteSet;
use graphdb_core::types::{CommitLsn, Timestamp, TransactionId};

/// Immutable lifecycle notification emitted after a transaction leaves the
/// active transaction table.
///
/// Observers receive `&TransactionEvent` only for the dispatch call: do not
/// retain the reference or the boxed `WriteSet` beyond the callback; clone
/// explicitly when longer-lived data is needed.
#[derive(Debug, Clone)]
pub enum TransactionEvent {
    Committed {
        txn_id: TransactionId,
        write_timestamp: Timestamp,
        commit_timestamp: Timestamp,
        write_set: Box<WriteSet>,
        schema_catalog_version: u64,
        /// True when re-emitted by recovery replay (`recover_pending_finalization`).
        /// Consumers counting commits should deduplicate on this flag.
        replayed: bool,
    },
    Aborted {
        txn_id: TransactionId,
        write_timestamp: Timestamp,
    },
    CommitDurableButUnfinalized {
        txn_id: TransactionId,
        write_timestamp: Timestamp,
        commit_lsn: CommitLsn,
    },
    BudgetWarning {
        txn_id: TransactionId,
        resource: String,
        current: u64,
        limit: u64,
    },
}

pub type CommitCallback = Arc<dyn Fn(&TransactionEvent) + Send + Sync>;
pub type RollbackCallback = Arc<dyn Fn(&TransactionEvent) + Send + Sync>;
/// Observer over the unified transaction registry (receives every lifecycle
/// event unless a filter narrows it). Same underlying type as the
/// commit/rollback compatibility vests.
pub type TxnCallback = Arc<dyn Fn(&TransactionEvent) + Send + Sync>;

/// Read-only pre-commit view handed to commit-veto observers.
///
/// Deliberately light: vetoes decide on identity and timestamps only, never
/// on mutable payloads.
#[derive(Debug, Clone, Copy)]
pub struct CommitVetoContext {
    pub txn_id: TransactionId,
    pub write_timestamp: Timestamp,
}

/// Outcome of one commit-veto observer.
#[derive(Debug, Clone)]
pub struct VetoDecision {
    /// True blocks the commit.
    pub veto: bool,
    /// Human-readable reason, surfaced in the veto error and logs.
    pub reason: Option<String>,
}

impl VetoDecision {
    /// Allow the commit to proceed.
    pub fn allow() -> Self {
        Self {
            veto: false,
            reason: None,
        }
    }

    /// Veto the commit with a reason.
    pub fn veto(reason: impl Into<String>) -> Self {
        Self {
            veto: true,
            reason: Some(reason.into()),
        }
    }
}

/// Pre-commit decision hook.
///
/// Evaluated synchronously after conflict certification and before any WAL
/// I/O. First veto wins; a panicking observer is logged and treated as
/// allow so a broken hook cannot block commits forever. A vetoed commit
/// returns a `CommitVetoed` error WITHOUT aborting: the transaction stays
/// active and the caller must roll it back.
pub type CommitVetoCallback = Arc<dyn Fn(&CommitVetoContext) -> VetoDecision + Send + Sync>;
