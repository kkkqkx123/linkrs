//! ResultBoundary: typed delivery mechanism for query results.
//!
//! The plan root carries a stable [`OutputContract`] describing the result
//! shape (layout, nullability, ordering, streaming capability).  At
//! instantiation time a `ResultBoundary` is bound to that contract and
//! determines how chunks are delivered to the API layer.
//!
//! Four delivery modes:
//! - [`DataSetSink`](ResultBoundary::DataSetSink): materialise all chunks
//!   into an in-memory [`DataSet`].
//! - [`PullHandle`](ResultBoundary::PullHandle): pull-based streaming via
//!   a thread-safe handle.
//! - [`ChunkStreamSink`](ResultBoundary::ChunkStreamSink): chunk-at-a-time
//!   callback-based streaming.
//! - [`DiscardSink`](ResultBoundary::DiscardSink): side-effect-only commands
//!   that discard all output.

use std::fmt;
use std::sync::mpsc;
use std::sync::Arc;

use super::chunk::{ColumnInfo, DataChunk, Schema};
use super::plan::types::OutputContract;
use super::runtime::ExecutionRuntime;
use super::stream::ResultStream;
use crate::core::error::QueryError;
use crate::query::data_set::DataSet;
use crate::query::executor::base::ExecutionResult;

/// Typed delivery mechanism for a query's result stream.
///
/// Each variant represents a different consumer contract.  The schema is
/// always published before the first data row — even when the result set
/// is empty — so that API clients can inspect column metadata without
/// receiving a row.
pub enum ResultBoundary {
    /// Materialise all chunks into a single in-memory [`ExecutionResult`].
    ///
    /// The accumulated rows are owned by this sink.  Total memory counts
    /// toward the query's memory pool.
    DataSetSink {
        output: OutputContract,
        accumulated: Vec<DataChunk>,
    },

    /// Pull-based streaming handle.
    ///
    /// The consumer pulls chunks one at a time via [`PullHandle::next_chunk`].
    /// Supports cancellation, early close, and schema-before-first-row.
    PullHandle {
        output: OutputContract,
        stream: Option<ResultStream>,
        runtime: Arc<ExecutionRuntime>,
    },

    /// Chunk-by-chunk callback stream.
    ///
    /// Each produced chunk is delivered through the channel.
    /// The receiver can apply back-pressure by blocking on receive.
    ChunkStreamSink {
        output: OutputContract,
        sender: mpsc::Sender<ChunkOrDone>,
        runtime: Arc<ExecutionRuntime>,
    },

    /// Discard all output.
    ///
    /// Used for side-effect-only commands (DML without RETURNING, DDL,
    /// transaction control).  The pipeline still opens, advances, and
    /// closes normally — chunks are simply dropped.
    DiscardSink {
        output: OutputContract,
    },
}

impl fmt::Debug for ResultBoundary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DataSetSink { output, accumulated } => f
                .debug_struct("DataSetSink")
                .field("output", output)
                .field("accumulated_count", &accumulated.len())
                .finish(),
            Self::PullHandle { output, .. } => f
                .debug_struct("PullHandle")
                .field("output", output)
                .finish(),
            Self::ChunkStreamSink { output, .. } => f
                .debug_struct("ChunkStreamSink")
                .field("output", output)
                .finish(),
            Self::DiscardSink { output } => f
                .debug_struct("DiscardSink")
                .field("output", output)
                .finish(),
        }
    }
}

/// A chunk or end-of-stream signal for the channel-based sink.
#[derive(Debug)]
pub enum ChunkOrDone {
    Chunk(DataChunk),
    Done,
    Error(String),
}

impl ResultBoundary {
    /// Create a [`DataSetSink`] with an empty accumulator.
    pub fn data_set(output: OutputContract) -> Self {
        Self::DataSetSink {
            output,
            accumulated: Vec::new(),
        }
    }

    /// Create a [`PullHandle`] wrapping a streaming engine.
    pub fn pull_handle(
        output: OutputContract,
        stream: ResultStream,
        runtime: Arc<ExecutionRuntime>,
    ) -> Self {
        Self::PullHandle {
            output,
            stream: Some(stream),
            runtime,
        }
    }

    /// Create a [`ChunkStreamSink`] with an unbounded channel.
    pub fn chunk_stream(
        output: OutputContract,
        _capacity: usize,
        runtime: Arc<ExecutionRuntime>,
    ) -> (Self, mpsc::Receiver<ChunkOrDone>) {
        let (sender, receiver) = mpsc::channel();
        (
            Self::ChunkStreamSink {
                output,
                sender,
                runtime,
            },
            receiver,
        )
    }

    /// Create a [`DiscardSink`].
    pub fn discard(output: OutputContract) -> Self {
        Self::DiscardSink { output }
    }

    /// Return the output contract for this boundary.
    pub fn output_contract(&self) -> &OutputContract {
        match self {
            Self::DataSetSink { output, .. } => output,
            Self::PullHandle { output, .. } => output,
            Self::ChunkStreamSink { output, .. } => output,
            Self::DiscardSink { output, .. } => output,
        }
    }

    /// Accept a chunk into this boundary.
    ///
    /// For [`DataSetSink`] the chunk is accumulated in memory.
    /// For [`PullHandle`] the chunk is returned via the stream.
    /// For [`ChunkStreamSink`] the chunk is sent through the channel.
    /// For [`DiscardSink`] the chunk is dropped.
    pub fn push_chunk(&mut self, chunk: DataChunk) -> Result<(), QueryError> {
        match self {
            Self::DataSetSink { accumulated, .. } => {
                accumulated.push(chunk);
                Ok(())
            }
            Self::PullHandle { .. } => {
                // PullHandle does not push; the consumer pulls via next_chunk().
                // Chunks are routed through the ResultStream directly.
                Ok(())
            }
            Self::ChunkStreamSink {
                sender, runtime, ..
            } => {
                if runtime.is_cancelled() {
                    let _ = sender.send(ChunkOrDone::Error(
                        runtime
                            .cancel_token_v2()
                            .reason()
                            .map(|r| r.to_string())
                            .unwrap_or_else(|| "Query cancelled".to_string()),
                    ));
                    return Err(QueryError::execution("Query cancelled"));
                }
                sender
                    .send(ChunkOrDone::Chunk(chunk))
                    .map_err(|_| QueryError::execution("Result channel closed"))?;
                Ok(())
            }
            Self::DiscardSink { .. } => {
                // Drop the chunk — memory reservation will be released.
                Ok(())
            }
        }
    }

    /// Signal that the stream is complete.
    pub fn finish(&mut self) {
        match self {
            Self::ChunkStreamSink { sender, .. } => {
                let _ = sender.send(ChunkOrDone::Done);
            }
            Self::DataSetSink { .. }
            | Self::PullHandle { .. }
            | Self::DiscardSink { .. } => {
                // No-op for other variants.
            }
        }
    }

    /// Signal an error — forwards to the appropriate error channel.
    pub fn fail(&mut self, error: QueryError) {
        match self {
            Self::ChunkStreamSink { sender, .. } => {
                let _ = sender.send(ChunkOrDone::Error(error.to_string()));
            }
            Self::PullHandle { runtime, .. } => {
                runtime.cancel();
            }
            Self::DataSetSink { .. } | Self::DiscardSink { .. } => {}
        }
    }

    /// Materialise the accumulated result (only for [`DataSetSink`]).
    pub fn into_execution_result(mut self) -> Result<ExecutionResult, QueryError> {
        match &mut self {
            Self::DataSetSink { accumulated, output } => {
                if accumulated.is_empty() {
                    // Publish schema even for zero-row results.
                    let col_names = output.output_layout.names();
                    return Ok(ExecutionResult::DataSet {
                        data: DataSet::with_columns(col_names),
                    });
                }
                let col_names = accumulated[0].col_names();
                let mut all_rows = Vec::new();
                for chunk in accumulated.drain(..) {
                    for row in chunk.rows {
                        all_rows.push(row);
                    }
                }
                Ok(ExecutionResult::DataSet {
                    data: DataSet::from_rows(all_rows, col_names),
                })
            }
            Self::PullHandle { .. } => Err(QueryError::execution(
                "Cannot materialise PullHandle via into_execution_result",
            )),
            Self::ChunkStreamSink { .. } => Err(QueryError::execution(
                "Cannot materialise ChunkStreamSink via into_execution_result",
            )),
            Self::DiscardSink { .. } => Ok(ExecutionResult::Success),
        }
    }
}

/// RAII guard that finishes the boundary on drop.
pub struct ResultBoundaryGuard {
    boundary: Option<ResultBoundary>,
}

impl ResultBoundaryGuard {
    pub fn new(boundary: ResultBoundary) -> Self {
        Self {
            boundary: Some(boundary),
        }
    }

    /// Take the boundary back (e.g. after successful completion).
    pub fn take(&mut self) -> Option<ResultBoundary> {
        self.boundary.take()
    }

    /// Mark the boundary as failed.
    pub fn fail(&mut self, error: QueryError) {
        if let Some(ref mut b) = self.boundary {
            b.fail(error);
        }
    }
}

impl Drop for ResultBoundaryGuard {
    fn drop(&mut self) {
        if let Some(boundary) = self.boundary.take() {
            // On drop without explicit finish, signal done for channel-based sinks.
            let mut b = boundary;
            b.finish();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::Value;
    use crate::query::executor::base::MemoryBudget;
    use crate::query::executor::streaming::runtime::QueryIdentity;
    use crate::query::executor::streaming::slot::SlotLayout;

    fn test_output_contract() -> OutputContract {
        OutputContract {
            output_layout: SlotLayout::from_names(&[]),
            always_produces_row: false,
            nullability: Vec::new(),
            ordering: Vec::new(),
            streamable: true,
        }
    }

    fn make_chunk(rows: Vec<Vec<Value>>) -> DataChunk {
        DataChunk::from_rows(rows)
    }

    #[test]
    fn test_data_set_sink_accumulates_chunks() {
        let mut sink = ResultBoundary::data_set(test_output_contract());
        sink.push_chunk(make_chunk(vec![vec![Value::Int(1)]])).unwrap();
        sink.push_chunk(make_chunk(vec![vec![Value::Int(2)]])).unwrap();
        let result = sink.into_execution_result().unwrap();
        if let ExecutionResult::DataSet { data } = result {
            assert_eq!(data.row_count(), 2);
        } else {
            panic!("Expected DataSet");
        }
    }

    #[test]
    fn test_data_set_sink_empty_publishes_schema() {
        let contract = OutputContract {
            output_layout: SlotLayout::from_names(&["id".to_string(), "name".to_string()]),
            always_produces_row: false,
            nullability: vec![true, true],
            ordering: Vec::new(),
            streamable: true,
        };
        let sink = ResultBoundary::data_set(contract);
        let result = sink.into_execution_result().unwrap();
        if let ExecutionResult::DataSet { data } = result {
            assert_eq!(data.col_names, vec!["id", "name"]);
            assert_eq!(data.row_count(), 0);
        } else {
            panic!("Expected DataSet");
        }
    }

    #[test]
    fn test_discard_sink_returns_success() {
        let sink = ResultBoundary::discard(test_output_contract());
        let result = sink.into_execution_result().unwrap();
        assert!(matches!(result, ExecutionResult::Success));
    }

    #[test]
    fn test_chunk_stream_sink_delivers_chunks() {
        let runtime = Arc::new(ExecutionRuntime::new(
            QueryIdentity::default(),
            MemoryBudget::default_budget(),
            None,
            #[cfg(feature = "fulltext-search")]
            None,
            #[cfg(feature = "qdrant")]
            None,
        ));

        let (mut sink, receiver) =
            ResultBoundary::chunk_stream(test_output_contract(), 16, runtime);

        sink.push_chunk(make_chunk(vec![vec![Value::Int(42)]])).unwrap();
        sink.finish();

        let received: Vec<ChunkOrDone> = receiver.iter().collect();
        assert_eq!(received.len(), 2); // chunk + done
    }

    #[test]
    fn test_guard_finishes_on_drop() {
        let (sink, receiver) = ResultBoundary::chunk_stream(
            test_output_contract(),
            16,
            Arc::new(ExecutionRuntime::new(
                QueryIdentity::default(),
                MemoryBudget::default_budget(),
                None,
                #[cfg(feature = "fulltext-search")]
                None,
                #[cfg(feature = "qdrant")]
                None,
            )),
        );
        let guard = ResultBoundaryGuard::new(sink);
        drop(guard);

        let received: Vec<ChunkOrDone> = receiver.iter().collect();
        assert!(matches!(received.last(), Some(ChunkOrDone::Done)));
    }

    #[test]
    fn test_pull_handle_requires_explicit_conversion() {
        use crate::query::executor::streaming::engine::StreamingExecutionEngine;
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
        let stream = engine.into_stream().unwrap();
        let sink = ResultBoundary::pull_handle(
            test_output_contract(),
            stream,
            runtime,
        );
        let result = sink.into_execution_result();
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("PullHandle"));
    }
}
