use std::sync::Arc;

use super::chunk::DataChunk;
use super::engine::StreamingExecutionEngine;
use super::runtime::ExecutionRuntime;
use crate::core::error::QueryError;
use crate::query::data_set::DataSet;

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
}

impl ResultStream {
    /// Wrap an engine into a streaming result.
    ///
    /// The engine must have a root executor registered.  The stream
    /// takes ownership of the engine and calls `open()` on the first
    /// `next_chunk()` call (or immediately if `open_now` is true).
    pub fn new(
        engine: StreamingExecutionEngine,
        runtime: Arc<ExecutionRuntime>,
    ) -> Self {
        Self {
            engine: Some(engine),
            runtime,
            opened: false,
            exhausted: false,
        }
    }

    /// Ensure the root executor is open.
    fn ensure_opened(&mut self) -> Result<(), QueryError> {
        if !self.opened {
            if let Some(ref mut engine) = self.engine {
                engine.open_root()?;
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
        self.runtime.ensure_not_cancelled()?;

        if self.exhausted {
            return Ok(None);
        }

        self.ensure_opened()?;

        if let Some(ref mut engine) = self.engine {
            match engine.next_chunk_from_root()? {
                Some(chunk) => {
                    let count = chunk.len() as u64;
                    self.runtime.profile_add_rows(count);
                    Ok(Some(chunk))
                }
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
        if self.exhausted {
            return Ok(());
        }
        self.exhausted = true;
        self.close_inner()
    }

    fn close_inner(&mut self) -> Result<(), QueryError> {
        let result = if let Some(ref mut engine) = self.engine {
            engine.close_root()
        } else {
            Ok(())
        };
        self.runtime.release_resources();
        result
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
        if !self.exhausted {
            let _ = self.close_inner();
        }
    }
}
