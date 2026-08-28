use std::sync::Arc;

use super::chunk::DataChunk;
use super::engine::StreamingExecutionEngine;
use super::runtime::ExecutionRuntime;
use crate::data_set::DataSet;
use graphdb_core::error::QueryError;

/// Streaming result handle for pull-based chunk consumption.
///
/// Created by `StreamingExecutionEngine::into_stream()`. The caller
/// pulls chunks one at a time via `next_chunk()`, enabling the API
/// layer to stream results back without materialising the full result
/// set.  Call `close()` or let the handle drop to clean up resources.
///
/// When a full materialised `DataSet` is needed, use `collect()`.
pub struct ResultStream {
    engine: Option<StreamingExecutionEngine>,
    runtime: Arc<ExecutionRuntime>,
    opened: bool,
    exhausted: bool,
    closed: bool,
    /// Buffered chunks for the partitioned fallback path.
    buffered: Vec<DataChunk>,
    buffered_idx: usize,
}

impl ResultStream {
    /// Wrap an engine into a streaming result.
    ///
    /// The engine must have a root executor registered.  The stream
    /// takes ownership of the engine and calls `open()` on the first
    /// `next_chunk()` call (or immediately if `open_now` is true).
    pub fn new(engine: StreamingExecutionEngine, runtime: Arc<ExecutionRuntime>) -> Self {
        Self {
            engine: Some(engine),
            runtime,
            opened: false,
            exhausted: false,
            closed: false,
            buffered: Vec::new(),
            buffered_idx: 0,
        }
    }

    /// Create a stream from pre-collected chunks (used for partitioned fallback).
    ///
    /// The chunks are served one at a time via `next_chunk()`. The engine
    /// is retained for proper cleanup via `close_inner()`.
    pub fn from_collected(
        chunks: Vec<DataChunk>,
        engine: StreamingExecutionEngine,
        runtime: Arc<ExecutionRuntime>,
    ) -> Self {
        Self {
            engine: Some(engine),
            runtime,
            opened: true,
            exhausted: false,
            closed: false,
            buffered: chunks,
            buffered_idx: 0,
        }
    }

    /// Ensure the root executor is open.
    fn ensure_opened(&mut self) -> Result<(), QueryError> {
        if !self.opened {
            if let Some(ref mut engine) = self.engine {
                if let Err(error) = engine.open_root() {
                    return self.return_execution_error(error);
                }
            }
            self.opened = true;
        }
        Ok(())
    }

    /// Pull the next chunk of results.
    ///
    /// Returns `Ok(None)` when the stream is exhausted.
    /// The runtime's cancel token is checked on each call.
    pub fn next_chunk(&mut self) -> Result<Option<DataChunk>, QueryError> {
        if let Err(error) = self.runtime.ensure_not_cancelled() {
            return self.return_execution_error(error);
        }

        if self.exhausted {
            return Ok(None);
        }

        // Serve from buffered chunks first (partitioned fallback).
        if self.buffered_idx < self.buffered.len() {
            let chunk = &self.buffered[self.buffered_idx];
            self.buffered_idx += 1;
            return Ok(Some(chunk.clone()));
        }

        self.ensure_opened()?;

        if let Some(ref mut engine) = self.engine {
            let next = match engine.next_chunk_from_root() {
                Ok(next) => next,
                Err(error) => return self.return_execution_error(error),
            };
            match next {
                Some(chunk) => Ok(Some(chunk)),
                None => {
                    self.exhausted = true;
                    self.close_inner()?;
                    Ok(None)
                }
            }
        } else {
            Ok(None)
        }
    }

    /// Close the stream and release resources.
    pub fn close(&mut self) -> Result<(), QueryError> {
        if self.closed {
            return Ok(());
        }
        self.exhausted = true;
        self.close_inner()
    }

    fn close_inner(&mut self) -> Result<(), QueryError> {
        if self.closed {
            return Ok(());
        }
        let result = if let Some(ref mut engine) = self.engine {
            let stop_error = engine.stop_root().err();
            let close_error = engine.close_root().err();
            match (stop_error, close_error) {
                (Some(stop_err), Some(close_err)) => {
                    log::warn!(
                        "Both stop and close failed during stream teardown; \
                         stop error: {stop_err}; close error: {close_err}"
                    );
                    Err(stop_err)
                }
                (Some(stop_err), None) | (None, Some(stop_err)) => Err(stop_err),
                (None, None) => Ok(()),
            }
        } else {
            Ok(())
        };
        self.runtime.profile_end();
        self.runtime.release_resources();
        self.runtime.reset_arena();
        self.closed = true;
        result
    }

    /// Preserve an execution error while guaranteeing immediate, idempotent
    /// teardown. A cleanup failure is logged because the caller must receive
    /// the original execution failure.
    fn return_execution_error<T>(&mut self, error: QueryError) -> Result<T, QueryError> {
        if let Err(close_error) = self.close_inner() {
            log::warn!(
                "Failed to clean up streaming result after execution error: {}",
                close_error
            );
        }
        Err(error)
    }

    /// Consume the stream and materialise all remaining chunks into a `DataSet`.
    pub fn collect(mut self) -> Result<DataSet, QueryError> {
        let mut all_rows = Vec::new();
        let mut col_names: Option<Vec<String>> = None;

        while let Some(chunk) = self.next_chunk()? {
            if col_names.is_none() {
                col_names = Some(chunk.col_names());
            }
            for row in chunk.rows {
                all_rows.push(row);
            }
        }

        let names = col_names.unwrap_or_default();
        Ok(DataSet::from_rows(all_rows, names))
    }
}

impl Drop for ResultStream {
    fn drop(&mut self) {
        if !self.closed {
            let _ = self.close_inner();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::*;
    use crate::executor::base::MemoryBudget;
    use crate::executor::streaming::executor::StreamingExecutor;
    use crate::executor::streaming::operators::base::OperatorBase;
    use crate::executor::streaming::operators::source_operator::SourceOperator;
    use crate::executor::streaming::operators::source_operator::SourceOperatorKind;
    use crate::executor::streaming::runtime::QueryIdentity;
    use crate::executor::streaming::slot::SlotLayout;
    use graphdb_core::Value;

    #[test]
    fn cancellation_error_releases_resources_immediately() {
        let runtime = Arc::new(ExecutionRuntime::new(
            QueryIdentity::default(),
            MemoryBudget::default_budget(),
            None,
            #[cfg(feature = "fulltext")]
            None,
            #[cfg(feature = "vector")]
            None,
        ));
        let released = Arc::new(AtomicBool::new(false));
        let released_for_cleanup = released.clone();
        runtime.on_cleanup(move || {
            released_for_cleanup.store(true, Ordering::Relaxed);
        });

        let mut engine = StreamingExecutionEngine::new();
        engine.register_executor(
            0,
            StreamingExecutor::Source(
                OperatorBase::new(1),
                SourceOperator::new(
                    SourceOperatorKind::ScanVertices {
                        buffer: vec![vec![Value::BigInt(1)]],
                        current_index: 0,
                        col_names: vec!["id".to_string()],
                    },
                    Arc::new(SlotLayout::from_names(&["id".to_string()])),
                ),
            ),
        );
        engine.set_runtime(runtime.clone());
        let mut stream = engine.into_stream().expect("create stream");

        runtime.cancel();
        assert!(stream.next_chunk().is_err());
        assert!(released.load(Ordering::Relaxed));
        assert!(stream.close().is_ok());
    }
}
