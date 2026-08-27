//! Memory estimation trait for plan nodes
//!
//! This trait provides a common interface for estimating memory usage
//! of different plan node types.

pub use crate::core::types::memory_estimation::{
    estimate_option_string_memory, estimate_string_memory, estimate_vec_memory,
    estimate_vec_string_memory, MemoryEstimatable,
};

/// Macro to implement a default estimate_memory for plan nodes
/// This macro estimates the base struct size and col_names vector
#[macro_export]
macro_rules! impl_default_estimate_memory {
    ($node_type:ty) => {
        impl $crate::core::types::memory_estimation::MemoryEstimatable for $node_type {
            fn estimate_memory(&self) -> usize {
                let base = std::mem::size_of::<$node_type>();

                let col_names_size =
                    $crate::core::types::memory_estimation::estimate_vec_string_memory(
                        &self.col_names(),
                    );

                let output_var_size = std::mem::size_of::<Option<String>>()
                    + self
                        .output_var()
                        .map(|s| std::mem::size_of::<String>() + s.capacity())
                        .unwrap_or(0);

                base + col_names_size + output_var_size
            }
        }
    };
}
