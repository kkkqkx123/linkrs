use std::sync::Arc;

use crate::core::error::QueryError;
use crate::core::Value;
use crate::query::executor::streaming::chunk::DataChunk;
use crate::query::executor::streaming::executor::StreamingExecutor;
use crate::query::executor::streaming::operators::source_operator::OperatorConfig;
use crate::query::executor::streaming::runtime::ExecutionRuntime;
use crate::query::executor::streaming::slot::SlotLayout;
use crate::query::executor::streaming::transaction_scope::{
    SessionTransactionController, TransactionCommandResult,
};

/// Transaction command operator kind.
///
/// Validates state transitions through the [`SessionTransactionController`]
/// and produces a structured result chunk.  The actual TransactionManager
/// operations (begin/commit/rollback) are performed by the API layer before
/// this operator runs.
#[derive(Debug)]
pub enum TxnOperatorKind {
    BeginTransaction { emitted: bool },
    Commit { emitted: bool },
    Rollback { emitted: bool },
}

/// Transaction command operator.
///
/// Wraps [`TxnOperatorKind`] with the runtime context injected at `open()`.
/// Lifecycle state is owned exclusively by the executor; operators never
/// write it.
#[derive(Debug)]
pub struct TxnOperator {
    pub kind: TxnOperatorKind,
    pub runtime: Option<Arc<ExecutionRuntime>>,
    pub output_layout: Arc<SlotLayout>,
    pub config: OperatorConfig,
}

impl TxnOperator {
    pub fn new(kind: TxnOperatorKind, output_layout: Arc<SlotLayout>) -> Self {
        Self {
            kind,
            runtime: None,
            output_layout,
            config: OperatorConfig::default(),
        }
    }

    pub fn from_spec(spec: &super::spec::TxnSpec, output_layout: Arc<SlotLayout>) -> Self {
        let kind = match spec {
            super::spec::TxnSpec::BeginTransaction => {
                TxnOperatorKind::BeginTransaction { emitted: false }
            }
            super::spec::TxnSpec::Commit => TxnOperatorKind::Commit { emitted: false },
            super::spec::TxnSpec::Rollback => TxnOperatorKind::Rollback { emitted: false },
        };
        Self::new(kind, output_layout)
    }

    /// Inject the runtime and execution config (called once by the executor
    /// before this operator produces any data).
    pub fn inject_context(
        &mut self,
        runtime: Option<&Arc<ExecutionRuntime>>,
        config: OperatorConfig,
    ) {
        if let Some(rt) = runtime {
            self.runtime = Some(rt.clone());
        }
        self.config = config;
    }

    fn controller(
        runtime: &Option<Arc<ExecutionRuntime>>,
    ) -> Result<Arc<SessionTransactionController>, QueryError> {
        runtime
            .as_ref()
            .and_then(|rt| rt.session_controller())
            .ok_or_else(|| {
                QueryError::execution(
                    "Transaction controller not available in execution runtime".to_string(),
                )
            })
    }

    pub fn open(&mut self, input: &mut StreamingExecutor) -> Result<(), QueryError> {
        input.open()
    }

    pub fn next(
        &mut self,
        _input: &mut StreamingExecutor,
    ) -> Result<Option<DataChunk>, QueryError> {
        let Self {
            kind,
            runtime,
            output_layout,
            config: _,
        } = self;
        match kind {
            TxnOperatorKind::BeginTransaction { emitted } => {
                if *emitted {
                    return Ok(None);
                }
                *emitted = true;
                let ctrl = Self::controller(runtime)?;
                // The API layer should have already created the transaction.
                // We validate that begin_tracking succeeds (no state conflict).
                let result = ctrl.current_scope();
                match result {
                    crate::query::executor::streaming::TransactionScope::ExplicitBorrowed {
                        transaction_id, ..
                    } => Ok(Some(
                        TransactionCommandResult::begin(transaction_id)
                            .into_data_chunk(Arc::clone(output_layout)),
                    )),
                    _ => Err(QueryError::execution(
                        "BEGIN: no active transaction found; API layer must call begin before executing this plan".to_string(),
                    )),
                }
            }
            TxnOperatorKind::Commit { emitted } => {
                if *emitted {
                    return Ok(None);
                }
                *emitted = true;
                let ctrl = Self::controller(runtime)?;
                let txn_id = ctrl.begin_commit()?;
                ctrl.commit_finalize();
                Ok(Some(
                    TransactionCommandResult::commit(txn_id)
                        .into_data_chunk(Arc::clone(output_layout)),
                ))
            }
            TxnOperatorKind::Rollback { emitted } => {
                if *emitted {
                    return Ok(None);
                }
                *emitted = true;
                let ctrl = Self::controller(runtime)?;
                let txn_id = ctrl.begin_rollback()?;
                ctrl.rollback_finalize();
                Ok(Some(
                    TransactionCommandResult::rollback(txn_id)
                        .into_data_chunk(Arc::clone(output_layout)),
                ))
            }
        }
    }

    pub fn stop(&mut self) -> Result<(), QueryError> {
        Ok(())
    }

    pub fn close(&mut self) -> Result<(), QueryError> {
        Ok(())
    }
}

impl TransactionCommandResult {
    fn into_data_chunk(self, output_layout: Arc<SlotLayout>) -> DataChunk {
        let message = Value::string(self.message);
        let command = Value::string(self.command);
        DataChunk::new_with_layout(vec![vec![command, message]], output_layout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::executor::streaming::operators::base::OperatorBase;
    use crate::query::executor::streaming::operators::source_operator::SourceOperator;
    use crate::query::executor::streaming::operators::source_operator::SourceOperatorKind;
    use crate::query::executor::streaming::operators::spec::TxnSpec;
    use crate::query::executor::streaming::runtime::ExecutionRuntime;

    fn input() -> StreamingExecutor {
        StreamingExecutor::Source(
            OperatorBase::new(0),
            SourceOperator::new(
                SourceOperatorKind::ScanVertices {
                    buffer: Vec::new(),
                    current_index: 0,
                    col_names: Vec::new(),
                },
                Arc::new(SlotLayout::new(Vec::new())),
            ),
        )
    }

    #[test]
    fn transaction_command_requires_controller() {
        let input = input();
        let operator = TxnOperator::from_spec(
            &TxnSpec::BeginTransaction,
            Arc::new(SlotLayout::new(Vec::new())),
        );
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
        let operator = TxnOperator::from_spec(
            &TxnSpec::BeginTransaction,
            Arc::new(SlotLayout::new(Vec::new())),
        );
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
        let operator =
            TxnOperator::from_spec(&TxnSpec::Commit, Arc::new(SlotLayout::new(Vec::new())));
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
