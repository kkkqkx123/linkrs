//! Aggregation strategy types and decision container.

/// Aggregation policy type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AggregateStrategy {
    /// Hash Aggregation – Storing Group Status Using a HashMap
    /// Applicable to: cases where the number of group keys is high and there is sufficient memory available.
    HashAggregate,
    /// Sorting and Aggregation: First sort the data, then perform the aggregation.
    /// Applicable to: inputs that are already sorted or nearly sorted, and in cases where memory is limited.
    SortAggregate,
    /// Stream Aggregation – An optimized aggregation method when the input data is already sorted.
    /// Applicable to: The input data has already been sorted according to the grouping key.
    StreamingAggregate,
}

impl AggregateStrategy {
    /// Obtain the policy name
    pub fn name(&self) -> &'static str {
        match self {
            AggregateStrategy::HashAggregate => "HashAggregate",
            AggregateStrategy::SortAggregate => "SortAggregate",
            AggregateStrategy::StreamingAggregate => "StreamingAggregate",
        }
    }
}

/// Aggregation policy decision-making
#[derive(Debug, Clone)]
pub struct AggregateStrategyDecision {
    /// Selected Aggregation Strategy
    pub strategy: AggregateStrategy,
    /// Estimated number of output lines
    pub estimated_output_rows: u64,
    /// Estimated cost
    pub estimated_cost: f64,
    /// Estimated memory usage (in bytes)
    pub estimated_memory_bytes: u64,
    /// Reason for the choice
    pub reason: SelectionReason,
}

/// Reasons for the choice of strategy
#[derive(Debug, Clone)]
pub enum SelectionReason {
    /// The input is already sorted, and stream aggregation is being used.
    InputAlreadySorted,
    /// A high cardinality of the grouping key results in better performance for hash aggregation.
    HighCardinality,
    /// The base value of the grouping key is low, which makes sorting and aggregation more efficient.
    LowCardinality,
    /// Memory is limited; choose the sorting algorithm for aggregation.
    MemoryConstrained,
    /// With small amounts of data, hash aggregation is much simpler.
    SmallDataSet,
    /// Large volumes of data: Sorting and aggregation should be performed to avoid memory overflow.
    LargeDataSet,
    /// Cost-based decision-making
    CostBased { hash_cost: f64, sort_cost: f64 },
}
