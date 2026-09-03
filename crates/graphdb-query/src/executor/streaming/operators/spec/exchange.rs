//! Immutable configuration for exchange (gather / merge / repartition) operators.

use crate::executor::streaming::executor::SortDirection;
use crate::executor::streaming::slot::SlotLayout;
use graphdb_core::types::expr::Expression;

/// Immutable config for exchange (gather / merge / repartition) operators.
///
/// Workers in the shared engine-level scheduler execute partition tasks
/// dynamically via a morsel-style shared atomic counter.
#[derive(Debug, Clone)]
pub enum ExchangeSpec {
    /// Concatenate N partition outputs in partition order.
    Concatenate { partition_count: usize },
    /// N-way merge-sort of pre-sorted partition inputs.
    MergeSort {
        sort_expressions: Vec<Expression>,
        sort_directions: Vec<SortDirection>,
        limit: Option<usize>,
    },
    /// Hash-based repartition: partition rows by hash of keys into N buckets.
    ///
    /// Each child produces output for one partition; the operator collects,
    /// rehashes, and routes rows to the correct output bucket.  Used by hash
    /// join and hash aggregate to align partition boundaries.
    RepartitionHash {
        /// Number of output buckets / partitions.
        num_partitions: usize,
        /// Expressions whose hash determines the output partition.
        hash_expressions: Vec<Expression>,
        /// Column names / slot layout of the input rows.
        input_layout: Option<SlotLayout>,
        /// Column names / slot layout of the output rows.
        output_layout: Option<SlotLayout>,
    },
    /// Broadcast: replicate every input row to all consumers.
    ///
    /// Used to distribute a small build-side to all probe-side partitions.
    /// The input chunk is shallow-copied (Arc-like) or deep-cloned for each
    /// consumer depending on size.
    Broadcast {
        /// Number of output channels.
        num_consumers: usize,
    },
    /// Barrier: wait for all input fragments to complete before producing
    /// any output row.
    ///
    /// Used to sequence blocking stages (e.g. wait for build side before
    /// probe).  No data rearrangement; the first input's layout passes
    /// through.
    Barrier,
    /// Materialize: force an upstream fragment to fully materialise before
    /// the consumer fragment starts.
    ///
    /// Used for explicit spooling / break-fanout patterns and to isolate
    /// lifecycle across fragment boundaries.  Behaves like Concatenate but
    /// signals a pipeline break to the scheduler and validator.
    Materialize {
        /// Expected number of child inputs (all must be consumed).
        child_count: usize,
    },
}
