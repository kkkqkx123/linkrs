use crate::core::error::QueryError;
use crate::core::Value;
use crate::query::executor::streaming::chunk::DataChunk;
use crate::query::executor::streaming::executor::StreamingExecutor;
use crate::query::executor::streaming::operator_base::OperatorBase;

#[derive(Debug)]
pub enum TxnOperator {
    BeginTransaction { transaction_id: Option<String> },
    Commit { transaction_id: Option<String> },
    Rollback { transaction_id: Option<String> },
}

impl TxnOperator {
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
            Self::BeginTransaction { .. } => {
                Ok(Some(DataChunk::from_rows(vec![vec![Value::String(
                    "transaction started".to_string(),
                )]])))
            }
            Self::Commit { .. } => {
                if let Some(chunk) = input.advance()? {
                    return Ok(Some(chunk));
                }
                Ok(Some(DataChunk::from_rows(vec![vec![Value::String(
                    "committed".to_string(),
                )]])))
            }
            Self::Rollback { .. } => Ok(Some(DataChunk::from_rows(vec![vec![Value::String(
                "rolled back".to_string(),
            )]]))),
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
