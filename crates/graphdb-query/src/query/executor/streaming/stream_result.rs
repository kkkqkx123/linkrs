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
type TransactionCallback = Box<dyn FnOnce() -> Result<(), String> + Send>;
type TransactionFinalizer =
    Arc<Mutex<Option<(TransactionCallback, TransactionCallback)>>>;

#[derive(Clone)]
pub struct StreamingQueryResult {
    inner: Arc<Mutex<StreamState>>,
    runtime: Arc<ExecutionRuntime>,
    on_drop: DropCallback,
    transaction_finalizer: TransactionFinalizer,
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
        if let Err(error) = self.finalize_transaction() {
            log::error!("Streaming transaction finalization failed: {}", error);
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
    /// Stream was explicitly closed before normal exhaustion.
    Closed,
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
            transaction_finalizer: Arc::new(Mutex::new(None)),
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
                    transaction_finalizer: Arc::new(Mutex::new(None)),
                    dropped: Arc::new(AtomicBool::new(false)),
                }
            }
            ExecutionResult::Success | ExecutionResult::Empty => {
                let runtime = Arc::new(ExecutionRuntime::default_budget());
                Self {
                    inner: Arc::new(Mutex::new(StreamState::Exhausted)),
                    runtime,
                    on_drop: Arc::new(Mutex::new(None)),
                    transaction_finalizer: Arc::new(Mutex::new(None)),
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
                    transaction_finalizer: Arc::new(Mutex::new(None)),
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
                    transaction_finalizer: Arc::new(Mutex::new(None)),
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
                let result = match stream.next_chunk() {
                    Ok(result) => result,
                    Err(error) => {
                        *guard = StreamState::Closed;
                        drop(guard);
                        self.finalize_transaction()
                            .map_err(QueryError::execution)?;
                        return Err(error);
                    }
                };
                if let Some(ref chunk) = result {
                    if cached.is_none() {
                        *cached = Some(chunk.col_names());
                    }
                }
                let exhausted = result.is_none();
                if exhausted {
                    *guard = StreamState::Exhausted;
                }
                drop(guard);
                if exhausted {
                    self.finalize_transaction()
                        .map_err(QueryError::execution)?;
                }
                Ok(result)
            }
            StreamState::Materialized {
                rows,
                col_names,
                exhausted,
            } => {
                if *exhausted {
                    *guard = StreamState::Exhausted;
                    drop(guard);
                    self.finalize_transaction()
                        .map_err(QueryError::execution)?;
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
            StreamState::Exhausted | StreamState::Closed => Ok(None),
        }
    }

    /// Cancel the query execution.
    pub fn cancel(&self) {
        self.runtime.cancel();
        if let Err(error) = self.abort_transaction() {
            log::error!("Streaming transaction cancellation failed: {}", error);
        }
    }

    /// Check whether the query has been cancelled.
    pub fn is_cancelled(&self) -> bool {
        self.runtime.is_cancelled()
    }

    /// Close the stream and release resources.
    pub fn close(&self) -> Result<(), QueryError> {
        let mut guard = self.inner.lock();
        let result = match &mut *guard {
            StreamState::Streaming(ref mut stream, _) => stream.close(),
            _ => Ok(()),
        };
        *guard = StreamState::Closed;
        drop(guard);
        result?;
        self.abort_transaction().map_err(QueryError::execution)
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
            StreamState::Exhausted | StreamState::Closed => None,
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
        let previous = self.on_drop.lock().take();
        *self.on_drop.lock() = Some(Box::new(move || {
            if let Some(previous) = previous {
                previous();
            }
            f();
        }));
    }

    /// Register a transaction finalizer that fires when the stream is dropped.
    ///
    /// - `commit`: called when the stream is fully consumed without error.
    /// - `abort`: called on error, cancellation, or premature drop.
    ///
    /// Only one finalizer can be registered. The `on_drop` callback is used
    /// internally to invoke the appropriate branch.
    pub fn set_transaction_finalizer(
        &self,
        commit: Box<dyn FnOnce() + Send>,
        abort: Box<dyn FnOnce() + Send>,
    ) {
        self.set_transaction_finalizer_with_result(
            Box::new(move || {
                commit();
                Ok(())
            }),
            Box::new(move || {
                abort();
                Ok(())
            }),
        );
    }

    /// Register a finalizer whose error is returned at stream exhaustion.
    /// A finalizer triggered by `Drop` logs the error because no result can be
    /// returned after ownership has been released.
    pub fn set_transaction_finalizer_with_result(
        &self,
        commit: TransactionCallback,
        abort: TransactionCallback,
    ) {
        *self.transaction_finalizer.lock() = Some((commit, abort));
    }

    fn finalize_transaction(&self) -> Result<(), String> {
        let is_exhausted = matches!(*self.inner.lock(), StreamState::Exhausted);
        let Some((commit, abort)) = self.transaction_finalizer.lock().take() else {
            return Ok(());
        };
        if is_exhausted {
            commit()
        } else {
            abort()
        }
    }

    fn abort_transaction(&self) -> Result<(), String> {
        let Some((_, abort)) = self.transaction_finalizer.lock().take() else {
            return Ok(());
        };
        abort()
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
