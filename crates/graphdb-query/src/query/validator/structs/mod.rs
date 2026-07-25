//! Data Structures Module
//!
//! **DEPRECATED**: Types have been moved to `crate::query::binder::validation`.
//! This module re-exports them for backward compatibility.

pub mod alias_structs;
pub mod clause_structs;
pub mod common_structs;
pub mod path_structs;
pub mod validation_info;

pub use alias_structs::*;
pub use clause_structs::*;
pub use common_structs::*;
pub use path_structs::*;
pub use validation_info::*;
