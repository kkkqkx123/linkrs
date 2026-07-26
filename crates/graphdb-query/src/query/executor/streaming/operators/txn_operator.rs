use std::sync::Arc;

use crate::core::error::QueryError;
use crate::core::Value;
use crate::query::executor::streaming::chunk::DataChunk;
use crate::query::executor::streaming::executor::StreamingExecutor;
use crate::query::executor::streaming::operators::base::OperatorBase;
use crate::query::executor::streaming::transaction_scope::{
    SessionTransactionController, TransactionCommandResult,
};

/// Transaction command operator.
///
/// Validates state transitions through the [`SessionTransactionController`]
/// and produces a structured result chunk.  The actual TransactionManager
/// operations (begin/commit/rollback) are performed by the API layer before
/// this operator runs.
#[derive(Debug)]
pub enum TxnOperator {
    BeginTransaction { emitted: bool },
    Commit { emitted: bool },
    Rollback { emitted: bool },
}

impl TxnOperator {
    pub fn from_spec(spec: &super::spec::TxnSpec) -> Self {
        match spec {
            super::spec::TxnSpec::BeginTransaction => {
                TxnOperator::BeginTransaction { emitted: false }
            }
            super::spec::TxnSpec::Commit => TxnOperator::Commit { emitted: false },
            super::spec::TxnSpec::Rollback => TxnOperator::Rollback { emitted: false },
        }
    }

    fn controller(base: &OperatorBase) -> Result<Arc<SessionTransactionController>, QueryError> {
        base.runtime
            .as_ref()
            .and_then(|rt| rt.session_controller())
            .ok_or_else(|| {
                QueryError::execution(
                    "Transaction controller not available in execution runtime".to_string(),
                )
            })
    }

    pub fn open(
        &mut self,
        _base: &mut OperatorBase,
        input: &mut StreamingExecutor,
    ) -> Result<(), QueryError> {
        input.open()?;
        _base.lifecycle.mark_opened();
        Ok(())
    }

    pub fn next(
        &mut self,
        base: &mut OperatorBase,
        _input: &mut StreamingExecutor,
    ) -> Result<Option<DataChunk>, QueryError> {
        match self {
            Self::BeginTransaction { emitted } => {
                if *emitted {
                    return Ok(None);
                }
                *emitted = true;
                let ctrl = Self::controller(base)?;
                // The API layer should have already created the transaction.
                // We validate that begin_tracking succeeds (no state conflict).
                let result = ctrl.current_scope();
                match result {
                    crate::query::executor::streaming::TransactionScope::ExplicitBorrowed {
                        transaction_id, ..
                    } => Ok(Some(
                        TransactionCommandResult::begin(transaction_id)
                            .into_data_chunk(Arc::clone(&base.output_layout)),
                    )),
                    _ => Err(QueryError::execution(
                        "BEGIN: no active transaction found; API layer must call begin before executing this plan".to_string(),
                    )),
                }
            }
            Self::Commit { emitted } => {
                if *emitted {
                    return Ok(None);
                }
                *emitted = true;
                let ctrl = Self::controller(base)?;
                let txn_id = ctrl.begin_commit()?;
                ctrl.commit_finalize();
                Ok(Some(
                    TransactionCommandResult::commit(txn_id)
                        .into_data_chunk(Arc::clone(&base.output_layout)),
                ))
            }
            Self::Rollback { emitted } => {
                if *emitted {
                    return Ok(None);
                }
                *emitted = true;
                let ctrl = Self::controller(base)?;
                let txn_id = ctrl.begin_rollback()?;
                ctrl.rollback_finalize();
                Ok(Some(
                    TransactionCommandResult::rollback(txn_id)
                        .into_data_chunk(Arc::clone(&base.output_layout)),
                ))
            }
        }
    }

    pub fn stop(
        &mut self,
        base: &mut OperatorBase,
        _input: &mut StreamingExecutor,
    ) -> Result<(), QueryError> {
        base.lifecycle.mark_stopped();
        Ok(())
    }

    pub fn close(
        &mut self,
        base: &mut OperatorBase,
        _input: &mut StreamingExecutor,
    ) -> Result<(), QueryError> {
        if base.lifecycle.can_close() {
            base.lifecycle.mark_closed();
        }
        Ok(())
    }
}

impl TransactionCommandResult {
    fn into_data_chunk(
        self,
        output_layout: Arc<crate::query::executor::streaming::slot::SlotLayout>,
    ) -> DataChunk {
        let message = Value::string(self.message);
        let command = Value::string(self.command);
        DataChunk::new_with_layout(vec![vec![command, message]], output_layout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::executor::streaming::operators::source_operator::SourceOperator;
    use crate::query::executor::streaming::operators::spec::TxnSpec;
    use crate::query::executor::streaming::runtime::ExecutionRuntime;

    fn input() -> StreamingExecutor {
        StreamingExecutor::Source(
            OperatorBase::new(0),
            SourceOperator::ScanVertices {
                buffer: Vec::new(),
                current_index: 0,
                col_names: Vec::new(),
            },
        )
    }

    #[test]
    fn transaction_command_requires_controller() {
        let input = input();
        let operator = TxnOperator::from_spec(&TxnSpec::BeginTransaction);
        let mut executor = StreamingExecutor::Txn(OperatorBase::new(1), Box::new(input), operator);
        executor.open().expect("should open");
        let result = executor.advance();
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("controller not available"));
        executor.close().expect("should close");
    }

    #[test]
    fn transaction_command_emits_once() {
        let input = input();
        let operator = TxnOperator::from_spec(&TxnSpec::BeginTransaction);
        let mut executor = StreamingExecutor::Txn(OperatorBase::new(1), Box::new(input), operator);

        let ctrl = Arc::new(SessionTransactionController::new());
        let rt = ExecutionRuntime::default_budget();
        rt.set_session_controller(ctrl.clone());
        executor.set_runtime(Some(Arc::new(rt)));

        executor.open().expect("transaction command should open");

        // First advance: without a pre-registered transaction, the BEGIN
        // will fail because current_scope returns None.
        let result = executor.advance();
        assert!(result.is_err(), "expected error (no active transaction)");

        // emitted is now true — second call should return None
        let second = executor.advance().expect("second advance should not fail");
        assert!(second.is_none());
        executor.close().expect("transaction command should close");
    }

    #[test]
    fn test_commit_without_active_transaction() {
        let input = input();
        let operator = TxnOperator::from_spec(&TxnSpec::Commit);
        let mut executor = StreamingExecutor::Txn(OperatorBase::new(1), Box::new(input), operator);

        let ctrl = Arc::new(SessionTransactionController::new());
        let rt = ExecutionRuntime::default_budget();
        rt.set_session_controller(ctrl.clone());
        executor.set_runtime(Some(Arc::new(rt)));

        executor.open().expect("should open");
        let result = executor.advance();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Cannot COMMIT"));
        executor.close().expect("should close");
    }
}
