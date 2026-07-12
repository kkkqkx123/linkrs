//! Helper modules for streaming executor
//!
//! - comparison: value comparison and sorting
//! - aggregation: aggregate function computation
//! - accumulator_states: typed accumulator state for partial+final aggregates
//! - conversion: data type conversions

pub mod accumulator_states;
pub mod aggregation;
pub mod comparison;
pub mod conversion;

pub use accumulator_states::{
    accumulator_to_value, finalize_accumulator_value, AggregateAccumulator,
};
pub use aggregation::compute_aggregate;
pub use comparison::compare_values;
pub use conversion::{edge_to_row, edges_to_rows, vertex_to_row, vertices_to_rows};
