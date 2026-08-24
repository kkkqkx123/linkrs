//! Helper modules for streaming executor
//!
//! - comparison: value comparison and sorting
//! - accumulator_states: typed accumulator state for partial+final aggregates
//! - conversion: data type conversions
//! - runtime_bridge: runtime-aware await for async coordinator calls

pub mod accumulator_states;
pub mod comparison;
pub mod conversion;
pub mod runtime_bridge;

pub use accumulator_states::{
    accumulator_to_value, decode_partial, finalize_accumulator_value, AggregateAccumulator,
};
pub use comparison::compare_values;
pub use conversion::{edge_to_row, edges_to_rows, vertex_to_row, vertices_to_rows};
