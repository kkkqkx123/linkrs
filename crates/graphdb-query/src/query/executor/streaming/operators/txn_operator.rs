use crate::core::error::QueryError;
use crate::core::Value;
use crate::query::executor::streaming::chunk::DataChunk;
use crate::query::executor::streaming::executor::StreamingExecutor;
use crate::query::executor::streaming::operator_base::OperatorBase;

#[derive(Debug)]
pub enum TxnOperator {
    BeginTransaction {
        transaction_id: Option<String>,
        emitted: bool,
    },
    Commit {
        transaction_id: Option<String>,
        emitted: bool,
    },
    Rollback {
        transaction_id: Option<String>,
        emitted: bool,
    },
}

impl TxnOperator {
    /// Create a TxnOperator from an immutable spec.
    pub fn from_spec(spec: &super::super::operator_spec::TxnSpec) -> Self {
        match spec {
            super::super::operator_spec::TxnSpec::BeginTransaction { transaction_id } => {
                TxnOperator::BeginTransaction {
                    transaction_id: transaction_id.clone(),
                    emitted: false,
                }
            }
            super::super::operator_spec::TxnSpec::Commit { transaction_id } => {
                TxnOperator::Commit {
                    transaction_id: transaction_id.clone(),
                    emitted: false,
                }
            }
            super::super::operator_spec::TxnSpec::Rollback { transaction_id } => {
                TxnOperator::Rollback {
                    transaction_id: transaction_id.clone(),
                    emitted: false,
                }
            }
        }
    }

    pub fn open(
        &mut self,
        _base: &mut OperatorBase,
        input: &mut StreamingExecutor,
    ) -> Result<(), QueryError> {
        match self {
            Self::BeginTransaction { .. } | Self::Commit { .. } | Self::Rollback { .. } => {
                input.open()?;
                _base.lifecycle.mark_opened();
                Ok(())
            }
        }
    }

    pub fn next(
        &mut self,
        _base: &mut OperatorBase,
        input: &mut StreamingExecutor,
    ) -> Result<Option<DataChunk>, QueryError> {
        match self {
            Self::BeginTransaction { emitted, .. } => emit_once(emitted, "transaction started"),
            Self::Commit { emitted, .. } => {
                if *emitted {
                    return Ok(None);
                }
                if let Some(chunk) = input.advance()? {
                    return Ok(Some(chunk));
                }
                emit_once(emitted, "committed")
            }
            Self::Rollback { emitted, .. } => emit_once(emitted, "rolled back"),
        }
    }

    pub fn stop(
        &mut self,
        _base: &mut OperatorBase,
        input: &mut StreamingExecutor,
    ) -> Result<(), QueryError> {
        match self {
            Self::BeginTransaction { .. } | Self::Commit { .. } | Self::Rollback { .. } => {
                input.stop()
            }
        }
    }

    pub fn close(
        &mut self,
        _base: &mut OperatorBase,
        input: &mut StreamingExecutor,
    ) -> Result<(), QueryError> {
        match self {
            Self::BeginTransaction { .. } | Self::Commit { .. } | Self::Rollback { .. } => {
                if _base.lifecycle.can_close() {
                    input.close()?;
                    _base.lifecycle.mark_closed();
                }
                Ok(())
            }
        }
    }
}

fn emit_once(emitted: &mut bool, message: &str) -> Result<Option<DataChunk>, QueryError> {
    if *emitted {
        return Ok(None);
    }
    *emitted = true;
    Ok(Some(DataChunk::from_rows(vec![vec![Value::String(
        message.to_string(),
    )]])))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::executor::streaming::builder::StreamingExecutorBuilder;
    use crate::query::executor::streaming::operator_spec::TxnSpec;

    #[test]
    fn transaction_command_emits_once() {
        let input =
            StreamingExecutorBuilder::build_simple_scan(vec![]).expect("test source should build");
        let operator = TxnOperator::from_spec(&TxnSpec::BeginTransaction {
            transaction_id: None,
        });
        let mut executor = StreamingExecutor::Txn(OperatorBase::new(1), Box::new(input), operator);
        executor.open().expect("transaction command should open");
        assert!(executor
            .advance()
            .expect("first advance should succeed")
            .is_some());
        assert!(executor
            .advance()
            .expect("second advance should succeed")
            .is_none());
        executor.close().expect("transaction command should close");
    }
}
