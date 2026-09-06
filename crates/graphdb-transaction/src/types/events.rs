//! Transaction lifecycle events and callbacks

use std::sync::Arc;

use super::writeset::WriteSet;
use graphdb_core::types::{CommitLsn, Timestamp, TransactionId};

/// Immutable lifecycle notification emitted after a transaction leaves the
/// active transaction table.
#[derive(Debug, Clone)]
pub enum TransactionEvent {
    Committed {
        txn_id: TransactionId,
        write_timestamp: Timestamp,
        commit_timestamp: Timestamp,
        write_set: Box<WriteSet>,
        schema_catalog_version: u64,
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
