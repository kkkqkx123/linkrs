use std::sync::Arc;

use parking_lot::Mutex;

use super::chunk::{ColumnInfo, DataChunk, Schema};
use super::runtime::ExecutionRuntime;
use super::stream::ResultStream;
use crate::core::error::QueryError;
use crate::query::data_set::DataSet;
use crate::query::executor::base::ExecutionResult;

/// Thread-safe (Send + Sync) streaming result handle.
///
/// Wraps a [`ResultStream`] or pre-materialized [`ExecutionResult`] behind an
/// `Arc<Mutex<>>` so it can be shared across async tasks or API boundaries.
#[derive(Clone)]
pub struct StreamingQueryResult {
    inner: Arc<Mutex<StreamState>>,
    runtime: Arc<ExecutionRuntime>,
}

enum StreamState {
    /// Active streaming from an executor pipeline.
    Streaming(ResultStream),
    /// Pre-materialized result (EXPLAIN/PROFILE/SpaceSwitched).
    Materialized {
        rows: Vec<Vec<crate::core::Value>>,
        col_names: Vec<String>,
        exhausted: bool,
    },
    /// Stream is exhausted.
    Exhausted,
}

impl StreamingQueryResult {
    /// Wrap a [`ResultStream`] into a thread-safe handle.
    pub fn new(stream: ResultStream, runtime: Arc<ExecutionRuntime>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(StreamState::Streaming(stream))),
            runtime,
        }
    }

    /// Create a pre-materialized result from an [`ExecutionResult`].
    ///
    /// The result is returned as a single chunk on the first `next_chunk()` call.
    /// Useful for EXPLAIN/PROFILE results that are already materialized.
    pub fn from_execution_result(result: ExecutionResult) -> Self {
        match result {
            ExecutionResult::DataSet(ds) => {
                let exhausted = ds.rows.is_empty();
                let runtime = Arc::new(ExecutionRuntime::default_budget());
                Self {
                    inner: Arc::new(Mutex::new(StreamState::Materialized {
                        rows: ds.rows,
                        col_names: ds.col_names,
                        exhausted,
                    })),
                    runtime,
                }
            }
            ExecutionResult::Success | ExecutionResult::Empty => {
                let runtime = Arc::new(ExecutionRuntime::default_budget());
                Self {
                    inner: Arc::new(Mutex::new(StreamState::Exhausted)),
                    runtime,
                }
            }
            ExecutionResult::SpaceSwitched(summary) => {
                let row = vec![
                    crate::core::Value::String(summary.name.clone()),
                    crate::core::Value::BigInt(summary.id as i64),
                ];
                let col_names = vec!["space_name".to_string(), "space_id".to_string()];
                let runtime = Arc::new(ExecutionRuntime::default_budget());
                Self {
                    inner: Arc::new(Mutex::new(StreamState::Materialized {
                        rows: vec![row],
                        col_names,
                        exhausted: false,
                    })),
                    runtime,
                }
            }
            ExecutionResult::Error(msg) => {
                let col_names = vec!["error".to_string()];
                let runtime = Arc::new(ExecutionRuntime::default_budget());
                Self {
                    inner: Arc::new(Mutex::new(StreamState::Materialized {
                        rows: vec![vec![crate::core::Value::String(msg)]],
                        col_names,
                        exhausted: false,
                    })),
                    runtime,
                }
            }
        }
    }

    /// Pull the next chunk of results.
    ///
    /// Returns `Ok(None)` when the stream is exhausted.
    /// Returns an error if the query has been cancelled.
    pub fn next_chunk(&self) -> Result<Option<DataChunk>, QueryError> {
        let mut guard = self.inner.lock();
        match &mut *guard {
            StreamState::Streaming(ref mut stream) => stream.next_chunk(),
            StreamState::Materialized {
                rows,
                col_names,
                exhausted,
            } => {
                if *exhausted {
                    return Ok(None);
                }
                *exhausted = true;
                let names = std::mem::take(col_names);
                let data = std::mem::take(rows);
                let columns: Vec<ColumnInfo> = names
                    .iter()
                    .map(|n| ColumnInfo {
                        name: n.clone(),
                        data_type: String::new(),
                    })
                    .collect();
                let chunk = DataChunk::new(data, Arc::new(Schema::new(columns)));
                Ok(Some(chunk))
            }
            StreamState::Exhausted => Ok(None),
        }
    }

    /// Cancel the query execution.
    pub fn cancel(&self) {
        self.runtime.cancel();
    }

    /// Check whether the query has been cancelled.
    pub fn is_cancelled(&self) -> bool {
        self.runtime.is_cancelled()
    }

    /// Close the stream and release resources.
    pub fn close(&self) -> Result<(), QueryError> {
        let mut guard = self.inner.lock();
        match &mut *guard {
            StreamState::Streaming(ref mut stream) => stream.close(),
            _ => {
                *guard = StreamState::Exhausted;
                Ok(())
            }
        }
    }

    /// Consume all remaining chunks and materialise into a `DataSet`.
    pub fn collect(&self) -> Result<DataSet, QueryError> {
        let mut all_rows = Vec::new();
        let mut col_names: Option<Vec<String>> = None;

        while let Some(chunk) = self.next_chunk()? {
            if col_names.is_none() {
                col_names = Some(chunk.col_names());
            }
            all_rows.extend(chunk.rows);
        }

        let names = col_names.unwrap_or_default();
        Ok(DataSet::from_rows(all_rows, names))
    }

    /// Access the underlying execution runtime (for profiling, cancel, etc.)
    pub fn runtime(&self) -> &ExecutionRuntime {
        &self.runtime
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::Value;
    use crate::query::executor::base::MemoryBudget;
    use crate::query::executor::streaming::engine::StreamingExecutionEngine;
    use crate::query::executor::streaming::operators::source_operator::SourceOperator;
    use crate::query::executor::streaming::executor::StreamingExecutor;
    use crate::query::executor::streaming::operator_base::OperatorBase;
    use crate::query::executor::streaming::runtime::QueryIdentity;

    fn create_test_stream(count: usize) -> StreamingQueryResult {
        let mut engine = StreamingExecutionEngine::new();
        let runtime = Arc::new(ExecutionRuntime::new(
            QueryIdentity::default(),
            MemoryBudget::default_budget(),
        ));
        engine.set_runtime(runtime.clone());

        let buffer: Vec<Vec<Value>> = (0..count)
            .map(|i| vec![Value::BigInt(i as i64)])
            .collect();

        let scan = StreamingExecutor::Source(
            OperatorBase::new(0),
            SourceOperator::ScanVertices {
                partition_id: 0,
                buffer,
                current_index: 0,
                col_names: vec!["id".to_string()],
            },
        );
        engine.register_executor(0, scan);
        let stream = engine.into_stream().unwrap();
        StreamingQueryResult::new(stream, runtime)
    }

    #[test]
    fn test_next_chunk() {
        let result = create_test_stream(10);
        let chunk = result.next_chunk().unwrap();
        assert!(chunk.is_some());
        assert_eq!(chunk.unwrap().len(), 10);
        let done = result.next_chunk().unwrap();
        assert!(done.is_none());
    }

    #[test]
    fn test_cancel() {
        let result = create_test_stream(100);
        result.cancel();
        assert!(result.is_cancelled());
        assert!(result.next_chunk().is_err());
    }

    #[test]
    fn test_collect() {
        let result = create_test_stream(25);
        let ds = result.collect().unwrap();
        assert_eq!(ds.row_count(), 25);
    }

    #[test]
    fn test_clone_shared_stream() {
        let result = create_test_stream(10);
        let result2 = result.clone();
        // Both clones share the same underlying stream via Arc<Mutex<>>.
        // First call consumes the only chunk.
        assert!(result.next_chunk().unwrap().is_some());
        // Second call (via clone) sees the stream is exhausted.
        assert!(result2.next_chunk().unwrap().is_none());
    }

    #[test]
    fn test_from_execution_result_dataset() {
        let ds = DataSet::from_rows(
            vec![vec![Value::Int(1)], vec![Value::Int(2)]],
            vec!["id".to_string()],
        );
        let result = StreamingQueryResult::from_execution_result(ExecutionResult::DataSet(ds));
        let chunk = result.next_chunk().unwrap().unwrap();
        assert_eq!(chunk.len(), 2);
        assert_eq!(chunk.col_names(), vec!["id"]);
        assert!(result.next_chunk().unwrap().is_none());
    }

    #[test]
    fn test_from_execution_result_empty() {
        let result =
            StreamingQueryResult::from_execution_result(ExecutionResult::Empty);
        assert!(result.next_chunk().unwrap().is_none());
    }

    #[test]
    fn test_from_execution_result_error() {
        let result =
            StreamingQueryResult::from_execution_result(ExecutionResult::Error("oops".to_string()));
        let chunk = result.next_chunk().unwrap().unwrap();
        assert!(!chunk.is_empty());
    }
}
