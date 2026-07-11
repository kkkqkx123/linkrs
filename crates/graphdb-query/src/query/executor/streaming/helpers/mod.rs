//! Helper modules for streaming executor
//!
//! - comparison: value comparison and sorting
//! - aggregation: aggregate function computation
//! - conversion: data type conversions

pub mod aggregation;
pub mod comparison;
pub mod conversion;

pub use aggregation::compute_aggregate;
pub use comparison::compare_values;
pub use conversion::{edge_to_row, edges_to_rows, vertex_to_row, vertices_to_rows};
