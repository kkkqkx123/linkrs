use std::sync::{Arc, Weak};

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
use std::sync::atomic::{AtomicBool, Ordering};

type DropCallback = Arc<Mutex<Option<Box<dyn FnOnce() + Send>>>>;

#[derive(Clone)]
pub struct StreamingQueryResult {
    inner: Arc<Mutex<StreamState>>,
    runtime: Arc<ExecutionRuntime>,
    on_drop: DropCallback,
    dropped: Arc<AtomicBool>,
}

impl Drop for StreamingQueryResult {
    fn drop(&mut self) {
        // A streaming result is routinely cloned between the API task and the
        // blocking producer. Deregistration must happen only after the final
        // handle is gone; otherwise KILL QUERY can no longer find active work.
        if Arc::strong_count(&self.inner) != 1 {
            return;
        }
        if self.dropped.swap(true, Ordering::Relaxed) {
            return;
        }
        if let Some(f) = self.on_drop.lock().take() {
            f();
        }
    }
}

enum StreamState {
    /// Active streaming from an executor pipeline.
    /// The `Option<Vec<String>>` caches column names once the first chunk is
    /// received, so that callers can inspect the schema even after the stream
    /// is exhausted (provided at least one chunk was produced).
    Streaming(ResultStream, Option<Vec<String>>),
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
        Self::new_with_schema(stream, runtime, Vec::new())
    }

    /// Wrap a stream with the schema fixed by the physical output contract.
    /// This keeps empty results observable without waiting for a first chunk.
    pub fn new_with_schema(
        stream: ResultStream,
        runtime: Arc<ExecutionRuntime>,
        col_names: Vec<String>,
    ) -> Self {
        Self {
            inner: Arc::new(Mutex::new(StreamState::Streaming(stream, Some(col_names)))),
            runtime,
            on_drop: Arc::new(Mutex::new(None)),
            dropped: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Create a pre-materialized result from an [`ExecutionResult`].
    ///
    /// The result is returned as a single chunk on the first `next_chunk()` call.
    /// Useful for EXPLAIN/PROFILE results that are already materialized.
    pub fn from_execution_result(result: ExecutionResult) -> Self {
        match result {
            ExecutionResult::DataSet { data: ds, .. } => {
                let exhausted = ds.rows.is_empty();
                let runtime = Arc::new(ExecutionRuntime::default_budget());
                Self {
                    inner: Arc::new(Mutex::new(StreamState::Materialized {
                        rows: ds.rows,
                        col_names: ds.col_names,
                        exhausted,
                    })),
                    runtime,
                    on_drop: Arc::new(Mutex::new(None)),
                    dropped: Arc::new(AtomicBool::new(false)),
                }
            }
            ExecutionResult::Success | ExecutionResult::Empty => {
                let runtime = Arc::new(ExecutionRuntime::default_budget());
                Self {
                    inner: Arc::new(Mutex::new(StreamState::Exhausted)),
                    runtime,
                    on_drop: Arc::new(Mutex::new(None)),
                    dropped: Arc::new(AtomicBool::new(false)),
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
                    on_drop: Arc::new(Mutex::new(None)),
                    dropped: Arc::new(AtomicBool::new(false)),
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
                    on_drop: Arc::new(Mutex::new(None)),
                    dropped: Arc::new(AtomicBool::new(false)),
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
            StreamState::Streaming(ref mut stream, ref mut cached) => {
                let result = stream.next_chunk()?;
                if let Some(ref chunk) = result {
                    if cached.is_none() {
                        *cached = Some(chunk.col_names());
                    }
                }
                Ok(result)
            }
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
            StreamState::Streaming(ref mut stream, _) => stream.close(),
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

    /// Return column names if known.
    ///
    /// For materialized results, column names are available immediately.
    /// For streaming results, they are available after the first chunk is
    /// pulled (or `None` if no chunk was ever produced).
    pub fn column_names(&self) -> Option<Vec<String>> {
        let guard = self.inner.lock();
        match &*guard {
            StreamState::Streaming(_, cached) => cached.clone(),
            StreamState::Materialized { col_names, .. } => Some(col_names.clone()),
            StreamState::Exhausted => None,
        }
    }

    /// Access the underlying execution runtime (for profiling, cancel, etc.)
    pub fn runtime(&self) -> &ExecutionRuntime {
        &self.runtime
    }

    /// Return a [`Weak`] reference to the execution runtime for KILL QUERY registration.
    pub fn runtime_downgrade(&self) -> Weak<ExecutionRuntime> {
        Arc::downgrade(&self.runtime)
    }

    /// Register a cleanup callback that fires when this handle is dropped.
    ///
    /// Used by the API layer to deregister the query from the session
    /// when the stream ends (via either completion, error, or client disconnect).
    pub fn set_on_drop(&self, f: Box<dyn FnOnce() + Send>) {
        *self.on_drop.lock() = Some(f);
    }

    /// Set fallback column names that are available even before the first
    /// chunk is pulled (or when the result set is empty).
    ///
    /// Only takes effect when no chunk has been received yet (i.e. the
    /// cached column names are `None`).  Once a chunk arrives its column
    /// names take precedence and the fallback is ignored.
    pub fn set_fallback_column_names(&self, names: Vec<String>) {
        let mut guard = self.inner.lock();
        if let StreamState::Streaming(_, ref mut cached) = &mut *guard {
            if cached.is_none() {
                *cached = Some(names);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::Value;
    use crate::query::executor::base::MemoryBudget;
    use crate::query::executor::streaming::engine::StreamingExecutionEngine;
    use crate::query::executor::streaming::executor::StreamingExecutor;
    use crate::query::executor::streaming::operators::base::OperatorBase;
    use crate::query::executor::streaming::operators::source_operator::SourceOperator;
    use crate::query::executor::streaming::runtime::QueryIdentity;

    fn create_test_stream(count: usize) -> StreamingQueryResult {
        let mut engine = StreamingExecutionEngine::new();
        let runtime = Arc::new(ExecutionRuntime::new(
            QueryIdentity::default(),
            MemoryBudget::default_budget(),
            None,
            #[cfg(feature = "fulltext-search")]
            None,
            #[cfg(feature = "qdrant")]
            None,
        ));
        engine.set_runtime(runtime.clone());

        let buffer: Vec<Vec<Value>> = (0..count).map(|i| vec![Value::BigInt(i as i64)]).collect();

        let scan = StreamingExecutor::Source(
            OperatorBase::new(0),
            SourceOperator::ScanVertices {
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
        let result =
            StreamingQueryResult::from_execution_result(ExecutionResult::DataSet { data: ds });
        let chunk = result.next_chunk().unwrap().unwrap();
        assert_eq!(chunk.len(), 2);
        assert_eq!(chunk.col_names(), vec!["id"]);
        assert!(result.next_chunk().unwrap().is_none());
    }

    #[test]
    fn test_from_execution_result_empty() {
        let result = StreamingQueryResult::from_execution_result(ExecutionResult::Empty);
        assert!(result.next_chunk().unwrap().is_none());
    }

    #[test]
    fn test_from_execution_result_error() {
        let result =
            StreamingQueryResult::from_execution_result(ExecutionResult::Error("oops".to_string()));
        let chunk = result.next_chunk().unwrap().unwrap();
        assert!(!chunk.is_empty());
    }

    // ── Resource regression tests (R5) ──

    #[test]
    fn test_on_drop_callback_fires_on_drop() {
        let result = create_test_stream(5);
        let fired = Arc::new(AtomicBool::new(false));
        let fired_clone = fired.clone();
        result.set_on_drop(Box::new(move || {
            fired_clone.store(true, Ordering::Relaxed);
        }));
        drop(result);
        assert!(
            fired.load(Ordering::Relaxed),
            "on_drop must fire when handle is dropped"
        );
    }

    #[test]
    fn test_on_drop_waits_for_the_last_clone() {
        let result = create_test_stream(5);
        let call_count = Arc::new(AtomicBool::new(false));
        let count_clone = call_count.clone();
        result.set_on_drop(Box::new(move || {
            count_clone.store(true, Ordering::Relaxed);
        }));

        let r2 = result.clone();
        let r3 = r2.clone();

        // Intermediate handles must not deregister an active stream.
        drop(r2);
        assert!(
            !call_count.load(Ordering::Relaxed),
            "on_drop must wait for the last handle"
        );

        drop(r3);
        assert!(
            !call_count.load(Ordering::Relaxed),
            "on_drop must wait for the last handle"
        );

        drop(result);
        assert!(
            call_count.load(Ordering::Relaxed),
            "on_drop must fire when the final handle is dropped"
        );
    }

    #[test]
    fn test_on_drop_not_called_when_not_set() {
        let result = create_test_stream(5);
        // Must not panic when dropping without on_drop registered.
        drop(result);
    }

    #[test]
    fn test_cancel_after_partial_consumption() {
        let result = create_test_stream(50);
        let chunk = result.next_chunk().unwrap();
        assert!(chunk.is_some());
        // Cancel mid-stream
        result.cancel();
        assert!(result.is_cancelled());
        assert!(result.next_chunk().is_err());
    }

    #[test]
    fn test_double_cancel_is_safe() {
        let result = create_test_stream(10);
        result.cancel();
        result.cancel(); // Must not panic.
        assert!(result.is_cancelled());
    }

    #[test]
    fn test_cancel_with_zero_rows_stream() {
        let result = create_test_stream(0);
        result.cancel();
        assert!(result.is_cancelled());
        assert!(result.next_chunk().is_err());
    }

    #[test]
    fn test_from_execution_result_success_has_exhausted_stream() {
        let result = StreamingQueryResult::from_execution_result(ExecutionResult::Success);
        assert!(result.next_chunk().unwrap().is_none());
    }

    #[test]
    fn test_drop_without_consuming_does_not_leak() {
        // Streaming engine resources must be released on drop even
        // when no chunk was ever pulled.
        let result = create_test_stream(10);
        drop(result);
    }
}
