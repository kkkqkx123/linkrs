use crate::core::types::{CommitLsn, TransactionId};

pub trait TransactionCommitSink: Send + Sync {
    fn commit_transaction(&self, transaction_id: TransactionId) -> Result<CommitLsn, String>;

    fn abort_transaction(&self, transaction_id: TransactionId) -> Result<(), String>;
}
