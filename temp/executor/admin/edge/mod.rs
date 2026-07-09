//! Side type management actuator
//!
//! Provides creation, modification, description, deletion and listing functions for edge types.

pub mod alter_edge;
pub mod create_edge;
pub mod desc_edge;
pub mod drop_edge;
pub mod show_edges;

#[cfg(test)]
mod tests;

pub use alter_edge::AlterEdgeExecutor;
pub use create_edge::CreateEdgeExecutor;
pub use desc_edge::DescEdgeExecutor;
pub use drop_edge::DropEdgeExecutor;
pub use show_edges::ShowEdgesExecutor;
